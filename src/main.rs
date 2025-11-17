use crate::downloader::Downloader;
use crate::manifest::{ManifestFileManaged, load_manifests};
use crate::storage::{CdnReader, FileStatus, S3Storage, Storage};
use crate::utils::to_hex;
use anyhow::Error;
use clap::Parser;
use reqwest::Url;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

mod downloader;
mod manifest;
mod storage;
mod utils;

/// Manage mirrored files on rust-lang CDN.
#[derive(Debug, Parser)]
enum Cli {
    /// Upload files to the CDN and check that the local files are consistent.
    Upload(UploadArgs),
    /// Add a new mirrored file entry.
    AddFile(AddFileArgs),
}

#[derive(Debug, Parser)]
struct UploadArgs {
    /// Path to the manifest to synchronize.
    #[arg(default_value = "files/")]
    manifests_dir: PathBuf,

    /// Only check which changes are needed (no credentials required).
    #[arg(long)]
    skip_upload: bool,

    /// Base URL of the CDN where mirrored files are served.
    #[arg(long, default_value = "https://ci-mirrors.rust-lang.org")]
    cdn_url: String,

    /// Name of the S3 bucket containing the files.
    #[arg(long, default_value = "rust-lang-ci-mirrors")]
    s3_bucket: String,

    #[arg(short, long, default_value = "100")]
    jobs: usize,
}

#[derive(Debug, Parser)]
struct AddFileArgs {
    /// URL that should be mirrored.
    url: Url,
    /// Path under which the file should be available on the CDN.
    #[arg(long)]
    path: String,
    /// TOML file into which should the mirrored entry be added.
    #[arg(long)]
    toml_file: PathBuf,
    /// License of the file.
    #[arg(long)]
    license: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let args = Cli::parse();
    match args {
        Cli::Upload(args) => {
            upload(args).await?;
        }
        Cli::AddFile(args) => {
            add_file(args).await?;
        }
    }

    Ok(())
}

async fn upload(args: UploadArgs) -> anyhow::Result<()> {
    let (files, mut errors) = load_manifests(&args.manifests_dir)?;

    let storage = Arc::new(if args.skip_upload {
        Storage::ReadOnly(CdnReader::new(args.cdn_url))
    } else {
        Storage::ReadWrite(S3Storage::new(args.s3_bucket).await?)
    });

    // Collect all errors that happen during the check phase and show them at the end. This way, if
    // there are multiple errors in CI users won't have to retry the build multiple times.
    eprintln!(
        "calculating the changes to execute ({} files, {} parallelism)...",
        files.len(),
        args.jobs
    );

    // Check the status of all files in parallel.
    let concurrency_limiter = Arc::new(Semaphore::new(args.jobs));
    let mut taskset = JoinSet::new();
    for file in files {
        let storage = storage.clone();
        let concurrency_limiter = concurrency_limiter.clone();
        taskset.spawn(async move {
            let _permit = concurrency_limiter.acquire().await.unwrap();
            let status = storage.file_status(&file.name).await;
            (file, status)
        });
    }

    let mut to_upload = Vec::new();
    for (file, status) in taskset.join_all().await {
        let name = &file.name;
        match status? {
            FileStatus::Legacy => errors.push(format!(
                "file {name} was already uploaded without this tool"
            )),
            FileStatus::Present { sha256 } if sha256 != file.sha256 => errors.push(format!(
                "file {name} was already uploaded with different content"
            )),
            FileStatus::Missing => to_upload.push(file),
            FileStatus::Present { .. } => {}
        }
    }

    // We download eagerly to be able to detect errors during the check phase.
    let downloader = Downloader::new()?;
    for file in &to_upload {
        if let Err(err) = downloader.download(file).await {
            errors.push(format!("{err:?}"));
        }
    }

    if !errors.is_empty() {
        eprintln!("Found {} error(s)", errors.len());
        for error in errors {
            eprintln!("error: {error}");
        }
        std::process::exit(1);
    } else if to_upload.is_empty() {
        eprintln!("everything is up to date!");
        return Ok(());
    } else if args.skip_upload {
        eprintln!("skipping upload due to --skip-upload");
        return Ok(());
    }

    for file in &to_upload {
        eprintln!("uploading {}...", file.name);
        storage
            .upload_file(&file.name, &downloader.path_for(file))
            .await?;
        storage
            .write_contents(&format!("{}.sha256", &file.name), file.sha256.as_bytes())
            .await?;
    }
    Ok(())
}

async fn add_file(args: AddFileArgs) -> anyhow::Result<()> {
    use std::io::Write;

    let hash = Downloader::new()?.get_file_hash(&args.url).await?;

    let file_existed = args.toml_file.is_file();
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.toml_file)?;

    let rename_from = if let Some(file_name) = args.url.path().split('/').last()
        && let Some(path_name) = args.path.split('/').last()
        && file_name != path_name
    {
        Some(file_name.to_string())
    } else {
        None
    };

    let entry = ManifestFileManaged::new(
        args.path,
        to_hex(&hash),
        args.url,
        args.license.unwrap_or(String::new()),
        rename_from,
    );
    let entry = toml::to_string(&entry)?;

    let space = if file_existed { "\n" } else { "" };
    write!(
        file,
        r#"{space}[[files]]
{entry}"#,
    )?;

    Ok(())
}

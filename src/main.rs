use crate::downloader::Downloader;
use crate::manifest::load_manifests;
use crate::storage::{CdnReader, FileStatus, S3Storage, Storage};
use anyhow::Error;
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

mod downloader;
mod manifest;
mod storage;

#[derive(Debug, Parser)]
struct Cli {
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

#[tokio::main]
async fn main() -> Result<(), Error> {
    let args = Cli::parse();
    let files = load_manifests(&args.manifests_dir)?;

    let storage = Arc::new(if args.skip_upload {
        Storage::ReadOnly(CdnReader::new(args.cdn_url))
    } else {
        Storage::ReadWrite(S3Storage::new(args.s3_bucket).await?)
    });

    // Collect all errors that happen during the check phase and show them at the end. This way, if
    // there are multiple errors in CI users won't have to retry the build multiple times.
    let mut errors = Vec::new();

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
        eprintln!("downloading {}...", file.source);
        if let Err(err) = downloader.download(&file).await {
            errors.push(format!("{err:?}"));
        }
    }

    if !errors.is_empty() {
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

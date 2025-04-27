use crate::manifest::{MirrorFile, Source};
use crate::utils::to_hex;
use anyhow::{Error, bail};
use futures::TryStreamExt as _;
use reqwest::{Client, Url};
use sha2::{Digest as _, Sha256};
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};
use tempfile::TempDir;
use tokio::fs::File;
use tokio::io::{AsyncWrite, BufWriter};
use tokio_util::io::StreamReader;

pub(crate) struct Downloader {
    storage: TempDir,
    http: Client,
}

impl Downloader {
    pub(crate) fn new() -> Result<Self, Error> {
        Ok(Self {
            storage: TempDir::new()?,
            http: Client::new(),
        })
    }

    pub(crate) async fn download(&self, file: &MirrorFile) -> Result<(), Error> {
        let url = match &file.source {
            Source::Url(url) => url,
            Source::Legacy => bail!("cannot download legacy file {}", file.name),
        };
        eprintln!("downloading {url}...");

        let mut reader = StreamReader::new(
            self.http
                .get(url.clone())
                .send()
                .await?
                .error_for_status()?
                .bytes_stream()
                .map_err(std::io::Error::other),
        );

        let dest = File::create(self.path_for(file)).await?;
        let mut writer = Sha256Writer::new(BufWriter::new(dest));
        tokio::io::copy(&mut reader, &mut writer).await?;

        eprintln!("  -> success! the size is {}", format_size(writer.len));

        let sha256 = to_hex(&writer.sha256);
        if sha256 != file.sha256 {
            bail!(
                "the hash of {} doesn't match (expected {}, downloaded {})",
                url,
                file.sha256,
                sha256
            );
        }

        Ok(())
    }

    pub(crate) async fn get_file_hash(&self, url: &Url) -> Result<Sha256, Error> {
        let mut writer = Sha256Writer::new(tokio::io::sink());
        eprintln!("downloading {url}...");

        let mut reader = StreamReader::new(
            self.http
                .get(url.clone())
                .send()
                .await?
                .error_for_status()?
                .bytes_stream()
                .map_err(std::io::Error::other),
        );
        tokio::io::copy(&mut reader, &mut writer).await?;

        eprintln!("  -> success! the size is {}", format_size(writer.len));
        Ok(writer.sha256)
    }

    pub(crate) fn path_for(&self, file: &MirrorFile) -> PathBuf {
        self.storage.path().join(&file.sha256)
    }
}

fn format_size(size: usize) -> String {
    let mut size = size as f64;
    for unit in ["bytes", "kB", "MB", "GB"] {
        if size / 1000.0 < 1.0 {
            return format!("{size:.2} {unit}");
        }
        size /= 1000.0;
    }
    format!("{size:.2} TB")
}

pub struct Sha256Writer<W: AsyncWrite> {
    sha256: Sha256,
    len: usize,
    writer: Pin<Box<W>>,
}

impl<W: AsyncWrite> Sha256Writer<W> {
    fn new(writer: W) -> Self {
        Self {
            sha256: Sha256::new(),
            len: 0,
            writer: Box::pin(writer),
        }
    }
}

impl<W: AsyncWrite> AsyncWrite for Sha256Writer<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        use sha2::Digest;

        match self.writer.as_mut().poll_write(cx, buf) {
            Poll::Ready(Ok(written)) => {
                self.sha256.update(&buf[..written]);
                self.len += written;
                Poll::Ready(Ok(written))
            }
            other => other,
        }
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        self.writer.as_mut().poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        self.writer.as_mut().poll_shutdown(cx)
    }
}

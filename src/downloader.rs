use crate::manifest::ManifestFile;
use anyhow::{Error, bail};
use reqwest::blocking::Client;
use sha2::{Digest as _, Sha256};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use tempfile::TempDir;

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

    pub(crate) fn download(&self, file: &ManifestFile) -> Result<(), Error> {
        let mut response = self
            .http
            .get(file.source.clone())
            .send()?
            .error_for_status()?;

        let mut writer = Sha256Writer::new(BufWriter::new(File::create(self.path_for(&file))?));
        std::io::copy(&mut response, &mut writer)?;

        let sha256 = to_hex(writer.sha256.finalize().as_slice());
        if sha256 != file.sha256 {
            bail!(
                "the hash of {} doesn't match (expected {}, downloaded {})",
                file.source,
                file.sha256,
                sha256
            );
        }

        Ok(())
    }

    pub(crate) fn path_for(&self, file: &ManifestFile) -> PathBuf {
        self.storage.path().join(&file.sha256)
    }
}

fn to_hex(bytes: &[u8]) -> String {
    let mut result = String::new();
    for byte in bytes {
        result.push_str(&format!("{byte:0<2x}"));
    }
    result
}

struct Sha256Writer<W: Write> {
    sha256: Sha256,
    writer: W,
}

impl<W: Write> Sha256Writer<W> {
    fn new(writer: W) -> Self {
        Self {
            sha256: Sha256::new(),
            writer,
        }
    }
}

impl<W: Write> Write for Sha256Writer<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.writer.write(buf)?;
        self.sha256.update(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

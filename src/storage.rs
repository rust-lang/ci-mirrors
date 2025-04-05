use anyhow::{Error, bail};
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::primitives::ByteStream;
use reqwest::StatusCode;
use std::path::Path;
use tokio::runtime::Runtime;

pub(crate) enum Storage {
    ReadOnly(CdnReader),
    ReadWrite(S3Storage),
}

impl Storage {
    pub(crate) fn file_status(&self, path: &str) -> Result<FileStatus, Error> {
        if let Some(hash) = self.get_file(&format!("{path}.sha256"))? {
            Ok(FileStatus::Present {
                sha256: hash.trim().to_string(),
            })
        } else if self.file_exists(path)? {
            Ok(FileStatus::Legacy)
        } else {
            Ok(FileStatus::Missing)
        }
    }

    pub(crate) fn upload_file(&self, path: &str, file: &Path) -> Result<(), Error> {
        match self {
            Storage::ReadOnly(_) => panic!("unsupported in read-only mode"),
            Storage::ReadWrite(storage) => {
                storage.runtime.block_on(
                    storage
                        .s3
                        .put_object()
                        .bucket(&storage.s3_bucket)
                        .key(path)
                        .body(storage.runtime.block_on(ByteStream::from_path(file))?)
                        .send(),
                )?;
                Ok(())
            }
        }
    }

    pub(crate) fn write_contents(&self, path: &str, content: &[u8]) -> Result<(), Error> {
        match self {
            Storage::ReadOnly(_) => panic!("unsupported in read-only mode"),
            Storage::ReadWrite(storage) => {
                storage.runtime.block_on(
                    storage
                        .s3
                        .put_object()
                        .bucket(&storage.s3_bucket)
                        .key(path)
                        .body(ByteStream::from(content.to_vec()))
                        .send(),
                )?;
                Ok(())
            }
        }
    }

    fn get_file(&self, path: &str) -> Result<Option<String>, Error> {
        match self {
            Storage::ReadOnly(storage) => {
                let url = format!("{}/{path}", storage.cdn_url);
                let response = storage.http.get(&url).send()?;
                match response.status() {
                    StatusCode::OK => Ok(Some(response.text()?)),
                    StatusCode::NOT_FOUND | StatusCode::FORBIDDEN => Ok(None),
                    status => bail!("unexpected status {status} when requesting {url}"),
                }
            }
            Storage::ReadWrite(storage) => {
                let response = storage.runtime.block_on(
                    storage
                        .s3
                        .get_object()
                        .bucket(&storage.s3_bucket)
                        .key(path)
                        .send(),
                );
                match response {
                    Ok(success) => Ok(Some(String::from_utf8(
                        storage.runtime.block_on(success.body.collect())?.to_vec(),
                    )?)),
                    Err(error) => {
                        if let SdkError::ServiceError(service) = &error {
                            if let GetObjectError::NoSuchKey(_) = service.err() {
                                return Ok(None);
                            }
                        }
                        Err(error.into())
                    }
                }
            }
        }
    }

    fn file_exists(&self, path: &str) -> Result<bool, Error> {
        match self {
            Storage::ReadOnly(storage) => {
                let url = format!("{}/{path}", storage.cdn_url);
                let response = storage.http.head(&url).send()?;
                match response.status() {
                    StatusCode::OK => Ok(true),
                    StatusCode::NOT_FOUND | StatusCode::FORBIDDEN => Ok(false),
                    status => bail!("unexpected status {status} when requesting {url}"),
                }
            }
            Storage::ReadWrite(storage) => {
                let response = storage.runtime.block_on(
                    storage
                        .s3
                        .head_object()
                        .bucket(&storage.s3_bucket)
                        .key(path)
                        .send(),
                );
                match response {
                    Ok(_) => Ok(true),
                    Err(error) => {
                        if let SdkError::ServiceError(service) = &error {
                            if let HeadObjectError::NotFound(_) = service.err() {
                                return Ok(false);
                            }
                        }
                        Err(error.into())
                    }
                }
            }
        }
    }
}

pub(crate) struct CdnReader {
    http: reqwest::blocking::Client,
    cdn_url: String,
}

impl CdnReader {
    pub(crate) fn new(cdn_url: String) -> Self {
        Self {
            http: reqwest::blocking::Client::new(),
            cdn_url,
        }
    }
}

pub(crate) struct S3Storage {
    runtime: Runtime,
    s3: aws_sdk_s3::Client,
    s3_bucket: String,
}

impl S3Storage {
    pub(crate) fn new(s3_bucket: String) -> Result<Self, Error> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let config = runtime.block_on(aws_config::load_from_env());
        Ok(S3Storage {
            runtime,
            s3: aws_sdk_s3::Client::new(&config),
            s3_bucket,
        })
    }
}

pub(crate) enum FileStatus {
    Missing,
    Legacy,
    Present { sha256: String },
}

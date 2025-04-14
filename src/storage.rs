use anyhow::{Error, bail};
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::primitives::ByteStream;
use reqwest::StatusCode;
use std::path::Path;

pub(crate) enum Storage {
    ReadOnly(CdnReader),
    ReadWrite(S3Storage),
}

impl Storage {
    pub(crate) async fn file_status(&self, path: &str) -> Result<FileStatus, Error> {
        if let Some(hash) = self.get_file(&format!("{path}.sha256")).await? {
            Ok(FileStatus::Present {
                sha256: hash.trim().to_string(),
            })
        } else if self.file_exists(path).await? {
            Ok(FileStatus::Legacy)
        } else {
            Ok(FileStatus::Missing)
        }
    }

    pub(crate) async fn upload_file(&self, path: &str, file: &Path) -> Result<(), Error> {
        match self {
            Storage::ReadOnly(_) => panic!("unsupported in read-only mode"),
            Storage::ReadWrite(s3) => {
                s3.put_object(path, ByteStream::from_path(file).await?)
                    .await
            }
        }
    }

    pub(crate) async fn write_contents(&self, path: &str, content: &[u8]) -> Result<(), Error> {
        match self {
            Storage::ReadOnly(_) => panic!("unsupported in read-only mode"),
            Storage::ReadWrite(s3) => {
                s3.put_object(path, ByteStream::from(content.to_vec()))
                    .await
            }
        }
    }

    async fn get_file(&self, path: &str) -> Result<Option<String>, Error> {
        match self {
            Storage::ReadOnly(storage) => {
                let url = format!("{}/{}", storage.cdn_url, path.replace("+", "%2B"));
                let response = storage.http.get(&url).send().await?;
                match response.status() {
                    StatusCode::OK => Ok(Some(response.text().await?)),
                    StatusCode::NOT_FOUND | StatusCode::FORBIDDEN => Ok(None),
                    status => bail!("unexpected status {status} when requesting {url}"),
                }
            }
            Storage::ReadWrite(storage) => {
                let response = storage
                    .s3
                    .get_object()
                    .bucket(&storage.s3_bucket)
                    .key(path)
                    .send()
                    .await;
                match response {
                    Ok(success) => Ok(Some(String::from_utf8(
                        success.body.collect().await?.to_vec(),
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

    async fn file_exists(&self, path: &str) -> Result<bool, Error> {
        match self {
            Storage::ReadOnly(storage) => {
                let url = format!("{}/{path}", storage.cdn_url);
                let response = storage.http.head(&url).send().await?;
                match response.status() {
                    StatusCode::OK => Ok(true),
                    StatusCode::NOT_FOUND | StatusCode::FORBIDDEN => Ok(false),
                    status => bail!("unexpected status {status} when requesting {url}"),
                }
            }
            Storage::ReadWrite(storage) => {
                let response = storage
                    .s3
                    .head_object()
                    .bucket(&storage.s3_bucket)
                    .key(path)
                    .send()
                    .await;
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
    http: reqwest::Client,
    cdn_url: String,
}

impl CdnReader {
    pub(crate) fn new(cdn_url: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            cdn_url,
        }
    }
}

pub(crate) struct S3Storage {
    s3: aws_sdk_s3::Client,
    s3_bucket: String,
}

impl S3Storage {
    pub(crate) async fn new(s3_bucket: String) -> Result<Self, Error> {
        let config = aws_config::load_from_env().await;
        Ok(S3Storage {
            s3: aws_sdk_s3::Client::new(&config),
            s3_bucket,
        })
    }

    async fn put_object(&self, key: &str, body: ByteStream) -> Result<(), Error> {
        self.s3
            .put_object()
            .bucket(&self.s3_bucket)
            .key(key)
            .body(body)
            // Prevent overriding an existing file. Note that the IAM policy used to upload
            // objects in CI *enforces* the present of this line. If you remove it without
            // first changing the policy, the request will fail.
            .if_none_match("*")
            .send()
            .await?;
        Ok(())
    }
}

pub(crate) enum FileStatus {
    Missing,
    Legacy,
    Present { sha256: String },
}

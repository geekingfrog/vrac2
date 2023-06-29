use std::{
    io::ErrorKind,
    path::PathBuf,
    pin::Pin,
    task::{Context, Poll},
};

use async_trait::async_trait;
use bytes::Bytes;
use futures::{future::FutureExt, Future};
use pin_project::pin_project;
use serde::{Deserialize, Serialize};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
};

use crate::error::AppError;
use aws_sdk_s3 as s3;
use s3::primitives::{ByteStream, SdkBody};

// The metadata representing an incoming file to be persisted.
// It's a combination of the token information and index of the file since multiple file
// can be uploaded using the same token
// as well as some optional file metadata coming from the uploading client
#[allow(dead_code)]
pub struct InitFile<'token, 'file> {
    pub token_id: i64,
    pub token_path: &'token str,
    pub file_index: u64,
    pub attempt_counter: i64,
    pub mime_type: Option<&'file str>,
    pub file_name: Option<&'file str>,
}

#[async_trait]
pub trait Finalize {
    /// Must be called right after all the bytes have been uploaded, to let
    /// the backend perform any cleanup operation required.
    /// can also optionally return some data to be persisted
    async fn finalize_upload(self: Box<Self>) -> Result<Option<String>, AppError>;
}

pub trait WriteBlob: AsyncWrite + Unpin + Send + Finalize {}
pub trait ReadBlob: AsyncRead + Unpin + Send {}

/// A trait to persist an upload somewhere. That could be on the local
/// file system, in a db as raw bytes, in S3 or whatever.
#[async_trait]
pub trait StorageBackend {
    /// identifier to know which implementation to use when
    /// one wants to manipulate a file.
    fn get_type(&self) -> &'static str;

    /// To be called just before starting to upload a file to the backend.
    /// WriteBlob will use its AsyncWrite operation to persist the data
    /// and the Finalize trait is used to complete the upload
    /// the returned String will be stored in a database and can be used as an handle
    /// to manipulate the blob object.
    async fn initiate_upload(
        &self,
        init_file: &InitFile,
    ) -> Result<(Box<dyn WriteBlob>, String), AppError>;

    async fn delete_blob(&self, blob_raw_data: String) -> Result<(), AppError>;

    async fn read_blob(&self, blob_raw_data: String) -> Result<Box<dyn ReadBlob>, AppError>;
}

pub trait BackendErrorContext<T, E> {
    fn with_context<C, F>(self, f: F) -> Result<T, AppError>
    where
        C: ToString + Send + Sync + 'static,
        F: FnOnce() -> C;
}

impl<T, E> BackendErrorContext<T, E> for Result<T, E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn with_context<C, F>(self, f: F) -> Result<T, AppError>
    where
        C: ToString + Send + Sync + 'static,
        F: FnOnce() -> C,
    {
        self.map_err(|err| AppError::UploadBackendError {
            message: f().to_string(),
            source: Box::new(err),
        })
    }
}

#[derive(Debug, Clone)]
pub struct LocalFsUploader {
    base_path: PathBuf,
    version: u8,
}

impl LocalFsUploader {
    pub fn new<P>(base_path: P) -> Self
    where
        P: Into<PathBuf>,
    {
        Self {
            base_path: base_path.into(),
            version: 0,
        }
    }
}

#[pin_project::pin_project]
pub struct LocalFsBlob {
    #[pin]
    inner: File,
    path: PathBuf,
}

impl ReadBlob for LocalFsBlob {}
impl WriteBlob for LocalFsBlob {}

impl AsyncWrite for LocalFsBlob {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.project().inner.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.project().inner.poll_shutdown(cx)
    }
}

impl AsyncRead for LocalFsBlob {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.project().inner.poll_read(cx, buf)
    }
}

#[async_trait]
impl Finalize for LocalFsBlob {
    async fn finalize_upload(self: Box<Self>) -> Result<Option<String>, AppError> {
        self.inner
            .sync_all()
            .await
            .with_context(|| format!("Cannot sync all to {:?}", &self.path))?;
        Ok(None)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LocalFsData {
    path: PathBuf,
    version: u8,
}

#[async_trait]
impl StorageBackend for LocalFsUploader {
    fn get_type(&self) -> &'static str {
        "local_fs"
    }

    async fn initiate_upload(
        &self,
        init_file: &InitFile,
    ) -> Result<(Box<dyn WriteBlob>, String), AppError> {
        let mut path = self.base_path.clone();
        path.push(format!(
            "{}_{:02}_{:03}",
            init_file.token_id, init_file.attempt_counter, init_file.file_index
        ));

        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(&path)
            .await
            .with_context(|| format!("Cannot save file to {:?}", &path))?;
        Ok((
            Box::new(LocalFsBlob {
                inner: file,
                path: path.clone(),
            }),
            serde_json::to_string(&LocalFsData {
                path,
                version: self.version,
            })?,
        ))
    }

    async fn delete_blob(&self, blob_raw_data: String) -> Result<(), AppError> {
        let blob_data: LocalFsData = serde_json::from_str(&blob_raw_data)?;
        match fs::remove_file(&blob_data.path).await {
            Ok(_) => Ok(()),
            Err(err) => {
                // trying to delete something that doesn't exist isn't fatal.
                if err.kind() == std::io::ErrorKind::NotFound {
                    tracing::warn!("Blob not found at path {:?}", blob_data.path);
                    Ok(())
                } else {
                    Err(err).with_context(|| format!("Cannot delete file at {:?}", &blob_data.path))
                }
            }
        }
    }

    async fn read_blob(&self, blob_raw_data: String) -> Result<Box<dyn ReadBlob>, AppError> {
        let blob_data: LocalFsData = serde_json::from_str(&blob_raw_data)?;
        let file = fs::File::open(&blob_data.path)
            .await
            .with_context(|| format!("Cannot open file at {:?}", blob_data.path))?;
        Ok(Box::new(LocalFsBlob {
            inner: file,
            path: blob_data.path,
        }))
    }
}

#[derive(Debug, Clone)]
pub struct GarageUploader {
    client: s3::Client,
    bucket: String,
}

impl GarageUploader {
    pub async fn new() -> Result<Self, AppError> {
        let endpoint_url = "http://localhost:3900";
        let bucket = "vrac".to_string();
        let builder: s3::config::Builder = (&aws_config::from_env()
            .endpoint_url(endpoint_url)
            .load()
            .await)
            .into();
        let config = builder.force_path_style(true).build();
        let client = s3::Client::from_conf(config);
        Ok(Self { client, bucket })
    }
}

#[async_trait]
impl StorageBackend for GarageUploader {
    // type WriteBlob = GarageWriteBlob;
    // type ReadBlob = GarageReadBlob;
    // type Data = GarageData;

    fn get_type(&self) -> &'static str {
        "garage"
    }

    async fn initiate_upload(
        &self,
        init_file: &InitFile,
    ) -> Result<(Box<dyn WriteBlob>, String), AppError> {
        let (send_chan, channel_body) = hyper::body::Body::channel();
        let key = format!(
            "{}_{:02}_{:03}",
            init_file.token_id, init_file.attempt_counter, init_file.file_index
        );

        let stream = ByteStream::new(SdkBody::from(channel_body));
        let request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key.clone())
            .body(stream)
            .set_content_type(init_file.mime_type.map(str::to_string));

        let send_future = request.send().map(|res| match res {
            Ok(_) => Ok(()),
            Err(err) => {
                tracing::error!("Cannot send request to garage: {err:?}");
                Err(ErrorKind::Other.into())
            }
        });

        let data = GarageData {
            bucket: self.bucket.clone(),
            key,
        };

        let blob = GarageWriteBlob {
            send_chan: Some(send_chan),
            send_future: Box::pin(send_future),
        };

        Ok((Box::new(blob), serde_json::to_string(&data)?))
    }

    async fn delete_blob(&self, blob_raw_data: String) -> Result<(), AppError> {
        let blob_data: GarageData = serde_json::from_str(&blob_raw_data)?;

        self.client
            .delete_object()
            .bucket(blob_data.bucket)
            .key(blob_data.key)
            .send()
            .await?;
        Ok(())
    }

    async fn read_blob(&self, blob_raw_data: String) -> Result<Box<dyn ReadBlob>, AppError> {
        let blob_data: GarageData = serde_json::from_str(&blob_raw_data)?;
        let response = self
            .client
            .get_object()
            .bucket(blob_data.bucket)
            .key(blob_data.key)
            .send()
            .await?;

        let blob = GarageReadBlob {
            body: Box::new(BufReader::new(response.body.into_async_read())),
        };

        Ok(Box::new(blob) as _)
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GarageData {
    bucket: String,
    key: String,
}

#[pin_project]
pub struct GarageWriteBlob {
    #[pin]
    send_chan: Option<hyper::body::Sender>,
    /// the future holding the s3 request.send()
    send_future: Pin<Box<dyn Future<Output = std::io::Result<()>> + Send + 'static>>,
}

// trait Foo: Send + Sync {}
// impl Foo for GarageWriteBlob {}

impl WriteBlob for GarageWriteBlob {}

impl AsyncWrite for GarageWriteBlob {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        tracing::trace!(
            "asyncwrite called for GarageWriteBlob with a buffer of length {}",
            buf.len()
        );

        // first, attempt to drive the future sending stuff to garage
        match self.send_future.poll_unpin(cx) {
            // when that fails, we abort everything
            Poll::Ready(Err(err)) => {
                tracing::error!("ERROR ! {err:?}");
                if let Some(chan) = self.send_chan.take() {
                    chan.abort();
                }
                return Poll::Ready(Err(err));
            }
            x => {
                tracing::info!("result of polling send_future: {x:?}");
            }
        }

        let this = self.project();

        tracing::trace!("starting to shove bytes into the SdkBody");
        if let Some(mut chan) = this.send_chan.as_pin_mut() {
            let mut chunk = Bytes::copy_from_slice(buf);
            loop {
                futures::ready!(chan.poll_ready(cx))
                    .map_err(|err| std::io::Error::new(ErrorKind::Other, err))?;

                let len = chunk.len();
                tracing::trace!("Sending {} bytes to the streaming body", len);
                match chan.try_send_data(chunk) {
                    Ok(_) => break Poll::Ready(Ok(len)),
                    Err(c) => chunk = c,
                }
            }
        } else {
            // this branch should never be taken really.
            // that would mean poll_write was called again after we returned a
            // Poll::Ready(Err(…)), which the only way we unset the option
            tracing::error!("send_chan has been aborted but poll_write has been called again");
            Poll::Ready(Err(ErrorKind::Other.into()))
        }
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        // remove the channel from the option, and drop it, so that it gets closed and
        // EOF will be sent to the body
        self.send_chan.take();
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        self.send_future.poll_unpin(cx)
    }
}

#[async_trait]
impl Finalize for GarageWriteBlob {
    async fn finalize_upload(mut self: Box<Self>) -> Result<Option<String>, AppError> {
        self.flush().await?;
        self.shutdown().await?;
        Ok(None)
    }
}

#[pin_project]
pub struct GarageReadBlob {
    #[pin]
    body: Box<dyn AsyncRead + Unpin + Send + Sync>,
}

impl ReadBlob for GarageReadBlob {}

impl AsyncRead for GarageReadBlob {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.project().body.poll_read(cx, buf)
    }
}

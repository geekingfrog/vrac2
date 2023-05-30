use std::{
    path::PathBuf,
    pin::Pin,
    task::{Context, Poll},
};

use async_trait::async_trait;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::fs::{self, File, OpenOptions};

use crate::error::AppError;

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

/// A trait to persist an upload somewhere. That could be on the local
/// file system, in a db as raw bytes, in S3 or whatever.
#[async_trait]
pub trait StorageBackend
where
    Self::Blob: Send + tokio::io::AsyncWrite + tokio::io::AsyncRead,
    Self::Data: Serialize + DeserializeOwned,
{
    /// An internal type that can be used to carry information
    /// between starting and finalizing the upload. For example,
    /// marking the transfer as completed in a metadata service, or
    /// finalizing a multipart upload to S3
    /// The AsyncWrite method will be used to store the actual data
    /// The AsyncRead should be used to return what was stored
    type Blob;

    /// Some datatype to be persisted to the DB
    /// This should be used to store anything that's required to retrieve the
    /// stored blob later on.
    type Data;

    /// identifier to know which implementation to use when
    /// one wants to manipulate a file.
    fn get_type(&self) -> &'static str;

    /// To be called just before starting to upload a file to the backend.
    /// Self::Blob will use its AsyncWrite operation to persist the data
    /// Self::Data will be stored in a database and can be used as an handle
    /// to manipulate the Self::Blob object.
    async fn initiate_upload(
        &self,
        init_file: &InitFile,
    ) -> Result<(Self::Blob, Self::Data), AppError>;

    /// Must be called right after all the bytes have been uploaded, to let
    /// the backend perform any cleanup operation required.
    /// can also optionally return some data to be persisted
    async fn finalize_upload(&self, _blob: Self::Blob) -> Result<Option<Self::Data>, AppError> {
        Ok(None)
    }

    async fn delete_blob(&self, blob_data: Self::Data) -> Result<(), AppError>;

    async fn read_blob(&self, blob_data: Self::Data) -> Result<Self::Blob, AppError>;
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

impl tokio::io::AsyncWrite for LocalFsBlob {
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

impl tokio::io::AsyncRead for LocalFsBlob {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.project().inner.poll_read(cx, buf)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LocalFsData {
    path: PathBuf,
    version: u8,
}

#[async_trait]
impl StorageBackend for LocalFsUploader {
    type Blob = LocalFsBlob;
    type Data = LocalFsData;

    fn get_type(&self) -> &'static str {
        "local_fs"
    }

    async fn initiate_upload(
        &self,
        init_file: &InitFile,
    ) -> Result<(LocalFsBlob, LocalFsData), AppError> {
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
            LocalFsBlob {
                inner: file,
                path: path.clone(),
            },
            LocalFsData {
                path,
                version: self.version,
            },
        ))
    }

    async fn finalize_upload(&self, blob: Self::Blob) -> Result<Option<Self::Data>, AppError> {
        blob.inner
            .sync_all()
            .await
            .with_context(|| format!("Cannot sync all to {:?}", &blob.path))?;
        Ok(None)
    }

    async fn delete_blob(&self, blob_data: Self::Data) -> Result<(), AppError> {
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

    async fn read_blob(&self, blob_data: Self::Data) -> Result<Self::Blob, AppError> {
        let file = fs::File::open(&blob_data.path)
            .await
            .with_context(|| format!("Cannot open file at {:?}", blob_data.path))?;
        Ok(LocalFsBlob {
            inner: file,
            path: blob_data.path,
        })
    }
}
use std::{
    path::{Path, PathBuf},
    pin::Pin,
    task::{Context, Poll},
};

use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
// use futures::AsyncWrite;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::AsyncWrite;

use crate::error::AppError;

pub(crate) struct InitFile<'token, 'file> {
    pub(crate) token_id: i64,
    pub(crate) token_path: &'token str,
    pub(crate) attempt_counter: i64,
    pub(crate) mime_type: Option<&'file str>,
    pub(crate) file_name: Option<&'file str>,
}

/// A trait to persist an upload somewhere. That could be on the local
/// file system, in a db as raw bytes, in S3 or whatever.
#[async_trait]
pub(crate) trait StorageBackend
where
    Self::Blob: Send + futures::AsyncWrite,
    Self::Data: Serialize + DeserializeOwned,
{
    /// An internal type that can be used to carry information
    /// between starting and finalizing the upload. For example,
    /// marking the transfer as completed in a metadata service, or
    /// finalizing a multipart upload to S3
    type Blob;

    /// Some datatype to be persisted to the DB
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
    async fn finalize_upload(&self, blob: Self::Blob) -> Result<(), AppError> {
        Ok(())
    }

    async fn delete_blob(&self, blob_data: Self::Data) -> Result<(), AppError>;
}

pub(crate) trait BackendErrorContext<T, E> {
    // fn with_context(self, message: String) -> Result<T, AppError>;
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

pub(crate) struct LocalFsUploader {
    base_path: PathBuf,
}

#[pin_project::pin_project]
pub(crate) struct LocalFsBlob {
    #[pin]
    inner: File,
    path: PathBuf,
}

impl futures::AsyncWrite for LocalFsBlob {
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

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.project().inner.poll_shutdown(cx)
    }
}

#[async_trait]
impl StorageBackend for LocalFsUploader {
    type Blob = LocalFsBlob;
    type Data = PathBuf;

    fn get_type(&self) -> &'static str {
        "local_fs"
    }

    async fn initiate_upload(
        &self,
        init_file: &InitFile,
    ) -> Result<(LocalFsBlob, PathBuf), AppError> {
        let mut path = self.base_path.clone();
        path.push(format!(
            "{}_{:03}",
            init_file.token_id, init_file.attempt_counter
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
            path,
        ))
    }

    async fn finalize_upload(&self, blob: Self::Blob) -> Result<(), AppError> {
        blob.inner
            .sync_all()
            .await
            .with_context(|| format!("Cannot sync all to {:?}", &blob.path))
    }

    async fn delete_blob(&self, blob_data: Self::Data) -> Result<(), AppError> {
        fs::remove_file(&blob_data)
            .await
            .with_context(|| format!("Cannot delete file at {:?}", &blob_data))?;
        Ok(())
    }
}

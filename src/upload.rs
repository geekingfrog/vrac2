use std::{
    path::{Path, PathBuf},
    pin::Pin,
    task::{Context, Poll},
};

use async_trait::async_trait;
// use futures::AsyncWrite;
use tokio::fs::File;
use tokio::io::AsyncWrite;

use crate::error::AppError;

pub(crate) struct InitFile<'token, 'file> {
    pub(crate) token_id: i64,
    pub(crate) token_path: &'token str,
    pub(crate) mime_type: Option<&'file str>,
    pub(crate) file_name: Option<&'file str>,
}

/// A trait to persist an upload somewhere. That could be on the local
/// file system, in a db as raw bytes, in S3 or whatever.
#[async_trait]
pub(crate) trait Uploader
where
    Self::Blob: Send + AsyncWrite,
{
    /// An internal type that can be used to carry information
    /// between starting and finalizing the upload. For example,
    /// marking the transfer as completed in a metadata service, or
    /// finalizing a multipart upload to S3
    type Blob;

    /// identifier to know which implementation to use when
    /// one wants to manipulate a file.
    fn get_type(&self) -> &'static str;

    /// To be called just before starting to upload a file to the backend.
    async fn initiate_upload(&self, init_file: &InitFile) -> Result<Self::Blob, AppError>;

    /// Must be called right after all the bytes have been uploaded, to let
    /// the backend perform any cleanup operation required.
    async fn finalize_upload(&self, blob: Self::Blob) -> Result<(), AppError>;
}


// for posterity, if/when going into conflict of AsyncRead/Write definition.
// tokio provides io::File, but doesn't provide Stream::into_async_read()
// that is used to copy the incoming bytestream for a field into an
// AsyncWriter provided by a backend
// impl<T> tokio_io::AsyncRead for Compat<T>
// where
//     T: futures_io::AsyncRead,
// {
//     fn poll_read(
//         self: Pin<&mut Self>,
//         cx: &mut task::Context,
//         buf: &mut [u8],
//     ) -> Poll<io::Result<usize>> {
//         futures_io::AsyncRead::poll_read(self.project().inner, cx, buf)
//     }
// }

pub(crate) struct LocalFsUploader {
    base_path: PathBuf,
}

pub(crate) struct LocalFsBlob(File);

// impl<T: ?Sized + AsyncWrite + Unpin> AsyncWrite for Box<T> {

impl AsyncWrite for LocalFsBlob {
    fn poll_write(
        mut self: Pin<&mut Self>,
        mut cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(&mut cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, mut cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(&mut cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, mut cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(&mut cx)
    }
}

#[async_trait]
impl Uploader for LocalFsUploader {
    type Blob = LocalFsBlob;

    fn get_type(&self) -> &'static str {
        "local_fs"
    }

    async fn initiate_upload(&self, init_file: &InitFile) -> Result<LocalFsBlob, AppError> {
        todo!()
    }

    async fn finalize_upload(&self, blob: LocalFsBlob) -> Result<(), AppError> {
        todo!()
    }
}

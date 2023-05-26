use async_zip::tokio::write::ZipFileWriter;
use async_zip::{base::write::owned_writer, Compression, ZipEntry, ZipEntryBuilder};
use bytes::{Buf, BufMut, BytesMut};
use futures::TryStreamExt;
use pin_project::pin_project;
use std::error::Error;
use std::iter::IntoIterator;
use std::pin::Pin;
use std::task::Poll;
use tokio::fs::File;
// use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use futures::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use vrac::{db::DBService, state::AppState, upload::StorageBackend};

type BoxResult<T> = Result<T, Box<dyn Error>>;

/// adapter which wraps a given number of (ZipEntry, AsyncRead) to create
/// an AsyncRead that will yield the bytes of the compressed archive of all
/// these entries.
#[pin_project]
struct ZipWriter {
    zip_writer: ZipFileWriter<Vec<u8>>,
    // entries: std::vec::IntoIter<(ZipEntry, File)>,
    #[pin]
    entry: ZipEntry,
    #[pin]
    reader: Compat<File>,
    buf: BytesMut,
}

impl AsyncRead for ZipWriter {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        let mut this = self.project();
        let mut zip_buf = vec![0; 4096];

        match Pin::new(&mut this.reader).poll_read(cx, &mut zip_buf) {
            Poll::Ready(Ok(n)) => {
                if n == 0 {
                    // We reached EOF on this reader
                    todo!()
                } else {
                    // this.entry.as_mut().poll_write(cx, &zip_buf[0..n]);
                }
                todo!()
            }
            x => return x,
        };

        todo!()
    }
}

impl ZipWriter {
    async fn new() -> BoxResult<Self> {
        Ok(ZipWriter {
            zip_writer: ZipFileWriter::new(Vec::with_capacity(4096).compat_write()),
            entry: ZipEntryBuilder::new("coucou.txt".into(), Compression::Deflate).build(),
            reader: File::open("./testfiles/dt131103.jpg").await?.compat(),
            buf: BytesMut::with_capacity(4096),
        })

        // Ok(ZipWriter {
        //     zip_writer: ZipFileWriter::new(Vec::with_capacity(4096).compat_write()),
        //     entries: vec![(
        //         ZipEntryBuilder::new("coucou.txt".into(), Compression::Deflate).build(),
        //         File::open("./testfiles/dt131103.jpg").await?,
        //     )]
        //     .into_iter(),
        // })
    }
}

#[tokio::main]
async fn main() -> BoxResult<()> {
    // let mut zw = ZipWriter::new().await?;
    // let x = zw.entries.next();

    let state = AppState::new("templates/**/*.html", "./test.sqlite", "./vracfiles")
        .await
        .expect("appstate");
    let db = state.db;
    let storage = state.storage_fs;
    let files = db.get_files(24, 1).await?;

    let mut output_file = tokio::fs::File::create("test_output.zip").await?;
    let mut zip_writer = ZipFileWriter::with_tokio(&mut output_file);

    // for file in files {
    //     dbg!(&file);
    //     let fs_data = serde_json::from_str(&file.backend_data)?;
    //     let data = storage.read_blob(fs_data).await?;
    //     let mut data = futures::io::BufReader::new(data.compat());
    //     let filename = format!("testing/{}", file.name.unwrap());
    //     let entry = ZipEntryBuilder::new(filename.into(), Compression::Deflate).build();
    //     let mut entry_writer = zip_writer.write_entry_stream(entry).await?;
    //     futures::io::copy_buf(&mut data, &mut entry_writer).await?;
    //     entry_writer.close().await?;
    // }
    // zip_writer.close().await?;

    let entries = vec![
        (
            ZipEntryBuilder::new("dilbert.gif".into(), Compression::Deflate).build(),
            "testfiles/dilbert.gif",
        ),
        (
            ZipEntryBuilder::new("Suprise [fqyjOc3EpT4].webm".into(), Compression::Deflate).build(),
            "testfiles/Suprise [fqyjOc3EpT4].webm",
        ),
    ];

    let mut zip_writer = owned_writer::ZipWriterArchive::new(output_file.compat_write());
    let mut entry_writer;

    for (entry, path) in entries {
        let input_file = File::open(path).await?;
        entry_writer = zip_writer.write_entry_stream(entry).await?;
        futures::io::copy(input_file.compat(), &mut entry_writer).await?;
        zip_writer = entry_writer.close().await?;
    }
    zip_writer.close().await?;

    Ok(())
}

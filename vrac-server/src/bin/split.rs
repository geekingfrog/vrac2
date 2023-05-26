use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use async_zip::{
    base::write::{self, owned_writer, EntryStreamWriter, ZipFileWriter},
    Compression, ZipEntryBuilder,
};
use futures::io::{copy, copy_buf, BufReader, Cursor};
use futures::{io::AsyncWriteExt, AsyncBufRead, AsyncRead, AsyncWrite};
use pin_project::pin_project;
use tokio::{fs::File, io::DuplexStream};
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;

#[pin_project]
struct TestPipe<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> {
    #[pin]
    writer: W,
    #[pin]
    reader: R,
    // #[pin]
    // buf_reader: BufReader<Compat<DuplexStream>>,
    // #[pin]
    // buf_writer: Compat<DuplexStream>,
    // has_eof: bool,
}

impl<R, W> TestPipe<R, W>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    fn new(reader: R, writer: W) -> Self {
        let (buf_reader, buf_writer) = tokio::io::duplex(3);
        // let buf_reader = BufReader::new(buf_reader.compat());
        // let buf_writer = buf_writer.compat_write();
        Self {
            writer,
            reader,
            // buf_reader,
            // buf_writer,
            // has_eof: false,
        }
    }
}

impl<R, W> futures::Future for TestPipe<R, W>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        // let buffer = futures::ready!(this.buf_reader.as_mut().poll_fill_buf(cx))?;
        let buffer = futures::ready!(this.reader.as_mut().poll_fill_buf(cx))?;

        if buffer.is_empty() {
            futures::ready!(this.writer.as_mut().poll_flush(cx))?;
            return Poll::Ready(Ok(()));
        }

        let i = futures::ready!(Pin::new(&mut this.writer).poll_write(cx, buffer))?;
        if i == 0 {
            return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
        }
        this.reader.as_mut().consume(i);
        println!("poll pending");
        Poll::Pending
    }
}

#[tokio::main]
async fn main() -> BoxResult<()> {
    let mut output = tokio::io::stdout().compat_write();
    let raw = "coucou1\n".to_string();
    let input = Cursor::new(raw.as_bytes());
    let (wrt, rdr) = tokio::io::duplex(3);
    let wrt = wrt.compat();
    let mut pipe = TestPipe {
        writer: wrt,
        reader: input,
    };

    let x = tokio::join!(
        async move {
            pipe.await.expect("coucou");

            // TestPipe {
            //     writer: wrt,
            //     reader: input,
            // }
            // .await
            // .expect("copy input into duplex");

            // copy_buf(input, &mut wrt).await
        },
        async move { copy(rdr.compat(), &mut output).await },
    );

    println!("{:?}", x);

    Ok(())
}

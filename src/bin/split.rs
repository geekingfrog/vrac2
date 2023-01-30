use futures::{Future, FutureExt};
use pin_project::pin_project;
use std::{
    error::Error,
    path::{Path, PathBuf},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

type IoResult<T> = std::io::Result<T>;

macro_rules! ready {
    ($e:expr $(,)?) => {
        match $e {
            std::task::Poll::Ready(t) => t,
            std::task::Poll::Pending => return std::task::Poll::Pending,
        }
    };
}

enum WriteState {
    NotStarted,
    Opening {
        inner: Pin<Box<dyn Future<Output = IoResult<File>>>>,
    },
    // Writing {
    //     inner: Pin<Box<File>>,
    //     written_so_far: usize,
    // },
    // Writing { foo: usize },
}

#[pin_project]
struct SplitWriter {
    base_path: PathBuf,
    idx: usize,
    /// the number of bytes to write to each file before moving to the next
    limit: usize,
    #[pin]
    write_state: WriteState,
}

impl SplitWriter {
    fn new(base_path: PathBuf, limit: usize) -> Self {
        Self {
            base_path,
            idx: 0,
            limit,
            write_state: WriteState::NotStarted,
        }
    }
}

impl AsyncWrite for SplitWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        // let me = self.project();
        // match self.write_state {
        //     WriteState::NotStarted => self.write_state = WriteState::Writing { foo: 10 },
        //     WriteState::Writing { foo: _ } => self.write_state = WriteState::NotStarted,
        // }
        // Poll::Ready(Ok(0))

        match self.write_state {
            WriteState::NotStarted => {
                let mut open = File::open("/tmp/coucou").boxed(); // as dyn Future<Output = _>;

                self.write_state = WriteState::Opening { inner: open };

                // let mut f = ready!(open.poll_unpin(cx))?;
                // let to_write = std::cmp::max(buf.len(), self.limit);
                // let wrote = ready!(Pin::new(&mut f).poll_write(cx, &buf[..to_write]))?;
                // self.write_state = WriteState::Writing {
                //     inner: Box::pin(f),
                //     written_so_far: wrote,
                // };
                // Poll::Ready(Ok(wrote))
            }
            WriteState::Opening { inner: _ } => {
                todo!()
            }
        };

        match &self.get_mut().write_state {
            WriteState::NotStarted => todo!(),
            WriteState::Opening { ref mut inner } => {
                inner.poll_unpin(cx);
                todo!()
            },
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        todo!()
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        todo!()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut input = File::open("./storage/5M.data").await?;
    let mut output = File::open("/tmp/5M.data").await?;
    tokio::io::copy(&mut input, &mut output).await?;
    Ok(())
}

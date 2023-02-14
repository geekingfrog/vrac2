use std::{error::Error, path::PathBuf};
use tokio::{fs::File, sync::mpsc};
use tokio::{
    fs::OpenOptions,
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
};

type IoResult<T> = std::io::Result<T>;

#[derive(Debug)]
enum SplitStream {
    ChunkStart,
    ChunkEnd,
    Chunk(Box<[u8]>),
}

async fn read_and_split<R>(rdr: R, limit: u64, out_chan: mpsc::Sender<SplitStream>) -> IoResult<()>
where
    R: AsyncRead + Unpin,
{
    use SplitStream::*;
    let mut is_new_chunk = true;
    let mut rdr = rdr.take(limit);
    let mut first_empty_read = true;
    loop {
        let mut buf = [0_u8; 1024];
        let n = rdr.read(&mut buf).await?;
        if n == 0 {
            if first_empty_read {
                first_empty_read = false;
                out_chan
                    .send(ChunkEnd)
                    .await
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                rdr = rdr.into_inner().take(limit);
                is_new_chunk = true;
                continue;
            } else {
                break Ok(());
            }
        } else {
            first_empty_read = true;
            if is_new_chunk {
                out_chan
                    .send(ChunkStart)
                    .await
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                is_new_chunk = false;
            }
            out_chan
                .send(Chunk(Box::new(buf)))
                .await
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        }
    }
}

struct FileSplitWriter {
    limit: u64,
    dest: Option<(PathBuf, File)>,
}

impl FileSplitWriter {
    fn new(limit: u64) -> Self {
        Self { limit, dest: None }
    }

    async fn create_dest(&mut self, index: usize) -> IoResult<()> {
        let path = PathBuf::from(format!("/tmp/split_{:04}", index));
        println!("creating {path:?}");
        let file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(&path)
            .await?;
        self.dest = Some((path, file));
        Ok(())
    }

    async fn split_write(
        &mut self,
        mut receiver: mpsc::Receiver<SplitStream>,
    ) -> IoResult<Vec<PathBuf>> {
        let mut index = 0;
        let mut results = vec![];

        while let Some(chunk) = receiver.recv().await {
            use SplitStream::*;
            match chunk {
                ChunkStart => {
                    self.create_dest(index).await?;
                }
                ChunkEnd => {
                    index += 1;
                    let (path, mut dest) = self.dest.take().expect("Destination isn't initialized");
                    dest.flush().await?;
                    results.push(path);
                }
                Chunk(chunk) => {
                    let (_path, dest) = self.dest.as_mut().expect("Destination isn't initialized");
                    dest.write_all(&chunk).await?;
                }
            }
        }

        Ok(results)
    }

    async fn copy<R>(mut self, rdr: R) -> IoResult<Vec<PathBuf>>
    where
        R: AsyncRead + Unpin,
    {
        let (sender, receiver) = mpsc::channel(10);

        let (_, paths) = tokio::try_join!(
            read_and_split(rdr, self.limit, sender),
            self.split_write(receiver)
        )?;

        Ok(paths)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let input = File::open("./storage/5M.data").await?;
    println!("input done");

    let mib = 1024 * 1024;
    let paths = FileSplitWriter::new(2 * mib).copy(input).await?;
    for path in paths {
        println!("{path:?}");
    }

    Ok(())
}

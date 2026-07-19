#![allow(unused_imports)]
use async_zip::{Compression, ZipEntryBuilder};
use scrypt::password_hash::PasswordHasher;
use scrypt::{phc::PasswordHash, Scrypt};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio_util::compat::{
    Compat, FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt,
};
use vrac::handlers::gen::{GenTokenForm, StorageBackendType};

type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> BoxResult<()> {
    setup().await?;

    let (rdr, wrt) = tokio::io::simplex(4096 * 2);

    let handle = tokio::spawn(async move {
        match create_archive(wrt).await {
            Ok(_) => println!("done creating archive"),
            Err(err) => println!("error creating archive {err:?}"),
        }
    });

    let archive = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open("/tmp/archive.zip")
        .await?;

    futures::io::copy(rdr.compat(), &mut archive.compat()).await?;

    handle.await?;

    Ok(())
}

async fn setup() -> BoxResult<()> {
    let mut f1 = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open("/tmp/file1.txt")
        .await?;
    f1.write_all("file1\ncoucou\n".as_bytes()).await?;
    f1.flush().await?;

    let mut f2 = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open("/tmp/file2.txt")
        .await?;
    f2.write_all("file2\nblah moo\n".as_bytes()).await?;
    f2.flush().await?;

    Ok(())
}

async fn create_archive<W>(mut wrt: W) -> BoxResult<()>
where
    W: AsyncWrite + Unpin,
{
    let mut zip_wrt = async_zip::base::write::ZipFileWriter::with_tokio(&mut wrt);

    for filename in ["file1.txt", "file2.txt"] {
        let f = File::open(&format!("/tmp/{filename}")).await?;
        let opts = ZipEntryBuilder::new(filename.into(), Compression::Deflate);
        let mut entry = zip_wrt.write_entry_stream(opts).await?;
        let bytes = futures::io::copy(f.compat(), &mut entry).await?;
        entry.close().await?;
        eprintln!("done writing {bytes} bytes to entry {:?}", filename);
    }

    zip_wrt.close().await?;
    wrt.shutdown().await?;

    Ok(())
}

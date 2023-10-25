/// Temporary binary to fill missing metadata for existing files
use anyhow::Context;
use clap::Parser;
use sqlx::{sqlite::SqlitePoolOptions, Executor};
use vrac::state::AppState;

#[derive(Debug, Parser)]
struct Args {
    #[arg(long, default_value = "./test.sqlite")]
    sqlite_path: String,

    #[arg(long, default_value = "/tmp/vrac/")]
    storage_path: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    let state = AppState::new(
        "templates/**/*.html",
        &args.sqlite_path,
        &args.storage_path,
        "useless".to_string(),
    )
    .await
    .context("cannot construct app state")?;

    state.db.migrate().await?;

    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .after_connect(|conn, _meta| {
            // sqlite doesn't allow multiple writer at the same time
            // Since this application is basically going to do nothing most of the time
            // a chance of a write collision is incredibly unlikely.
            // If that ever happens, we get a database locked error but :shrug:
            // So only bother to allow multiple read tx to be execute alongside at most
            // one write transaction.
            // See https://www.sqlite.org/wal.html
            Box::pin(async move {
                conn.execute("PRAGMA journal_mode=WAL;").await?;
                Ok(())
            })
        })
        .connect(&args.sqlite_path)
        .await?;

    let to_fix = sqlx::query_as::<_, (i64, String, String, Option<String>)>("select f.id, f.backend_type, f.backend_data, f.mime_type from file as f where not exists (select file_id from file_metadata as m where m.file_id = f.id)")
        .fetch_all(&pool)
        .await?;

    tracing::info!(
        "number of file to fix for missing metadata: {}",
        to_fix.len()
    );
    for (file_id, typ, data, mime_type) in to_fix {
        tracing::info!("stuff to fix: {:?} - {} - {}", mime_type, typ, data);
        let mut blob = state.get_blob(&typ, data).await?;
        let mut sink = tokio::io::sink();
        let size_b = tokio::io::copy(&mut blob, &mut sink).await?;
        let size_b: i64 = size_b.try_into()?;
        tracing::info!("file {file_id} got size: {size_b}");
        sqlx::query("INSERT INTO file_metadata (file_id, size_b, mime_type) VALUES (?, ?, ?)")
            .bind(file_id)
            .bind(size_b)
            .bind(mime_type)
            .execute(&pool)
            .await
            .with_context(|| {
                format!(
                    "error writing metadata for file upload with file_id {}",
                    file_id
                )
            })?;
    }

    Ok(())
}

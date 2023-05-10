use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

use axum::Router;
use clap::Parser;
use vrac::{app::build, state::AppState};

#[derive(Parser, Debug)]
#[command(version)]
struct Cli {
    #[arg(long, default_value = "./test.sqlite")]
    sqlite_path: String,

    #[arg(long, default_value = "/tmp/vrac/")]
    storage_path: String,

    #[arg(long, default_value_t = 8000)]
    port: u16,

    #[arg(long, default_value = "127.0.0.1")]
    bind_address: String,
}

#[tokio::main]
async fn main() -> Result<(), axum::BoxError> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    tracing::info!("Local fs for storage at {}", cli.storage_path);
    tokio::fs::create_dir_all(&cli.storage_path).await?;

    tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&cli.sqlite_path)
        .await?;

    let state = AppState::new("templates/**/*.html", &cli.sqlite_path, &cli.storage_path).await?;
    state.db.migrate().await?;

    let addr = IpAddr::from_str(&cli.bind_address)?;
    let addr = SocketAddr::from((addr, cli.port));
    let app = build(state.clone());

    tokio::try_join!(
        webserver(addr, app),
        background_cleanup(&state.db, &state.storage_fs)
    )?;

    Ok(())
}

async fn webserver(addr: SocketAddr, app: Router) -> Result<(), axum::BoxError> {
    tracing::info!("Listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;
    Ok(())
}

async fn background_cleanup(
    db: &vrac::db::DBService,
    storage_fs: &vrac::upload::LocalFsUploader,
) -> Result<(), axum::BoxError> {
    loop {
        vrac::cleanup::cleanup(&db, &storage_fs).await?;
        tokio::time::sleep(std::time::Duration::from_secs(60 * 5)).await;
    }
}

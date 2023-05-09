use std::net::SocketAddr;

use axum::Router;
use vrac::{app::build, state::AppState};

#[tokio::main]
async fn main() -> Result<(), axum::BoxError> {
    tracing_subscriber::fmt::init();
    let storage_path = "/tmp/vrac";
    tokio::fs::create_dir_all(storage_path).await?;

    let db_path = "test.sqlite";
    tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(db_path)
        .await?;

    let state = AppState::new("templates/**/*.html", db_path, storage_path).await?;
    state.db.migrate().await?;

    // let app = build(state.clone());
    let addr = SocketAddr::from(([127, 0, 0, 1], 8000));
    let app = build(state.clone());
    // tracing::info!("Listening on {}", addr);
    // axum::Server::bind(&addr)
    //     .serve(app.into_make_service())
    //     .await?;

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

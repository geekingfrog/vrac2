use std::net::SocketAddr;

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
    let app = build(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 8000));
    tracing::info!("Listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;
    Ok(())
}

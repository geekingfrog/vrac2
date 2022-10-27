use std::net::SocketAddr;

use vrac::{app::build, state::AppState};

#[tokio::main]
async fn main() -> Result<(), axum::BoxError> {
    tracing_subscriber::fmt::init();
    let state = AppState::new("templates/**/*.html", "test.sqlite").await?;
    state.db.migrate().await?;
    let app = build(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 8000));
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;
    Ok(())
}

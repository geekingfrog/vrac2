use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::{routing, Router};
use tower::ServiceBuilder;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::handlers;
use crate::state::AppState;

pub fn build(state: AppState) -> Router<()> {
    let service = ServiceBuilder::new().layer(TraceLayer::new_for_http());
    Router::new()
        .layer(service)
        .route(
            "/gen",
            routing::get(handlers::gen::get_token).post(handlers::gen::create_token),
        )
        .merge(
            Router::new()
                .route(
                    "/f/:path",
                    routing::get(handlers::upload::get_upload_form)
                        .post(handlers::upload::post_upload_form),
                )
                .layer(DefaultBodyLimit::max(usize::MAX))
                .with_state(state.clone()),
        )
        .nest_service(
            "/static",
            routing::get_service(ServeDir::new("static")).handle_error(
                |err: std::io::Error| async move {
                    tracing::error!("Error serving static file: {err:?}");
                    (StatusCode::INTERNAL_SERVER_ERROR, format!("{err:?}"))
                },
            ),
        )
        .with_state(state)
}

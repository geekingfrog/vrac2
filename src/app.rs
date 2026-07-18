use axum::{routing, Router};
use axum_login::{login_required, AuthManagerLayerBuilder};
use tower::ServiceBuilder;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use tower_sessions::{MemoryStore, SessionManagerLayer};

use crate::auth;
use crate::handlers;
use crate::state::AppState;

pub fn build(state: AppState) -> Router<()> {
    let service = ServiceBuilder::new().layer(TraceLayer::new_for_http());
    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store);

    let auth_backend = auth::Backend::new(state.db.clone());
    let auth_layer = AuthManagerLayerBuilder::new(auth_backend, session_layer).build();

    Router::new()
        .layer(service)
        .merge(handlers::gen::router(state.clone()))
        .route_layer(login_required!(auth::Backend, login_url = "/login"))
        .route(
            "/",
            routing::get(|| async { axum::response::Redirect::temporary("/gen") }),
        )
        .merge(handlers::auth::router(state.clone()))
        .merge(handlers::upload::router(state.clone()))
        .nest_service("/static", ServeDir::new("static"))
        .layer(axum_messages::MessagesManagerLayer)
        .layer(auth_layer)
}

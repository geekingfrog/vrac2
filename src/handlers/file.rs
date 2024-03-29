use axum::{
    body::StreamBody,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use tokio_util::io::ReaderStream;

use crate::{error::Result, state::AppState};

#[derive(serde::Deserialize, Debug)]
pub(crate) struct Params {
    dl: Option<bool>,
}

pub(crate) async fn get_file(
    Path((tok_path, file_id)): Path<(String, i64)>,
    state: State<AppState>,
    params: Query<Params>,
) -> Result<Response> {
    let file = match state.db.get_valid_file(&tok_path, file_id).await? {
        None => return Ok((StatusCode::NOT_FOUND, "not found").into_response()),
        Some(file) => file,
    };

    let mut headers = HeaderMap::new();
    let mime_type = file
        .mime_type
        .unwrap_or_else(|| "application/octet-stream".to_string());

    headers.insert(
        header::CONTENT_TYPE,
        mime_type
            .parse()
            .unwrap_or_else(|_| "application/octet-stream".parse().unwrap()),
    );

    let file_name = match file.name {
        Some(n) => n,
        None => format!("{:04}_{:04}", file.token_id, file.id),
    };
    let content_disp_type = if params.dl.unwrap_or(false) {
        "attachment"
    } else {
        "inline"
    };

    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("{content_disp_type}; filename=\"{}\"", file_name)
            .parse()
            .unwrap(),
    );

    tracing::debug!(
        "{} reading backend data {}",
        file.backend_type,
        file.backend_data
    );
    let blob = state
        .get_blob(file.backend_type.as_str(), file.backend_data)
        .await?;

    // stream an AsyncRead as a response
    // https://github.com/tokio-rs/axum/discussions/608
    let stream = ReaderStream::new(blob);
    let body = StreamBody::new(stream);

    Ok((headers, body).into_response())
}

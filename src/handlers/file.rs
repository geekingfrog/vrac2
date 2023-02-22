use axum::{
    body::StreamBody,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use tokio_util::io::ReaderStream;

use crate::{
    error::{AppError, Result},
    state::AppState,
    upload::{LocalFsData, StorageBackend},
};

#[derive(serde::Deserialize)]
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
    let (mime_type, extension) = match file.mime_type {
        Some(m) => {
            let ext = guess_extension(&m);
            (m, ext)
        }
        None => ("application/octet-stream".to_string(), ""),
    };

    headers.insert(
        header::CONTENT_TYPE,
        mime_type
            .parse()
            .unwrap_or_else(|_| "application/octet-stream".parse().unwrap()),
    );

    if params.dl.unwrap_or(false) {
        let file_name = match file.name {
            Some(n) => n,
            None => format!("{:04}_{:04}", file.token_id, file.id),
        };

        headers.insert(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}.{}\"", file_name, extension)
                .parse()
                .unwrap(),
        );
    }

    if file.backend_type.as_str() != "local_fs" {
        return Err(AppError::UnknownStorageBackend(
            file.backend_type.to_string(),
        ));
    }

    // stream an AsyncRead as a response
    // https://github.com/tokio-rs/axum/discussions/608
    let backend_data: LocalFsData = serde_json::from_str(&file.backend_data)?;
    let blob = state.storage_fs.read_blob(backend_data).await?;
    let stream = ReaderStream::new(blob);
    let body = StreamBody::new(stream);

    Ok((headers, body).into_response())
}

fn guess_extension(mime_type: &str) -> &'static str {
    // https://developer.mozilla.org/en-US/docs/Web/HTTP/Basics_of_HTTP/MIME_types/Common_types
    match mime_type {
        "aac" => "audio/aac",
        "abw" => "application/x-abiword",
        "arc" => "application/x-freearc",
        "avif" => "image/avif",
        "avi" => "video/x-msvideo",
        "azw" => "application/vnd.amazon.ebook",
        "bin" => "application/octet-stream",
        "bmp" => "image/bmp",
        "bz" => "application/x-bzip",
        "bz2" => "application/x-bzip2",
        "cda" => "application/x-cdf",
        "csh" => "application/x-csh",
        "css" => "text/css",
        "csv" => "text/csv",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "eot" => "application/vnd.ms-fontobject",
        "epub" => "application/epub+zip",
        "gz" => "application/gzip",
        "gif" => "image/gif",
        "html" => "text/html",
        "ico" => "image/vnd.microsoft.icon",
        "ics" => "text/calendar",
        "jar" => "application/java-archive",
        "jpeg" => "image/jpeg",
        "js" => "text/javascript",
        "json" => "application/json",
        "jsonld" => "application/ld+json",
        "midi" => "audio/midi",
        "mjs" => "text/javascript",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "mpeg" => "video/mpeg",
        "mpkg" => "application/vnd.apple.installer+xml",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odt" => "application/vnd.oasis.opendocument.text",
        "oga" => "audio/ogg",
        "ogv" => "video/ogg",
        "ogx" => "application/ogg",
        "opus" => "audio/opus",
        "otf" => "font/otf",
        "png" => "image/png",
        "pdf" => "application/pdf",
        "php" => "application/x-httpd-php",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "rar" => "application/vnd.rar",
        "rtf" => "application/rtf",
        "sh" => "application/x-sh",
        "svg" => "image/svg+xml",
        "tar" => "application/x-tar",
        "tiff" => "image/tiff",
        "ts" => "video/mp2t",
        "ttf" => "font/ttf",
        "txt" => "text/plain",
        "vsd" => "application/vnd.visio",
        "wav" => "audio/wav",
        "weba" => "audio/webm",
        "webm" => "video/webm",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "xhtml" => "application/xhtml+xml",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xml" => "application/xml",
        "xul" => "application/vnd.mozilla.xul+xml",
        "zip" => "application/zip",
        "3gp" => "video/3gpp",
        "3g2" => "video/3gpp2",
        "7z" => "application/x-7z-compressed",
        _ => {
            tracing::warn!("unknown mime type: {mime_type}");
            ""
        }
    }
}

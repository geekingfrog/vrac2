use async_zip::error::ZipError;
use async_zip::{Compression, ZipEntryBuilder};
use axum::http::{header, HeaderMap, StatusCode};
use axum::{routing, Form, Router};
use axum_messages::{Message, Messages};
use scrypt::password_hash::PasswordVerifier;
use scrypt::phc::PasswordHash;
use scrypt::Scrypt;
use std::io::ErrorKind;
use std::pin::Pin;
use std::str::FromStr;
use std::task::{Context, Poll};
use tower_sessions::Session;

use axum::extract::{DefaultBodyLimit, FromRequestParts, Multipart, Path, Query};
use axum::response::{Redirect, Response};
use axum::{extract::State, response::Html, response::IntoResponse};
use humantime::format_duration;
use serde::{de, Deserialize};
use time::{Duration, OffsetDateTime};
use tracing::Instrument;

use futures::TryStreamExt;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use pin_project::pin_project;

use crate::db::{DbFile, DbFileMetadata, DbToken, GetTokenResult};
use crate::error::{AppError, Result};
use crate::state::AppState;
use crate::upload::{InitFile, StorageBackend};

// wrapper because I later need a futures::AsyncWrite, but tokio's File implements
// tokio::io::AsyncWrite so this bridges the two.
#[pin_project]
struct FutureFile {
    #[pin]
    inner: tokio::fs::File,
}

impl futures::AsyncWrite for FutureFile {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.project().inner.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.project().inner.poll_shutdown(cx)
    }
}

/// How to render a File in a template from a DB file
#[derive(serde::Serialize, Debug)]
struct TplFile {
    id: i64,
    mime_type: String,
    mime_prefix: String,
    name: Option<String>,
    size: Option<i64>,
}

impl std::convert::From<(DbFile, DbFileMetadata)> for TplFile {
    fn from((f, m): (DbFile, DbFileMetadata)) -> Self {
        Self {
            id: f.id,
            mime_type: f.mime_type.clone().unwrap_or("".to_string()),
            mime_prefix: f
                .mime_type
                .and_then(|m| match m.split_once('/') {
                    Some((x, _)) => Some(x.to_string()),
                    None => Some("".to_string()),
                })
                .unwrap_or("".to_string()),
            name: f.name,
            size: m.size_b,
        }
    }
}

pub(crate) fn router(state: AppState) -> Router<()> {
    Router::new()
        .route(
            "/f",
            routing::get(|| async { axum::response::Redirect::temporary("/gen") }),
        )
        .route(
            "/f/{path}",
            routing::get(get_upload_form).post(post_upload_form),
        )
        .route(
            "/f/{path}/",
            routing::get(|Path(p): Path<String>| async move {
                axum::response::Redirect::temporary(&format!("/f/{p}"))
            }),
        )
        .route(
            "/f/{path}/_password",
            routing::get(file_password_get).post(file_password_post),
        )
        .route(
            "/f/{path}/{file_id}",
            routing::get(crate::handlers::file::get_file),
        )
        .layer(DefaultBodyLimit::max(usize::MAX))
        .with_state(state)
}

// impl<S> FromRequestParts<S> for GetTokenResult
// where
//     S: Send + Sync,
// {
//     type Rejection = AppError;
//
//     async fn from_request_parts(
//         parts: &mut axum::http::request::Parts,
//         _state: &S,
//     ) -> Result<Self> {
//         todo!()
//     }
// }

async fn get_upload_form(
    state: State<AppState>,
    Path(tok_path): Path<String>,
    session: Session,
    Query(file_query): Query<FileQuery>,
) -> Result<Response> {
    let tok_path =
        urlencoding::decode(&tok_path).map_err(|e| crate::error::AppError::InvalidUrlToken {
            token: tok_path.clone(),
            source: e,
        })?;

    match state.db.get_valid_token(&tok_path).await? {
        GetTokenResult::NotFound => {
            let html: Html<String> = state
                .get_templates()
                .render("no_link_found.html", &tera::Context::new())?
                .into();
            let rsp = (StatusCode::NOT_FOUND, html);
            Ok(rsp.into_response())
        }
        GetTokenResult::Fresh(tok) => upload_form(state, tok).await,
        GetTokenResult::Used(tok) => {
            let span = tracing::info_span!("token {}-{}", tok.id, tok.path);
            if file_query.zip {
                get_files_zip(state, tok).instrument(span).await
            } else {
                get_files_html(state, session, tok).instrument(span).await
            }
        }
    }
}

async fn post_upload_form(
    Path(tok_path): Path<String>,
    state: State<AppState>,
    mut multipart: Multipart,
) -> Result<Response> {
    // TODO: maybe make a custom extractor for the token which handles the
    // urldecoding itself to reduce duplication?
    let tok_path =
        urlencoding::decode(&tok_path).map_err(|e| crate::error::AppError::InvalidUrlToken {
            token: tok_path.clone(),
            source: e,
        })?;

    let token = match state.db.get_valid_token(&tok_path).await? {
        GetTokenResult::Fresh(t) => t,
        GetTokenResult::NotFound | GetTokenResult::Used(_) => {
            let not_found = state
                .get_templates()
                .render("no_link_found.html", &tera::Context::new())?;
            return Ok(not_found.into_response());
        }
    };

    let backend: Box<dyn StorageBackend + Send + Sync> =
        if token.backend_type == state.storage_fs.get_type() {
            Box::new(state.storage_fs.clone())
        } else if token.backend_type == state.garage.get_type() {
            Box::new(state.garage.clone())
        } else {
            return Err(crate::error::AppError::UnknownStorageBackend(
                token.backend_type,
            ));
        };

    let mut token = state.db.initiate_upload(token).await?;

    let mut total_bytes = 0;
    let mut file_idx = 0;
    while let Some(field) = multipart.next_field().await? {
        if field.name() == Some("password") {
            let plain_pass = field.text().await?;
            if plain_pass != "" {
                token.set_password(plain_pass.as_bytes())?;
            }
            continue;
        }

        file_idx += 1;
        tracing::info!(
            "got a new field here {:?} of type {:?} for file {:?}",
            field.name(),
            field.content_type(),
            field.file_name(),
        );

        let mime_type = field.content_type();
        tracing::info!("mime type: {mime_type:?}");
        let init_file = InitFile {
            token_id: token.id,
            token_path: &token.path,
            file_index: file_idx,
            attempt_counter: token.attempt_counter,
            mime_type,
            file_name: field.file_name(),
        };

        let (writer, data) = backend.initiate_upload(&init_file).await?;
        let mut writer = writer.compat_write();
        let db_file = state
            .db
            .create_file(
                &token,
                backend.get_type(),
                data.clone(),
                mime_type,
                field.file_name(),
            )
            .await?;

        let mime_type = mime_type.map(str::to_string);

        let reader =
            field.map_err(|err| std::io::Error::new(ErrorKind::Other, format!("oops {err:?}")));
        let bytes_copied =
            futures::io::copy_buf(&mut reader.into_async_read(), &mut writer).await?;
        total_bytes += bytes_copied;

        if bytes_copied == 0 {
            tracing::info!("No bytes uploaded for token {} - {}", token.id, token.path);
            backend.delete_blob(data).await?;
            state.db.delete_files([db_file.id]).await?;
        } else {
            let mb_data = writer.into_inner().finalize_upload().await?;
            let metadata = DbFileMetadata {
                size_b: Some(bytes_copied as _),
                mime_type,
            };
            state
                .db
                .finalise_file_upload(db_file, mb_data, metadata)
                .await?;

            tracing::info!("total uploaded for field: {}Kib", bytes_copied / 1024);
        }
    }

    if total_bytes == 0 {
        tracing::info!(
            "No bytes uploaded at all for token {} - {}",
            token.id,
            token.path
        );
    } else {
        state.db.finalise_token_upload(token).await?;
        tracing::info!("done with upload");
    }

    // TODO: maybe use https://docs.rs/axum/0.6.0-rc.4/axum/extract/struct.OriginalUri.html
    // instead of reconstructing the path here
    Ok(Redirect::to(&format!("/f/{}", tok_path)).into_response())
}

async fn upload_form(state: State<AppState>, tok: DbToken) -> Result<Response> {
    tracing::info!("fresh token {} - {}", tok.id, tok.path);
    let now = OffsetDateTime::now_utc();
    let duration = tok.valid_until - now;
    let duration = std::time::Duration::from_secs(duration.as_seconds_f64().round() as u64);

    let mut ctx = tera::Context::new();
    ctx.insert("max_size", &tok.max_size_mib);
    ctx.insert("valid_for", &format_duration(duration).to_string());
    if let Some(d) = tok.content_expires_after_hours {
        let d = std::time::Duration::new((d as u64) * 3600, 0);
        ctx.insert("content_duration", &format_duration(d).to_string());
    }

    let html: Html<String> = state
        .get_templates()
        .render("upload_form.html", &ctx)?
        .into();
    Ok(html.into_response())
}

async fn get_files_html(
    state: State<AppState>,
    session: Session,
    tok: DbToken,
) -> Result<Response> {
    tracing::debug!("db token? {tok:?}");
    if should_ask_password(&tok, &session).await? {
        let uri = format!("{}/_password", tok.get_url());
        Ok(Redirect::to(&uri).into_response())
    } else {
        render_files(state, tok).await
    }
}

async fn render_files(state: State<AppState>, tok: DbToken) -> Result<Response> {
    let mut ctx = tera::Context::new();
    ctx.insert(
        "expires_at",
        &tok.content_expires_at.map(|d| {
            let fmt = time::macros::format_description!("[year]/[month]/[day] [hour]:[minute]");
            d.format(&fmt).expect("formatting offsetdatetime")
        }),
    );
    ctx.insert("base_url", &state.base_url);

    ctx.insert(
        "expires_in",
        &tok.content_expires_at.map(|expires_at| {
            let now = OffsetDateTime::now_utc();
            let mut d = expires_at - now;
            let mut res = String::new();
            let days = d.whole_days();
            d = d - Duration::days(days);
            let hours = d.whole_hours();
            d = d - Duration::hours(hours);
            let minutes = d.whole_minutes();
            if days > 0 {
                res.push_str(&format!("{} days ", days));
            }
            if hours > 0 {
                res.push_str(&format!("{} hours ", hours));
            }
            if minutes > 0 {
                res.push_str(&format!("{} minutes ", minutes));
            }

            res
        }),
    );

    ctx.insert("token_path", &tok.path);

    let files = state.db.get_files(tok.id, tok.attempt_counter).await?;
    let files: Vec<TplFile> = files.into_iter().map(|x| x.into()).collect();

    ctx.insert("files", &files);
    ctx.insert("tok_path", &tok.path);

    let html: Html<String> = state.get_templates().render("get_files.html", &ctx)?.into();
    Ok(html.into_response())
}

trait IntoIOError {
    // fn into_io_error<E: std::error::Error + Send + Sync + 'static>(self: E) -> std::io::Error;
    fn into_io_error(self) -> std::io::Error;
}

impl IntoIOError for ZipError {
    fn into_io_error(self) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::Other, self)
    }
}

impl IntoIOError for crate::error::AppError {
    fn into_io_error(self) -> std::io::Error {
        tracing::error!("app error into IoError {:?}", self);
        std::io::Error::new(std::io::ErrorKind::Other, self)
    }
}

async fn get_files_zip(state: State<AppState>, tok: DbToken) -> Result<Response> {
    tracing::debug!("getting zip files for {tok:?}");
    let files = state.db.get_files(tok.id, tok.attempt_counter).await?;

    let state = state.clone();
    let (rdr, wrt) = tokio::io::simplex(4096 * 2);

    let tok_path = tok.path.clone();
    tokio::spawn(async move {
        match stream_archive(&state, files, wrt).await {
            Ok(()) => tracing::debug!("Done writing archive at {}", tok_path),
            Err(err) => tracing::error!("Error writing archive at {}: {err:?}", tok_path),
        }
    });

    let stream = tokio_util::io::ReaderStream::new(rdr);
    let body = axum::body::Body::from_stream(stream);

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "application/zip".parse().unwrap());
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{}.zip\"", tok.path)
            .parse()
            .unwrap(),
    );

    Ok((headers, body).into_response())
}

async fn stream_archive<W>(
    state: &AppState,
    files: Vec<(DbFile, DbFileMetadata)>,
    mut wrt: W,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut zip_wrt = async_zip::base::write::ZipFileWriter::with_tokio(&mut wrt);

    for (file, _metadata) in files {
        let blob = state
            .get_blob(&file.backend_type, file.backend_data)
            .await?;
        let filename = file.name.unwrap_or_else(|| format!("{}", file.id));
        let opts = ZipEntryBuilder::new(filename.clone().into(), Compression::Deflate);
        let mut entry = zip_wrt
            .write_entry_stream(opts)
            .await
            .map_err(|e| e.into_io_error())?;
        let bytes = futures::io::copy(blob.compat(), &mut entry).await?;
        entry.close().await.map_err(|e| e.into_io_error())?;
        tracing::debug!("done writing {bytes} bytes to entry {:?}", filename);
    }

    match zip_wrt.close().await {
        Ok(blah) => {
            blah.into_inner().shutdown().await?;
            wrt.shutdown().await?;
        }
        Err(err) => {
            tracing::error!("error closing zip file! {err:?}");
            return Err(AppError::InternalError {
                message: format!("{err:?}"),
            });
        }
    };

    Ok(())
}

#[derive(serde::Deserialize, Debug, Default)]
pub(crate) struct FileQuery {
    #[serde(default, deserialize_with = "true_if_present")]
    zip: bool,
}

// if the field is present at all, treat it as true, and ignore any associated value
fn true_if_present<'de, D>(de: D) -> std::result::Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(de)?;
    match opt.as_deref() {
        None | Some("") => Ok(true),
        Some(s) => FromStr::from_str(s).map_err(de::Error::custom),
    }
}

#[derive(Deserialize)]
struct FilePasswordForm {
    password: String,
}

async fn file_password_get(
    state: State<AppState>,
    messages: Messages,
    session: Session,
    Path(tok_path): Path<String>,
) -> Result<Response> {
    let tok_path =
        urlencoding::decode(&tok_path).map_err(|e| crate::error::AppError::InvalidUrlToken {
            token: tok_path.clone(),
            source: e,
        })?;

    let token = match state.db.get_valid_token(&tok_path).await? {
        GetTokenResult::Fresh(t) => return Ok(Redirect::to(&t.get_url()).into_response()),
        GetTokenResult::NotFound => {
            let not_found = state
                .get_templates()
                .render("no_link_found.html", &tera::Context::new())?;
            return Ok(not_found.into_response());
        }
        GetTokenResult::Used(t) => t,
    };

    if !should_ask_password(&token, &session).await? {
        return Ok(Redirect::to(&token.get_url()).into_response());
    }

    let uri = format!("{}/_password", token.get_url());
    tracing::debug!("uri for form action? {uri}");
    let mut ctx = tera::Context::new();
    ctx.insert("action", &uri);
    let messages: Vec<Message> = messages.into_iter().collect();
    ctx.insert("messages", &messages);
    let html: Html<String> = state
        .get_templates()
        .render("ask_for_password.html", &ctx)?
        .into();
    Ok(html.into_response())
}

async fn file_password_post(
    state: State<AppState>,
    session: Session,
    messages: Messages,
    Path(tok_path): Path<String>,
    Form(file_password): Form<FilePasswordForm>,
) -> Result<Response> {
    let tok_path =
        urlencoding::decode(&tok_path).map_err(|e| crate::error::AppError::InvalidUrlToken {
            token: tok_path.clone(),
            source: e,
        })?;

    let token = match state.db.get_valid_token(&tok_path).await? {
        GetTokenResult::Fresh(t) => return Ok(Redirect::to(&t.get_url()).into_response()),
        GetTokenResult::NotFound => {
            let not_found = state
                .get_templates()
                .render("no_link_found.html", &tera::Context::new())?;
            return Ok(not_found.into_response());
        }
        GetTokenResult::Used(t) => t,
    };

    let verifier = match &token.password {
        None => return Ok(Redirect::to(&token.get_url()).into_response()),
        Some(hash) => PasswordHash::new(hash).map_err(|_e| AppError::InternalError {
            message: format!("Invalid hash for token {}", token.id),
        })?,
    };

    match Scrypt::default().verify_password(file_password.password.as_bytes(), &verifier) {
        Err(_) => {
            messages.error("Invalid password");
            let uri = format!("{}/_password", token.get_url());
            Ok(Redirect::to(&uri).into_response())
        }
        Ok(()) => {
            session
                .insert_value(&token.id.to_string(), serde_json::Value::Bool(true))
                .await?;
            Ok(Redirect::to(&token.get_url()).into_response())
        }
    }
}

impl DbToken {
    fn get_url(&self) -> String {
        let path = urlencoding::encode(&self.path);
        format!("/f/{path}")
    }
}

async fn should_ask_password(tok: &DbToken, session: &Session) -> Result<bool> {
    let key = tok.id.to_string();
    if tok.password.is_some() {
        return if let Some(serde_json::Value::Bool(true)) = session.get_value(&key).await? {
            Ok(false)
        } else {
            Ok(true)
        };
    }
    Ok(false)
}

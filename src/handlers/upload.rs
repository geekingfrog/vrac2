use async_zip::error::ZipError;
use async_zip::{Compression, ZipEntryBuilder};
use futures::{Future, FutureExt};
use hyper::{header, HeaderMap};
use std::io::ErrorKind;
use std::pin::Pin;
use std::str::FromStr;
use std::task::{Context, Poll};

use axum::extract::{Multipart, Path, Query};
use axum::response::{Redirect, Response};
use axum::{extract::State, response::Html, response::IntoResponse};
use axum_flash::IncomingFlashes;
use humantime::format_duration;
use serde::{de, Deserialize};
use time::{Duration, OffsetDateTime};
use tracing::Instrument;

use futures::TryStreamExt;
use tokio::io::{AsyncWrite, DuplexStream};
use tokio_util::compat::{
    Compat, FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt,
};

use pin_project::pin_project;

use crate::db::{DbFile, DbToken, GetTokenResult};
use crate::error::{AppError, Result};
use crate::handlers::flash_utils::ctx_from_flashes;
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
    mime_type: Option<String>,
    mime_prefix: Option<String>,
    name: Option<String>,
}

impl std::convert::From<DbFile> for TplFile {
    fn from(f: DbFile) -> Self {
        Self {
            id: f.id,
            mime_type: f.mime_type.clone(),
            mime_prefix: f.mime_type.and_then(|m| match m.split_once('/') {
                Some((x, _)) => Some(x.to_string()),
                None => None,
            }),
            name: f.name,
        }
    }
}

#[tracing::instrument(skip(state, incoming_flashes))]
pub(crate) async fn get_upload_form(
    state: State<AppState>,
    incoming_flashes: IncomingFlashes,
    Path(tok_path): Path<String>,
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
                .templates
                .read()
                .render("no_link_found.html", &tera::Context::new())?
                .into();
            let rsp = (hyper::StatusCode::NOT_FOUND, html);
            Ok((incoming_flashes, rsp).into_response())
        }
        GetTokenResult::Fresh(tok) => upload_form(state, incoming_flashes, tok).await,
        GetTokenResult::Used(tok) => {
            let span = tracing::info_span!("token {}-{}", tok.id, tok.path);
            if file_query.zip {
                get_files_zip(state, incoming_flashes, tok)
                    .instrument(span)
                    .await
            } else {
                get_files_html(state, incoming_flashes, tok)
                    .instrument(span)
                    .await
            }
        }
    }
}

pub(crate) async fn post_upload_form(
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
                .templates
                .read()
                .render("no_link_found.html", &tera::Context::new())?;
            return Ok(not_found.into_response());
        }
    };
    let token = state.db.initiate_upload(token).await?;

    let mut total_bytes = 0;
    let mut file_idx = 0;
    while let Some(field) = multipart.next_field().await? {
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

        let (writer, data) = state.storage_fs.initiate_upload(&init_file).await?;
        let mut writer = writer.compat_write();
        let db_file = state
            .db
            .create_file(
                &token,
                state.storage_fs.get_type(),
                serde_json::to_string(&data)?,
                mime_type,
                field.file_name(),
            )
            .await?;

        let reader =
            field.map_err(|err| std::io::Error::new(ErrorKind::Other, format!("oops {err:?}")));
        let bytes_copied =
            futures::io::copy_buf(&mut reader.into_async_read(), &mut writer).await?;
        total_bytes += bytes_copied;

        if bytes_copied == 0 {
            tracing::info!("No bytes uploaded for token {} - {}", token.id, token.path);
            state.storage_fs.delete_blob(data).await?;
            state.db.delete_files([db_file.id]).await?;
        } else {
            let mb_data = state.storage_fs.finalize_upload(writer.into_inner()).await?;
            state
                .db
                .finalise_file_upload(
                    db_file,
                    mb_data.map(|d| serde_json::to_string(&d)).transpose()?,
                )
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

async fn upload_form(
    state: State<AppState>,
    incoming_flashes: IncomingFlashes,
    tok: DbToken,
) -> Result<Response> {
    tracing::info!("fresh token {} - {}", tok.id, tok.path);
    let now = OffsetDateTime::now_utc();
    let duration = tok.valid_until - now;
    let duration = std::time::Duration::from_secs(duration.as_seconds_f64().round() as u64);

    let mut ctx = ctx_from_flashes(&incoming_flashes);
    ctx.insert("max_size", &tok.max_size_mib);
    ctx.insert("valid_for", &format_duration(duration).to_string());
    if let Some(d) = tok.content_expires_after_hours {
        let d = std::time::Duration::new((d as u64) * 3600, 0);
        ctx.insert("content_duration", &format_duration(d).to_string());
    }

    let html: Html<String> = state
        .templates
        .read()
        .render("upload_form.html", &ctx)?
        .into();
    Ok((incoming_flashes, html).into_response())
}

async fn get_files_html(
    state: State<AppState>,
    incoming_flashes: IncomingFlashes,
    tok: DbToken,
) -> Result<Response> {
    let mut ctx = tera::Context::new();
    ctx.insert(
        "expires_at",
        &tok.content_expires_at.map(|d| {
            let fmt = time::macros::format_description!("[year]/[month]/[day] [hour]:[minute]");
            d.format(&fmt).expect("formatting offsetdatetime")
        }),
    );

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
    let files: Vec<TplFile> = files.into_iter().map(|f| f.into()).collect();

    ctx.insert("files", &files);
    ctx.insert("tok_path", &tok.path);

    let html: Html<String> = state
        .templates
        .read()
        .render("get_files.html", &ctx)?
        .into();
    Ok((incoming_flashes, html).into_response())
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

#[pin_project]
struct ZipAsyncReader {
    #[pin]
    rdr: Compat<DuplexStream>,
    fut_wrt: Pin<Box<dyn Future<Output = std::io::Result<()>> + Send>>,
}

impl futures::io::AsyncRead for ZipAsyncReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        // attempt to write more into the buffer
        match self.fut_wrt.poll_unpin(cx) {
            Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
            _ => (),
        };

        let n = futures::ready!(self.project().rdr.poll_read(cx, buf))?;
        Poll::Ready(Ok(n))
    }
}

async fn get_files_zip(
    state: State<AppState>,
    incoming_flashes: IncomingFlashes,
    tok: DbToken,
) -> Result<Response> {
    let files = state.db.get_files(tok.id, tok.attempt_counter).await?;

    let state = state.clone();
    let (rdr, wrt) = tokio::io::duplex(4096);
    let fut = async move {
        let mut zip_wrt = async_zip::base::write::ZipFileWriter::new(wrt.compat());
        for file in files {
            match file.backend_type.as_str() {
                "local_fs" => {
                    let data = serde_json::from_str(&file.backend_data)?;
                    let blob = state
                        .garage
                        .read_blob(data)
                        .await
                        .map_err(|e| e.into_io_error())?
                        .compat();
                    let filename = file.name.unwrap_or_else(|| format!("{}", file.id));
                    let opts = ZipEntryBuilder::new(filename.into(), Compression::Deflate);
                    let mut entry = zip_wrt
                        .write_entry_stream(opts)
                        .await
                        .map_err(|e| e.into_io_error())?;
                    futures::io::copy(blob, &mut entry).await?;
                    entry.close().await.map_err(|e| e.into_io_error())?;
                }
                x => {
                    tracing::error!("Unexpected backend type {} for file {}", x, file.id);
                    return Err(AppError::UnknownStorageBackend(x.to_string()).into_io_error());
                }
            }
        }

        zip_wrt.close().await.map_err(|e| e.into_io_error())?;

        let result: std::io::Result<()> = Ok(());
        result
    };

    let zar = ZipAsyncReader {
        rdr: rdr.compat(),
        fut_wrt: Box::pin(fut.fuse()),
    };

    let stream = tokio_util::io::ReaderStream::new(zar.compat());
    let body = axum::body::StreamBody::new(stream);

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "application/zip".parse().unwrap());
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{}.zip\"", tok.path)
            .parse()
            .unwrap(),
    );

    Ok((incoming_flashes, (headers, body)).into_response())
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

use std::io::ErrorKind;

use axum::extract::{Multipart, Path};
use axum::response::{Redirect, Response};
use axum::{extract::State, response::Html, response::IntoResponse};
use axum_flash::IncomingFlashes;
use humantime::format_duration;
use time::{Duration, OffsetDateTime};

// use futures_util::stream::TryStreamExt;
use futures::TryStreamExt;
use tokio::io::AsyncWrite;

use pin_project::pin_project;

use crate::db::{DbFile, GetTokenResult};
use crate::error::Result;
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
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        self.project().inner.poll_write(cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
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

pub(crate) async fn get_upload_form(
    state: State<AppState>,
    incoming_flashes: IncomingFlashes,
    Path(tok_path): Path<String>,
) -> Result<Response> {
    let now = OffsetDateTime::now_utc();

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
        GetTokenResult::Fresh(tok) => {
            tracing::info!("fresh token {} - {}", tok.id, tok.path);
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
        GetTokenResult::Used(tok) => {
            let mut ctx = tera::Context::new();
            ctx.insert(
                "expires_at",
                &tok.content_expires_at.map(|d| {
                    let fmt =
                        time::macros::format_description!("[year]/[month]/[day] [hour]:[minute]");
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

            let html: Html<String> = state
                .templates
                .read()
                .render("get_files.html", &ctx)?
                .into();
            Ok((incoming_flashes, html).into_response())
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

        let (mut writer, data) = state.storage_fs.initiate_upload(&init_file).await?;
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
            let mb_data = state.storage_fs.finalize_upload(writer).await?;
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

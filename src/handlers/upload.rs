use std::borrow::Cow;
use std::io::ErrorKind;

use axum::extract::{Multipart, Path};
use axum::response::{Redirect, Response};
use axum::{extract::State, response::Html, response::IntoResponse};
use axum_flash::IncomingFlashes;
use humantime::format_duration;
use time::OffsetDateTime;

// use futures_util::stream::TryStreamExt;
use futures::TryStreamExt;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWrite;

use pin_project::pin_project;

use crate::db::ValidToken;
use crate::error::Result;
use crate::handlers::flash_utils::TplFlash;
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
        None => {
            let html: Html<String> = state
                .templates
                .read()
                .render("no_link_found.html", &tera::Context::new())?
                .into();
            let rsp = (hyper::StatusCode::NOT_FOUND, html);
            Ok((incoming_flashes, rsp).into_response())
        }
        Some(ValidToken::Fresh(tok)) => {
            let duration = tok.valid_until - now;
            let duration = std::time::Duration::from_secs(duration.as_seconds_f64().round() as u64);

            let mut ctx = tera::Context::new();

            let flash_messages = incoming_flashes
                .iter()
                .map(|x| x.into())
                .collect::<Vec<TplFlash>>();

            ctx.insert("flash_messages", &flash_messages);
            tracing::info!("flash messages in context: {flash_messages:?}");
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
    }
}

pub(crate) async fn post_upload_form(
    Path(tok_path): Path<String>,
    state: State<AppState>,
    mut multipart: Multipart,
) -> Result<Html<String>> {
    // TODO: maybe make a custom extractor for the token which handles the
    // urldecoding itself to reduce duplication?
    let tok_path =
        urlencoding::decode(&tok_path).map_err(|e| crate::error::AppError::InvalidUrlToken {
            token: tok_path.clone(),
            source: e,
        })?;

    let token = match state.db.get_valid_token(&tok_path).await? {
        Some(t) => t,
        None => {
            let not_found = state
                .templates
                .read()
                .render("no_link_found.html", &tera::Context::new())?
                .into();
            return Ok(not_found);
        }
    };
    let token = state.db.initiate_upload(token).await?;

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
                serde_json::to_string(&data).expect("serialize storage backend json data"),
                mime_type,
            )
            .await?;

        let reader =
            field.map_err(|err| std::io::Error::new(ErrorKind::Other, format!("oops {err:?}")));
        let byte_copied = futures::io::copy_buf(&mut reader.into_async_read(), &mut writer).await?;

        let mb_data = state.storage_fs.finalize_upload(writer).await?;
        state
            .db
            .finalise_file_upload(
                db_file,
                mb_data.map(|d| {
                    serde_json::to_string(&d)
                        .expect("data from storage backend cannot be json serialized")
                }),
            )
            .await?;

        tracing::info!("total uploaded for field: {}Kib", byte_copied / 1024);
    }

    state.db.finalise_token_upload(token).await?;

    // // TODO: maybe use https://docs.rs/axum/0.6.0-rc.4/axum/extract/struct.OriginalUri.html
    // // instead of reconstructing the path here
    // let redir = Redirect::temporary(&format!("/f/{}", tok_path));
    tracing::info!("done with upload");
    Ok("".to_string().into())
}

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

use crate::db::ValidToken;
use crate::error::Result;
use crate::handlers::flash_utils::TplFlash;
use crate::state::AppState;

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

    let mut file_idx = 0;
    while let Some(mut field) = multipart.next_field().await? {
        file_idx += 1;
        tracing::info!(
            "got a new field here {:?} of type {:?} for file {:?}",
            field.name(),
            field.content_type(),
            field.file_name(),
        );
        tracing::info!("hdr: {:?}", field.headers());

        {
            let file_name = field
                .file_name()
                .map(Cow::Borrowed)
                .unwrap_or_else(|| format!("file-{}", file_idx).into());
            tracing::info!("file name is {file_name}");
        }

        let s = field.map_err(|err| std::io::Error::new(ErrorKind::Other, format!("{err:?}")));

        let mut writer = futures::io::sink();
        let byte_copied = futures::io::copy_buf(&mut s.into_async_read(), &mut writer).await?;

        // while let Some(chunk) = field.chunk().await? {
        //     total += chunk.len() / 1024;
        //     // tracing::info!("{:04}kib / {:08}kib", chunk.len() / 1024, total);
        //     // tokio::time::sleep(Duration::from_millis(50)).await;
        // }

        tracing::info!("total uploaded for field: {}Kib", byte_copied / 1024);
    }
    // // TODO: maybe use https://docs.rs/axum/0.6.0-rc.4/axum/extract/struct.OriginalUri.html
    // // instead of reconstructing the path here
    // let redir = Redirect::temporary(&format!("/f/{}", tok_path));
    tracing::info!("done with upload");
    Ok("".to_string().into())
}

use axum::extract::{Multipart, Path};
use axum::{extract::State, response::Html};
use axum_flash::IncomingFlashes;
use humantime::format_duration;
use time::OffsetDateTime;

use crate::db::ValidToken;
use crate::error::Result;
use crate::state::AppState;

pub(crate) async fn get_upload_form(
    state: State<AppState>,
    incoming_flashes: IncomingFlashes,
    Path(tok_path): Path<String>,
) -> Result<(IncomingFlashes, Html<String>)> {
    let now = OffsetDateTime::now_utc();
    for (level, flash) in &incoming_flashes {
        tracing::info!("FLASH: [{level:?}] {flash:?}");
    }

    match state.db.get_valid_token(&tok_path).await? {
        None => {
            todo!()
        }
        Some(ValidToken::Fresh(tok)) => {
            let duration = tok.valid_until - now;
            let duration = std::time::Duration::from_secs(duration.as_seconds_f64().round() as u64);

            let mut ctx = tera::Context::new();
            ctx.insert("max_size", &tok.max_size_mib);
            ctx.insert("valid_for", &format_duration(duration).to_string());
            if let Some(d) = tok.content_expires_after_hours {
                let d = std::time::Duration::new((d as u64) * 3600, 0);
                ctx.insert("content_duration", &format_duration(d).to_string());
            }

            Ok((
                incoming_flashes,
                state
                    .templates
                    .read()
                    .render("upload_form.html", &ctx)?
                    .into(),
            ))
        }
    }
}

pub(crate) async fn post_upload_form(mut multipart: Multipart) -> Result<()> {
    while let Some(mut field) = multipart.next_field().await? {
        tracing::info!(
            "got a new field here {:?} of type {:?}",
            field.name(),
            field.content_type()
        );

        let mut total = 0;
        while let Some(chunk) = field.chunk().await? {
            total += chunk.len() / 1024;
            tracing::info!("{:04}kib / {:08}kib", chunk.len() / 1024, total);
            // tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    // // TODO: maybe use https://docs.rs/axum/0.6.0-rc.4/axum/extract/struct.OriginalUri.html
    // // instead of reconstructing the path here
    // let redir = Redirect::temporary(&format!("/f/{}", tok_path));
    tracing::info!("done with upload");
    Ok(())
}

use axum::extract::Path;
use axum::{extract::State, response::Html};
use humantime::format_duration;
use time::OffsetDateTime;

use crate::db::ValidToken;
use crate::error::Result;
use crate::state::AppState;

pub(crate) async fn get_upload_form(
    state: State<AppState>,
    Path(tok_path): Path<String>,
) -> Result<Html<String>> {
    let now = OffsetDateTime::now_utc();
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

            Ok(state
                .templates
                .read()
                .render("upload_form.html", &ctx)?
                .into())
        }
    }
}

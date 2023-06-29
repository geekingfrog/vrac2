use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use axum::{extract::State, response::Html};
use axum_flash::{Flash, IncomingFlashes};
use hyper::StatusCode;
use serde::{Deserialize, Deserializer};
use std::result::Result as StdResult;
use std::time::Duration;
use time::OffsetDateTime;

use crate::auth::Admin;
use crate::error::Result;
use crate::handlers::flash_utils::NotifLevel;
use crate::state::AppState;
use crate::upload::StorageBackend;

use super::flash_utils::Notif;

// need the serialize_with bits to ensure we serialize into a string.
// because a browser will send these fields as string, this ensure consistent
// serialization. There may be a way to accept both an integer and a string, but
// I don't know how.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct GenTokenForm {
    pub path: String,
    #[serde(
        rename = "max-size-mib",
        deserialize_with = "deserialize_sentinel",
        serialize_with = "serialize_opt_str",
        default
    )]
    pub max_size_mib: Option<i64>,

    #[serde(
        rename = "content-expires",
        deserialize_with = "deserialize_sentinel",
        serialize_with = "serialize_opt_str"
    )]
    pub content_expires_after_hours: Option<i64>,
    #[serde(rename = "token-valid-for-hour")]
    pub token_valid_for_hour: u64,

    #[serde(rename = "storage-backend")]
    pub storage_backend: StorageBackendType,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub enum StorageBackendType {
    #[serde(rename = "local_fs")]
    LocalFS,
    #[serde(rename = "garage")]
    Garage,
}

#[tracing::instrument(skip(flashes, state), level = "debug")]
pub(crate) async fn get_token(
    flashes: IncomingFlashes,
    State(state): State<AppState>,
    _: Admin,
) -> Result<(IncomingFlashes, Html<String>)> {
    let mut ctx = tera::Context::new();
    let mut notifications = Vec::with_capacity(flashes.len());
    for (level, message) in &flashes {
        notifications.push(Notif {
            level: level.into(),
            message: message.to_owned(),
        })
    }

    ctx.insert("notifications", &notifications);

    Ok((
        flashes,
        state
            .templates
            .read()
            .render("get_gen_token.html", &ctx)?
            .into(),
    ))
}

#[tracing::instrument(skip(state, form, flash), level = "debug")]
pub(crate) async fn create_token(
    State(state): State<AppState>,
    flash: Flash,
    _: Admin,
    form: StdResult<Form<GenTokenForm>, axum::extract::rejection::FormRejection>,
) -> Result<(Flash, Response)> {
    let form = match form {
        Ok(Form(f)) => f,
        Err(err) => {
            tracing::error!("Invalid form submitted {err:?}");
            let flash = flash.error(&format!("Invalid request submitted: {err:?}"));
            let ctx = tera::Context::new();
            let page: Html<String> = state
                .templates
                .read()
                .render("get_gen_token.html", &ctx)?
                .into();
            return Ok((flash, (StatusCode::BAD_REQUEST, page).into_response()));
        }
    };
    tracing::debug!("got GenFormToken: {:?}", form);

    let valid_until =
        OffsetDateTime::now_utc() + Duration::from_secs(form.token_valid_for_hour * 3600);

    let backend_type = match form.storage_backend {
        StorageBackendType::LocalFS => state.storage_fs.get_type(),
        StorageBackendType::Garage => state.garage.get_type(),
    };

    let ct = crate::db::CreateToken {
        path: &form.path,
        max_size_mib: form.max_size_mib,
        valid_until,
        content_expires_after_hours: form.content_expires_after_hours,
        backend_type,
    };

    let r = state.db.create_token(ct).await?;

    match r {
        Err(crate::db::TokenError::AlreadyExist) => {
            let mut ctx = tera::Context::new();
            tracing::debug!("serializing form into context: {:?}", form);
            ctx.insert("full_form", &form);
            ctx.insert(
                "notifications",
                &vec![Notif {
                    level: NotifLevel::Error,
                    message: "A valid token already exist for this path.".to_string(),
                }],
            );
            let page: Html<String> = state
                .templates
                .read()
                .render("get_gen_token.html", &ctx)?
                .into();

            Ok((
                flash.error("A valid token already exist for this path."),
                (StatusCode::CONFLICT, page).into_response(),
            ))
        }
        Ok(tok) => Ok((
            flash.success("Token created."),
            Redirect::to(&format!("/f/{}", urlencoding::encode(&tok.path))).into_response(),
        )),
    }
}

// See:
// https://stackoverflow.com/questions/56384447/how-do-i-transform-special-values-into-optionnone-when-using-serde-to-deserial
fn deserialize_sentinel<'de, T, D>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: std::str::FromStr,
{
    let value: std::result::Result<Maybe<T>, _> = Deserialize::deserialize(deserializer);

    match value {
        Ok(Maybe::Just(x)) => Ok(x),
        Ok(Maybe::Nothing(raw)) => {
            if raw == "None" {
                Ok(None)
            } else {
                Err(serde::de::Error::custom(format!(
                    "Unexpected string {}",
                    raw
                )))
            }
        }
        Err(e) => {
            tracing::error!("got error while deserializing: {:?}", e);
            Err(e)
        }
    }
}

// serde(untagged) and serde(flatten) are buggy with serde_qs and serde_urlencoded
// there is a workaround:
// https://github.com/nox/serde_urlencoded/issues/33
// https://github.com/samscott89/serde_qs/issues/14#issuecomment-456865916
// the following is an adaptation to wrap the value into an Option
#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum Maybe<U: std::str::FromStr> {
    #[serde(deserialize_with = "from_option_str")]
    Just(Option<U>),
    // #[serde(deserialize_with = "from_str")]
    Nothing(String),
}

fn from_option_str<'de, D, S>(deserializer: D) -> std::result::Result<Option<S>, D::Error>
where
    D: serde::Deserializer<'de>,
    S: std::str::FromStr,
{
    let s: Option<&str> = Deserialize::deserialize(deserializer)?;
    match s {
        Some(s) => S::from_str(s)
            .map(Some)
            .map_err(|_| serde::de::Error::custom("could not parse string")),
        None => Ok(None),
    }
}

fn serialize_opt_str<F, S>(field: &Option<F>, s: S) -> std::result::Result<S::Ok, S::Error>
where
    F: ToString,
    S: serde::Serializer,
{
    match field {
        Some(v) => s.serialize_some(&v.to_string()),
        None => s.serialize_none(),
    }
}

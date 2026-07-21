use axum::response::{IntoResponse, Redirect, Response};
use axum::{extract::State, http::StatusCode, response::Html, routing};
use axum::{Form, Router};
use axum_messages::{Message, Messages};
use serde::{Deserialize, Deserializer};
use std::result::Result as StdResult;
use std::time::Duration;
use time::OffsetDateTime;

use crate::error::Result;
use crate::state::AppState;
use crate::upload::StorageBackend;

pub(crate) fn router(state: AppState) -> Router<()> {
    Router::new()
        .route("/gen", routing::get(get_token))
        .route("/gen", routing::post(create_token))
        .with_state(state)
}

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

#[tracing::instrument(skip(state), level = "debug")]
#[axum::debug_handler]
async fn get_token(
    messages: Messages,
    State(state): State<AppState>,
) -> Result<Html<String>> {
    let mut ctx = tera::Context::new();
    let messages: Vec<Message> = messages.into_iter().collect();
    ctx.insert("messages", &messages);

    Ok(state
        .get_templates()
        .render("get_gen_token.html", &ctx)?
        .into())
}

#[tracing::instrument(skip(state, form, messages), level = "debug")]
async fn create_token(
    State(state): State<AppState>,
    messages: Messages,
    form: StdResult<Form<GenTokenForm>, axum::extract::rejection::FormRejection>,
) -> Result<Response> {
    let form = match form {
        Ok(Form(f)) => f,
        Err(err) => {
            tracing::error!("Invalid form submitted {err:?}");
            messages.error(format!("Invalid request submitted: {err:?}"));
            let ctx = tera::Context::new();
            let page: Html<String> = state
                .get_templates()
                .render("get_gen_token.html", &ctx)?
                .into();
            return Ok((StatusCode::BAD_REQUEST, page).into_response());
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
        password: None,
    };

    let r = state.db.create_token(ct).await?;

    match r {
        Err(crate::db::TokenError::AlreadyExist) => {
            let mut ctx = tera::Context::new();
            tracing::debug!("serializing form into context: {:?}", form);
            ctx.insert("full_form", &form);
            ctx.insert("error", "A valid token already exist for this path.");
            let page: Html<String> = state
                .get_templates()
                .render("get_gen_token.html", &ctx)?
                .into();

            messages.error("A valid token already exist for this path.");
            Ok((StatusCode::CONFLICT, page).into_response())
        }
        Ok(tok) => {
            messages.success("Token created.");
            Ok(Redirect::to(&format!("/f/{}", urlencoding::encode(&tok.path))).into_response())
        }
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
        None => s.serialize_str("None"),
    }
}

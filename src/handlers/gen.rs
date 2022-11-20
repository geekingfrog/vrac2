use axum::response::Redirect;
use axum::Form;
use axum::{extract::State, response::Html};
use axum_flash::{Flash, IncomingFlashes, Level};
use serde::{Deserialize, Deserializer};
use std::result::Result as StdResult;
use std::time::Duration;
use time::OffsetDateTime;

use crate::error::Result;
use crate::state::AppState;

// need the serialize_with bits to ensure we serialize into a string.
// because a browser will send these fields as string, this ensure consistent
// serialization. There may be a way to accept both an integer and a string, but
// I don't know how.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct GenTokenForm {
    pub(crate) path: String,
    #[serde(
        rename = "max-size-mib",
        deserialize_with = "deserialize_sentinel",
        serialize_with = "serialize_opt_str"
    )]
    max_size_mib: Option<i64>,

    #[serde(
        rename = "content-expires",
        deserialize_with = "deserialize_sentinel",
        serialize_with = "serialize_opt_str"
    )]
    content_expires_after_hours: Option<i64>,
    #[serde(rename = "token-valid-for-hour")]
    token_valid_for_hour: u64,
}

#[tracing::instrument(skip(flashes, state), level = "debug")]
pub(crate) async fn get_token(
    flashes: IncomingFlashes,
    State(state): State<AppState>,
) -> Result<(IncomingFlashes, Html<String>)> {
    let mut ctx = tera::Context::new();
    for (level, msg) in &flashes {
        match level {
            Level::Success => ctx.insert("message", msg),
            Level::Error => ctx.insert("error", msg),
            Level::Info => match serde_json::from_str(msg) {
                Err(err) => {
                    tracing::error!("message: {msg}");
                    tracing::error!("invalid form in flash {err:?}");
                }
                Ok::<GenTokenForm, _>(x) => {
                    ctx.insert("max_size_mib", &x.max_size_mib);
                }
            },
            _ => (),
        }
    }

    Ok((
        flashes,
        state
            .templates
            .read()
            .render("get_gen_token.html", &ctx)?
            .into(),
    ))
}

#[tracing::instrument(skip(state, form), level = "debug")]
pub(crate) async fn create_token(
    State(state): State<AppState>,
    flash: Flash,
    form: StdResult<Form<GenTokenForm>, axum::extract::rejection::FormRejection>,
) -> Result<(Flash, Redirect)> {

    let form = match form {
        Ok(Form(f)) => f,
        Err(err) => {
            tracing::error!("Invalid form submitted {err:?}");
            let flash = flash.error(&format!("Invalid request submitted: {err:?}"));
            return Ok((flash, Redirect::to("/gen")));
        }
    };

    let valid_until =
        OffsetDateTime::now_utc() + Duration::from_secs(form.token_valid_for_hour * 3600);
    let ct = crate::db::CreateToken {
        path: &form.path,
        max_size_mib: form.max_size_mib,
        valid_until,
        content_expires_after_hours: form.content_expires_after_hours,
    };

    let r = state.db.create_token(ct).await?;
    tracing::info!(
        "serialized form is: {}",
        serde_json::to_string(&form).unwrap()
    );

    match r {
        // TODO if the token already exists, serialize the form (somehow), store
        // it in the flash so that the form can be prefilled after the redirect.
        Err(crate::db::TokenError::AlreadyExist) => Ok((
            flash
                .error("A valid token already exist for this path.")
                .info(serde_json::to_string(&form).unwrap()),
            Redirect::to("/gen"),
        )),
        Ok(tok) => Ok((
            flash.success("Token created."),
            Redirect::to(&format!("/f/{}", tok.path)),
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

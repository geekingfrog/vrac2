use axum::Form;
use axum::{extract::State, response::Html};
use serde::{Deserialize, Deserializer};
use std::result::Result as StdResult;
use std::time::Duration;
use time::OffsetDateTime;

use crate::error::Result;
use crate::state::AppState;

#[derive(Debug, serde::Deserialize)]
pub(crate) struct GenTokenForm {
    pub(crate) path: String,
    #[serde(rename = "max-size-mib", deserialize_with = "deserialize_sentinel")]
    max_size_mib: Option<i64>,

    #[serde(rename = "content-expires", deserialize_with = "deserialize_sentinel")]
    content_expires_after_hours: Option<i64>,
    #[serde(rename = "token-valid-for-hour")]
    token_valid_for_hour: u64,
}

#[tracing::instrument(skip(state), level = "debug")]
pub(crate) async fn get_token(
    State(state): State<AppState>,
) -> Result<Html<String>> {
    let ctx = tera::Context::new();
    Ok(state
        .templates
        .read()
        .render("get_gen_token.html", &ctx)?
        .into())
}

#[tracing::instrument(skip(state, form), level = "debug")]
pub(crate) async fn create_token(
    State(state): State<AppState>,
    form: StdResult<Form<GenTokenForm>, axum::extract::rejection::FormRejection>,
) -> Result<Html<String>> {
    let form = match form {
        Ok(f) => f,
        Err(err) => {
            let mut ctx = tera::Context::new();
            tracing::error!("Invalid form submitted {err:?}");
            ctx.insert("error", &format!("Invalid request submitted: {err:?}"));
            return Ok(state
                .templates
                .read()
                .render("get_gen_token.html", &ctx)?
                .into());
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

    let mut ctx = tera::Context::new();

    match r {
        Err(crate::db::TokenError::AlreadyExist) => {
            ctx.insert("error", "a valid token already exist for this path")
        }
        _ => ctx.insert("message", "token created"),
    };

    Ok(state
        .templates
        .read()
        .render("get_gen_token.html", &ctx)?
        .into())
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
            eprintln!("got err: {:?}", e);
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

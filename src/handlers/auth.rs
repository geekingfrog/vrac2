use crate::{
    auth::{AuthSession, Credentials},
    error::{AppError, Result},
    state::AppState,
};
use axum::{
    extract::{Query, State},
    response::{Html, IntoResponse, Redirect},
    routing, Form, Router,
};
use axum_messages::Messages;

pub(crate) fn router(state: AppState) -> Router<()> {
    Router::new()
        .route("/login", routing::get(login_get))
        .route("/login", routing::post(login_post))
        .with_state(state)
}

// to redirect after login
#[derive(Debug, serde::Deserialize)]
pub struct NextUrl {
    next: Option<String>,
}

#[axum::debug_handler]
async fn login_get(
    messages: Messages,
    Query(next): Query<NextUrl>,
    State(state): State<AppState>,
) -> Result<Html<String>> {
    let mut ctx = tera::Context::new();
    let msgs = messages.into_iter().collect::<Vec<_>>();
    ctx.insert("messages", &msgs);
    if let Some(next) = next.next {
        ctx.insert("next", &next);
    }
    Ok(state.templates.read().render("login.html", &ctx)?.into())
}

async fn login_post(
    mut auth_session: AuthSession,
    State(state): State<AppState>,
    messages: Messages,
    Form(creds): Form<Credentials>,
) -> Result<impl IntoResponse> {
    let creds_next = creds.next.clone();
    let user = auth_session.authenticate(creds).await;
    let user = match user {
        Err(axum_login::Error::Backend(_err)) => {
            let ctx = tera::Context::new();
            messages.error("Invalid credentials");
            return Ok(state
                .templates
                .read()
                .render("login.html", &ctx)?
                .into_response());
        }
        Err(err) => {
            tracing::error!("error on authenticate: {:?}", err);
            return Err(AppError::InternalError {
                message: format!("{:?}", err),
            });
        }
        Ok(None) => {
            let ctx = tera::Context::new();
            messages.error("Invalid credentials");
            return Ok(state
                .templates
                .read()
                .render("login.html", &ctx)?
                .into_response());
        }
        Ok(Some(user)) => {
            user
        }
    };

    if let Err(err) = auth_session.login(&user).await {
        return Err(AppError::InternalError {
            message: format!("{err:?}"),
        });
    }
    messages.info(format!("Logged in as {}", user.username));
    let resp = if let Some(ref next) = creds_next {
        Redirect::to(next)
    } else {
        Redirect::to("/")
    }
    .into_response();
    Ok(resp)
}

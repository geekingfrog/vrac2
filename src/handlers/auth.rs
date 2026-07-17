use crate::{
    auth::{AuthSession, Credentials},
    error::{AppError, Result},
    state::AppState,
};
use axum::{extract::State, response::Html, routing, Form, Router};
use axum_messages::Messages;

pub(crate) fn router(state: AppState) -> Router<()> {
    Router::new()
        .route("/login", routing::get(login_get))
        .route("/login", routing::post(login_post))
        .with_state(state)
}

async fn login_get(messages: Messages, State(state): State<AppState>) -> Result<Html<String>> {
    let mut ctx = tera::Context::new();
    let msgs = messages.into_iter().collect::<Vec<_>>();
    tracing::debug!("messages: {:?}", msgs.clone());
    ctx.insert("messages", &msgs);
    tracing::debug!("raw ctx? {:?}", ctx);
    Ok(state.templates.read().render("login.html", &ctx)?.into())
}

async fn login_post(
    mut auth_session: AuthSession,
    State(state): State<AppState>,
    messages: Messages,
    Form(creds): Form<Credentials>,
) -> Result<Html<String>> {
    tracing::info!("post debug stuff {:?}", creds);
    let user = auth_session.authenticate(creds).await;
    tracing::debug!("user? {:?}", user);
    let ctx = tera::Context::new();
    let user = match user {
        Err(axum_login::Error::Backend(err)) => {
            tracing::debug!("backend error? {err:?}");
            let ctx = tera::Context::new();
            messages.error("Invalid credentials");
            return Ok(state.templates.read().render("login.html", &ctx)?.into())
        }
        Err(err) => {
            tracing::error!("error on authenticate: {:?}", err);
            return Err(AppError::InternalError {
                message: format!("{:?}", err),
            })
        }
        Ok(None) => {
            let ctx = tera::Context::new();
            messages.error("Invalid credentials");
            return Ok(state.templates.read().render("login.html", &ctx)?.into())
        }
        Ok(Some(user)) => {
            tracing::info!("logged in as {}", user.username);
            user
        }
    };

    if let Err(err) = auth_session.login(&user).await {
        return Err(AppError::InternalError { message: format!("{err:?}") });
    }
    messages.info(format!("Logged in as {}", user.username));
    Ok(state.templates.read().render("login.html", &ctx)?.into())
}

use crate::{
    auth::{AuthSession, Credentials},
    error::{AppError, Result},
    state::AppState,
};
use axum::{extract::Request, middleware::Next};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect},
    routing, Form, Router,
};
use axum_extra::{
    headers::{authorization::Basic, Authorization},
    TypedHeader,
};
use axum_messages::Messages;

pub(crate) fn router(state: AppState) -> Router<()> {
    Router::new()
        .route("/login", routing::get(login_get))
        .route("/login", routing::post(login_post))
        .route("/logout", routing::get(logout_get))
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
        Ok(Some(user)) => user,
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

async fn logout_get(mut auth_session: AuthSession) -> impl IntoResponse {
    match auth_session.logout().await {
        Ok(_) => Redirect::to("/login").into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub(crate) async fn basic_auth(
    basic: Option<TypedHeader<Authorization<Basic>>>,
    mut request: Request,
    next: Next,
) -> Result<impl IntoResponse> {
    let auth_session = match (&mut request).extensions_mut().get_mut::<AuthSession>() {
        None => return Ok(next.run(request).await),
        Some(sess) => sess,
    };

    if let Some(TypedHeader(basic)) = basic {
        let creds = Credentials {
            username: basic.username().to_string(),
            password: basic.password().to_string(),
            next: None,
        };
        let user = match auth_session.authenticate(creds).await {
            Ok(Some(user)) => user,
            _ => return Err(AppError::Unauthorized),
        };
        if let Err(err) = auth_session.login(&user).await {
            return Err(AppError::InternalError {
                message: format!("{err:?}"),
            });
        }
        tracing::debug!("logged in through basic auth: {}", &user.username);
        let response = next.run(request).await;
        Ok(response)
    } else {
        Ok(next.run(request).await)
    }
}

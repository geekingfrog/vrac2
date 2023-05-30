use std::convert::Infallible;

use async_trait::async_trait;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum_auth::AuthBasic;
use hyper::{HeaderMap, StatusCode};
use password_hash::PasswordHash;
use scrypt::password_hash::PasswordVerifier;
use scrypt::Scrypt;

use crate::db::Account;
use crate::state::AppState;

pub(crate) type Rejection = Response;
pub(crate) struct Admin(Account);

// hugh, the name
#[async_trait]
trait AccountGrabber {
    async fn get_account(&self, username: &str) -> crate::error::Result<Option<Account>>;
}

#[async_trait]
impl AccountGrabber for AppState {
    async fn get_account(&self, username: &str) -> crate::error::Result<Option<Account>> {
        self.db.get_account(username).await
    }
}

impl Admin {
    async fn decode_request_parts<S>(parts: &mut Parts, state: &S) -> Result<Account, Rejection>
    where
        S: Send + Sync + AccountGrabber,
    {
        let auth_header: Result<_, Infallible> =
            Option::<AuthBasic>::from_request_parts(parts, state).await;

        let (username, password) = match auth_header {
            Ok(Some(AuthBasic((username, password)))) => (username, password),
            Ok(None) => {
                let mut headers = HeaderMap::new();
                headers.insert(
                    axum::http::header::WWW_AUTHENTICATE,
                    r#"Basic realm="access to vrac""#.parse().unwrap(),
                );
                return Err((StatusCode::UNAUTHORIZED, headers).into_response());
            }
            Err(_) => unreachable!("Infallible"),
        };

        let password = match password {
            Some(p) => p,
            None => return Err(StatusCode::UNAUTHORIZED.into_response()),
        };

        let account = state.get_account(&username).await.map_err(|err| {
            tracing::error!("Error while getting account: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        })?;

        let account = match account {
            Some(x) => x,
            None => return Err(StatusCode::UNAUTHORIZED.into_response()),
        };

        let parsed_phc = PasswordHash::new(&account.phc).map_err(|err| {
            tracing::error!(
                "Invalid phc in DB for user {} - {}: {:?}",
                account.id,
                account.username,
                err
            );
            StatusCode::UNAUTHORIZED.into_response()
        })?;
        match Scrypt.verify_password(password.as_bytes(), &parsed_phc) {
            Ok(_) => {
                tracing::info!("Authenticated user {}", account.username);
                Ok(account)
            },
            Err(_) => Err(StatusCode::UNAUTHORIZED.into_response()),
        }
    }
}

#[async_trait::async_trait]
impl<S> axum::extract::FromRequestParts<S> for Admin
where
    S: Send + Sync + AccountGrabber,
{
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let account = Admin::decode_request_parts(parts, state).await?;
        Ok(Admin(account))
    }
}

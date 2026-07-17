use crate::{db, error::AppError};
use axum_login::{AuthUser, AuthnBackend};
use scrypt::{password_hash::PasswordVerifier, phc::PasswordHash, Scrypt};

impl AuthUser for db::Account {
    type Id = String;

    fn id(&self) -> Self::Id {
        self.username.to_string()
    }

    fn session_auth_hash(&self) -> &[u8] {
        self.phc.as_bytes()
    }
}

#[derive(serde::Deserialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .field("password", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct Backend {
    db_service: db::DBService,
}

impl Backend {
    pub fn new(db_service: db::DBService) -> Self {
        Self { db_service }
    }
}

impl AuthnBackend for Backend {
    type User = db::Account;

    type Credentials = Credentials;

    type Error = crate::error::AppError;

    async fn authenticate(
        &self,
        creds: Self::Credentials,
    ) -> Result<Option<Self::User>, Self::Error> {
        tracing::debug!("authenticating from creds: {:?}", creds);
        let account = self.get_user(&creds.username).await;
        tracing::debug!("account? {:?}", account);
        let account = self
            .get_user(&creds.username)
            .await?
            .ok_or(AppError::Unauthorized)?;
        let parsed_phc =
            PasswordHash::new(&account.phc).map_err(|err| AppError::InternalError {
                message: format!(
                    "invalid stored phc for user {}: {:?}",
                    account.username, err
                ),
            })?;
        match Scrypt::default().verify_password(creds.password.as_bytes(), &parsed_phc) {
            Ok(()) => Ok(Some(account)),
            Err(_err) => Err(AppError::Unauthorized),
        }
    }

    async fn get_user(
        &self,
        user_id: &axum_login::UserId<Self>,
    ) -> Result<Option<Self::User>, Self::Error> {
        tracing::debug!("getting user with id: {user_id:?}");
        self.db_service.get_account(user_id).await
    }
}

pub type AuthSession = axum_login::AuthSession<Backend>;

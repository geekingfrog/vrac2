use axum::response::{IntoResponse, Response};
use hyper::StatusCode;

pub type Result<T> = std::result::Result<T, AppError>;

#[derive(thiserror::Error, Debug)]
pub enum AppError {
    #[error("Templating error")]
    TemplateError(#[from] tera::Error),

    #[error("DB error at {path}: {source}")]
    DBInitError { path: String, source: sqlx::Error },

    #[error("DB error {0}")]
    DBError(#[from] sqlx::Error),

    #[error("No token found {reason}")]
    NoTokenFound{reason: String},

    #[error("Migration error {0}")]
    MigrationError(#[from] sqlx::migrate::MigrateError),

    #[error("Upload error {0}")]
    UploadError(#[from] axum::extract::multipart::MultipartError),

    #[error("Invalid Token in URL {token} - {source}")]
    InvalidUrlToken {
        token: String,
        source: std::string::FromUtf8Error,
    },
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let res = match self {
            AppError::TemplateError(ref _err) => {
                tracing::error!("Server error: {self:?}");
                (StatusCode::INTERNAL_SERVER_ERROR, format!("{self:?}"))
            }
            _ => (StatusCode::INTERNAL_SERVER_ERROR, format!("{self:?}")),
        };
        res.into_response()
    }
}

// pub trait IOContext {
//     type Out;
//     fn io_context(self, ctx: &'static str) -> Self::Out;
// }
//
// impl<T> IOContext for std::result::Result<T, std::io::Error> {
//     type Out = std::result::Result<T, AppError>;
//     fn io_context(self, ctx: &'static str) -> std::result::Result<T, AppError> {
//         self.map_err(|e| AppError::IOError(e, ctx))
//     }
// }

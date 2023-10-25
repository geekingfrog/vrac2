use aws_sdk_s3 as s3;
use axum::{
    extract::multipart::MultipartError,
    response::{Html, IntoResponse, Response},
};
use hyper::StatusCode;

pub type Result<T> = std::result::Result<T, AppError>;

#[derive(thiserror::Error, Debug)]
pub enum AppError {
    #[error("Templating error")]
    TemplateError(#[from] tera::Error),

    #[error("DB error at {path}: {source}")]
    DBInitError { path: String, source: sqlx::Error },

    #[error("DB error {message} - {source}")]
    DBError {
        message: String,
        source: sqlx::Error,
    },

    #[error("No token found {reason}")]
    NoTokenFound { reason: String },

    #[error("Migration error {0}")]
    MigrationError(#[from] sqlx::migrate::MigrateError),

    // #[error("Upload error {0}")]
    // UploadError(#[from] axum::extract::multipart::MultipartError),
    #[error("Upload error {0}")]
    UploadError(#[from] std::io::Error),

    #[error("Invalid Token in URL {token} - {source}")]
    InvalidUrlToken {
        token: String,
        source: std::string::FromUtf8Error,
    },

    #[error("Corrupted data, unknown storage backend: {0}")]
    UnknownStorageBackend(String),

    #[error("Cannot save blob {message} - {source}")]
    UploadBackendError {
        message: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("Invalid json data for storage backend {source}")]
    InvalidStorageBackendJSON {
        #[from]
        source: serde_json::Error,
    },

    #[error("not found")]
    NotFound { body: Html<String> },

    #[error("Cannot read remote blob")]
    S3ReadError(#[from] s3::error::SdkError<s3::operation::get_object::GetObjectError>),

    #[error("Cannot delete remote blob")]
    S3DeleteError(#[from] s3::error::SdkError<s3::operation::delete_object::DeleteObjectError>),

    #[error("Delete blob failed for file id {file_id} and token id {token_id}")]
    DeleteBlobError {
        file_id: i64,
        token_id: i64,
        #[source]
        source: Box<AppError>,
    },
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let res = match self {
            AppError::NotFound { body } => (StatusCode::NOT_FOUND, body).into_response(),
            AppError::TemplateError(ref _err) => {
                tracing::error!("Server error: {self:?}");
                (StatusCode::INTERNAL_SERVER_ERROR, format!("{self:?}")).into_response()
            }
            AppError::DBError { .. } => {
                tracing::error!("DB error: {self:?}");
                (StatusCode::INTERNAL_SERVER_ERROR, format!("{self:?}")).into_response()
            }
            _ => (StatusCode::INTERNAL_SERVER_ERROR, format!("{self:?}")).into_response(),
        };
        res.into_response()
    }
}

impl From<MultipartError> for AppError {
    fn from(err: MultipartError) -> Self {
        AppError::UploadError(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("{err:?}"),
        ))
    }
}

pub(crate) trait DBErrorContext<T> {
    fn with_context<C, F>(self, f: F) -> Result<T>
    where
        C: ToString + Send + Sync + 'static,
        F: FnOnce() -> C;
}

impl<T> DBErrorContext<T> for sqlx::Result<T> {
    fn with_context<C, F>(self, f: F) -> Result<T>
    where
        C: ToString + Send + Sync + 'static,
        F: FnOnce() -> C,
    {
        self.map_err(|source| AppError::DBError {
            message: f().to_string(),
            source,
        })
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

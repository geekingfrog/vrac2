use sqlx::types::time::OffsetDateTime;
use sqlx::Transaction;
use sqlx::{sqlite::SqlitePoolOptions, Executor, Pool, Row, Sqlite};
use std::result::Result as StdResult;

use crate::error::{AppError, Result};

#[derive(Debug, Clone)]
pub struct DBService {
    pool: Pool<Sqlite>,
}

#[derive(sqlx::FromRow, Debug)]
pub(crate) struct Token {
    pub(crate) id: i64,
    pub(crate) path: String,
    pub(crate) max_size_mib: Option<i64>,
    pub(crate) valid_until: OffsetDateTime,
    pub(crate) created_at: OffsetDateTime,
    pub(crate) content_expires_after_hours: Option<i64>,
    pub(crate) deleted_at: Option<OffsetDateTime>,
    pub(crate) attempt_counter: i64,
}

#[derive(Debug)]
pub(crate) struct CreateToken<'input> {
    pub(crate) path: &'input str,
    pub(crate) max_size_mib: Option<i64>,
    pub(crate) valid_until: OffsetDateTime,
    pub(crate) content_expires_after_hours: Option<i64>,
}

#[derive(sqlx::FromRow, Debug)]
pub(crate) struct File {
    pub(crate) id: i64,
    pub(crate) token_id: i64,
    pub(crate) attempt_counter: i64,
    pub(crate) mime_type: Option<String>,
    pub(crate) backend_type: String,
    pub(crate) backend_data: String,
    pub(crate) created_at: OffsetDateTime,
    pub(crate) completed_at: Option<OffsetDateTime>,
}

/// Must be created before being able to upload files for a given token
/// it's an opaque structure that forces the user to call
/// an init function on the db to prepare an upload
#[must_use]
pub(crate) struct UploadToken {
    token_id: i64,
    attempt_counter: i64,
}

#[derive(Debug)]
pub(crate) enum TokenError {
    /// valid token already exist
    AlreadyExist,
}

pub(crate) enum ValidToken {
    /// a valid token that can be used to upload some files
    Fresh(Token),
    // TODO: a used token with some files attached to it.
}

impl DBService {
    pub(crate) async fn new(db_path: &str) -> Result<Self> {
        let pool_res = SqlitePoolOptions::new()
            .max_connections(2)
            .after_connect(|conn, _meta| {
                // sqlite doesn't allow multiple writer at the same time
                // Since this application is basically going to do nothing most of the time
                // a chance of a write collision is incredibly unlikely.
                // If that ever happens, we get a database locked error but :shrug:
                // So only bother to allow multiple read tx to be execute alongside at most
                // one write transaction.
                // See https://www.sqlite.org/wal.html
                Box::pin(async move {
                    conn.execute("PRAGMA journal_mode=WAL;").await?;
                    Ok(())
                })
            })
            .connect(db_path)
            .await;
        match pool_res {
            Ok(pool) => Ok(DBService { pool }),
            Err(err) => Err(AppError::DBInitError {
                path: db_path.to_owned(),
                source: err,
            }),
        }
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }

    pub(crate) async fn get_valid_token(&self, path: &str) -> Result<Option<ValidToken>> {
        let tok = get_valid_token(&self.pool, path).await?;
        Ok(tok.map(ValidToken::Fresh))
    }

    pub(crate) async fn create_token<'input>(
        &self,
        ct: CreateToken<'input>,
    ) -> Result<StdResult<Token, TokenError>> {
        tracing::info!("Creating a token: {ct:?}");

        let mut tx = self.pool.begin().await?;

        match get_valid_token(&mut tx, ct.path).await? {
            None => (),
            Some(t) => {
                tracing::info!("Token already exist for {} at id {}", t.path, t.id);
                return Ok(Err(TokenError::AlreadyExist));
            }
        };

        let tok = sqlx::query_as::<_, Token>(
            "INSERT INTO token
            (path, max_size_mib, valid_until, content_expires_after_hours)
            VALUES (?,?,?,?)
            RETURNING *",
        )
        .bind(ct.path)
        .bind(ct.max_size_mib)
        .bind(ct.valid_until)
        .bind(ct.content_expires_after_hours)
        .fetch_one(&mut tx)
        .await?;
        tx.commit().await?;

        tracing::info!("Token created at path {} with id {}", tok.path, tok.id);

        Ok(Ok(tok))
    }

    pub(crate) async fn initiate_upload(&self, token_id: i64) -> Result<UploadToken> {
        let now = time::OffsetDateTime::now_utc();

        let mut tx = self.pool.begin().await?;

        let mut tok = sqlx::query_as::<_, Token>(
            "SELECT * FROM token
            WHERE id=?
            AND deleted_at IS NULL
            AND valid_until > ?
            AND used_at IS NULL
            ",
        )
        .bind(token_id)
        .bind(now)
        .fetch_optional(&mut tx)
        .await?
        .ok_or_else(|| AppError::NoTokenFound {
            reason: format!("No valid token found for id {token_id}"),
        })?;

        tok.attempt_counter += 1;

        sqlx::query(
            "UPDATE token WHERE id=?
            SET attempt_counter=?",
        )
        .bind(token_id)
        .bind(tok.attempt_counter)
        .execute(&mut tx)
        .await?;

        tx.commit().await?;

        Ok(UploadToken {
            token_id,
            attempt_counter: tok.attempt_counter,
        })
    }

    pub(crate) async fn create_file(&self, ut: &UploadToken, mime_type: &str) -> Result<File> {
        let f = sqlx::query_as::<_, File>(
            "INSERT INTO file
            (token_id, attempt_counter, mime_type)
            RETURNING *
            ",
        )
        .bind(ut.token_id)
        .bind(ut.attempt_counter)
        .bind(mime_type)
        .fetch_one(&self.pool)
        .await?;

        Ok(f)
    }

    pub(crate) async fn finalise_file_upload(&self, file: File) -> Result<()> {
        let now = time::OffsetDateTime::now_utc();
        sqlx::query("UPDATE file WHERE id=? SET completed_at=?")
            .bind(file.id)
            .bind(now)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub(crate) async fn finalise_token_upload(&self, ut: UploadToken) -> Result<()> {
        let now = time::OffsetDateTime::now_utc();
        sqlx::query("UPDATE token WHERE id=? SET used_at=?")
            .bind(ut.token_id)
            .bind(now)
            .execute(&self.pool)
            .await?;

        Ok(())
    }
}

async fn get_valid_token<'t, E>(executor: E, path: &str) -> Result<Option<Token>>
where
    E: sqlx::SqliteExecutor<'t>,
{
    let now = time::OffsetDateTime::now_utc();
    let tok = sqlx::query_as::<_, Token>(
        "SELECT * FROM token WHERE path=?
        AND deleted_at IS NULL
        AND valid_until > ?
        AND used_at IS NULL
        LIMIT 1",
    )
    .bind(path)
    .bind(now)
    .fetch_optional(executor)
    .await?;

    Ok(tok)
}

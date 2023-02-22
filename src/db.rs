use sqlx::types::time::OffsetDateTime;
use sqlx::Transaction;
use sqlx::{sqlite::SqlitePoolOptions, Executor, Pool, Row, Sqlite};
use std::result::Result as StdResult;

use crate::error::{AppError, DBErrorContext, Result};

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
    pub(crate) id: i64,
    pub(crate) path: String,
    pub(crate) attempt_counter: i64,
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

impl ValidToken {
    pub(crate) fn get_token(&self) -> Option<&Token> {
        match self {
            ValidToken::Fresh(tok) => Some(tok),
        }
    }

    pub(crate) fn into_inner(self) -> Token {
        match self {
            ValidToken::Fresh(tok) => tok,
        }
    }
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

        let mut tx = self.pool.begin().await.with_context(|| {
            format!(
                "cannot begin transaction to create token at path {}",
                ct.path
            )
        })?;

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
        .await
        .with_context(|| format!("cannot create token for path {}", ct.path))?;
        tx.commit()
            .await
            .with_context(|| "cannot commit transaction to create token")?;

        tracing::info!("Token created at path {} with id {}", tok.path, tok.id);

        Ok(Ok(tok))
    }

    pub(crate) async fn initiate_upload(&self, token: ValidToken) -> Result<UploadToken> {
        let now = time::OffsetDateTime::now_utc();

        let token = token.into_inner();
        let mut tx = self.pool.begin().await.with_context(|| {
            format!(
                "cannot begin transaction to initiate upload for token {}",
                token.id
            )
        })?;

        let mut tok = sqlx::query_as::<_, Token>(
            "SELECT * FROM token
            WHERE id=?
            AND deleted_at IS NULL
            AND valid_until > ?
            AND used_at IS NULL
            ",
        )
        .bind(token.id)
        .bind(now)
        .fetch_optional(&mut tx)
        .await
        .with_context(|| format!("failed to find a valid token for id {}", token.id))?
        .ok_or_else(|| AppError::NoTokenFound {
            reason: format!("no valid token found for id {}", token.id),
        })?;

        tok.attempt_counter += 1;

        sqlx::query("UPDATE token SET attempt_counter=? WHERE id=?")
            .bind(tok.attempt_counter)
            .bind(token.id)
            .execute(&mut tx)
            .await
            .with_context(|| format!("cannot set attempt counter for token {}", token.id))?;

        tx.commit().await.with_context(|| {
            format!(
                "cannot commit tx when initiating upload for token {}",
                token.id
            )
        })?;

        Ok(UploadToken {
            id: token.id,
            path: token.path,
            attempt_counter: tok.attempt_counter,
        })
    }

    pub(crate) async fn create_file(
        &self,
        ut: &UploadToken,
        backend_type: &str,
        backend_data: String,
        mime_type: Option<&str>,
    ) -> Result<File> {
        let f = sqlx::query_as::<_, File>(
            "INSERT INTO file
            (token_id, attempt_counter, backend_type, backend_data, mime_type)
            VALUES
            (?,?,?,?,?)
            RETURNING *",
        )
        .bind(ut.id)
        .bind(ut.attempt_counter)
        .bind(backend_type)
        .bind(backend_data)
        .bind(mime_type)
        .fetch_one(&self.pool)
        .await
        .with_context(|| {
            format!(
                "cannot create file for token {} and attempt {}",
                ut.id, ut.attempt_counter
            )
        })?;

        Ok(f)
    }

    pub(crate) async fn finalise_file_upload(
        &self,
        file: File,
        backend_data: Option<String>,
    ) -> Result<()> {
        if let Some(data) = backend_data {
            sqlx::query("UPDATE file SET backend_data=? WHERE id=?")
                .bind(data)
                .bind(file.id)
                .execute(&self.pool)
                .await
                .with_context(|| {
                    format!("error seting final data for file upload for id {}", file.id)
                })?;
        }
        tracing::info!("setting completed at for id {}", file.id);
        sqlx::query("UPDATE file SET completed_at=? WHERE id=?")
            .bind(time::OffsetDateTime::now_utc())
            .bind(file.id)
            .execute(&self.pool)
            .await
            .with_context(|| format!("error finalising file upload for id {}", file.id))?;

        Ok(())
    }

    pub(crate) async fn finalise_token_upload(&self, ut: UploadToken) -> Result<()> {
        let now = time::OffsetDateTime::now_utc();
        let token = sqlx::query_as::<_, Token>("SELECT * from token where id=?")
            .bind(ut.id)
            .fetch_one(&self.pool)
            .await
            .with_context(|| format!("cannot find token to finalise upload for id {}", ut.id))?;

        let expires_at = token
            .content_expires_after_hours
            .map(|h| now + std::time::Duration::from_secs(60 * (h as u64)));

        let x = sqlx::query(
            "UPDATE token SET used_at=?, content_expires_at=? WHERE id=? AND attempt_counter=?",
        )
        .bind(now)
        .bind(expires_at)
        .bind(ut.id)
        // need to add the attempt counter in the where to avoid races if two
        // concurrent uploads (vanishingly unlikely)
        .bind(ut.attempt_counter)
        .execute(&self.pool)
        .await
        .with_context(|| format!("cannot finalize token upload for id {}", ut.id))?;

        tracing::info!("return from the update finalise token {:?}", x);

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
    .await
    .with_context(|| format!("cannot get a valid token at path {}", &path))?;

    Ok(tok)
}

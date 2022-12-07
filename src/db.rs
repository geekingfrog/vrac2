// pub(crate) type Pool = sqlx

use sqlx::types::time::OffsetDateTime;
use sqlx::Transaction;
use sqlx::{sqlite::SqlitePoolOptions, Executor, Pool, Row, Sqlite};
use std::result::Result as StdResult;

use crate::error::Result;

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
}

#[derive(Debug)]
pub(crate) struct CreateToken<'input> {
    pub(crate) path: &'input str,
    pub(crate) max_size_mib: Option<i64>,
    pub(crate) valid_until: OffsetDateTime,
    pub(crate) content_expires_after_hours: Option<i64>,
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
        let pool = SqlitePoolOptions::new()
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
            .await?;
        Ok(DBService { pool })
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
}

async fn get_valid_token<'t, E>(executor: E, path: &str) -> Result<Option<Token>>
where
    E: sqlx::SqliteExecutor<'t>,
{
    let now = time::OffsetDateTime::now_utc();
    let tok = sqlx::query_as::<_, Token>(
        "select * from token where path=?
        and ((deleted_at is NULL) or (valid_until < ?))
        LIMIT 1",
    )
    .bind(&path)
    .bind(now)
    .fetch_optional(executor)
    .await?;

    Ok(tok)
}

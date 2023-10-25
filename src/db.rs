use sqlx::types::time::OffsetDateTime;
use sqlx::{sqlite::SqlitePoolOptions, Executor, Pool, Sqlite};
use std::result::Result as StdResult;

use crate::error::{AppError, DBErrorContext, Result};

#[derive(Debug, Clone)]
pub struct DBService {
    pool: Pool<Sqlite>,
}

#[derive(sqlx::FromRow, Debug)]
#[allow(dead_code)]
pub(crate) struct DbToken {
    pub(crate) id: i64,
    /// the path in the url
    pub(crate) path: String,
    /// at most that many MiB for the sum of all files to be associated with this token
    pub(crate) max_size_mib: Option<i64>,

    /// This token will expires after this date. This field only has meaning until
    /// some files are uploaded sucessfully, after which it becomes moot.
    pub(crate) valid_until: OffsetDateTime,

    /// Creation date
    pub(crate) created_at: OffsetDateTime,

    /// How long this token (and the associated files) should be kept after the upload
    pub(crate) content_expires_after_hours: Option<i64>,

    /// When is this token has been deleted (not sure I need that)
    pub(crate) deleted_at: Option<OffsetDateTime>,

    /// Counter to keep track of which files are associated to this token.
    /// This is required because a request can fail midway when uploading some files.
    /// In this case, the associated files should be considered up for deletion and
    /// not be displayed for this token.
    pub(crate) attempt_counter: i64,

    /// When has this token be used to sucessfully upload some files
    pub(crate) used_at: Option<OffsetDateTime>,

    /// this token and the associated files are considered expired (and will be deleted
    /// asynchronously) after this date
    pub(crate) content_expires_at: Option<OffsetDateTime>,

    /// an identifier for the type of storage to use for this token.
    pub(crate) backend_type: String,
}

#[derive(Debug)]
pub(crate) struct CreateToken<'input> {
    pub(crate) path: &'input str,
    pub(crate) max_size_mib: Option<i64>,
    pub(crate) valid_until: OffsetDateTime,
    pub(crate) content_expires_after_hours: Option<i64>,
    pub(crate) backend_type: &'input str,
}

#[derive(sqlx::FromRow, Debug)]
pub struct DbFile {
    pub id: i64,
    pub token_id: i64,
    pub attempt_counter: i64,
    pub mime_type: Option<String>,
    pub name: Option<String>,
    pub backend_type: String,
    pub backend_data: String,
    pub created_at: OffsetDateTime,
    pub completed_at: Option<OffsetDateTime>,
}

#[derive(sqlx::FromRow, Debug)]
pub struct DbFileMetadata {
    pub size_b: Option<i64>,
    pub mime_type: Option<String>,
    // TODO: would be cool to have a sha256
}

// used to deserialize from join
#[derive(sqlx::FromRow, Debug)]
struct FileAndMetadata {
    id: i64,
    token_id: i64,
    attempt_counter: i64,
    mime_type: Option<String>,
    name: Option<String>,
    backend_type: String,
    backend_data: String,
    created_at: OffsetDateTime,
    completed_at: Option<OffsetDateTime>,
    size_b: Option<i64>,
}

impl std::convert::From<FileAndMetadata> for (DbFile, DbFileMetadata) {
    fn from(x: FileAndMetadata) -> Self {
        (
            DbFile {
                id: x.id,
                token_id: x.token_id,
                attempt_counter: x.attempt_counter,
                mime_type: x.mime_type.clone(),
                name: x.name,
                backend_type: x.backend_type,
                backend_data: x.backend_data,
                created_at: x.created_at,
                completed_at: x.completed_at,
            },
            DbFileMetadata {
                size_b: x.size_b,
                mime_type: x.mime_type,
            },
        )
    }
}

#[derive(sqlx::FromRow, Debug)]
pub struct Account {
    pub id: i64,
    pub username: String,
    pub phc: String,
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

#[derive(Debug)]
pub(crate) enum GetTokenResult {
    /// not found (or expired, but since there can be many expired token,
    /// just ignore that, since in practice this difference shouldn't have any
    /// impact)
    NotFound,
    /// token exists and can be used to upload stuff
    Fresh(DbToken),
    /// token exists, is valid, and can be used to see/display stuff
    Used(DbToken),
}

impl DBService {
    pub async fn new(db_path: &str) -> Result<Self> {
        tracing::info!("starting sqlite at {db_path}");
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
            Ok(pool) => {
                tracing::info!("Using sqlite at {}", db_path);
                Ok(DBService { pool })
            }
            Err(err) => Err(AppError::DBInitError {
                path: db_path.to_owned(),
                source: err,
            }),
        }
    }

    /// close the underlying connection pool. This is required when
    /// running a short query in a self contained binary, since
    /// some transaction may not have been flushed to disk yet
    pub async fn close(&self) {
        self.pool.close().await;
    }

    pub async fn migrate(&self) -> Result<()> {
        tracing::info!("starting migration");
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        tracing::info!("migration done");
        Ok(())
    }

    /// a non deleted token that can be used to upload some files.
    pub(crate) async fn get_valid_token(&self, path: &str) -> Result<GetTokenResult> {
        get_valid_token(&self.pool, path).await
    }

    /// a non deleted token already associated with files.
    pub(crate) async fn get_valid_file(&self, path: &str, file_id: i64) -> Result<Option<DbFile>> {
        get_valid_file(&self.pool, path, file_id).await
    }

    pub async fn get_files(
        &self,
        token_id: i64,
        attempt_counter: i64,
    ) -> Result<Vec<(DbFile, DbFileMetadata)>> {
        let tmp = sqlx::query_as::<_, FileAndMetadata>("SELECT f.*, m.* from file as f JOIN file_metadata as m ON f.id = m.file_id where f.token_id = ? AND f.attempt_counter=?")
            .bind(token_id)
            .bind(attempt_counter)
            .fetch_all(&self.pool)
            .await
            .with_context(|| format!("cannot get files for token with id {token_id}"))?;

        let res = tmp.into_iter().map(|x| x.into()).collect();

        Ok(res)
    }

    pub(crate) async fn create_token<'input>(
        &self,
        ct: CreateToken<'input>,
    ) -> Result<StdResult<DbToken, TokenError>> {
        tracing::info!("Creating a token: {ct:?}");

        let mut tx = self.pool.begin().await.with_context(|| {
            format!(
                "cannot begin transaction to create token at path {}",
                ct.path
            )
        })?;

        let t = get_valid_token(&mut *tx, ct.path).await?;
        tracing::debug!("valid token for path {} is: {:?}", ct.path, t);

        match get_valid_token(&mut *tx, ct.path).await? {
            GetTokenResult::Used(t) | GetTokenResult::Fresh(t) => {
                tracing::info!("Token already exist for {} at id {}", t.path, t.id);
                return Ok(Err(TokenError::AlreadyExist));
            }
            _ => (),
        };

        let tok = sqlx::query_as::<_, DbToken>(
            "INSERT INTO token
            (path, max_size_mib, valid_until, content_expires_after_hours, backend_type)
            VALUES (?,?,?,?,?)
            RETURNING *",
        )
        .bind(ct.path)
        .bind(ct.max_size_mib)
        .bind(ct.valid_until)
        .bind(ct.content_expires_after_hours)
        .bind(ct.backend_type)
        .fetch_one(&mut *tx)
        .await
        .with_context(|| format!("cannot create token for path {}", ct.path))?;
        tx.commit()
            .await
            .with_context(|| "cannot commit transaction to create token")?;

        tracing::info!("Token created at path {} with id {}", tok.path, tok.id);

        Ok(Ok(tok))
    }

    pub(crate) async fn initiate_upload(&self, token: DbToken) -> Result<UploadToken> {
        let now = time::OffsetDateTime::now_utc();

        let mut tx = self.pool.begin().await.with_context(|| {
            format!(
                "cannot begin transaction to initiate upload for token {}",
                token.id
            )
        })?;

        let mut tok = sqlx::query_as::<_, DbToken>(
            "SELECT * FROM token
            WHERE id=?
            AND deleted_at IS NULL
            AND valid_until > ?
            AND used_at IS NULL
            ",
        )
        .bind(token.id)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await
        .with_context(|| format!("failed to find a valid token for id {}", token.id))?
        .ok_or_else(|| AppError::NoTokenFound {
            reason: format!("no valid token found for id {}", token.id),
        })?;

        tok.attempt_counter += 1;

        sqlx::query("UPDATE token SET attempt_counter=? WHERE id=?")
            .bind(tok.attempt_counter)
            .bind(token.id)
            .execute(&mut *tx)
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
        file_name: Option<&str>,
    ) -> Result<DbFile> {
        let f = sqlx::query_as::<_, DbFile>(
            "INSERT INTO file
            (token_id, attempt_counter, backend_type, backend_data, mime_type, name)
            VALUES
            (?,?,?,?,?,?)
            RETURNING *",
        )
        .bind(ut.id)
        .bind(ut.attempt_counter)
        .bind(backend_type)
        .bind(backend_data)
        .bind(mime_type)
        .bind(file_name)
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
        file: DbFile,
        backend_data: Option<String>,
        metadata: DbFileMetadata,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await.with_context(|| {
            format!(
                "cannot begin transaction finalize file upload with file_id {}",
                file.id
            )
        })?;

        if let Some(data) = backend_data {
            sqlx::query("UPDATE file SET backend_data=? WHERE id=?")
                .bind(data)
                .bind(file.id)
                .execute(&mut *tx)
                .await
                .with_context(|| {
                    format!("error seting final data for file upload for id {}", file.id)
                })?;
        }
        tracing::info!("setting completed at for id {}", file.id);
        sqlx::query("UPDATE file SET completed_at=? WHERE id=?")
            .bind(time::OffsetDateTime::now_utc())
            .bind(file.id)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("error finalising file upload for id {}", file.id))?;

        sqlx::query("INSERT INTO file_metadata (file_id, size_b, mime_type) VALUES (?, ?, ?)")
            .bind(file.id)
            .bind(metadata.size_b)
            .bind(metadata.mime_type)
            .execute(&mut *tx)
            .await
            .with_context(|| {
                format!(
                    "error writing metadata for file upload with file_id {}",
                    file.id
                )
            })?;

        tx.commit().await.with_context(|| {
            format!(
                "failed to commit transaction when finalizing file upload for file_id {}",
                file.id
            )
        })?;

        Ok(())
    }

    pub(crate) async fn finalise_token_upload(&self, ut: UploadToken) -> Result<()> {
        let now = time::OffsetDateTime::now_utc();
        let token = sqlx::query_as::<_, DbToken>("SELECT * from token where id=?")
            .bind(ut.id)
            .fetch_one(&self.pool)
            .await
            .with_context(|| format!("cannot find token to finalise upload for id {}", ut.id))?;

        let expires_at = token
            .content_expires_after_hours
            .map(|h| now + std::time::Duration::from_secs(3600 * (h as u64)));

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

    pub(crate) async fn get_files_to_delete(&self, now: &OffsetDateTime) -> Result<Vec<DbFile>> {
        sqlx::query_as::<_, DbFile>(
            "SELECT f.* from file as f
            INNER JOIN token as t
            ON t.id = f.token_id
            WHERE (t.content_expires_at <= ?)
            OR (t.attempt_counter > f.attempt_counter)
            OR (used_at IS NULL AND valid_until <= ?)",
        )
        .bind(now)
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .with_context(|| "failed to fetch files to delete".to_string())
    }

    /// Delete the token in DB that are expired (used or not)
    /// This doesn't do anything with the potential associated files.
    pub(crate) async fn delete_expired_tokens(
        &self,
        now: &OffsetDateTime,
    ) -> Result<Vec<(i64, String)>> {
        let deleted_ids = sqlx::query_as::<_, (i64, String)>(
            "DELETE from token
            WHERE (content_expires_at <= ?)
            OR (used_at IS NULL AND valid_until <= ?)
            RETURNING id,path",
        )
        .bind(now)
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .with_context(|| "Cannot delete expired tokens")?;
        Ok(deleted_ids)
    }

    /// Remove from the DB the files for the given ids
    pub(crate) async fn delete_files<Ids>(&self, ids: Ids) -> Result<()>
    where
        Ids: IntoIterator<Item = i64>,
    {
        let mut tx = self
            .pool
            .begin()
            .await
            .with_context(|| "Cannot begin transaction to delete files")?;

        // A loop is fine with sqlite (in a tx, to avoid fsync after each call).
        // If using a remote DB like postgres, would need
        // something different, and at that point, this doc may be handy:
        // https://github.com/launchbadge/sqlx/blob/main/FAQ.md#how-can-i-do-a-select--where-foo-in--query
        for id in ids {
            sqlx::query("DELETE from file where id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await
                .with_context(|| format!("Cannot delete db file with id {}", id))?;
        }

        tx.commit()
            .await
            .with_context(|| "Cannot commit transaction to delete files")?;
        Ok(())
    }

    pub(crate) async fn get_account(&self, username: &str) -> Result<Option<Account>> {
        sqlx::query_as::<_, Account>("SELECT * from account where username = ?")
            .bind(username)
            .fetch_optional(&self.pool)
            .await
            .with_context(|| format!("Unable to find account with username {}", username))
    }

    pub async fn create_account(&self, username: &str, phc: &str) -> Result<Account> {
        sqlx::query_as::<_, Account>(
            "INSERT INTO account
            (username, phc) VALUES (?,?)
            RETURNING *",
        )
        .bind(username)
        .bind(phc)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("Unable to create account with username {username}"))
    }

    pub async fn change_password(&self, username: &str, phc: &str) -> Result<Account> {
        sqlx::query_as::<_, Account>(
            "UPDATE account
            SET phc=?
            WHERE username = ?
            RETURNING *",
        )
        .bind(phc)
        .bind(username)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("Unable to update account with username {username}"))
    }
}

// CREATE TABLE token
// ( id INTEGER PRIMARY KEY NOT NULL
// , path TEXT NOT NULL
// , max_size_mib INTEGER
// , valid_until TEXT NOT NULL -- datetime
// , content_expires_after_hours INTEGER
// , created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now', 'utc')) -- datetime
// , deleted_at TEXT -- datetime
// -- because we can't resume aborted upload, a strictly increasing counter
// -- is associated to uploaded files, and that allow one to delete stray
// -- files from previous attempts, without having to invalidate the token.
// , attempt_counter INTEGER DEFAULT 0
// , used_at TEXT -- datetime
// , content_expires_at TEXT -- datetime
// ) STRICT;

// CREATE TABLE file
// ( id INTEGER PRIMARY KEY NOT NULL
// , token_id INTEGER NOT NULL
// , attempt_counter INTEGER NOT NULL
// , mime_type TEXT
// , name TEXT
// -- identifier to allow different backend, like local filesystem, or S3
// , backend_type TEXT NOT NULL
// , backend_data TEXT NOT NULL -- JSON
// , created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now', 'utc')) -- datetime
// , completed_at TEXT -- datetime
// , FOREIGN KEY(token_id) REFERENCES token(id)
// ) STRICT;

async fn get_valid_token<'t, E>(executor: E, path: &str) -> Result<GetTokenResult>
where
    E: sqlx::SqliteExecutor<'t>,
{
    let now = time::OffsetDateTime::now_utc();
    let tokens = sqlx::query_as::<_, DbToken>(
        "SELECT * FROM token WHERE path=?
        AND deleted_at IS NULL
        AND (
            valid_until > ?
            OR (content_expires_at is NULL OR content_expires_at > ?)
        )
        LIMIT 1",
    )
    .bind(path)
    .bind(now)
    .bind(now)
    // there can be several used token.
    .fetch_all(executor)
    .await
    .with_context(|| format!("cannot get a valid fresh token at path {}", &path))?;

    // for all the tokens we got here, we can have three states:
    //   * fresh
    //   * used and not yet expired
    //   * used and expired
    // but by construction, at any point in time, there can only be one token that at most
    // that is either fresh or used and not yet expired
    for tok in tokens {
        if tok.used_at.is_none() {
            return Ok(GetTokenResult::Fresh(tok));
        } else {
            let now = OffsetDateTime::now_utc();
            match (tok.content_expires_after_hours, tok.content_expires_at) {
                (None, _) | (_, None) => return Ok(GetTokenResult::Used(tok)),
                (_, Some(expires_at)) if expires_at > now => return Ok(GetTokenResult::Used(tok)),
                _ => (),
            }
        }
    }

    Ok(GetTokenResult::NotFound)
}

async fn get_valid_file<'t, E>(executor: E, path: &str, file_id: i64) -> Result<Option<DbFile>>
where
    E: sqlx::SqliteExecutor<'t>,
{
    let now = time::OffsetDateTime::now_utc();

    sqlx::query_as::<_, DbFile>(
        "SELECT f.* from file as f INNER JOIN token as t ON f.token_id = t.id
        WHERE t.path=?
        AND f.id=?
        AND t.deleted_at IS NULL
        AND t.used_at IS NOT NULL
        AND (t.content_expires_after_hours IS NULL
            OR t.content_expires_at > ?
        )
        AND f.completed_at IS NOT NULL",
    )
    .bind(path)
    .bind(file_id)
    .bind(now)
    .fetch_optional(executor)
    .await
    .with_context(|| {
        format!(
            "cannot select a valid file for token at path {} and file id {}",
            path, file_id
        )
    })
}

use std::collections::BTreeSet;

use futures::prelude::*;
use time::OffsetDateTime;

use crate::{
    db::{DBService, DbFile},
    error::Result,
    upload::{LocalFsUploader, StorageBackend},
};

pub async fn cleanup(db: &DBService, storage: &LocalFsUploader) -> Result<()> {
    let now = OffsetDateTime::now_utc();
    let files = db.get_files_to_delete(&now).await?;

    if files.is_empty() {
        return Ok(());
    }

    future::try_join_all(
        files
            .iter()
            .map(|f| async move { delete_file(storage, f).await }),
    )
    .await?;

    let token_ids: BTreeSet<_> = files.iter().map(|f| f.token_id).collect();
    tracing::info!(
        "deleted {} files associated with {} tokens",
        files.len(),
        token_ids.len()
    );

    db.delete_files(files.iter().map(|f| f.id)).await?;
    let deleted_ids = db.delete_expired_tokens(&now).await?;
    tracing::info!(
        "deleted expired tokens with ids and paths: {:?}",
        deleted_ids
    );

    Ok(())
}

async fn delete_file(storage: &LocalFsUploader, file: &DbFile) -> Result<()> {
    tracing::info!("Attempting to delete file {}", file.id);
    match file.backend_type.as_str() {
        "local_fs" => {
            let data = serde_json::from_str(&file.backend_data)?;
            storage.delete_blob(data).await?;
            tracing::info!("Successfully deleted file with id {}", file.id);
            Ok(())
        }
        "garage" => {
            let data = serde_json::from_str(&file.backend_data)?;
            storage.delete_blob(data).await?;
            tracing::info!("Successfully deleted file with id {}", file.id);
            Ok(())
        }
        bt => {
            tracing::error!("Unknown backend type {bt} for file {}", file.id);
            Ok(())
        }
    }
}

use std::collections::BTreeSet;

use futures::prelude::*;
use time::OffsetDateTime;

use crate::{
    db::{DBService, DbFile},
    error::{AppError, Result},
    upload::{GarageUploader, LocalFsUploader, StorageBackend},
};

pub async fn cleanup(
    db: &DBService,
    storage: &LocalFsUploader,
    garage: &GarageUploader,
) -> Result<()> {
    let now = OffsetDateTime::now_utc();
    let files = db.get_files_to_delete(&now).await?;

    if files.is_empty() {
        return Ok(());
    }

    future::try_join_all(
        files
            .iter()
            .map(|f| async move { delete_file(storage, garage, f).await }),
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

async fn delete_file(
    storage: &LocalFsUploader,
    garage: &GarageUploader,
    file: &DbFile,
) -> Result<()> {
    tracing::info!(
        "Attempting to delete file {} (token {})",
        file.id,
        file.token_id
    );
    let res = match file.backend_type.as_str() {
        "local_fs" => {
            storage.delete_blob(file.backend_data.clone()).await
        }
        "garage" => {
            garage.delete_blob(file.backend_data.clone()).await
        }
        bt => {
            tracing::error!("Unknown backend type {bt} for file {}", file.id);
            Ok(())
        }
    };
    match res {
        Ok(_) => {
            tracing::info!("Successfully deleted file with id {}", file.id);
            Ok(())
        }
        Err(err) => Err(AppError::DeleteBlobError {
            file_id: file.id,
            token_id: file.token_id,
            source: Box::new(err),
        }),
    }
}

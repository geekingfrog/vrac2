use axum::extract::FromRef;
use parking_lot::RwLock;
use std::sync::Arc;
use tera::Tera;

use crate::{
    db::DBService,
    error::{AppError, Result},
    filters::humanize_size,
    upload::{GarageUploader, LocalFsUploader, StorageBackend},
};

#[derive(Debug, Clone)]
pub struct AppState {
    pub(crate) templates: Arc<RwLock<Tera>>,
    pub base_url: String,
    pub db: DBService,
    pub(crate) flash_config: axum_flash::Config,
    pub storage_fs: LocalFsUploader,
    pub garage: GarageUploader,
}

impl AppState {
    pub async fn new(
        template_path: &str,
        db_path: &str,
        storage_path: &str,
        base_url: String,
    ) -> Result<Self> {
        let mut tera = Tera::new(template_path)?;
        tera.register_filter("humanize_size", humanize_size);
        let db = DBService::new(db_path).await?;
        let flash_config = axum_flash::Config::new(axum_flash::Key::generate());
        let garage = GarageUploader::new().await?;

        Ok(Self {
            templates: Arc::new(RwLock::new(tera)),
            base_url,
            db,
            flash_config,
            storage_fs: LocalFsUploader::new(storage_path),
            garage,
        })
    }

    pub async fn get_blob(
        &self,
        backend_type: &str,
        backend_data: String,
    ) -> Result<Box<dyn tokio::io::AsyncRead + Unpin + Send>> {
        let blob: Box<dyn tokio::io::AsyncRead + Unpin + Send> = match backend_type {
            "local_fs" => {
                let blob = self.storage_fs.read_blob(backend_data).await?;
                Box::new(blob)
            }
            "garage" => {
                let blob = self.garage.read_blob(backend_data).await?;
                Box::new(blob)
            }
            wut => {
                tracing::warn!("Unknown storage backend: {wut}");
                return Err(AppError::UnknownStorageBackend(wut.to_string()));
            }
        };
        Ok(blob)
    }
}

impl FromRef<AppState> for axum_flash::Config {
    fn from_ref(state: &AppState) -> Self {
        state.flash_config.clone()
    }
}

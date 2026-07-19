use parking_lot::{RwLock, RwLockReadGuard};
use std::sync::Arc;
use tera::Tera;

use crate::{
    db::DBService,
    error::{AppError, Result},
    filters::humanize_size,
    upload::{GarageUploader, LocalFsUploader, StorageBackend},
};

pub type AppState = Arc<State>;

#[derive(Debug, Clone)]
pub struct State {
    pub(crate) templates: Arc<RwLock<Tera>>,
    pub base_url: String,
    pub db: DBService,
    pub storage_fs: LocalFsUploader,
    pub garage: GarageUploader,
}

impl State {
    pub async fn new(
        template_path: &str,
        db_path: &str,
        storage_path: &str,
        base_url: String,
    ) -> Result<AppState> {
        let mut tera = Tera::default();
        tera.register_filter("humanize_size", humanize_size);
        tera.load_from_glob(template_path)?;
        let db = DBService::new(db_path).await?;
        let garage = GarageUploader::new().await?;

        Ok(Arc::new(Self {
            templates: Arc::new(RwLock::new(tera)),
            base_url,
            db,
            storage_fs: LocalFsUploader::new(storage_path),
            garage,
        }))
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

    pub fn get_templates(&'_ self) -> RwLockReadGuard<'_, Tera> {
        if cfg!(debug_assertions) {
            let mut tera = self.templates.write();
            tera.full_reload().expect("reloading template");
        }
        self.templates.read()
    }
}

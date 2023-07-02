use axum::extract::FromRef;
use parking_lot::RwLock;
use std::sync::Arc;
use tera::Tera;

use crate::{
    db::DBService,
    upload::{GarageUploader, LocalFsUploader},
    error::Result,
};

#[derive(Debug, Clone)]
pub struct AppState {
    pub(crate) templates: Arc<RwLock<Tera>>,
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
    ) -> Result<Self> {
        let tera = Arc::new(RwLock::new(Tera::new(template_path)?));
        let db = DBService::new(db_path).await?;
        let flash_config = axum_flash::Config::new(axum_flash::Key::generate());
        let garage = GarageUploader::new().await?;

        Ok(Self {
            templates: tera,
            db,
            flash_config,
            storage_fs: LocalFsUploader::new(storage_path),
            garage,
        })
    }
}

impl FromRef<AppState> for axum_flash::Config {
    fn from_ref(state: &AppState) -> Self {
        state.flash_config.clone()
    }
}

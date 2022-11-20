use axum::extract::FromRef;
use parking_lot::RwLock;
use std::sync::Arc;
use tera::Tera;

use crate::db::DBService;

#[derive(Debug, Clone)]
pub struct AppState {
    pub(crate) templates: Arc<RwLock<Tera>>,
    pub db: DBService,
    pub(crate) flash_config: axum_flash::Config,
}

impl AppState {
    pub async fn new(template_path: &str, db_path: &str) -> Result<Self, axum::BoxError> {
        let tera = Arc::new(RwLock::new(Tera::new(template_path)?));
        let db = DBService::new(db_path).await?;
        let flash_config = axum_flash::Config::new(axum_flash::Key::generate());
        Ok(Self {
            templates: tera,
            db,
            flash_config,
        })
    }
}

impl FromRef<AppState> for axum_flash::Config {
    fn from_ref(state: &AppState) -> Self {
        state.flash_config.clone()
    }
}

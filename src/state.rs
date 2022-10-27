use parking_lot::RwLock;
use std::sync::Arc;
use tera::Tera;

use crate::db::DBService;

#[derive(Debug, Clone)]
pub struct AppState {
    pub(crate) templates: Arc<RwLock<Tera>>,
    pub db: DBService,
}

impl AppState {
    pub async fn new(template_path: &str, db_path: &str) -> Result<Self, axum::BoxError> {
        let tera = Arc::new(RwLock::new(Tera::new(template_path)?));
        let db = DBService::new(db_path).await?;
        Ok(Self { templates: tera, db })
    }
}

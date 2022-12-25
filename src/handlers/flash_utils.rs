use axum_flash::{IncomingFlashes, Level};

#[derive(serde::Serialize, Debug)]
pub(crate) struct TplFlash<'a> {
    pub(crate) level: axum_flash::Level,
    pub(crate) message: &'a str,
}

impl<'a> std::convert::From<(Level, &'a str)> for TplFlash<'a> {
    fn from((level, message): (Level, &'a str)) -> Self {
        Self { level, message }
    }
}

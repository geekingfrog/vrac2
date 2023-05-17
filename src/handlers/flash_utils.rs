use axum_flash::{IncomingFlashes, Level};
use tera::Context;

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

#[derive(serde::Serialize)]
pub(crate) struct Notif {
    pub(crate) level: NotifLevel,
    pub(crate) message: String,
}

#[derive(serde::Serialize)]
pub(crate) enum NotifLevel {
    Debug,
    Error,
    Info,
    Success,
    Warning,
}

impl std::convert::From<axum_flash::Level> for NotifLevel {
    fn from(level: axum_flash::Level) -> Self {
        match level {
            Level::Debug => NotifLevel::Debug,
            Level::Info => NotifLevel::Info,
            Level::Success => NotifLevel::Success,
            Level::Warning => NotifLevel::Warning,
            Level::Error => NotifLevel::Error,
        }
    }
}

pub(crate) fn ctx_from_flashes(flashes: &IncomingFlashes) -> Context {
    let mut ctx = Context::new();
    let mut notifications = Vec::with_capacity(flashes.len());
    for (level, message) in flashes {
        notifications.push(Notif {
            level: level.into(),
            message: message.to_owned(),
        })
    }

    ctx.insert("notifications", &notifications);
    ctx
}

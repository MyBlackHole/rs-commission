pub mod connector;
pub mod normalized;
pub mod taobao;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("{0}")]
    Invalid(String),
    #[error("平台传输失败：{0}")]
    Transport(String),
    #[error("平台返回错误：{0}")]
    Remote(String),
    #[error("平台数据持久化失败")]
    Database(#[from] sqlx::Error),
    #[error("平台 JSON 解析失败")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Shared(#[from] commission_types::error::Error),
}

pub type Result<T> = std::result::Result<T, PlatformError>;

impl PlatformError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

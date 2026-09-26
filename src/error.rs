use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("请提供有效且未过期的访问令牌")]
    Unauthorized,
    #[error("当前角色无权执行此操作")]
    Forbidden,
    #[error("记录不存在或不可访问")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("服务暂时繁忙，请使用相同幂等键重试")]
    Busy,
    #[error("内部错误")]
    Internal,
}
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }
}
impl From<commission_types::error::Error> for Error {
    fn from(value: commission_types::error::Error) -> Self {
        use commission_types::error::Error as Shared;
        match value {
            Shared::Invalid(message) => Self::Invalid(message),
            Shared::Forbidden => Self::Forbidden,
            Shared::NotFound => Self::NotFound,
            Shared::Internal => Self::Internal,
        }
    }
}
impl From<sqlx::Error> for Error {
    fn from(value: sqlx::Error) -> Self {
        if let sqlx::Error::Database(db) = &value {
            match db.code().as_deref() {
                Some("23505") => return Self::conflict("业务编号已存在，不能重复创建"),
                Some("23503") => return Self::invalid("关联记录不存在或仍被引用"),
                Some("23514") => return Self::conflict("操作违反资金或数据约束"),
                Some("40001" | "40P01" | "55P03" | "57014") => return Self::Busy,
                _ => {}
            }
        }
        tracing::error!(error = %value, "database operation failed");
        Self::Internal
    }
}
impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        tracing::error!(error = %value, "serialization failed");
        Self::Internal
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            Self::Invalid(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            Self::Busy => (StatusCode::SERVICE_UNAVAILABLE, "retryable"),
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        };
        (
            status,
            Json(json!({"error": {"code": code, "message": self.to_string()}})),
        )
            .into_response()
    }
}

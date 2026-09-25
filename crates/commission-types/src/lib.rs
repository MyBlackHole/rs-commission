//! Shared wire contracts and deterministic arithmetic. No HTTP server, runtime,
//! environment variables or database access. PostgreSQL derives are opt-in only.
pub mod domain;
pub mod error;
pub mod model;
pub mod money;
pub use domain::{Split, Terms};
pub use model::*;
pub use money::{Money, MAX_OPERATION_MINOR};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApiErrorDetail {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApiErrorEnvelope {
    pub error: ApiErrorDetail,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PageResult<T> {
    pub items: Vec<T>,
    pub limit: i64,
    pub offset: i64,
    pub has_more: bool,
}

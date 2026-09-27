pub mod auth;
pub mod client;
pub mod normalizer;
pub mod sync;
pub mod webhook;

pub use auth::{DouyinApiSigner, DouyinCredentials, DouyinMessageVerifier};
pub use client::{DouyinClient, DouyinTransport, ReqwestDouyinTransport};
pub use normalizer::{normalize_alliance_message, normalize_reconcile_page, DouyinMessage};
pub use sync::{DouyinAllianceSync, ReconcileOutcome, WebhookOutcome};

pub const RECONCILE_STREAM: &str = "douyin_alliance_reconcile";
pub const WEBHOOK_STREAM: &str = "douyin_alliance_webhook";
pub const ORDER_METHOD: &str = "alliance.getOrderList";
pub const ORDER_PATH: &str = "/alliance/getOrderList";
pub const DEFAULT_ENDPOINT: &str = "https://openapi-fxg.jinritemai.com";
pub const NORMALIZER_VERSION: i32 = 1;

pub const ALLIANCE_TAGS: &[&str] = &["804", "805", "806", "807", "808", "809"];

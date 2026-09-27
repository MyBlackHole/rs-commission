pub mod auth;
pub mod client;
pub mod normalizer;
pub mod sync;

pub use auth::{MeituanCredentials, MeituanOrderQuery, MeituanSigner};
pub use client::{MeituanClient, MeituanTransport, ReqwestMeituanTransport};
pub use normalizer::{normalize_order_page, MeituanNormalizedPage};
pub use sync::{MeituanOrderSync, SyncOutcome};

pub const ORDER_STREAM: &str = "meituan_union_order_updated";
pub const ORDER_PATH: &str = "/cps_open/common/api/v1/query_order";
pub const DEFAULT_ENDPOINT: &str = "https://media.meituan.com";
pub const NORMALIZER_VERSION: i32 = 1;

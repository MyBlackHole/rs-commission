pub mod auth;
pub mod client;
pub mod normalizer;
pub mod sync;

pub use auth::{TaobaoCredentials, TaobaoOrderQuery, TaobaoSigner};
pub use client::{ReqwestTaobaoTransport, TaobaoClient, TaobaoTransport};
pub use normalizer::normalize_order_page;
pub use sync::{SyncOutcome, TaobaoOrderSync};

pub const ORDER_STREAM: &str = "taobao_order_updated";
pub const ORDER_METHOD: &str = "taobao.tbk.sc.order.details.get";
pub const DEFAULT_ENDPOINT: &str = "https://gw.api.taobao.com/router/rest";

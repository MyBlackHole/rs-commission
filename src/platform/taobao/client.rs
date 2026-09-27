use crate::platform::{
    connector::{Capability, Platform, PlatformConnector},
    PlatformError, Result,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::BTreeMap;

use super::{auth::TaobaoOrderQuery, TaobaoSigner, DEFAULT_ENDPOINT};

#[async_trait]
pub trait TaobaoTransport: Send + Sync {
    async fn execute(&self, endpoint: &str, params: &BTreeMap<String, String>) -> Result<Value>;
}

#[derive(Clone)]
pub struct ReqwestTaobaoTransport {
    client: reqwest::Client,
}

impl Default for ReqwestTaobaoTransport {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl TaobaoTransport for ReqwestTaobaoTransport {
    async fn execute(&self, endpoint: &str, params: &BTreeMap<String, String>) -> Result<Value> {
        let response = self
            .client
            .get(endpoint)
            .query(params)
            .send()
            .await
            .map_err(|e| PlatformError::Transport(e.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| PlatformError::Transport(e.to_string()))?;
        if !status.is_success() {
            return Err(PlatformError::Transport(format!(
                "淘宝 HTTP {}",
                status.as_u16()
            )));
        }
        serde_json::from_str(&body).map_err(Into::into)
    }
}

pub struct TaobaoClient<T> {
    signer: TaobaoSigner,
    transport: T,
    endpoint: String,
}

impl<T> TaobaoClient<T> {
    pub fn new(signer: TaobaoSigner, transport: T) -> Self {
        Self {
            signer,
            transport,
            endpoint: DEFAULT_ENDPOINT.into(),
        }
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }
}

impl<T: TaobaoTransport> TaobaoClient<T> {
    pub async fn fetch_orders(
        &self,
        query: &TaobaoOrderQuery,
        request_time: DateTime<Utc>,
    ) -> Result<Value> {
        let params = self.signer.signed_order_params(query, request_time)?;
        let raw = self.transport.execute(&self.endpoint, &params).await?;
        if let Some(error) = raw.get("error_response") {
            let code = error
                .get("code")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let message = error
                .get("sub_msg")
                .or_else(|| error.get("msg"))
                .and_then(Value::as_str)
                .unwrap_or("未知淘宝错误");
            return Err(PlatformError::Remote(format!("{code}: {message}")));
        }
        Ok(raw)
    }
}

impl<T> PlatformConnector for TaobaoClient<T> {
    fn platform(&self) -> Platform {
        Platform::Taobao
    }

    fn capabilities(&self) -> &'static [Capability] {
        const CAPABILITIES: &[Capability] =
            &[Capability::OrderPull, Capability::CommissionPull];
        CAPABILITIES
    }
}

use crate::platform::{
    connector::{Capability, Platform, PlatformConnector},
    PlatformError, Result,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};

use super::{DouyinApiSigner, DEFAULT_ENDPOINT};

#[async_trait]
pub trait DouyinTransport: Send + Sync {
    async fn execute(
        &self,
        endpoint: &str,
        path: &str,
        common: &BTreeMap<String, String>,
        body: &str,
    ) -> Result<Value>;
}

#[derive(Clone)]
pub struct ReqwestDouyinTransport {
    client: reqwest::Client,
}

impl Default for ReqwestDouyinTransport {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl DouyinTransport for ReqwestDouyinTransport {
    async fn execute(
        &self,
        endpoint: &str,
        path: &str,
        common: &BTreeMap<String, String>,
        body: &str,
    ) -> Result<Value> {
        let url = format!("{}{}", endpoint.trim_end_matches('/'), path);
        let (status, response_body) = tokio::time::timeout(Duration::from_secs(15), async {
            let response = self
                .client
                .post(url)
                .query(common)
                .header("Content-Type", "application/json")
                .body(body.to_owned())
                .send()
                .await?;
            let status = response.status();
            let response_body = response.text().await?;
            Ok::<_, reqwest::Error>((status, response_body))
        })
        .await
        .map_err(|_| PlatformError::Transport("抖店 HTTP 请求超时".into()))?
        .map_err(|e| PlatformError::Transport(e.to_string()))?;

        if !status.is_success() {
            return Err(PlatformError::Transport(format!(
                "抖店 HTTP {}",
                status.as_u16()
            )));
        }
        serde_json::from_str(&response_body).map_err(Into::into)
    }
}

pub struct DouyinClient<T> {
    signer: DouyinApiSigner,
    transport: T,
    endpoint: String,
}

impl<T> DouyinClient<T> {
    pub fn new(signer: DouyinApiSigner, transport: T) -> Self {
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

impl<T: DouyinTransport> DouyinClient<T> {
    pub async fn call(
        &self,
        path: &str,
        method: &str,
        params: &Value,
        request_time: DateTime<Utc>,
    ) -> Result<Value> {
        let (common, body) = self.signer.signed_params(method, params, request_time)?;
        let raw = self
            .transport
            .execute(&self.endpoint, path, &common, &body)
            .await?;

        let code = raw
            .get("code")
            .and_then(Value::as_i64)
            .ok_or_else(|| PlatformError::Remote("抖店响应缺少 code".into()))?;
        if code != 10000 {
            let message = raw
                .get("sub_msg")
                .or_else(|| raw.get("msg"))
                .and_then(Value::as_str)
                .unwrap_or("未知抖店错误");
            return Err(PlatformError::Remote(format!("{code}: {message}")));
        }
        Ok(raw)
    }
}

impl<T> PlatformConnector for DouyinClient<T> {
    fn platform(&self) -> Platform {
        Platform::Douyin
    }

    fn capabilities(&self) -> &'static [Capability] {
        const CAPABILITIES: &[Capability] = &[
            Capability::OrderPull,
            Capability::OrderWebhook,
            Capability::RefundWebhook,
            Capability::CommissionPull,
            Capability::CommissionWebhook,
            Capability::SettlementPull,
            Capability::SettlementWebhook,
        ];
        CAPABILITIES
    }
}

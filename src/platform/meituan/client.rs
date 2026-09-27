use crate::platform::{
    connector::{Capability, Platform, PlatformConnector},
    meituan::{MeituanOrderQuery, MeituanSigner, DEFAULT_ENDPOINT, ORDER_PATH},
    PlatformError, Result,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};

#[async_trait]
pub trait MeituanTransport: Send + Sync {
    async fn execute(
        &self,
        endpoint: &str,
        path: &str,
        headers: &BTreeMap<String, String>,
        body: &str,
    ) -> Result<Value>;
}

#[derive(Clone)]
pub struct ReqwestMeituanTransport {
    client: reqwest::Client,
}

impl Default for ReqwestMeituanTransport {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl MeituanTransport for ReqwestMeituanTransport {
    async fn execute(
        &self,
        endpoint: &str,
        path: &str,
        headers: &BTreeMap<String, String>,
        body: &str,
    ) -> Result<Value> {
        let url = format!("{}{}", endpoint.trim_end_matches('/'), path);
        let (status, response_body) = tokio::time::timeout(Duration::from_secs(15), async {
            let mut request = self.client.post(url);
            for (name, value) in headers {
                request = request.header(name.as_str(), value.as_str());
            }
            let response = request.body(body.to_owned()).send().await?;
            let status = response.status();
            let response_body = response.text().await?;
            Ok::<_, reqwest::Error>((status, response_body))
        })
        .await
        .map_err(|_| PlatformError::Transport("美团联盟 HTTP 请求超时".into()))?
        .map_err(|error| PlatformError::Transport(error.to_string()))?;

        if !status.is_success() {
            return Err(PlatformError::Transport(format!(
                "美团联盟 HTTP {}",
                status.as_u16()
            )));
        }
        serde_json::from_str(&response_body).map_err(Into::into)
    }
}

pub struct MeituanClient<T> {
    signer: MeituanSigner,
    transport: T,
    endpoint: String,
}

impl<T> MeituanClient<T> {
    pub fn new(signer: MeituanSigner, transport: T) -> Self {
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OrderRequest<'a> {
    start_time: i64,
    end_time: i64,
    limit: u16,
    page: u8,
    query_time_type: u8,
    search_type: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    scroll_id: Option<&'a str>,
}

impl<T: MeituanTransport> MeituanClient<T> {
    pub async fn fetch_orders(
        &self,
        query: &MeituanOrderQuery,
        request_time: DateTime<Utc>,
    ) -> Result<Value> {
        query.validate()?;
        let request = OrderRequest {
            start_time: query.start.timestamp(),
            end_time: query.end.timestamp(),
            limit: 100,
            page: 1,
            query_time_type: 2,
            search_type: 2,
            scroll_id: query.scroll_id.as_deref(),
        };
        let body = serde_json::to_string(&request)?;
        let headers = self
            .signer
            .signed_headers(&body, request_time.timestamp_millis())?;
        let raw = self
            .transport
            .execute(&self.endpoint, ORDER_PATH, &headers, &body)
            .await?;

        let code = raw
            .get("code")
            .and_then(Value::as_i64)
            .ok_or_else(|| PlatformError::Remote("美团联盟响应缺少数值 code".into()))?;
        if code != 0 {
            let message = raw
                .get("message")
                .or_else(|| raw.get("msg"))
                .and_then(Value::as_str)
                .unwrap_or("未知美团联盟错误");
            return Err(PlatformError::Remote(format!("{code}: {message}")));
        }
        Ok(raw)
    }
}

impl<T> PlatformConnector for MeituanClient<T> {
    fn platform(&self) -> Platform {
        Platform::Meituan
    }

    fn capabilities(&self) -> &'static [Capability] {
        const CAPABILITIES: &[Capability] = &[
            Capability::OrderPull,
            Capability::CommissionPull,
            Capability::RefundPull,
            Capability::SettlementPull,
        ];
        CAPABILITIES
    }
}

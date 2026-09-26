//! Shared HTTP SDK. Tokens and prepared writes are memory-only. No automatic
//! financial retries: callers must retain and explicitly resend PreparedWrite.
pub mod bridge;
#[cfg(not(target_arch = "wasm32"))]
pub mod native;

use commission_types::*;
use reqwest::{
    header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE},
    Url,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, thiserror::Error, serde::Serialize, serde::Deserialize)]
pub enum ClientError {
    #[error("{0}")]
    Invalid(String),
    #[error("网络失败或响应无法确认；写操作应保留原幂等键核验/重试")]
    Unknown,
    #[error("HTTP {status} / {code}: {message}")]
    Api {
        status: u16,
        code: String,
        message: String,
    },
}
impl ClientError {
    pub fn unauthorized(&self) -> bool {
        matches!(self, Self::Api { status: 401, .. })
    }
    pub fn outcome_unknown(&self) -> bool {
        matches!(
            self,
            Self::Unknown
                | Self::Api {
                    status: 500..=599,
                    ..
                }
        )
    }
}
pub type Result<T> = std::result::Result<T, ClientError>;
fn invalid(message: impl Into<String>) -> ClientError {
    ClientError::Invalid(message.into())
}

#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    origin: Url,
    bearer: HeaderValue,
    session_id: Uuid,
}

/// Deliberately not Debug/Serialize: request bodies can contain sensitive data.
#[derive(Clone)]
pub struct PreparedWrite {
    session_id: Uuid,
    path: String,
    key: String,
    body: String,
}
impl PreparedWrite {
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn path(&self) -> &str {
        &self.path
    }
}

pub const RESOURCES: &[(&str, &str)] = &[
    ("dashboard", "业务总览"),
    ("accounts", "账户管理"),
    ("referrals", "推广关系"),
    ("rules", "抽佣规则"),
    ("orders", "订单管理"),
    ("commissions", "佣金明细"),
    ("wallets", "账户余额"),
    ("payouts", "提现结算"),
    ("ledger", "账本流水"),
    ("reconciliation", "内部对账"),
    ("audit", "操作审计"),
    ("credentials", "访问凭据"),
    ("outbox", "可靠事件"),
];

/// These match the backend API, not arbitrary URLs supplied by a UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    CreateAccount,
    BindReferral,
    CreateRule,
    DisableRule,
    CaptureOrder,
    Refund,
    Release,
    RequestPayout,
    ApprovePayout,
    ProcessPayout,
    RejectPayout,
    PayoutOutcome,
    CreateCredential,
    RevokeCredential,
}
impl Operation {
    pub const ALL: [Self; 14] = [
        Self::CreateAccount,
        Self::BindReferral,
        Self::CreateRule,
        Self::DisableRule,
        Self::CaptureOrder,
        Self::Refund,
        Self::Release,
        Self::RequestPayout,
        Self::ApprovePayout,
        Self::ProcessPayout,
        Self::RejectPayout,
        Self::PayoutOutcome,
        Self::CreateCredential,
        Self::RevokeCredential,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::CreateAccount => "创建账户",
            Self::BindReferral => "绑定推广关系",
            Self::CreateRule => "创建规则",
            Self::DisableRule => "停用规则",
            Self::CaptureOrder => "登记已支付订单",
            Self::Refund => "退款退佣",
            Self::Release => "到期解冻",
            Self::RequestPayout => "申请提现",
            Self::ApprovePayout => "审核提现",
            Self::ProcessPayout => "登记开始执行",
            Self::RejectPayout => "驳回提现",
            Self::PayoutOutcome => "核验转账结果",
            Self::CreateCredential => "创建访问凭据",
            Self::RevokeCredential => "吊销访问凭据",
        }
    }
    pub fn needs_id(self) -> bool {
        matches!(
            self,
            Self::DisableRule
                | Self::Refund
                | Self::Release
                | Self::ApprovePayout
                | Self::ProcessPayout
                | Self::RejectPayout
                | Self::PayoutOutcome
                | Self::RevokeCredential
        )
    }
    pub fn allowed(self, actor: &Actor) -> bool {
        let roles: &[&str] = match self {
            Self::CreateAccount | Self::CreateRule | Self::DisableRule => &["operator"],
            Self::BindReferral | Self::CaptureOrder | Self::Refund => &["operator", "integrator"],
            Self::RequestPayout => &["operator", "member"],
            Self::Release => &["operator", "finance"],
            Self::ApprovePayout
            | Self::ProcessPayout
            | Self::RejectPayout
            | Self::PayoutOutcome => &["finance"],
            Self::CreateCredential | Self::RevokeCredential => &[],
        };
        actor.require(roles).is_ok()
    }
    fn encode(self, id: Option<Uuid>, json: &str) -> Result<(String, String)> {
        fn body<T: DeserializeOwned + Serialize>(json: &str) -> Result<String> {
            let value: T =
                serde_json::from_str(json).map_err(|e| invalid(format!("请求字段无效：{e}")))?;
            serde_json::to_string(&value).map_err(|_| invalid("请求编码失败"))
        }
        let id = if self.needs_id() {
            Some(id.ok_or_else(|| invalid("此操作必须提供目标 UUID"))?)
        } else {
            None
        };
        let route = |resource: &str, action: &str| -> String {
            format!("{resource}/{}/{action}", id.expect("validated target UUID"))
        };
        let (path, data) = match self {
            Self::CreateAccount => ("accounts".into(), body::<CreateAccount>(json)?),
            Self::BindReferral => ("referrals".into(), body::<BindReferral>(json)?),
            Self::CreateRule => ("rules".into(), body::<CreateRule>(json)?),
            Self::DisableRule => (route("rules", "disable"), body::<Empty>(json)?),
            Self::CaptureOrder => ("orders".into(), body::<CaptureOrder>(json)?),
            Self::Refund => (route("orders", "refunds"), body::<RefundInput>(json)?),
            Self::Release => (route("orders", "release"), body::<Empty>(json)?),
            Self::RequestPayout => ("payouts".into(), body::<RequestPayout>(json)?),
            Self::ApprovePayout => (route("payouts", "approve"), body::<Empty>(json)?),
            Self::ProcessPayout => (route("payouts", "processing"), body::<ReasonInput>(json)?),
            Self::RejectPayout => (route("payouts", "reject"), body::<ReasonInput>(json)?),
            Self::PayoutOutcome => (route("payouts", "outcome"), body::<PayoutOutcome>(json)?),
            Self::CreateCredential => ("credentials".into(), body::<CreateCredential>(json)?),
            Self::RevokeCredential => (route("credentials", "revoke"), body::<Empty>(json)?),
        };
        Ok((path, data))
    }
    pub fn example(self) -> &'static str {
        match self {
            Self::CreateAccount => {
                r#"{"external_id":"merchant-001","name":"示例商家","kind":"merchant","parent_id":null}"#
            }
            Self::BindReferral => {
                r#"{"customer_external_id":"customer-001","promoter_id":"填写推广员 UUID"}"#
            }
            Self::CreateRule => {
                r#"{"name":"默认规则","merchant_id":null,"priority":0,"min_base_minor":"0","max_base_minor":null,"terms":{"rate_bps":1000,"fixed_minor":"0","cap_minor":null,"direct_bps":3000,"indirect_bps":1000,"freeze_seconds":604800},"effective_from":null,"effective_until":null}"#
            }
            Self::CaptureOrder => {
                r#"{"external_id":"填写唯一订单号","currency":"CNY","merchant_id":"填写商家 UUID","customer_external_id":null,"paid_minor":"10000","commission_base_minor":"10000"}"#
            }
            Self::Refund => {
                r#"{"external_id":"填写唯一退款号","amount_minor":"100","reason":"填写退款原因"}"#
            }
            Self::RequestPayout => {
                r#"{"external_id":"填写唯一提现号","account_id":"填写账户 UUID","amount_minor":"100","destination_ref":"填写已核验的收款目标标识"}"#
            }
            Self::ProcessPayout | Self::RejectPayout => r#"{"reason":"填写操作依据"}"#,
            Self::PayoutOutcome => {
                r#"{"status":"unknown","provider_reference":null,"evidence":"填写外部核验依据"}"#
            }
            Self::CreateCredential => {
                r#"{"name":"业务接入","role":"integrator","account_id":null,"expires_in_days":30,"secret":"请替换为满足 API 长度要求的随机令牌"}"#
            }
            _ => "{}",
        }
    }
}

impl ApiClient {
    pub fn new(origin: &str, token: &str) -> Result<Self> {
        let origin = validate_origin(origin)?;
        if token.is_empty() || token.trim() != token {
            return Err(invalid("请输入有效访问令牌"));
        }
        let mut bearer = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| invalid("令牌格式无效"))?;
        bearer.set_sensitive(true);
        let builder = reqwest::Client::builder();
        #[cfg(not(target_arch = "wasm32"))]
        let builder = builder
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30));
        let http = builder
            .build()
            .map_err(|_| invalid("无法初始化 HTTP 客户端"))?;
        Ok(Self {
            http,
            origin,
            bearer,
            session_id: Uuid::new_v4(),
        })
    }
    pub fn origin(&self) -> &str {
        self.origin.as_str()
    }
    fn url(&self, path: &str) -> Result<Url> {
        self.origin
            .join(&format!("api/v1/{path}"))
            .map_err(|_| invalid("无效 API 路径"))
    }
    pub async fn me(&self) -> Result<Actor> {
        self.get("me").await
    }
    pub async fn accounts(&self, offset: u32) -> Result<PageResult<Account>> {
        self.get(&format!("accounts?limit=50&offset={offset}"))
            .await
    }
    pub async fn resource(&self, resource: &str, offset: u32) -> Result<Value> {
        if !RESOURCES.iter().any(|(r, _)| *r == resource) {
            return Err(invalid("未知业务资源"));
        }
        self.get(&format!("{resource}?limit=50&offset={offset}"))
            .await
    }
    pub async fn order(&self, id: Uuid) -> Result<Value> {
        self.get(&format!("orders/{id}")).await
    }
    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send(
            self.http
                .get(self.url(path)?)
                .header(AUTHORIZATION, self.bearer.clone()),
        )
        .await
    }
    pub async fn quote(&self, input: &QuoteInput) -> Result<Value> {
        self.send(
            self.http
                .post(self.url("quotes")?)
                .header(AUTHORIZATION, self.bearer.clone())
                .json(input),
        )
        .await
    }
    pub fn prepare(
        &self,
        operation: Operation,
        id: Option<Uuid>,
        json: &str,
    ) -> Result<PreparedWrite> {
        if json.len() > 60 * 1024 {
            return Err(invalid("请求体过大"));
        }
        let (path, body) = operation.encode(id, json)?;
        Ok(PreparedWrite {
            session_id: self.session_id,
            path,
            key: Uuid::new_v4().to_string(),
            body,
        })
    }
    pub async fn execute(&self, write: &PreparedWrite) -> Result<Value> {
        if write.session_id != self.session_id {
            return Err(invalid("不能跨登录会话重放写请求"));
        }
        self.send(
            self.http
                .post(self.url(&write.path)?)
                .header(AUTHORIZATION, self.bearer.clone())
                .header(CONTENT_TYPE, "application/json")
                .header("Idempotency-Key", &write.key)
                .body(write.body.clone()),
        )
        .await
    }
    async fn send<T: DeserializeOwned>(&self, request: reqwest::RequestBuilder) -> Result<T> {
        let operation = async {
            let response = request.send().await.map_err(|_| ClientError::Unknown)?;
            let status = response.status();
            let bytes = response.bytes().await.map_err(|_| ClientError::Unknown)?;
            if !status.is_success() {
                let error = serde_json::from_slice::<ApiErrorEnvelope>(&bytes).ok();
                return Err(ClientError::Api {
                    status: status.as_u16(),
                    code: error
                        .as_ref()
                        .map_or_else(|| "http_error".into(), |e| e.error.code.clone()),
                    message: error.map_or_else(
                        || "服务返回非业务响应；请核验原请求状态".into(),
                        |e| e.error.message,
                    ),
                });
            }
            serde_json::from_slice(&bytes).map_err(|_| ClientError::Unknown)
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            operation.await
        }
        #[cfg(target_arch = "wasm32")]
        {
            use futures_util::{
                future::{select, Either},
                pin_mut,
            };
            let deadline = gloo_timers::future::TimeoutFuture::new(30_000);
            pin_mut!(operation, deadline);
            match select(operation, deadline).await {
                Either::Left((result, _)) => result,
                Either::Right(_) => Err(ClientError::Unknown),
            }
        }
    }
}

pub fn validate_origin(value: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|_| invalid("服务地址必须是完整的 HTTP(S) 地址"))?;
    let local = match url.host() {
        Some(url::Host::Domain(host)) => host == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if url.scheme() != "https" && !(url.scheme() == "http" && local) {
        return Err(invalid("非本机服务必须使用 HTTPS"));
    }
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(invalid("仅填写服务源地址，不含账号、路径、查询或片段"));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reject_insecure_or_credential_bearing_origins() {
        for url in [
            "http://example.com",
            "https://user:secret@example.com",
            "https://example.com/api",
            "https://example.com?x=1",
            "file:///tmp/a",
            "http://localhost.evil",
        ] {
            assert!(validate_origin(url).is_err(), "{url}");
        }
        for url in [
            "https://example.com",
            "http://127.0.0.1:8081",
            "http://[::1]:8081",
        ] {
            assert!(validate_origin(url).is_ok());
        }
    }
    #[test]
    fn typed_writes_reject_numeric_money_and_extra_fields() {
        let id = Uuid::nil();
        assert!(Operation::Refund
            .encode(
                Some(id),
                r#"{"external_id":"r","amount_minor":100,"reason":"r"}"#
            )
            .is_err());
        assert!(Operation::Release
            .encode(Some(id), r#"{"surprise":true}"#)
            .is_err());
        assert!(Operation::Release.encode(None, "{}").is_err());
    }
    #[test]
    fn retry_material_is_immutable_and_session_scoped() {
        let a = ApiClient::new("https://example.com", "token").unwrap();
        let write = a
            .prepare(Operation::Release, Some(Uuid::nil()), "{}")
            .unwrap();
        let retry = write.clone();
        assert_eq!(write.key(), retry.key());
        assert_eq!(write.body, retry.body);
        assert_eq!(a.session_id, write.session_id);
        assert_ne!(
            a.session_id,
            ApiClient::new("https://example.com", "other")
                .unwrap()
                .session_id
        );
    }
}

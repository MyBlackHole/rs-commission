use crate::platform::{PlatformError, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use md5::{Digest, Md5};
use sha2::Sha256;
use std::collections::BTreeMap;

use super::ORDER_PATH;

type HmacSha256 = Hmac<Sha256>;

const ACCEPT: &str = "application/json";
const CONTENT_TYPE: &str = "application/json; charset=UTF-8";
const SIGNATURE_HEADERS: &str = "S-Ca-App,S-Ca-Timestamp";

pub struct MeituanCredentials {
    app_key: String,
    app_secret: String,
}

impl MeituanCredentials {
    pub fn new(app_key: impl Into<String>, app_secret: impl Into<String>) -> Result<Self> {
        let app_key = app_key.into();
        let app_secret = app_secret.into();
        if app_key.trim().is_empty() || app_secret.is_empty() {
            return Err(PlatformError::invalid(
                "美团联盟 app_key、app_secret 均不能为空",
            ));
        }
        Ok(Self {
            app_key,
            app_secret,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeituanOrderQuery {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub scroll_id: Option<String>,
}

impl MeituanOrderQuery {
    pub fn validate(&self) -> Result<()> {
        if self.end <= self.start {
            return Err(PlatformError::invalid("美团同步结束时间必须晚于开始时间"));
        }
        if self.end - self.start > Duration::days(90) {
            return Err(PlatformError::invalid(
                "美团联盟订单查询只能覆盖最近 3 个月范围",
            ));
        }
        if self.scroll_id.as_deref().is_some_and(str::is_empty) {
            return Err(PlatformError::invalid("美团 scrollId 不能为空字符串"));
        }
        Ok(())
    }
}

pub struct MeituanSigner {
    credentials: MeituanCredentials,
}

impl MeituanSigner {
    pub fn new(credentials: MeituanCredentials) -> Self {
        Self { credentials }
    }

    pub fn signed_headers(
        &self,
        body: &str,
        timestamp_ms: i64,
    ) -> Result<BTreeMap<String, String>> {
        let content_md5 = STANDARD.encode(Md5::digest(body.as_bytes()));
        let canonical = format!(
            "POST\n{ACCEPT}\n{content_md5}\n{CONTENT_TYPE}\nS-Ca-App:{}\nS-Ca-Timestamp:{timestamp_ms}\n{ORDER_PATH}",
            self.credentials.app_key
        );
        let mut mac = HmacSha256::new_from_slice(self.credentials.app_secret.as_bytes())
            .map_err(|_| PlatformError::invalid("无法初始化美团 HMAC 签名"))?;
        mac.update(canonical.as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());

        Ok(BTreeMap::from([
            ("Accept".into(), ACCEPT.into()),
            ("Content-MD5".into(), content_md5),
            ("Content-Type".into(), CONTENT_TYPE.into()),
            ("S-Ca-App".into(), self.credentials.app_key.clone()),
            ("S-Ca-Signature".into(), signature),
            (
                "S-Ca-Signature-Headers".into(),
                SIGNATURE_HEADERS.into(),
            ),
            ("S-Ca-Timestamp".into(), timestamp_ms.to_string()),
        ]))
    }
}

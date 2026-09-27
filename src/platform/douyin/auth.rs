use crate::platform::{PlatformError, Result};
use chrono::{DateTime, FixedOffset, Utc};
use hmac::{Hmac, Mac};
use md5::{Digest as Md5Digest, Md5};
use serde_json::{Map, Number, Value};
use sha2::Sha256;
use std::collections::BTreeMap;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct DouyinCredentials {
    app_key: String,
    app_secret: String,
    access_token: String,
}

impl DouyinCredentials {
    pub fn new(
        app_key: impl Into<String>,
        app_secret: impl Into<String>,
        access_token: impl Into<String>,
    ) -> Result<Self> {
        let app_key = app_key.into();
        let app_secret = app_secret.into();
        let access_token = access_token.into();
        if app_key.trim().is_empty() || app_secret.is_empty() || access_token.trim().is_empty() {
            return Err(PlatformError::invalid(
                "抖店 app_key、app_secret、access_token 均不能为空",
            ));
        }
        Ok(Self {
            app_key,
            app_secret,
            access_token,
        })
    }

    pub fn app_key(&self) -> &str {
        &self.app_key
    }
}

pub struct DouyinApiSigner {
    credentials: DouyinCredentials,
}

impl DouyinApiSigner {
    pub fn new(credentials: DouyinCredentials) -> Self {
        Self { credentials }
    }

    pub fn signed_params(
        &self,
        method: &str,
        params: &Value,
        request_time: DateTime<Utc>,
    ) -> Result<(BTreeMap<String, String>, String)> {
        if method.trim().is_empty() {
            return Err(PlatformError::invalid("抖店 method 不能为空"));
        }
        let param_json = canonical_json(params)?;
        let china =
            FixedOffset::east_opt(8 * 3600).ok_or_else(|| PlatformError::invalid("无效时区"))?;
        let timestamp = request_time
            .with_timezone(&china)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        let v = "2";
        let pattern = format!(
            "app_key{}method{}param_json{}timestamp{}v{}",
            self.credentials.app_key, method, param_json, timestamp, v
        );
        let sign_pattern = format!(
            "{}{}{}",
            self.credentials.app_secret, pattern, self.credentials.app_secret
        );
        let mut mac = HmacSha256::new_from_slice(self.credentials.app_secret.as_bytes())
            .map_err(|_| PlatformError::invalid("无法初始化抖店 HMAC-SHA256 签名"))?;
        mac.update(sign_pattern.as_bytes());
        let sign = hex::encode(mac.finalize().into_bytes());

        let common = BTreeMap::from([
            ("method".into(), method.to_owned()),
            ("app_key".into(), self.credentials.app_key.clone()),
            ("access_token".into(), self.credentials.access_token.clone()),
            ("timestamp".into(), timestamp),
            ("v".into(), v.into()),
            ("sign_method".into(), "hmac-sha256".into()),
            ("sign".into(), sign),
        ]);
        Ok((common, param_json))
    }
}

pub struct DouyinMessageVerifier {
    app_key: String,
    app_secret: String,
}

impl DouyinMessageVerifier {
    pub fn new(app_key: impl Into<String>, app_secret: impl Into<String>) -> Result<Self> {
        let app_key = app_key.into();
        let app_secret = app_secret.into();
        if app_key.trim().is_empty() || app_secret.is_empty() {
            return Err(PlatformError::invalid(
                "抖店消息验签 app_key、app_secret 不能为空",
            ));
        }
        Ok(Self {
            app_key,
            app_secret,
        })
    }

    pub fn verify(&self, app_id: &str, event_sign: &str, body: &[u8]) -> Result<()> {
        if app_id != self.app_key {
            return Err(PlatformError::invalid("抖店消息 app-id 与当前连接不匹配"));
        }
        if event_sign.len() != 32 || !event_sign.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(PlatformError::invalid("抖店 event-sign 格式无效"));
        }
        let mut hasher = Md5::new();
        hasher.update(app_id.as_bytes());
        hasher.update(body);
        hasher.update(self.app_secret.as_bytes());
        let expected = hex::encode(hasher.finalize());
        if !expected.eq_ignore_ascii_case(event_sign) {
            return Err(PlatformError::invalid("抖店消息验签失败"));
        }
        Ok(())
    }

}

fn canonical_json(value: &Value) -> Result<String> {
    serde_json::to_string(&canonical_value(value))
        .map_err(|e| PlatformError::invalid(format!("无法序列化抖店 param_json：{e}")))
}

fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort_unstable();
            let mut sorted = Map::new();
            for key in keys {
                sorted.insert(key.clone(), canonical_value(&map[key]));
            }
            Value::Object(sorted)
        }
        Value::Array(values) => Value::Array(values.iter().map(canonical_value).collect()),
        Value::Number(number) => normalize_number(number),
        other => other.clone(),
    }
}

fn normalize_number(number: &Number) -> Value {
    if let Some(v) = number.as_i64() {
        return Value::Number(Number::from(v));
    }
    if let Some(v) = number.as_u64() {
        return Value::Number(Number::from(v));
    }
    if let Some(v) = number.as_f64() {
        if v.is_finite() && v.fract() == 0.0 && v >= i64::MIN as f64 && v <= i64::MAX as f64 {
            return Value::Number(Number::from(v as i64));
        }
    }
    Value::Number(number.clone())
}

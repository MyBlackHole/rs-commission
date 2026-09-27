use crate::platform::{PlatformError, Result};
use chrono::{DateTime, FixedOffset, Utc};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::BTreeMap;

use super::ORDER_METHOD;

type HmacSha256 = Hmac<Sha256>;

pub struct TaobaoCredentials {
    app_key: String,
    app_secret: String,
    session: String,
}

impl TaobaoCredentials {
    pub fn new(
        app_key: impl Into<String>,
        app_secret: impl Into<String>,
        session: impl Into<String>,
    ) -> Result<Self> {
        let app_key = app_key.into();
        let app_secret = app_secret.into();
        let session = session.into();
        if app_key.trim().is_empty() || app_secret.is_empty() || session.trim().is_empty() {
            return Err(PlatformError::invalid(
                "淘宝 app_key、app_secret、session 均不能为空",
            ));
        }
        Ok(Self {
            app_key,
            app_secret,
            session,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaobaoOrderQuery {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub position_index: Option<String>,
}

impl TaobaoOrderQuery {
    pub fn validate(&self) -> Result<()> {
        if self.end <= self.start {
            return Err(PlatformError::invalid("淘宝同步结束时间必须晚于开始时间"));
        }
        if self.end - self.start > chrono::Duration::hours(3) {
            return Err(PlatformError::invalid(
                "淘宝订单查询窗口不能超过官方日常上限 3 小时",
            ));
        }
        Ok(())
    }
}

pub struct TaobaoSigner {
    credentials: TaobaoCredentials,
}

impl TaobaoSigner {
    pub fn new(credentials: TaobaoCredentials) -> Self {
        Self { credentials }
    }

    pub fn signed_order_params(
        &self,
        query: &TaobaoOrderQuery,
        request_time: DateTime<Utc>,
    ) -> Result<BTreeMap<String, String>> {
        query.validate()?;
        let china =
            FixedOffset::east_opt(8 * 3600).ok_or_else(|| PlatformError::invalid("无效时区"))?;
        let fmt = |v: DateTime<Utc>| {
            v.with_timezone(&china)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        };

        let mut params = BTreeMap::from([
            ("method".into(), ORDER_METHOD.into()),
            ("app_key".into(), self.credentials.app_key.clone()),
            ("session".into(), self.credentials.session.clone()),
            ("timestamp".into(), fmt(request_time)),
            ("v".into(), "2.0".into()),
            ("format".into(), "json".into()),
            ("simplify".into(), "false".into()),
            ("sign_method".into(), "hmac-sha256".into()),
            ("query_type".into(), "4".into()),
            ("page_size".into(), "100".into()),
            ("jump_type".into(), "1".into()),
            ("order_scene".into(), "1".into()),
            ("start_time".into(), fmt(query.start)),
            ("end_time".into(), fmt(query.end)),
        ]);
        if let Some(cursor) = &query.position_index {
            if cursor.trim().is_empty() {
                return Err(PlatformError::invalid("淘宝 position_index 不能为空字符串"));
            }
            params.insert("position_index".into(), cursor.clone());
        }
        let sign = self.sign(&params)?;
        params.insert("sign".into(), sign);
        Ok(params)
    }

    fn sign(&self, params: &BTreeMap<String, String>) -> Result<String> {
        let canonical: String = params.iter().map(|(k, v)| format!("{k}{v}")).collect();
        let mut mac = HmacSha256::new_from_slice(self.credentials.app_secret.as_bytes())
            .map_err(|_| PlatformError::invalid("无法初始化淘宝 HMAC 签名"))?;
        mac.update(canonical.as_bytes());
        Ok(hex::encode_upper(mac.finalize().into_bytes()))
    }
}

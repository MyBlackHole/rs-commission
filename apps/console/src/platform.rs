//! Only transport differs by target. Components never call Tauri or fetch directly.
use commission_client::{
    bridge::WritePhase, ClientError, Operation, Result,
};
use commission_types::{Actor, QuoteInput};
use serde_json::Value;
use uuid::Uuid;

#[cfg(feature = "tauri")]
use commission_client::bridge::{NativeSessionInfo, RecoveredWriteReceipt, WriteReceipt};
#[cfg(not(feature = "tauri"))]
use commission_client::{ApiClient, PersistedWrite, PreparedWrite};
#[cfg(not(feature = "tauri"))]
use serde::{Deserialize, Serialize};

const BROWSER_PENDING_KEY: &str = "rs-commission.pending-write.v1";

#[derive(Clone)]
pub struct Client {
    #[cfg(not(feature = "tauri"))]
    inner: ApiClient,
    #[cfg(not(feature = "tauri"))]
    actor_id: Uuid,
    #[cfg(feature = "tauri")]
    id: Uuid,
}
#[derive(Clone)]
pub struct Write {
    #[cfg(not(feature = "tauri"))]
    inner: PreparedWrite,
    #[cfg(feature = "tauri")]
    inner: WriteReceipt,
}
#[derive(Clone)]
pub struct Recovery {
    pub write: Write,
    pub phase: WritePhase,
}

#[cfg(not(feature = "tauri"))]
#[derive(Serialize, Deserialize)]
struct BrowserPending {
    version: u8,
    origin: String,
    actor_id: Uuid,
    attempted: bool,
    write: PersistedWrite,
}

impl Write {
    pub fn key(&self) -> &str {
        #[cfg(not(feature = "tauri"))]
        {
            self.inner.key()
        }
        #[cfg(feature = "tauri")]
        {
            &self.inner.key
        }
    }
    pub fn path(&self) -> &str {
        #[cfg(not(feature = "tauri"))]
        {
            self.inner.path()
        }
        #[cfg(feature = "tauri")]
        {
            &self.inner.path
        }
    }
}

#[cfg(not(feature = "tauri"))]
fn browser_storage() -> Result<web_sys::Storage> {
    web_sys::window()
        .ok_or_else(|| ClientError::Invalid("浏览器窗口不可用".into()))?
        .local_storage()
        .map_err(|_| ClientError::Invalid("浏览器持久化存储不可用".into()))?
        .ok_or_else(|| ClientError::Invalid("浏览器持久化存储已禁用".into()))
}
#[cfg(not(feature = "tauri"))]
fn load_browser_pending() -> Result<Option<BrowserPending>> {
    let Some(raw) = browser_storage()?
        .get_item(BROWSER_PENDING_KEY)
        .map_err(|_| ClientError::Invalid("无法读取本地待恢复请求".into()))?
    else {
        return Ok(None);
    };
    let pending: BrowserPending = serde_json::from_str(&raw)
        .map_err(|_| ClientError::Invalid("本地待恢复请求损坏；请先核验服务器状态".into()))?;
    if pending.version != 1 {
        return Err(ClientError::Invalid(
            "本地待恢复请求版本不兼容；请先核验服务器状态".into(),
        ));
    }
    Ok(Some(pending))
}
#[cfg(not(feature = "tauri"))]
fn save_browser_pending(pending: &BrowserPending) -> Result<()> {
    let raw = serde_json::to_string(pending)
        .map_err(|_| ClientError::Invalid("无法编码本地待恢复请求".into()))?;
    browser_storage()?
        .set_item(BROWSER_PENDING_KEY, &raw)
        .map_err(|_| ClientError::Invalid("无法持久化待恢复请求；本次不会发送".into()))
}
#[cfg(not(feature = "tauri"))]
fn clear_browser_pending() -> Result<()> {
    browser_storage()?
        .remove_item(BROWSER_PENDING_KEY)
        .map_err(|_| ClientError::Invalid("无法清除本地待恢复请求".into()))
}

pub fn default_origin() -> String {
    #[cfg(not(feature = "tauri"))]
    {
        web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_default()
    }
    #[cfg(feature = "tauri")]
    {
        option_env!("COMMISSION_API_ORIGIN")
            .unwrap_or("http://127.0.0.1:8081")
            .to_owned()
    }
}

impl Client {
    pub async fn login(origin: &str, token: &str) -> Result<(Self, Actor)> {
        #[cfg(not(feature = "tauri"))]
        {
            if origin != default_origin() {
                return Err(ClientError::Invalid("浏览器仅允许同源服务".into()));
            }
            let inner = ApiClient::new(origin, token)?;
            let actor = inner.me().await?;
            Ok((
                Self {
                    inner,
                    actor_id: actor.id,
                },
                actor,
            ))
        }
        #[cfg(feature = "tauri")]
        {
            let session: NativeSessionInfo = invoke(
                "session_login",
                serde_json::json!({"origin": origin,"token": token}),
            )
            .await?;
            Ok((Self { id: session.id }, session.actor))
        }
    }

    pub async fn recover(&self) -> Result<Option<Recovery>> {
        #[cfg(not(feature = "tauri"))]
        {
            let Some(pending) = load_browser_pending()? else {
                return Ok(None);
            };
            if pending.origin != self.inner.origin() || pending.actor_id != self.actor_id {
                return Err(ClientError::Invalid(
                    "待恢复请求属于其他服务或登录身份，不能在当前会话重放".into(),
                ));
            }
            let write = self.inner.restore(&pending.write)?;
            Ok(Some(Recovery {
                write: Write { inner: write },
                phase: if pending.attempted {
                    WritePhase::Unknown
                } else {
                    WritePhase::Prepared
                },
            }))
        }
        #[cfg(feature = "tauri")]
        {
            let recovered: Option<RecoveredWriteReceipt> =
                invoke("recover_write", serde_json::json!({"session": self.id})).await?;
            Ok(recovered.map(|recovered| Recovery {
                write: Write {
                    inner: recovered.receipt,
                },
                phase: recovered.phase,
            }))
        }
    }

    pub async fn logout(&self) -> Result<()> {
        #[cfg(not(feature = "tauri"))]
        {
            Ok(())
        }
        #[cfg(feature = "tauri")]
        {
            invoke("session_logout", serde_json::json!({"session": self.id})).await
        }
    }
    pub async fn resource(&self, resource: &str, offset: u32) -> Result<Value> {
        #[cfg(not(feature = "tauri"))]
        {
            self.inner.resource(resource, offset).await
        }
        #[cfg(feature = "tauri")]
        {
            invoke(
                "read_resource",
                serde_json::json!({"session": self.id,"resource": resource,"offset": offset}),
            )
            .await
        }
    }
    pub async fn quote(&self, input: &QuoteInput) -> Result<Value> {
        #[cfg(not(feature = "tauri"))]
        {
            self.inner.quote(input).await
        }
        #[cfg(feature = "tauri")]
        {
            invoke(
                "quote_commission",
                serde_json::json!({"session": self.id,"input": input}),
            )
            .await
        }
    }
    pub async fn order(&self, order_id: Uuid) -> Result<Value> {
        #[cfg(not(feature = "tauri"))]
        {
            self.inner.order(order_id).await
        }
        #[cfg(feature = "tauri")]
        {
            invoke(
                "order_detail",
                serde_json::json!({"session": self.id,"order": order_id}),
            )
            .await
        }
    }
    pub async fn payout(&self, payout_id: Uuid) -> Result<Value> {
        #[cfg(not(feature = "tauri"))]
        {
            self.inner.payout(payout_id).await
        }
        #[cfg(feature = "tauri")]
        {
            invoke(
                "payout_detail",
                serde_json::json!({"session": self.id,"payout": payout_id}),
            )
            .await
        }
    }

    pub async fn prepare(
        &self,
        operation: Operation,
        target: Option<Uuid>,
        body: &str,
    ) -> Result<Write> {
        #[cfg(not(feature = "tauri"))]
        {
            if load_browser_pending()?.is_some() {
                return Err(ClientError::Invalid(
                    "已有待恢复请求，不能创建替代请求".into(),
                ));
            }
            let inner = self.inner.prepare(operation, target, body)?;
            save_browser_pending(&BrowserPending {
                version: 1,
                origin: self.inner.origin().to_owned(),
                actor_id: self.actor_id,
                attempted: false,
                write: inner.persisted(),
            })?;
            Ok(Write { inner })
        }
        #[cfg(feature = "tauri")]
        {
            Ok(Write { inner: invoke("prepare_write", serde_json::json!({"session": self.id,"operation": operation,"target": target,"body": body})).await? })
        }
    }

    pub async fn execute(&self, write: &Write) -> Result<Value> {
        #[cfg(not(feature = "tauri"))]
        {
            let mut pending = load_browser_pending()?
                .ok_or_else(|| ClientError::Invalid("待恢复请求不存在，本次不会发送".into()))?;
            if pending.origin != self.inner.origin()
                || pending.actor_id != self.actor_id
                || pending.write.key != write.inner.key()
            {
                return Err(ClientError::Invalid(
                    "待恢复请求与当前请求不一致，本次不会发送".into(),
                ));
            }
            pending.attempted = true;
            save_browser_pending(&pending)?;
            self.inner.execute(&write.inner).await
        }
        #[cfg(feature = "tauri")]
        {
            invoke(
                "execute_write",
                serde_json::json!({"session": self.id,"request": write.inner.id}),
            )
            .await
        }
    }

    pub async fn discard(&self, write: &Write) -> Result<()> {
        #[cfg(not(feature = "tauri"))]
        {
            if let Some(pending) = load_browser_pending()? {
                if pending.origin != self.inner.origin()
                    || pending.actor_id != self.actor_id
                    || pending.write.key != write.inner.key()
                {
                    return Err(ClientError::Invalid(
                        "待恢复请求与当前请求不一致，拒绝清除".into(),
                    ));
                }
                clear_browser_pending()?;
            }
            Ok(())
        }
        #[cfg(feature = "tauri")]
        {
            invoke(
                "discard_write",
                serde_json::json!({"session": self.id,"request": write.inner.id}),
            )
            .await
        }
    }
}

#[cfg(feature = "tauri")]
async fn invoke<T: serde::de::DeserializeOwned>(command: &str, args: Value) -> Result<T> {
    use wasm_bindgen::prelude::*;
    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "core"], js_name = invoke)]
        async fn native_invoke(
            command: &str,
            args: JsValue,
        ) -> std::result::Result<JsValue, JsValue>;
    }
    // JSON-compatible serialization uses objects, not JS Map; Tauri decodes JSON.
    let args = args
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(|_| ClientError::Invalid("无法编码本地命令".into()))?;
    use serde::Serialize;
    match native_invoke(command, args).await {
        Ok(value) => serde_wasm_bindgen::from_value(value).map_err(|_| ClientError::Unknown),
        Err(error) => Err(serde_wasm_bindgen::from_value(error).unwrap_or(ClientError::Unknown)),
    }
}

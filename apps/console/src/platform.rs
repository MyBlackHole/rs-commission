//! Only transport differs by target. Components never call Tauri or fetch directly.
use commission_client::{ClientError, Operation, Result};
use commission_types::{Actor, QuoteInput};
use serde_json::Value;
use uuid::Uuid;

#[cfg(feature = "tauri")]
use commission_client::bridge::{NativeSessionInfo, WriteReceipt};
#[cfg(not(feature = "tauri"))]
use commission_client::{ApiClient, PreparedWrite};

#[derive(Clone)]
pub struct Client {
    #[cfg(not(feature = "tauri"))]
    inner: ApiClient,
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
            Ok((Self { inner }, actor))
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
    pub async fn prepare(
        &self,
        operation: Operation,
        target: Option<Uuid>,
        body: &str,
    ) -> Result<Write> {
        #[cfg(not(feature = "tauri"))]
        {
            Ok(Write {
                inner: self.inner.prepare(operation, target, body)?,
            })
        }
        #[cfg(feature = "tauri")]
        {
            Ok(Write { inner: invoke("prepare_write", serde_json::json!({"session": self.id,"operation": operation,"target": target,"body": body})).await? })
        }
    }
    pub async fn execute(&self, write: &Write) -> Result<Value> {
        #[cfg(not(feature = "tauri"))]
        {
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
    pub async fn discard(&self, _write: &Write) -> Result<()> {
        #[cfg(not(feature = "tauri"))]
        {
            Ok(())
        }
        #[cfg(feature = "tauri")]
        {
            invoke(
                "discard_write",
                serde_json::json!({"session": self.id,"request": _write.inner.id}),
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

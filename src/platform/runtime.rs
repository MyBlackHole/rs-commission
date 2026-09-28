use crate::platform::{
    douyin::{
        DouyinAllianceSync, DouyinApiSigner, DouyinClient, DouyinCredentials,
        DouyinMessageVerifier, ReqwestDouyinTransport,
    },
    meituan::{
        MeituanClient, MeituanCredentials, MeituanOrderSync, MeituanSigner, ReqwestMeituanTransport,
    },
    taobao::{
        ReqwestTaobaoTransport, TaobaoClient, TaobaoCredentials, TaobaoOrderSync, TaobaoSigner,
    },
    PlatformError, Result,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::env;
use uuid::Uuid;

const MAX_SYNC_ERROR_LEN: usize = 512;

#[derive(Debug, Clone)]
struct Connection {
    id: Uuid,
    platform: String,
    credential_ref: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Secret {
    app_key: String,
    app_secret: String,
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    endpoint: Option<String>,
}

pub async fn list_connections(pool: &PgPool) -> Result<Value> {
    let rows: Vec<(
        Uuid,
        String,
        String,
        String,
        String,
        String,
        Option<Uuid>,
        DateTime<Utc>,
        DateTime<Utc>,
        i64,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT c.id,c.platform,c.external_account_id,c.display_name,c.connection_type,c.status,
                c.settlement_owner_account_id,c.created_at,c.updated_at,
                count(cp.stream)::bigint,
                max(cp.last_attempt_at),
                max(cp.last_success_at),
                (
                    SELECT cp2.last_error
                    FROM platform_sync_checkpoints cp2
                    WHERE cp2.connection_id=c.id AND cp2.last_error IS NOT NULL
                    ORDER BY cp2.updated_at DESC,cp2.stream
                    LIMIT 1
                )
         FROM platform_connections c
         LEFT JOIN platform_sync_checkpoints cp ON cp.connection_id=c.id
         GROUP BY c.id,c.platform,c.external_account_id,c.display_name,c.connection_type,c.status,
                  c.settlement_owner_account_id,c.created_at,c.updated_at
         ORDER BY c.created_at,c.id",
    )
    .fetch_all(pool)
    .await?;

    Ok(Value::Array(
        rows.into_iter()
            .map(
                |(
                    id,
                    platform,
                    external_account_id,
                    display_name,
                    connection_type,
                    status,
                    settlement_owner_account_id,
                    created_at,
                    updated_at,
                    checkpoint_count,
                    last_attempt_at,
                    last_success_at,
                    last_error,
                )| {
                    json!({
                        "id": id,
                        "platform": platform,
                        "external_account_id": external_account_id,
                        "display_name": display_name,
                        "connection_type": connection_type,
                        "status": status,
                        "settlement_owner_account_id": settlement_owner_account_id,
                        "created_at": created_at,
                        "updated_at": updated_at,
                        "checkpoint_count": checkpoint_count,
                        "last_attempt_at": last_attempt_at,
                        "last_success_at": last_success_at,
                        "last_error": last_error,
                    })
                },
            )
            .collect(),
    ))
}

pub async fn sync_pull_connection(
    pool: &PgPool,
    connection_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Value> {
    let connection = connection(pool, connection_id).await?;
    let result = match connection.platform.as_str() {
        "taobao" => sync_taobao(pool, &connection, now).await,
        "meituan" => sync_meituan(pool, &connection, now).await,
        "douyin" => {
            return Err(PlatformError::invalid(
                "抖音连接通过 webhook 实时接入；漏单补偿请使用 reconcile 入口",
            ));
        }
        other => {
            return Err(PlatformError::invalid(format!(
                "不支持的平台运行时：{other}"
            )));
        }
    };

    if let Err(error) = &result {
        record_failure(pool, &connection, now, error).await?;
    }
    result
}

pub async fn reconcile_douyin(
    pool: &PgPool,
    connection_id: Uuid,
    params: &Value,
    now: DateTime<Utc>,
) -> Result<Value> {
    let connection = connection(pool, connection_id).await?;
    if connection.platform != "douyin" {
        return Err(PlatformError::invalid("该连接不是 douyin"));
    }
    let secret = load_secret(&connection)?;
    let credentials = DouyinCredentials::new(
        secret.app_key,
        secret.app_secret,
        required(secret.access_token, "抖音 access_token")?,
    )?;
    let mut client = DouyinClient::new(
        DouyinApiSigner::new(credentials),
        ReqwestDouyinTransport::default(),
    );
    if let Some(endpoint) = secret.endpoint {
        client = client.with_endpoint(endpoint);
    }
    let sync = DouyinAllianceSync::new(pool.clone(), connection.id, client);
    let outcome = sync.reconcile_page(params, now).await;

    match outcome {
        Ok(outcome) => Ok(json!({
            "platform": "douyin",
            "connection_id": connection.id,
            "raw_event_id": outcome.raw_event_id,
            "order_observations": outcome.order_observations,
            "commission_observations": outcome.commission_observations,
            "refund_observations": outcome.refund_observations,
            "settlement_observations": outcome.settlement_observations,
            "next_cursor": outcome.next_cursor,
        })),
        Err(error) => {
            record_failure(pool, &connection, now, &error).await?;
            Err(error)
        }
    }
}

pub async fn ingest_douyin_webhook(
    pool: &PgPool,
    connection_id: Uuid,
    app_id: &str,
    event_sign: &str,
    body: &[u8],
    received_at: DateTime<Utc>,
) -> Result<Value> {
    let connection = connection(pool, connection_id).await?;
    if connection.platform != "douyin" {
        return Err(PlatformError::invalid("该 webhook 连接不是 douyin"));
    }
    let secret = load_secret(&connection)?;
    let verifier = DouyinMessageVerifier::new(&secret.app_key, &secret.app_secret)?;
    let credentials = DouyinCredentials::new(
        secret.app_key,
        secret.app_secret,
        required(secret.access_token, "抖音 access_token")?,
    )?;
    let mut client =
        DouyinClient::new(DouyinApiSigner::new(credentials), ReqwestDouyinTransport::default());
    if let Some(endpoint) = secret.endpoint {
        client = client.with_endpoint(endpoint);
    }
    let sync = DouyinAllianceSync::new(pool.clone(), connection.id, client);
    let outcome = sync
        .ingest_webhook(&verifier, app_id, event_sign, body, received_at)
        .await?;

    Ok(json!({
        "raw_events": outcome.raw_events,
        "duplicates": outcome.duplicates,
        "order_observations": outcome.order_observations,
        "commission_observations": outcome.commission_observations,
        "refund_observations": outcome.refund_observations,
        "settlement_observations": outcome.settlement_observations,
        "ignored_messages": outcome.ignored_messages,
    }))
}

pub async fn run_pull_cycle(pool: &PgPool, now: DateTime<Utc>) {
    let rows: Vec<(Uuid, String)> = match sqlx::query_as(
        "SELECT id,platform
         FROM platform_connections
         WHERE status='active' AND platform IN ('taobao','meituan')
         ORDER BY id",
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(%error, "platform sync worker could not list connections");
            return;
        }
    };

    for (connection_id, platform) in rows {
        for page in 1..=32 {
            match sync_pull_connection(pool, connection_id, now).await {
                Ok(outcome) => {
                    let has_more = outcome
                        .get("has_next")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                        || outcome
                            .get("next_cursor")
                            .is_some_and(|value| !value.is_null());
                    tracing::info!(
                        %connection_id,
                        %platform,
                        page,
                        has_more,
                        outcome = %outcome,
                        "platform pull sync completed"
                    );
                    if !has_more {
                        break;
                    }
                }
                Err(error) => {
                    tracing::error!(
                        %connection_id,
                        %platform,
                        page,
                        %error,
                        "platform pull sync failed"
                    );
                    break;
                }
            }
        }
    }
}

async fn sync_taobao(pool: &PgPool, connection: &Connection, now: DateTime<Utc>) -> Result<Value> {
    let secret = load_secret(connection)?;
    let credentials = TaobaoCredentials::new(
        secret.app_key,
        secret.app_secret,
        required(secret.session, "淘宝 session")?,
    )?;
    let mut client = TaobaoClient::new(
        TaobaoSigner::new(credentials),
        ReqwestTaobaoTransport::default(),
    );
    if let Some(endpoint) = secret.endpoint {
        client = client.with_endpoint(endpoint);
    }
    let sync = TaobaoOrderSync::new(pool.clone(), connection.id, client);
    let outcome = sync.sync_next(now).await?;

    Ok(json!({
        "platform": "taobao",
        "connection_id": connection.id,
        "raw_event_id": outcome.raw_event_id,
        "window_start": outcome.window_start,
        "window_end": outcome.window_end,
        "next_cursor": outcome.next_cursor,
        "has_next": outcome.has_next,
        "order_observations": outcome.order_observations,
        "commission_observations": outcome.commission_observations,
    }))
}

async fn sync_meituan(pool: &PgPool, connection: &Connection, now: DateTime<Utc>) -> Result<Value> {
    let secret = load_secret(connection)?;
    let credentials = MeituanCredentials::new(secret.app_key, secret.app_secret)?;
    let mut client = MeituanClient::new(
        MeituanSigner::new(credentials),
        ReqwestMeituanTransport::default(),
    );
    if let Some(endpoint) = secret.endpoint {
        client = client.with_endpoint(endpoint);
    }
    let sync = MeituanOrderSync::new(pool.clone(), connection.id, client);
    let outcome = sync.sync_next(now).await?;

    Ok(json!({
        "platform": "meituan",
        "connection_id": connection.id,
        "raw_event_id": outcome.raw_event_id,
        "window_start": outcome.window_start,
        "window_end": outcome.window_end,
        "next_cursor": outcome.next_cursor,
        "order_observations": outcome.order_observations,
        "commission_observations": outcome.commission_observations,
        "refund_observations": outcome.refund_observations,
        "settlement_observations": outcome.settlement_observations,
    }))
}

async fn connection(pool: &PgPool, id: Uuid) -> Result<Connection> {
    let row: Option<(Uuid, String, String, Option<String>)> = sqlx::query_as(
        "SELECT id,platform,status,credential_ref
         FROM platform_connections
         WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    match row {
        Some((id, platform, status, credential_ref)) if status == "active" => Ok(Connection {
            id,
            platform,
            credential_ref,
        }),
        Some(_) => Err(PlatformError::invalid("平台连接未启用")),
        None => Err(PlatformError::invalid("平台连接不存在")),
    }
}

fn load_secret(connection: &Connection) -> Result<Secret> {
    let credential_ref = connection
        .credential_ref
        .as_deref()
        .ok_or_else(|| PlatformError::invalid("平台连接未配置 credential_ref"))?;
    let name = credential_ref
        .strip_prefix("env:")
        .ok_or_else(|| PlatformError::invalid("credential_ref 当前仅支持 env:NAME"))?;
    if name.is_empty()
        || name.len() > 128
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(PlatformError::invalid("credential_ref 环境变量名称无效"));
    }

    let raw = env::var(name)
        .map_err(|_| PlatformError::invalid(format!("平台凭据环境变量 {name} 未配置")))?;
    let secret: Secret = serde_json::from_str(&raw)
        .map_err(|_| PlatformError::invalid(format!("平台凭据环境变量 {name} JSON 无效")))?;
    if secret.app_key.trim().is_empty() || secret.app_secret.is_empty() {
        return Err(PlatformError::invalid("平台 app_key/app_secret 不能为空"));
    }
    if secret.endpoint.as_deref().is_some_and(str::is_empty) {
        return Err(PlatformError::invalid("平台 endpoint 不能为空字符串"));
    }
    Ok(secret)
}

fn required(value: Option<String>, name: &str) -> Result<String> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| PlatformError::invalid(format!("{name} 未配置")))
}

async fn record_failure(
    pool: &PgPool,
    connection: &Connection,
    now: DateTime<Utc>,
    error: &PlatformError,
) -> Result<()> {
    let stream = match connection.platform.as_str() {
        "taobao" => crate::platform::taobao::ORDER_STREAM,
        "meituan" => crate::platform::meituan::ORDER_STREAM,
        "douyin" => crate::platform::douyin::RECONCILE_STREAM,
        _ => return Ok(()),
    };
    let detail = truncate(&error.to_string(), MAX_SYNC_ERROR_LEN);
    sqlx::query(
        "INSERT INTO platform_sync_checkpoints
         (connection_id,stream,last_attempt_at,last_error,updated_at)
         VALUES($1,$2,$3,$4,$3)
         ON CONFLICT(connection_id,stream) DO UPDATE SET
           last_attempt_at=EXCLUDED.last_attempt_at,
           last_error=EXCLUDED.last_error,
           updated_at=EXCLUDED.updated_at",
    )
    .bind(connection.id)
    .bind(stream)
    .bind(now)
    .bind(detail)
    .execute(pool)
    .await?;
    Ok(())
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_is_character_safe() {
        assert_eq!(truncate("淘宝错误abcdef", 4), "淘宝错误a");
    }

    #[test]
    fn credential_ref_requires_env_scheme() {
        let connection = Connection {
            id: Uuid::nil(),
            platform: "taobao".into(),
            credential_ref: Some("vault:secret".into()),
        };
        assert!(load_secret(&connection).is_err());
    }
}

use crate::{
    error::{Error, Result},
    model::{text, Actor},
    transaction::{Start, Work},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePlatformConnection {
    pub platform: String,
    pub external_account_id: String,
    pub display_name: String,
    pub connection_type: String,
    pub credential_ref: String,
    pub settlement_owner_account_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetPlatformConnectionStatus {
    pub status: String,
}

pub async fn create_connection(
    pool: &PgPool,
    actor: &Actor,
    key: &str,
    input: CreatePlatformConnection,
) -> Result<Value> {
    actor.require(&["integrator"])?;
    validate_create(&input)?;

    let mut work =
        match Work::begin(pool, actor, "platform.connections.create", key, &input).await? {
            Start::Replay(value) => return Ok(value),
            Start::New(work) => work,
        };

    if let Some(account_id) = input.settlement_owner_account_id {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(
               SELECT 1 FROM accounts
               WHERE id=$1 AND active AND kind IN ('merchant','promoter','platform')
             )",
        )
        .bind(account_id)
        .fetch_one(&mut *work.tx)
        .await?;
        if !exists {
            return Err(Error::invalid("settlement_owner_account_id 不存在或不可用"));
        }
    }

    let id = Uuid::new_v4();
    let row: (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        "INSERT INTO platform_connections
         (id,platform,external_account_id,display_name,connection_type,credential_ref,
          settlement_owner_account_id)
         VALUES($1,$2,$3,$4,$5,$6,$7)
         RETURNING created_at,updated_at",
    )
    .bind(id)
    .bind(&input.platform)
    .bind(&input.external_account_id)
    .bind(&input.display_name)
    .bind(&input.connection_type)
    .bind(&input.credential_ref)
    .bind(input.settlement_owner_account_id)
    .fetch_one(&mut *work.tx)
    .await?;

    let response = json!({
        "id": id,
        "platform": input.platform,
        "external_account_id": input.external_account_id,
        "display_name": input.display_name,
        "connection_type": input.connection_type,
        "credential_configured": true,
        "settlement_owner_account_id": input.settlement_owner_account_id,
        "status": "active",
        "created_at": row.0,
        "updated_at": row.1,
    });
    work.complete(response, Some(id)).await
}

pub async fn set_connection_status(
    pool: &PgPool,
    actor: &Actor,
    key: &str,
    id: Uuid,
    input: SetPlatformConnectionStatus,
) -> Result<Value> {
    actor.require(&["integrator"])?;
    if !["active", "suspended", "revoked"].contains(&input.status.as_str()) {
        return Err(Error::invalid(
            "平台连接状态必须为 active/suspended/revoked",
        ));
    }

    let mut work = match Work::begin(
        pool,
        actor,
        "platform.connections.status",
        key,
        &json!({"connection_id": id, "status": input.status}),
    )
    .await?
    {
        Start::Replay(value) => return Ok(value),
        Start::New(work) => work,
    };

    let current: Option<(String, String)> =
        sqlx::query_as("SELECT platform,status FROM platform_connections WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *work.tx)
            .await?;
    let (platform, current_status) = current.ok_or(Error::NotFound)?;
    if current_status == "revoked" && input.status != "revoked" {
        return Err(Error::conflict("已撤销的平台连接不能重新启用"));
    }

    let updated_at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "UPDATE platform_connections
         SET status=$2,updated_at=now()
         WHERE id=$1
         RETURNING updated_at",
    )
    .bind(id)
    .bind(&input.status)
    .fetch_one(&mut *work.tx)
    .await?;

    let response = json!({
        "id": id,
        "platform": platform,
        "previous_status": current_status,
        "status": input.status,
        "updated_at": updated_at,
    });
    work.complete(response, Some(id)).await
}

fn validate_create(input: &CreatePlatformConnection) -> Result<()> {
    if !["taobao", "douyin", "meituan"].contains(&input.platform.as_str()) {
        return Err(Error::invalid("platform 必须为 taobao/douyin/meituan"));
    }
    if !["oauth", "app_credentials", "private_api"].contains(&input.connection_type.as_str()) {
        return Err(Error::invalid(
            "connection_type 必须为 oauth/app_credentials/private_api",
        ));
    }
    text(&input.external_account_id, 160, "平台外部账户编号")?;
    text(&input.display_name, 160, "平台连接名称")?;
    validate_credential_ref(&input.credential_ref)
}

fn validate_credential_ref(value: &str) -> Result<()> {
    let name = value
        .strip_prefix("env:")
        .ok_or_else(|| Error::invalid("credential_ref 当前仅支持 env:NAME"))?;
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(Error::invalid("credential_ref 环境变量名称无效"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_env_secret_references_are_accepted() {
        assert!(validate_credential_ref("env:TAOBAO_MAIN").is_ok());
        assert!(validate_credential_ref("vault:taobao").is_err());
        assert!(validate_credential_ref("env:").is_err());
        assert!(validate_credential_ref("env:bad-name").is_err());
    }
}

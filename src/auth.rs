use crate::{error::{Error, Result}, model::{Actor, CreateCredential, text}, transaction::{digest, record_event}, AppState};
use axum::{extract::{Request, State}, http::header, middleware::Next, response::Response};
use chrono::{Duration, Utc};
use rand::{rngs::OsRng, RngCore};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub fn generate_secret() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("cms_{}", hex::encode(bytes))
}

pub fn validate_secret(secret: &str) -> Result<()> {
    let rest = secret.strip_prefix("cms_").ok_or_else(|| Error::invalid("令牌格式为 cms_ 加 64 位小写十六进制随机字符"))?;
    if rest.len() != 64 || !rest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(Error::invalid("令牌格式为 cms_ 加 64 位小写十六进制随机字符"));
    }
    Ok(())
}

pub async fn authenticate(State(state): State<AppState>, mut request: Request, next: Next) -> Result<Response> {
    let token = request.headers().get(header::AUTHORIZATION).and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer ")).ok_or(Error::Unauthorized)?;
    validate_secret(token).map_err(|_| Error::Unauthorized)?;
    let actor = sqlx::query_as::<_, Actor>(
        "SELECT c.id,c.name,c.role,c.account_id,c.expires_at FROM credentials c LEFT JOIN accounts a ON a.id=c.account_id
         WHERE c.token_hash=$1 AND c.revoked_at IS NULL AND c.expires_at>now() AND (c.account_id IS NULL OR a.active)")
        .bind(digest(token.as_bytes())).fetch_optional(&state.pool).await?.ok_or(Error::Unauthorized)?;
    request.extensions_mut().insert(actor);
    Ok(next.run(request).await)
}

pub async fn insert_credential(tx: &mut Transaction<'_, Postgres>, input: &CreateCredential) -> Result<Value> {
    text(&input.name, 120, "令牌名称")?;
    validate_secret(&input.secret)?;
    if !["admin", "operator", "finance", "integrator", "auditor", "member"].contains(&input.role.as_str())
        || !(1..=365).contains(&input.expires_in_days)
        || (input.role == "member") != input.account_id.is_some()
    {
        return Err(Error::invalid("角色、关联账户或有效期不正确"));
    }
    if let Some(account) = input.account_id {
        let eligible: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM accounts WHERE id=$1 AND kind IN ('merchant','promoter') AND active)")
            .bind(account).fetch_one(&mut **tx).await?;
        if !eligible { return Err(Error::invalid("成员令牌必须关联启用的商家或推广员")); }
    }
    let id = Uuid::new_v4();
    let expires = Utc::now() + Duration::days(input.expires_in_days);
    sqlx::query("INSERT INTO credentials(id,name,token_hash,role,account_id,expires_at) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(id).bind(&input.name).bind(digest(input.secret.as_bytes())).bind(&input.role)
        .bind(input.account_id).bind(expires).execute(&mut **tx).await?;
    // Never return, audit, enqueue, or persist plaintext tokens in an idempotent response.
    Ok(json!({"id":id, "name":input.name, "role":input.role, "account_id":input.account_id, "expires_at":expires}))
}

/// CLI-only bootstrap. The advisory lock prevents simultaneous first-run admins.
pub async fn bootstrap(pool: &PgPool) -> Result<Value> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(734190821)").execute(&mut *tx).await?;
    let existing: i64 = sqlx::query_scalar("SELECT count(*) FROM credentials").fetch_one(&mut *tx).await?;
    if existing != 0 { return Err(Error::conflict("已有凭据，拒绝重复初始化。请通过现有管理员轮换令牌")); }
    let secret = generate_secret();
    let input = CreateCredential { name: "初始管理员".into(), role: "admin".into(), account_id: None, expires_in_days: 90, secret: secret.clone() };
    let mut response = insert_credential(&mut tx, &input).await?;
    record_event(&mut tx, None, "credentials.bootstrap", None, &response).await?;
    tx.commit().await?;
    response["secret"] = json!(secret);
    Ok(response)
}

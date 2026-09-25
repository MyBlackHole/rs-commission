//! Idempotency row, business changes, journals, audit, and outbox share ONE transaction.
use crate::{error::{Error, Result}, model::Actor};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub fn digest(bytes: &[u8]) -> String { hex::encode(Sha256::digest(bytes)) }

pub fn fingerprint<T: Serialize>(value: &T) -> Result<String> {
    // Struct serialization and serde_json's default ordered map are deterministic.
    Ok(digest(&serde_json::to_vec(value)?))
}

pub enum Start { Replay(Value), New(Work) }

pub struct Work {
    pub tx: Transaction<'static, Postgres>,
    pub actor_id: Uuid,
    operation: String,
    key: String,
}

impl Work {
    pub async fn begin<T: Serialize>(pool: &PgPool, actor: &Actor, operation: &str, key: &str, input: &T) -> Result<Start> {
        if !(8..=128).contains(&key.len()) || !key.bytes().all(|c| c.is_ascii_graphic()) {
            return Err(Error::invalid("Idempotency-Key 必须为 8..128 个可见 ASCII 字符"));
        }
        let hash = fingerprint(input)?;
        let mut tx = pool.begin().await?;
        configure(&mut tx).await?;
        sqlx::query("INSERT INTO idempotency(actor_id, operation, key, request_hash) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING")
            .bind(actor.id).bind(operation).bind(key).bind(&hash).execute(&mut *tx).await?;
        // A competing insert with the same unique key waits for the first commit.
        let (stored_hash, response): (String, Option<Value>) = sqlx::query_as(
            "SELECT request_hash, response FROM idempotency WHERE actor_id=$1 AND operation=$2 AND key=$3 FOR UPDATE")
            .bind(actor.id).bind(operation).bind(key).fetch_one(&mut *tx).await?;
        if stored_hash != hash { return Err(Error::conflict("同一幂等键不能用于不同请求")); }
        if let Some(response) = response {
            tx.rollback().await?;
            return Ok(Start::Replay(response));
        }
        Ok(Start::New(Self { tx, actor_id: actor.id, operation: operation.into(), key: key.into() }))
    }

    pub async fn complete(mut self, response: Value, target: Option<Uuid>) -> Result<Value> {
        record_event(&mut self.tx, Some(self.actor_id), &self.operation, target, &response).await?;
        let count = sqlx::query("UPDATE idempotency SET response=$4 WHERE actor_id=$1 AND operation=$2 AND key=$3 AND response IS NULL")
            .bind(self.actor_id).bind(&self.operation).bind(&self.key).bind(&response)
            .execute(&mut *self.tx).await?.rows_affected();
        if count != 1 { return Err(Error::Internal); }
        self.tx.commit().await?;
        Ok(response)
    }
}

pub async fn configure(tx: &mut Transaction<'_, Postgres>) -> Result<()> {
    sqlx::raw_sql("SET LOCAL lock_timeout = '5s'; SET LOCAL statement_timeout = '15s';")
        .execute(&mut **tx).await?;
    Ok(())
}

pub async fn record_event(
    tx: &mut Transaction<'_, Postgres>, actor: Option<Uuid>, action: &str,
    target: Option<Uuid>, detail: &Value,
) -> Result<()> {
    sqlx::query("INSERT INTO audit_events(actor_id, action, target_id, detail) VALUES($1,$2,$3,$4)")
        .bind(actor).bind(action).bind(target).bind(detail).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO outbox(id, topic, aggregate_id, payload) VALUES($1,$2,$3,$4)")
        .bind(Uuid::new_v4()).bind(action).bind(target).bind(detail).execute(&mut **tx).await?;
    Ok(())
}

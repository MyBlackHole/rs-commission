use crate::{error::{Error, Result}, model::{AckEvent, Actor, ClaimEvents, Page}};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

pub async fn list(pool: &PgPool, actor: &Actor, input: Page) -> Result<Value> {
    actor.require(&["integrator", "auditor"])?;
    let (limit, offset) = input.bounds()?;
    let mut rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(o) FROM outbox o ORDER BY created_at DESC,id DESC LIMIT $1 OFFSET $2")
        .bind(limit+1).bind(offset).fetch_all(pool).await?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    Ok(json!({"items":rows,"limit":limit,"offset":offset,"has_more":has_more}))
}

/// Pull delivery avoids holding a DB transaction across a network call and avoids
/// server-side arbitrary webhook URLs. Consumers must deduplicate using event.id.
pub async fn claim(pool: &PgPool, actor: &Actor, input: ClaimEvents) -> Result<Value> {
    actor.require(&["integrator"])?;
    if !(1..=100).contains(&input.limit) || !(10..=3600).contains(&input.lease_seconds) {
        return Err(Error::invalid("批量数为 1..100，租约为 10..3600 秒"));
    }
    let lease = Uuid::new_v4();
    let rows: Vec<Value> = sqlx::query_scalar(
        "WITH picked AS (SELECT id FROM outbox WHERE delivered_at IS NULL AND (lease_until IS NULL OR lease_until<=now())
         ORDER BY created_at,id LIMIT $1 FOR UPDATE SKIP LOCKED),
         claimed AS (UPDATE outbox o SET lease_token=$2,lease_until=now()+($3::BIGINT*INTERVAL '1 second'),attempts=attempts+1
         FROM picked p WHERE o.id=p.id RETURNING o.*) SELECT to_jsonb(c) FROM claimed c ORDER BY c.created_at,c.id")
        .bind(input.limit).bind(lease).bind(input.lease_seconds).fetch_all(pool).await?;
    Ok(json!({"items":rows,"lease_token":lease,"delivery":"at_least_once"}))
}

pub async fn ack(pool: &PgPool, actor: &Actor, id: Uuid, input: AckEvent) -> Result<Value> {
    actor.require(&["integrator"])?;
    let rows = sqlx::query("UPDATE outbox SET delivered_at=COALESCE(delivered_at,now()) WHERE id=$1 AND lease_token=$2 AND (delivered_at IS NOT NULL OR lease_until>now())")
        .bind(id).bind(input.lease_token).execute(pool).await?.rows_affected();
    if rows == 0 { return Err(Error::conflict("事件不存在、租约过期或已被其他消费者领取")); }
    // Do not produce an outbox event for an outbox ACK (would recurse indefinitely).
    Ok(json!({"id":id,"acknowledged":true}))
}

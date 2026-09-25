use crate::{
    domain,
    error::{Error, Result},
    ledger::{self, Bucket, Line, Posting},
    model::*,
    money::Money,
    service::catalog::quote_in,
    transaction::{configure, fingerprint, record_event, Start, Work},
};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub async fn capture(pool: &PgPool, actor: &Actor, key: &str, input: CaptureOrder) -> Result<Value> {
    actor.require(&["operator", "integrator"])?;
    let mut work = match Work::begin(pool, actor, "orders.capture", key, &input).await? {
        Start::Replay(v) => return Ok(v), Start::New(w) => w,
    };
    text(&input.external_id, 96, "订单业务编号")?;
    if input.currency != "CNY" { return Err(Error::invalid("当前版本仅支持 CNY，金额单位为分")); }
    if let Some(customer) = &input.customer_external_id {
        text(customer, 96, "客户业务编号")?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 9151))")
            .bind(customer).execute(&mut *work.tx).await?;
    }
    let quoted = quote_in(&mut work.tx, &input.quote_input()).await?;
    let id = Uuid::new_v4();
    let snapshot = json!({"rule":quoted.rule, "direct_account_id":quoted.direct, "indirect_account_id":quoted.indirect,
        "funding":"from_platform_fee", "rounding":"floor_fee_and_referrals", "refund_rounding":"sequential_cumulative_v1"});
    let order = sqlx::query_as::<_, Order>(
        "INSERT INTO orders(id,external_id,merchant_id,customer_external_id,currency,paid_minor,commission_base_minor,fee_pool_minor,rule_id,rule_snapshot,input_hash,unlock_at)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,now()+($12::BIGINT * INTERVAL '1 second')) RETURNING *")
        .bind(id).bind(&input.external_id).bind(input.merchant_id).bind(&input.customer_external_id)
        .bind(&input.currency).bind(input.paid_minor).bind(input.commission_base_minor).bind(quoted.split.fee_pool_minor)
        .bind(quoted.rule.id).bind(&snapshot).bind(fingerprint(&input)?).bind(quoted.rule.freeze_seconds)
        .fetch_one(&mut *work.tx).await?;
    let mut parts: Vec<(i16, Uuid, &str, Money)> = Vec::new();
    if let Some(direct) = quoted.direct { parts.push((0, direct, "direct", quoted.split.direct_minor)); }
    if let Some(indirect) = quoted.indirect { parts.push((1, indirect, "indirect", quoted.split.indirect_minor)); }
    parts.push((2, PLATFORM, "platform", quoted.split.platform_minor));
    parts.push((3, input.merchant_id, "merchant", quoted.split.merchant_minor));
    let mut allocations = Vec::new();
    let mut lines = vec![Line::new(CLEARING, Bucket::Available, -input.paid_minor.0)];
    for (ordinal, account, slot, amount) in parts {
        let allocation = sqlx::query_as::<_, Allocation>(
            "INSERT INTO allocations(order_id,ordinal,account_id,slot,original_minor) VALUES($1,$2,$3,$4,$5) RETURNING *")
            .bind(id).bind(ordinal).bind(account).bind(slot).bind(amount).fetch_one(&mut *work.tx).await?;
        allocations.push(allocation);
        lines.push(Line::new(account, Bucket::Frozen, amount.0));
    }
    ledger::post(&mut work.tx, Posting { event_key: format!("capture:{id}"), kind: "capture", order_id: Some(id),
        payout_id: None, actor_id: Some(actor.id), metadata: json!({"external_id":input.external_id}), lines }).await?;
    work.complete(json!({"order":order,"allocations":allocations,"split":quoted.split}), Some(id)).await
}

pub async fn lock_order(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> Result<Order> {
    sqlx::query_as::<_, Order>("SELECT * FROM orders WHERE id=$1 FOR UPDATE")
        .bind(id).fetch_optional(&mut **tx).await?.ok_or(Error::NotFound)
}

pub async fn allocations(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> Result<Vec<Allocation>> {
    Ok(sqlx::query_as::<_, Allocation>("SELECT * FROM allocations WHERE order_id=$1 ORDER BY ordinal")
        .bind(id).fetch_all(&mut **tx).await?)
}

pub async fn refund(pool: &PgPool, actor: &Actor, key: &str, order_id: Uuid, input: RefundInput) -> Result<Value> {
    actor.require(&["operator", "integrator"])?;
    let mut work = match Work::begin(pool, actor, "orders.refund", key, &json!({"order_id":order_id,"input":input})).await? {
        Start::Replay(v) => return Ok(v), Start::New(w) => w,
    };
    text(&input.external_id, 96, "退款业务编号")?;
    text(&input.reason, 400, "退款原因")?;
    domain::positive(input.amount_minor)?;
    // Capture, thaw, and refund serialize on the order row; unrelated orders can proceed.
    let order = lock_order(&mut work.tx, order_id).await?;
    let cumulative = order.refunded_minor.0.checked_add(input.amount_minor.0).ok_or_else(|| Error::invalid("金额溢出"))?;
    if cumulative > order.paid_minor.0 { return Err(Error::conflict("累计退款不能超过订单实付金额")); }
    let parts = allocations(&mut work.tx, order_id).await?;
    let shares: Vec<_> = parts.iter().map(|p| p.original_minor).collect();
    let targets = domain::cumulative_refund(&shares, Money(cumulative))?;
    let bucket = if order.released_at.is_some() { Bucket::Available } else { Bucket::Frozen };
    let refund_id = Uuid::new_v4();
    let mut deltas = Vec::new();
    let mut lines = vec![Line::new(CLEARING, Bucket::Available, input.amount_minor.0)];
    for (part, target) in parts.iter().zip(targets) {
        let delta = target.0 - part.refunded_minor.0;
        if delta < 0 { return Err(Error::Internal); }
        lines.push(Line::new(part.account_id, bucket, -delta));
        deltas.push(json!({"account_id":part.account_id,"slot":part.slot,"delta_minor":Money(delta),"cumulative_minor":target}));
        sqlx::query("UPDATE allocations SET refunded_minor=$3 WHERE order_id=$1 AND ordinal=$2")
            .bind(order_id).bind(part.ordinal).bind(target).execute(&mut *work.tx).await?;
    }
    sqlx::query("INSERT INTO refunds(id,order_id,external_id,amount_minor,cumulative_minor,reason,allocation_deltas) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(refund_id).bind(order_id).bind(&input.external_id).bind(input.amount_minor).bind(cumulative)
        .bind(&input.reason).bind(json!(deltas)).execute(&mut *work.tx).await?;
    ledger::post(&mut work.tx, Posting { event_key: format!("refund:{refund_id}"), kind:"refund", order_id:Some(order_id),
        payout_id:None, actor_id:Some(actor.id), metadata:json!({"refund_id":refund_id,"external_id":input.external_id}), lines }).await?;
    let updated = sqlx::query_as::<_, Order>("UPDATE orders SET refunded_minor=$2 WHERE id=$1 RETURNING *")
        .bind(order_id).bind(cumulative).fetch_one(&mut *work.tx).await?;
    work.complete(json!({"id":refund_id,"order":updated,"amount_minor":input.amount_minor,"allocation_deltas":deltas}), Some(order_id)).await
}

async fn release_locked(tx: &mut Transaction<'_, Postgres>, actor_id: Option<Uuid>, order: Order) -> Result<Value> {
    if order.released_at.is_some() { return Ok(json!({"order":order,"changed":false})); }
    let due: bool = sqlx::query_scalar("SELECT now() >= $1::TIMESTAMPTZ").bind(order.unlock_at).fetch_one(&mut **tx).await?;
    if !due { return Err(Error::conflict("冻结期尚未结束，不能提前解冻")); }
    let parts = allocations(tx, order.id).await?;
    let mut lines = Vec::new();
    for part in parts {
        let net = part.original_minor.0 - part.refunded_minor.0;
        lines.push(Line::new(part.account_id, Bucket::Frozen, -net));
        lines.push(Line::new(part.account_id, Bucket::Available, net));
    }
    ledger::post(tx, Posting { event_key:format!("release:{}",order.id), kind:"release", order_id:Some(order.id), payout_id:None,
        actor_id, metadata:json!({}), lines }).await?;
    let updated = sqlx::query_as::<_, Order>("UPDATE orders SET released_at=now() WHERE id=$1 RETURNING *")
        .bind(order.id).fetch_one(&mut **tx).await?;
    Ok(json!({"order":updated,"changed":true}))
}

pub async fn release(pool: &PgPool, actor: &Actor, key: &str, id: Uuid) -> Result<Value> {
    actor.require(&["operator", "finance"])?;
    let mut work = match Work::begin(pool, actor, "orders.release", key, &json!({"id":id})).await? {
        Start::Replay(v) => return Ok(v), Start::New(w) => w,
    };
    let order = lock_order(&mut work.tx, id).await?;
    let response = release_locked(&mut work.tx, Some(actor.id), order).await?;
    work.complete(response, Some(id)).await
}

/// One order per transaction, avoiding lock accumulation across unrelated orders.
pub async fn release_one_due(pool: &PgPool) -> Result<bool> {
    let mut tx = pool.begin().await?;
    configure(&mut tx).await?;
    let order = sqlx::query_as::<_, Order>(
        "SELECT * FROM orders WHERE released_at IS NULL AND unlock_at <= now() ORDER BY unlock_at,id LIMIT 1 FOR UPDATE SKIP LOCKED")
        .fetch_optional(&mut *tx).await?;
    let Some(order) = order else { tx.rollback().await?; return Ok(false); };
    let id = order.id;
    let response = release_locked(&mut tx, None, order).await?;
    record_event(&mut tx, None, "orders.release.worker", Some(id), &response).await?;
    tx.commit().await?;
    Ok(true)
}

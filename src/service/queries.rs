use crate::{error::{Error, Result}, model::*};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

fn page<T: Serialize>(mut items: Vec<T>, limit: i64, offset: i64) -> Result<Value> {
    let has_more = items.len() > limit as usize;
    items.truncate(limit as usize);
    Ok(json!({"items":items,"limit":limit,"offset":offset,"has_more":has_more}))
}

fn scope(actor: &Actor, requested: Option<Uuid>) -> Result<Option<Uuid>> {
    if let Some(id) = requested { actor.check_account(id)?; }
    Ok(if actor.role == "member" { actor.account_id } else { requested })
}

pub async fn accounts(pool: &PgPool, actor: &Actor, input: Page) -> Result<Value> {
    let (limit, offset) = input.bounds()?;
    let filter = scope(actor, input.account_id)?;
    let rows = sqlx::query_as::<_, Account>("SELECT * FROM accounts WHERE ($1::UUID IS NULL OR id=$1) ORDER BY created_at DESC,id DESC LIMIT $2 OFFSET $3")
        .bind(filter).bind(limit+1).bind(offset).fetch_all(pool).await?;
    page(rows, limit, offset)
}

pub async fn wallets(pool: &PgPool, actor: &Actor, input: Page) -> Result<Value> {
    let (limit, offset) = input.bounds()?;
    let filter = scope(actor, input.account_id)?;
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_object('account_id',w.account_id,'name',a.name,'kind',a.kind,
         'frozen_minor',w.frozen_minor::TEXT,'available_minor',w.available_minor::TEXT,'reserved_minor',w.reserved_minor::TEXT,
         'debt_minor',greatest(-w.available_minor::NUMERIC,0)::TEXT,'updated_at',w.updated_at)
         FROM wallets w JOIN accounts a ON a.id=w.account_id WHERE ($1::UUID IS NULL OR w.account_id=$1)
         ORDER BY a.created_at DESC,a.id DESC LIMIT $2 OFFSET $3")
        .bind(filter).bind(limit+1).bind(offset).fetch_all(pool).await?;
    page(rows, limit, offset)
}

pub async fn wallet(pool: &PgPool, actor: &Actor, id: Uuid) -> Result<Value> {
    actor.check_account(id)?;
    let row = sqlx::query_as::<_, Wallet>("SELECT * FROM wallets WHERE account_id=$1")
        .bind(id).fetch_optional(pool).await?.ok_or(Error::NotFound)?;
    Ok(serde_json::to_value(row)?)
}

pub async fn rules(pool: &PgPool, actor: &Actor, input: Page) -> Result<Value> {
    actor.require(&["operator", "integrator", "finance", "auditor"])?;
    let (limit, offset) = input.bounds()?;
    let rows = sqlx::query_as::<_, Rule>("SELECT * FROM rules ORDER BY version DESC LIMIT $1 OFFSET $2")
        .bind(limit+1).bind(offset).fetch_all(pool).await?;
    page(rows, limit, offset)
}

pub async fn referrals(pool: &PgPool, actor: &Actor, input: Page) -> Result<Value> {
    actor.require(&["operator", "integrator", "auditor"])?;
    let (limit, offset) = input.bounds()?;
    let rows: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('customer_external_id',r.customer_external_id,'promoter_id',r.promoter_id,'promoter_name',a.name,'created_at',r.created_at) FROM referral_bindings r JOIN accounts a ON a.id=r.promoter_id ORDER BY r.created_at DESC,r.customer_external_id LIMIT $1 OFFSET $2")
        .bind(limit+1).bind(offset).fetch_all(pool).await?;
    page(rows, limit, offset)
}

pub async fn orders(pool: &PgPool, actor: &Actor, input: Page) -> Result<Value> {
    actor.require(&["operator", "integrator", "finance", "auditor"])?;
    let (limit, offset) = input.bounds()?;
    let rows = sqlx::query_as::<_, Order>("SELECT * FROM orders WHERE ($1::UUID IS NULL OR merchant_id=$1) ORDER BY captured_at DESC,id DESC LIMIT $2 OFFSET $3")
        .bind(input.account_id).bind(limit+1).bind(offset).fetch_all(pool).await?;
    page(rows, limit, offset)
}

pub async fn order(pool: &PgPool, actor: &Actor, id: Uuid) -> Result<Value> {
    actor.require(&["operator", "integrator", "finance", "auditor"])?;
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY").execute(&mut *tx).await?;
    let order = sqlx::query_as::<_, Order>("SELECT * FROM orders WHERE id=$1")
        .bind(id).fetch_optional(&mut *tx).await?.ok_or(Error::NotFound)?;
    let allocations = sqlx::query_as::<_, Allocation>("SELECT * FROM allocations WHERE order_id=$1 ORDER BY ordinal")
        .bind(id).fetch_all(&mut *tx).await?;
    let refunds: Vec<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_object('id',id,'external_id',external_id,'amount_minor',amount_minor::TEXT,'cumulative_minor',cumulative_minor::TEXT,'reason',reason,'allocation_deltas',allocation_deltas,'created_at',created_at)
         FROM refunds WHERE order_id=$1 ORDER BY created_at,id")
        .bind(id).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(json!({"order":order,"allocations":allocations,"refunds":refunds}))
}

pub async fn commissions(pool: &PgPool, actor: &Actor, input: Page) -> Result<Value> {
    let (limit, offset) = input.bounds()?;
    let filter = scope(actor, input.account_id)?;
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_object('order_id',a.order_id,'external_id',o.external_id,'account_id',a.account_id,'name',p.name,'slot',a.slot,
         'original_minor',a.original_minor::TEXT,'refunded_minor',a.refunded_minor::TEXT,'net_minor',(a.original_minor-a.refunded_minor)::TEXT,
         'unlock_at',o.unlock_at,'released_at',o.released_at,'created_at',o.captured_at)
         FROM allocations a JOIN orders o ON o.id=a.order_id JOIN accounts p ON p.id=a.account_id
         WHERE ($1::UUID IS NULL OR a.account_id=$1) ORDER BY o.captured_at DESC,o.id DESC,a.ordinal LIMIT $2 OFFSET $3")
        .bind(filter).bind(limit+1).bind(offset).fetch_all(pool).await?;
    page(rows, limit, offset)
}

pub async fn ledger(pool: &PgPool, actor: &Actor, input: Page) -> Result<Value> {
    let (limit, offset) = input.bounds()?;
    let filter = scope(actor, input.account_id)?;
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_object('journal_id',j.id,'event_key',j.event_key,'kind',j.kind,'order_id',j.order_id,'payout_id',j.payout_id,
         'account_id',e.account_id,'name',a.name,'bucket',e.bucket,'delta_minor',e.delta_minor::TEXT,'created_at',j.created_at)
         FROM ledger_entries e JOIN journals j ON j.id=e.journal_id JOIN accounts a ON a.id=e.account_id
         WHERE ($1::UUID IS NULL OR e.account_id=$1) ORDER BY j.created_at DESC,j.id DESC,e.line_no LIMIT $2 OFFSET $3")
        .bind(filter).bind(limit+1).bind(offset).fetch_all(pool).await?;
    page(rows, limit, offset)
}

pub async fn payouts(pool: &PgPool, actor: &Actor, input: Page) -> Result<Value> {
    let (limit, offset) = input.bounds()?;
    let filter = scope(actor, input.account_id)?;
    let rows = sqlx::query_as::<_, Payout>("SELECT * FROM payouts WHERE ($1::UUID IS NULL OR account_id=$1) ORDER BY created_at DESC,id DESC LIMIT $2 OFFSET $3")
        .bind(filter).bind(limit+1).bind(offset).fetch_all(pool).await?;
    page(rows, limit, offset)
}

pub async fn audit(pool: &PgPool, actor: &Actor, input: Page) -> Result<Value> {
    actor.require(&["operator", "finance", "auditor"])?;
    let (limit, offset) = input.bounds()?;
    let rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(a) FROM audit_events a ORDER BY id DESC LIMIT $1 OFFSET $2")
        .bind(limit+1).bind(offset).fetch_all(pool).await?;
    page(rows, limit, offset)
}

pub async fn credentials(pool: &PgPool, actor: &Actor, input: Page) -> Result<Value> {
    actor.require(&[])?;
    let (limit, offset) = input.bounds()?;
    let rows: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',id,'name',name,'role',role,'account_id',account_id,'expires_at',expires_at,'revoked_at',revoked_at,'created_at',created_at) FROM credentials ORDER BY created_at DESC,id DESC LIMIT $1 OFFSET $2")
        .bind(limit+1).bind(offset).fetch_all(pool).await?;
    page(rows, limit, offset)
}

pub async fn dashboard(pool: &PgPool, actor: &Actor) -> Result<Value> {
    if actor.role == "member" {
        let id = actor.account_id.ok_or(Error::Forbidden)?;
        let row: Value = sqlx::query_scalar(
            "SELECT jsonb_build_object('scope','account','account_id',account_id,'frozen_minor',frozen_minor::TEXT,
             'available_minor',available_minor::TEXT,'reserved_minor',reserved_minor::TEXT,'debt_minor',greatest(-available_minor::NUMERIC,0)::TEXT)
             FROM wallets WHERE account_id=$1")
            .bind(id).fetch_one(pool).await?;
        return Ok(row);
    }
    let row: Value = sqlx::query_scalar(
        "SELECT jsonb_build_object('scope','platform','currency','CNY',
         'order_count',(SELECT count(*) FROM orders),
         'paid_minor',(SELECT COALESCE(sum(paid_minor),0)::TEXT FROM orders),
         'refunded_minor',(SELECT COALESCE(sum(refunded_minor),0)::TEXT FROM orders),
         'platform_net_minor',(SELECT COALESCE(sum(original_minor-refunded_minor),0)::TEXT FROM allocations WHERE slot='platform'),
         'frozen_minor',(SELECT COALESCE(sum(w.frozen_minor),0)::TEXT FROM wallets w JOIN accounts a ON a.id=w.account_id WHERE a.kind<>'clearing'),
         'available_minor',(SELECT COALESCE(sum(w.available_minor),0)::TEXT FROM wallets w JOIN accounts a ON a.id=w.account_id WHERE a.kind<>'clearing'),
         'reserved_minor',(SELECT COALESCE(sum(w.reserved_minor),0)::TEXT FROM wallets w JOIN accounts a ON a.id=w.account_id WHERE a.kind<>'clearing'),
         'debt_minor',(SELECT COALESCE(sum(greatest(-w.available_minor::NUMERIC,0)),0)::TEXT FROM wallets w JOIN accounts a ON a.id=w.account_id WHERE a.kind<>'clearing'),
         'due_orders',(SELECT count(*) FROM orders WHERE released_at IS NULL AND unlock_at<=now()),
         'unknown_payouts',(SELECT count(*) FROM payouts WHERE status='unknown'),
         'pending_payouts',(SELECT count(*) FROM payouts WHERE status IN ('requested','approved')),
         'outbox_pending',(SELECT count(*) FROM outbox WHERE delivered_at IS NULL))")
        .fetch_one(pool).await?;
    Ok(row)
}

pub async fn reconcile(pool: &PgPool, actor: &Actor) -> Result<Value> {
    actor.require(&["finance", "auditor"])?;
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY").execute(&mut *tx).await?;
    sqlx::query("SET LOCAL statement_timeout='30s'").execute(&mut *tx).await?;
    let unbalanced: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (SELECT j.id FROM journals j LEFT JOIN ledger_entries e ON e.journal_id=j.id
         GROUP BY j.id HAVING COALESCE(sum(e.delta_minor::NUMERIC),0)<>0 OR count(e.journal_id)<2) x")
        .fetch_one(&mut *tx).await?;
    let wallet_differences: Vec<Value> = sqlx::query_scalar(
        "WITH expected AS (SELECT account_id,
         COALESCE(sum(delta_minor::NUMERIC) FILTER (WHERE bucket='frozen'),0) AS frozen,
         COALESCE(sum(delta_minor::NUMERIC) FILTER (WHERE bucket='available'),0) AS available,
         COALESCE(sum(delta_minor::NUMERIC) FILTER (WHERE bucket='reserved'),0) AS reserved FROM ledger_entries GROUP BY account_id)
         SELECT jsonb_build_object('account_id',w.account_id,'frozen_actual',w.frozen_minor::TEXT,'frozen_expected',COALESCE(e.frozen,0)::TEXT,
         'available_actual',w.available_minor::TEXT,'available_expected',COALESCE(e.available,0)::TEXT,
         'reserved_actual',w.reserved_minor::TEXT,'reserved_expected',COALESCE(e.reserved,0)::TEXT)
         FROM wallets w LEFT JOIN expected e ON e.account_id=w.account_id
         WHERE w.frozen_minor<>COALESCE(e.frozen,0) OR w.available_minor<>COALESCE(e.available,0) OR w.reserved_minor<>COALESCE(e.reserved,0)
         ORDER BY w.account_id LIMIT 100")
        .fetch_all(&mut *tx).await?;
    let allocation_differences: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (SELECT o.id FROM orders o LEFT JOIN allocations a ON a.order_id=o.id
         GROUP BY o.id,o.paid_minor,o.refunded_minor HAVING COALESCE(sum(a.original_minor),0)<>o.paid_minor
         OR COALESCE(sum(a.refunded_minor),0)<>o.refunded_minor) x")
        .fetch_one(&mut *tx).await?;
    let refund_differences: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (SELECT o.id FROM orders o LEFT JOIN refunds r ON r.order_id=o.id
         GROUP BY o.id,o.refunded_minor HAVING COALESCE(sum(r.amount_minor),0)<>o.refunded_minor) x")
        .fetch_one(&mut *tx).await?;
    let reserved_differences: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM wallets w LEFT JOIN (SELECT account_id,sum(amount_minor) AS expected FROM payouts
         WHERE status IN ('requested','approved','processing','unknown') GROUP BY account_id) p ON p.account_id=w.account_id
         WHERE w.reserved_minor<>COALESCE(p.expected,0)")
        .fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(json!({"ok":unbalanced==0 && wallet_differences.is_empty() && allocation_differences==0 && refund_differences==0 && reserved_differences==0,
        "scope":"internal_only","unbalanced_journals":unbalanced,"wallet_differences":wallet_differences,
        "wallet_difference_sample_limit":100,"allocation_differences":allocation_differences,"refund_differences":refund_differences,
        "reserved_differences":reserved_differences,"external_payment_reconciled":false}))
}

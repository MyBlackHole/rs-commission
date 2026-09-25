//! Settlement accounting only. No endpoint pretends to call a bank/payment provider.
use crate::{
    domain,
    error::{Error, Result},
    ledger::{self, Bucket, Line, Posting},
    model::*,
    transaction::{Start, Work},
};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub async fn request(
    pool: &PgPool,
    actor: &Actor,
    key: &str,
    input: RequestPayout,
) -> Result<Value> {
    actor.require(&["operator", "member"])?;
    actor.check_account(input.account_id)?;
    let mut work = match Work::begin(pool, actor, "payouts.request", key, &input).await? {
        Start::Replay(v) => return Ok(v),
        Start::New(w) => w,
    };
    text(&input.external_id, 96, "提现业务编号")?;
    text(&input.destination_ref, 160, "收款方引用")?;
    domain::positive(input.amount_minor)?;
    let account: Option<(String, bool)> =
        sqlx::query_as("SELECT kind,active FROM accounts WHERE id=$1")
            .bind(input.account_id)
            .fetch_optional(&mut *work.tx)
            .await?;
    let (kind, active) = account.ok_or(Error::NotFound)?;
    if !active || kind == "clearing" {
        return Err(Error::conflict("该账户不可结算"));
    }
    let wallet = ledger::lock_wallet(&mut work.tx, input.account_id).await?;
    if wallet.available_minor < input.amount_minor {
        return Err(Error::conflict("可用余额不足或存在待追偿欠款"));
    }
    let id = Uuid::new_v4();
    let payout = sqlx::query_as::<_, Payout>(
        "INSERT INTO payouts(id,external_id,account_id,amount_minor,destination_ref,requested_by) VALUES($1,$2,$3,$4,$5,$6) RETURNING *")
        .bind(id).bind(input.external_id).bind(input.account_id).bind(input.amount_minor).bind(input.destination_ref)
        .bind(actor.id).fetch_one(&mut *work.tx).await?;
    ledger::post(
        &mut work.tx,
        Posting {
            event_key: format!("payout:reserve:{id}"),
            kind: "payout_reserve",
            order_id: None,
            payout_id: Some(id),
            actor_id: Some(actor.id),
            metadata: json!({}),
            lines: vec![
                Line::new(input.account_id, Bucket::Available, -input.amount_minor.0),
                Line::new(input.account_id, Bucket::Reserved, input.amount_minor.0),
            ],
        },
    )
    .await?;
    work.complete(serde_json::to_value(payout)?, Some(id)).await
}

async fn lock_payout(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> Result<Payout> {
    sqlx::query_as::<_, Payout>("SELECT * FROM payouts WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(Error::NotFound)
}

pub async fn approve(pool: &PgPool, actor: &Actor, key: &str, id: Uuid) -> Result<Value> {
    actor.require(&["finance"])?;
    let mut work = match Work::begin(pool, actor, "payouts.approve", key, &json!({"id":id})).await?
    {
        Start::Replay(v) => return Ok(v),
        Start::New(w) => w,
    };
    let payout = lock_payout(&mut work.tx, id).await?;
    if payout.status != "requested" {
        return Err(Error::conflict("只有待审核提现可以通过审核"));
    }
    if payout.requested_by == actor.id {
        return Err(Error::conflict("申请凭据与审核凭据必须不同"));
    }
    let wallet = ledger::lock_wallet(&mut work.tx, payout.account_id).await?;
    if wallet.available_minor.0 < 0 {
        return Err(Error::conflict("账户发生退款追偿，请先驳回提现或清理欠款"));
    }
    let updated = sqlx::query_as::<_, Payout>("UPDATE payouts SET status='approved',approved_by=$2,updated_at=now() WHERE id=$1 RETURNING *")
        .bind(id).bind(actor.id).fetch_one(&mut *work.tx).await?;
    work.complete(serde_json::to_value(updated)?, Some(id))
        .await
}

pub async fn processing(
    pool: &PgPool,
    actor: &Actor,
    key: &str,
    id: Uuid,
    input: ReasonInput,
) -> Result<Value> {
    actor.require(&["finance"])?;
    let mut work = match Work::begin(
        pool,
        actor,
        "payouts.processing",
        key,
        &json!({"id":id,"input":input}),
    )
    .await?
    {
        Start::Replay(v) => return Ok(v),
        Start::New(w) => w,
    };
    text(&input.reason, 400, "执行说明")?;
    let payout = lock_payout(&mut work.tx, id).await?;
    if payout.status != "approved" {
        return Err(Error::conflict("只有已审核提现可以开始执行"));
    }
    if payout.requested_by == actor.id {
        return Err(Error::conflict("申请凭据不能执行自己的提现"));
    }
    let wallet = ledger::lock_wallet(&mut work.tx, payout.account_id).await?;
    if wallet.available_minor.0 < 0 {
        return Err(Error::conflict("出现退款追偿，禁止继续出款"));
    }
    let updated = sqlx::query_as::<_, Payout>("UPDATE payouts SET status='processing',evidence=$2,updated_at=now() WHERE id=$1 RETURNING *")
        .bind(id).bind(input.reason).fetch_one(&mut *work.tx).await?;
    work.complete(
        json!({"payout":updated,"provider_idempotency_key":id,"mode":"manual_external_transfer"}),
        Some(id),
    )
    .await
}

pub async fn reject(
    pool: &PgPool,
    actor: &Actor,
    key: &str,
    id: Uuid,
    input: ReasonInput,
) -> Result<Value> {
    actor.require(&["finance"])?;
    let mut work = match Work::begin(
        pool,
        actor,
        "payouts.reject",
        key,
        &json!({"id":id,"input":input}),
    )
    .await?
    {
        Start::Replay(v) => return Ok(v),
        Start::New(w) => w,
    };
    text(&input.reason, 400, "驳回原因")?;
    let payout = lock_payout(&mut work.tx, id).await?;
    if !["requested", "approved"].contains(&payout.status.as_str()) {
        return Err(Error::conflict("执行中或结果未知的提现不能直接驳回"));
    }
    ledger::post(
        &mut work.tx,
        Posting {
            event_key: format!("payout:reject:{id}"),
            kind: "payout_reject",
            order_id: None,
            payout_id: Some(id),
            actor_id: Some(actor.id),
            metadata: json!({"reason":input.reason}),
            lines: vec![
                Line::new(payout.account_id, Bucket::Reserved, -payout.amount_minor.0),
                Line::new(payout.account_id, Bucket::Available, payout.amount_minor.0),
            ],
        },
    )
    .await?;
    let updated = sqlx::query_as::<_, Payout>(
        "UPDATE payouts SET status='rejected',evidence=$2,updated_at=now() WHERE id=$1 RETURNING *",
    )
    .bind(id)
    .bind(input.reason)
    .fetch_one(&mut *work.tx)
    .await?;
    work.complete(serde_json::to_value(updated)?, Some(id))
        .await
}

pub async fn outcome(
    pool: &PgPool,
    actor: &Actor,
    key: &str,
    id: Uuid,
    input: PayoutOutcome,
) -> Result<Value> {
    actor.require(&["finance"])?;
    let mut work = match Work::begin(
        pool,
        actor,
        "payouts.outcome",
        key,
        &json!({"id":id,"input":input}),
    )
    .await?
    {
        Start::Replay(v) => return Ok(v),
        Start::New(w) => w,
    };
    text(&input.evidence, 1000, "核验凭据或说明")?;
    if !["succeeded", "failed", "unknown"].contains(&input.status.as_str()) {
        return Err(Error::invalid("结果须为 succeeded、failed 或 unknown"));
    }
    if let Some(reference) = &input.provider_reference {
        text(reference, 160, "外部交易流水号")?;
    }
    if input.status == "succeeded" && input.provider_reference.is_none() {
        return Err(Error::invalid("确认成功必须提供唯一外部流水号"));
    }
    let payout = lock_payout(&mut work.tx, id).await?;
    if !["processing", "unknown"].contains(&payout.status.as_str()) {
        return Err(Error::conflict("只有执行中或结果未知的提现可以核验结果"));
    }
    if payout.requested_by == actor.id {
        return Err(Error::conflict("申请凭据不能确认自己的出款结果"));
    }
    if input.status != "unknown" {
        let (kind, destination) = if input.status == "succeeded" {
            ("payout_paid", CLEARING)
        } else {
            ("payout_failed", payout.account_id)
        };
        ledger::post(
            &mut work.tx,
            Posting {
                event_key: format!("payout:terminal:{id}"),
                kind,
                order_id: None,
                payout_id: Some(id),
                actor_id: Some(actor.id),
                metadata: json!({"reference":input.provider_reference,"evidence":input.evidence}),
                lines: vec![
                    Line::new(payout.account_id, Bucket::Reserved, -payout.amount_minor.0),
                    Line::new(destination, Bucket::Available, payout.amount_minor.0),
                ],
            },
        )
        .await?;
    }
    // Unknown keeps funds reserved. A timeout is NOT a confirmed failed transfer.
    let updated = sqlx::query_as::<_, Payout>(
        "UPDATE payouts SET status=$2,provider_reference=COALESCE($3,provider_reference),evidence=$4,updated_at=now() WHERE id=$1 RETURNING *")
        .bind(id).bind(input.status).bind(input.provider_reference).bind(input.evidence)
        .fetch_one(&mut *work.tx).await?;
    work.complete(serde_json::to_value(updated)?, Some(id))
        .await
}

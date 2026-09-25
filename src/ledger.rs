use crate::{error::{Error, Result}, model::Wallet};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bucket { Frozen, Available, Reserved }
impl Bucket {
    pub fn as_str(self) -> &'static str {
        match self { Self::Frozen => "frozen", Self::Available => "available", Self::Reserved => "reserved" }
    }
}

pub struct Line { pub account: Uuid, pub bucket: Bucket, pub delta: i64 }
impl Line {
    pub fn new(account: Uuid, bucket: Bucket, delta: i64) -> Self { Self { account, bucket, delta } }
}

pub struct Posting {
    pub event_key: String,
    pub kind: &'static str,
    pub order_id: Option<Uuid>,
    pub payout_id: Option<Uuid>,
    pub actor_id: Option<Uuid>,
    pub metadata: Value,
    pub lines: Vec<Line>,
}

pub async fn lock_wallet(tx: &mut Transaction<'_, Postgres>, account: Uuid) -> Result<Wallet> {
    sqlx::query_as::<_, Wallet>("SELECT * FROM wallets WHERE account_id=$1 FOR UPDATE")
        .bind(account).fetch_optional(&mut **tx).await?.ok_or(Error::NotFound)
}

pub async fn post(tx: &mut Transaction<'_, Postgres>, posting: Posting) -> Result<Option<Uuid>> {
    let mut combined: BTreeMap<(Uuid, Bucket), i128> = BTreeMap::new();
    for line in posting.lines {
        *combined.entry((line.account, line.bucket)).or_default() += i128::from(line.delta);
    }
    combined.retain(|_, delta| *delta != 0);
    if combined.is_empty() { return Ok(None); }
    if combined.len() < 2 || combined.values().sum::<i128>() != 0 {
        tracing::error!(event_key = %posting.event_key, "unbalanced posting rejected");
        return Err(Error::Internal);
    }
    // Global wallet lock order, before any entry trigger updates a wallet.
    let accounts: Vec<_> = combined.keys().map(|(id, _)| *id).collect::<BTreeSet<_>>().into_iter().collect();
    let locked: Vec<Uuid> = sqlx::query_scalar("SELECT account_id FROM wallets WHERE account_id = ANY($1) ORDER BY account_id FOR UPDATE")
        .bind(&accounts).fetch_all(&mut **tx).await?;
    if locked.len() != accounts.len() { return Err(Error::Internal); }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO journals(id,event_key,kind,order_id,payout_id,actor_id,metadata) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(id).bind(posting.event_key).bind(posting.kind).bind(posting.order_id)
        .bind(posting.payout_id).bind(posting.actor_id).bind(posting.metadata).execute(&mut **tx).await?;
    for (index, ((account, bucket), delta)) in combined.into_iter().enumerate() {
        let delta = i64::try_from(delta).map_err(|_| Error::Internal)?;
        let line = i16::try_from(index).map_err(|_| Error::Internal)?;
        sqlx::query("INSERT INTO ledger_entries(journal_id,line_no,account_id,bucket,delta_minor) VALUES($1,$2,$3,$4,$5)")
            .bind(id).bind(line).bind(account).bind(bucket.as_str()).bind(delta).execute(&mut **tx).await?;
    }
    Ok(Some(id))
}

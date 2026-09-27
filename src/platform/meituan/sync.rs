use crate::{
    platform::{
        meituan::{
            normalize_order_page, MeituanClient, MeituanOrderQuery, MeituanTransport,
            NORMALIZER_VERSION, ORDER_STREAM,
        },
        store::{
            insert_refund_observation, insert_settlement_observation,
            upsert_commission_observation, upsert_order_observation,
        },
        PlatformError, Result,
    },
    transaction::{configure, digest},
};
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

const WINDOW_MINUTES: i64 = 30;
const OVERLAP_MINUTES: i64 = 5;
const EVENT_TYPE: &str = "meituan.union.query_order.page";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOutcome {
    pub raw_event_id: Uuid,
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub next_cursor: Option<String>,
    pub order_observations: usize,
    pub commission_observations: usize,
    pub refund_observations: usize,
    pub settlement_observations: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Checkpoint {
    cursor: Option<String>,
    window_start: Option<DateTime<Utc>>,
    window_end: Option<DateTime<Utc>>,
}

type CheckpointRow = (Option<String>, Option<DateTime<Utc>>, Option<DateTime<Utc>>);

pub struct MeituanOrderSync<T> {
    pool: PgPool,
    connection_id: Uuid,
    client: MeituanClient<T>,
}

impl<T> MeituanOrderSync<T> {
    pub fn new(pool: PgPool, connection_id: Uuid, client: MeituanClient<T>) -> Self {
        Self {
            pool,
            connection_id,
            client,
        }
    }
}

impl<T: MeituanTransport> MeituanOrderSync<T> {
    pub async fn sync_next(&self, now: DateTime<Utc>) -> Result<SyncOutcome> {
        self.ensure_connection().await?;
        let checkpoint = self.checkpoint().await?;
        let (window_start, window_end, cursor) = next_window(checkpoint.as_ref(), now)?;
        let query = MeituanOrderQuery {
            start: window_start,
            end: window_end,
            scroll_id: cursor,
        };

        let raw = self.client.fetch_orders(&query, now).await?;
        let raw_event_id = self.persist_raw(&raw).await?;
        let page = match normalize_order_page(&raw) {
            Ok(page) => page,
            Err(error) => {
                self.reject_raw(raw_event_id, &error.to_string()).await?;
                return Err(error);
            }
        };

        let mut tx = self.pool.begin().await?;
        configure(&mut tx).await?;
        let current_checkpoint = lock_checkpoint(&mut tx, self.connection_id).await?;
        if current_checkpoint != checkpoint {
            return Err(PlatformError::ConcurrentSync);
        }

        for order in &page.batch.orders {
            upsert_order_observation(
                &mut tx,
                self.connection_id,
                raw_event_id,
                order,
                NORMALIZER_VERSION,
            )
            .await?;
        }
        for commission in &page.batch.commissions {
            upsert_commission_observation(
                &mut tx,
                self.connection_id,
                raw_event_id,
                commission,
                NORMALIZER_VERSION,
            )
            .await?;
        }
        for refund in &page.batch.refunds {
            insert_refund_observation(
                &mut tx,
                self.connection_id,
                raw_event_id,
                refund,
                NORMALIZER_VERSION,
            )
            .await?;
        }
        for settlement in &page.batch.settlements {
            insert_settlement_observation(
                &mut tx,
                self.connection_id,
                raw_event_id,
                settlement,
                NORMALIZER_VERSION,
            )
            .await?;
        }

        sqlx::query(
            "UPDATE platform_raw_events
             SET processing_status='normalized',normalizer_version=$2,error_code=NULL,error_detail=NULL
             WHERE id=$1",
        )
        .bind(raw_event_id)
        .bind(NORMALIZER_VERSION)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO platform_sync_checkpoints
             (connection_id,stream,cursor,window_start,window_end,last_platform_updated_at,
              last_attempt_at,last_success_at,updated_at)
             VALUES($1,$2,$3,$4,$5,$6,$7,$7,$7)
             ON CONFLICT(connection_id,stream) DO UPDATE SET
               cursor=EXCLUDED.cursor,
               window_start=EXCLUDED.window_start,
               window_end=EXCLUDED.window_end,
               last_platform_updated_at=COALESCE(
                   GREATEST(platform_sync_checkpoints.last_platform_updated_at,
                            EXCLUDED.last_platform_updated_at),
                   platform_sync_checkpoints.last_platform_updated_at,
                   EXCLUDED.last_platform_updated_at
               ),
               last_attempt_at=EXCLUDED.last_attempt_at,
               last_success_at=EXCLUDED.last_success_at,
               last_error=NULL,
               updated_at=EXCLUDED.updated_at",
        )
        .bind(self.connection_id)
        .bind(ORDER_STREAM)
        .bind(&page.next_cursor)
        .bind(window_start)
        .bind(window_end)
        .bind(page.batch.max_source_updated_at)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(SyncOutcome {
            raw_event_id,
            window_start,
            window_end,
            next_cursor: page.next_cursor,
            order_observations: page.batch.orders.len(),
            commission_observations: page.batch.commissions.len(),
            refund_observations: page.batch.refunds.len(),
            settlement_observations: page.batch.settlements.len(),
        })
    }

    async fn ensure_connection(&self) -> Result<()> {
        let found: Option<(String, String)> =
            sqlx::query_as("SELECT platform,status FROM platform_connections WHERE id=$1")
                .bind(self.connection_id)
                .fetch_optional(&self.pool)
                .await?;
        match found {
            Some((platform, status)) if platform == "meituan" && status == "active" => Ok(()),
            Some((platform, _)) if platform != "meituan" => Err(PlatformError::invalid(
                "MeituanOrderSync 只能使用 meituan platform connection",
            )),
            Some(_) => Err(PlatformError::invalid("美团 platform connection 未启用")),
            None => Err(PlatformError::invalid("美团 platform connection 不存在")),
        }
    }

    async fn checkpoint(&self) -> Result<Option<Checkpoint>> {
        let row: Option<CheckpointRow> = sqlx::query_as(
            "SELECT cursor,window_start,window_end
             FROM platform_sync_checkpoints
             WHERE connection_id=$1 AND stream=$2",
        )
        .bind(self.connection_id)
        .bind(ORDER_STREAM)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(checkpoint_from_row))
    }

    async fn persist_raw(&self, raw: &Value) -> Result<Uuid> {
        let bytes = serde_json::to_vec(raw)?;
        let payload_hash = digest(&bytes);
        let id = Uuid::new_v4();
        let inserted: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO platform_raw_events
             (id,connection_id,stream,event_type,payload,payload_hash)
             VALUES($1,$2,$3,$4,$5,$6)
             ON CONFLICT DO NOTHING
             RETURNING id",
        )
        .bind(id)
        .bind(self.connection_id)
        .bind(ORDER_STREAM)
        .bind(EVENT_TYPE)
        .bind(raw)
        .bind(&payload_hash)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(id) = inserted {
            return Ok(id);
        }

        sqlx::query_scalar(
            "SELECT id FROM platform_raw_events
             WHERE connection_id=$1 AND stream=$2 AND event_type=$3
               AND external_event_id IS NULL AND payload_hash=$4",
        )
        .bind(self.connection_id)
        .bind(ORDER_STREAM)
        .bind(EVENT_TYPE)
        .bind(payload_hash)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    async fn reject_raw(&self, id: Uuid, detail: &str) -> Result<()> {
        sqlx::query(
            "UPDATE platform_raw_events
             SET processing_status='rejected',normalizer_version=$2,
                 error_code='meituan_normalize',error_detail=$3
             WHERE id=$1",
        )
        .bind(id)
        .bind(NORMALIZER_VERSION)
        .bind(detail)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn checkpoint_from_row(row: CheckpointRow) -> Checkpoint {
    let (cursor, window_start, window_end) = row;
    Checkpoint {
        cursor,
        window_start,
        window_end,
    }
}

async fn lock_checkpoint(
    tx: &mut Transaction<'_, Postgres>,
    connection_id: Uuid,
) -> Result<Option<Checkpoint>> {
    let row: Option<CheckpointRow> = sqlx::query_as(
        "SELECT cursor,window_start,window_end
         FROM platform_sync_checkpoints
         WHERE connection_id=$1 AND stream=$2
         FOR UPDATE",
    )
    .bind(connection_id)
    .bind(ORDER_STREAM)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(checkpoint_from_row))
}

fn next_window(
    checkpoint: Option<&Checkpoint>,
    now: DateTime<Utc>,
) -> Result<(DateTime<Utc>, DateTime<Utc>, Option<String>)> {
    if let Some(checkpoint) = checkpoint {
        if let Some(cursor) = &checkpoint.cursor {
            let start = checkpoint.window_start.ok_or_else(|| {
                PlatformError::invalid("美团 checkpoint cursor 缺少 window_start")
            })?;
            let end = checkpoint
                .window_end
                .ok_or_else(|| PlatformError::invalid("美团 checkpoint cursor 缺少 window_end"))?;
            return Ok((start, end, Some(cursor.clone())));
        }

        if let Some(previous_end) = checkpoint.window_end {
            let start = previous_end - Duration::minutes(OVERLAP_MINUTES);
            let end = (start + Duration::minutes(WINDOW_MINUTES)).min(now);
            if end > start {
                return Ok((start, end, None));
            }
        }
    }
    Ok((now - Duration::minutes(WINDOW_MINUTES), now, None))
}

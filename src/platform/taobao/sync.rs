use crate::{
    platform::{
        normalized::{CommissionObservation, OrderObservation},
        taobao::{
            client::TaobaoTransport, normalize_order_page, TaobaoClient, TaobaoOrderQuery,
            ORDER_STREAM,
        },
        PlatformError, Result,
    },
    transaction::{configure, digest},
};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

const WINDOW_MINUTES: i64 = 20;
const OVERLAP_MINUTES: i64 = 5;
const EVENT_TYPE: &str = "taobao.tbk.sc.order.details.get.page";
const NORMALIZER_VERSION: i32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOutcome {
    pub raw_event_id: Uuid,
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub next_cursor: Option<String>,
    pub has_next: bool,
    pub order_observations: usize,
    pub commission_observations: usize,
}

#[derive(Debug, Clone)]
struct Checkpoint {
    cursor: Option<String>,
    window_start: Option<DateTime<Utc>>,
    window_end: Option<DateTime<Utc>>,
}

pub struct TaobaoOrderSync<T> {
    pool: PgPool,
    connection_id: Uuid,
    client: TaobaoClient<T>,
}

impl<T> TaobaoOrderSync<T> {
    pub fn new(pool: PgPool, connection_id: Uuid, client: TaobaoClient<T>) -> Self {
        Self {
            pool,
            connection_id,
            client,
        }
    }
}

impl<T: TaobaoTransport> TaobaoOrderSync<T> {
    pub async fn sync_next(&self, now: DateTime<Utc>) -> Result<SyncOutcome> {
        self.ensure_connection().await?;
        let checkpoint = self.checkpoint().await?;
        let (window_start, window_end, cursor) = next_window(checkpoint.as_ref(), now)?;
        let query = TaobaoOrderQuery {
            start: window_start,
            end: window_end,
            position_index: cursor,
        };

        // Network I/O happens outside a database transaction. The fetched page is then
        // durably persisted before any normalization/checkpoint advancement.
        let raw = self.client.fetch_orders(&query, now).await?;
        let raw_event_id = self.persist_raw(&raw).await?;

        let page = match normalize_order_page(&raw) {
            Ok(page) => page,
            Err(error) => {
                self.reject_raw(raw_event_id, &error.to_string()).await?;
                return Err(error);
            }
        };

        if page.has_next && page.next_cursor.is_none() {
            self.reject_raw(raw_event_id, "has_next=true without position_index")
                .await?;
            return Err(PlatformError::invalid(
                "淘宝分页响应缺少下一页 position_index",
            ));
        }

        let mut tx = self.pool.begin().await?;
        configure(&mut tx).await?;

        for order in &page.orders {
            upsert_order_observation(&mut tx, self.connection_id, raw_event_id, order).await?;
        }
        for commission in &page.commissions {
            upsert_commission_observation(&mut tx, self.connection_id, raw_event_id, commission)
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

        let checkpoint_cursor = if page.has_next {
            page.next_cursor.clone()
        } else {
            None
        };
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
        .bind(&checkpoint_cursor)
        .bind(window_start)
        .bind(window_end)
        .bind(page.max_source_updated_at)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(SyncOutcome {
            raw_event_id,
            window_start,
            window_end,
            next_cursor: checkpoint_cursor,
            has_next: page.has_next,
            order_observations: page.orders.len(),
            commission_observations: page.commissions.len(),
        })
    }

    async fn ensure_connection(&self) -> Result<()> {
        let found: Option<(String, String)> =
            sqlx::query_as("SELECT platform,status FROM platform_connections WHERE id=$1")
                .bind(self.connection_id)
                .fetch_optional(&self.pool)
                .await?;
        match found {
            Some((platform, status)) if platform == "taobao" && status == "active" => Ok(()),
            Some((platform, _)) if platform != "taobao" => Err(PlatformError::invalid(
                "TaobaoOrderSync 只能使用 taobao platform connection",
            )),
            Some(_) => Err(PlatformError::invalid("淘宝 platform connection 未启用")),
            None => Err(PlatformError::invalid("淘宝 platform connection 不存在")),
        }
    }

    async fn checkpoint(&self) -> Result<Option<Checkpoint>> {
        let row: Option<(Option<String>, Option<DateTime<Utc>>, Option<DateTime<Utc>>)> =
            sqlx::query_as(
                "SELECT cursor,window_start,window_end
             FROM platform_sync_checkpoints
             WHERE connection_id=$1 AND stream=$2",
            )
            .bind(self.connection_id)
            .bind(ORDER_STREAM)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(cursor, window_start, window_end)| Checkpoint {
            cursor,
            window_start,
            window_end,
        }))
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
                 error_code='taobao_normalize',error_detail=$3
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

fn next_window(
    checkpoint: Option<&Checkpoint>,
    now: DateTime<Utc>,
) -> Result<(DateTime<Utc>, DateTime<Utc>, Option<String>)> {
    if let Some(checkpoint) = checkpoint {
        if let Some(cursor) = &checkpoint.cursor {
            let start = checkpoint.window_start.ok_or_else(|| {
                PlatformError::invalid("淘宝 checkpoint cursor 缺少 window_start")
            })?;
            let end = checkpoint
                .window_end
                .ok_or_else(|| PlatformError::invalid("淘宝 checkpoint cursor 缺少 window_end"))?;
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

async fn upsert_order_observation(
    tx: &mut Transaction<'_, Postgres>,
    connection_id: Uuid,
    raw_event_id: Uuid,
    order: &OrderObservation,
) -> Result<()> {
    let observation_hash = hash(order)?;
    let candidate = Uuid::new_v4();
    let observation_id: Uuid = match sqlx::query_scalar(
        "INSERT INTO external_order_observations
         (id,connection_id,raw_event_id,external_parent_order_id,external_order_line_id,
          external_product_id,external_promoter_id,external_position_id,merchant_ref,customer_ref,
          currency,paid_minor,settlement_base_minor,raw_status,paid_at,completed_at,
          source_updated_at,normalizer_version,observation_hash,normalized_payload)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)
         ON CONFLICT(raw_event_id,observation_hash) DO NOTHING
         RETURNING id",
    )
    .bind(candidate)
    .bind(connection_id)
    .bind(raw_event_id)
    .bind(&order.external_parent_order_id)
    .bind(&order.external_order_line_id)
    .bind(&order.external_product_id)
    .bind(&order.external_promoter_id)
    .bind(&order.external_position_id)
    .bind(&order.merchant_ref)
    .bind(&order.customer_ref)
    .bind(&order.currency)
    .bind(order.paid_minor)
    .bind(order.settlement_base_minor)
    .bind(&order.raw_status)
    .bind(order.paid_at)
    .bind(order.completed_at)
    .bind(order.source_updated_at)
    .bind(NORMALIZER_VERSION)
    .bind(&observation_hash)
    .bind(&order.normalized_payload)
    .fetch_optional(&mut **tx)
    .await?
    {
        Some(id) => id,
        None => {
            sqlx::query_scalar(
                "SELECT id FROM external_order_observations
                 WHERE raw_event_id=$1 AND observation_hash=$2",
            )
            .bind(raw_event_id)
            .bind(&observation_hash)
            .fetch_one(&mut **tx)
            .await?
        }
    };

    sqlx::query(
        "INSERT INTO external_orders
         (connection_id,external_order_line_id,latest_observation_id,external_parent_order_id,
          normalized_status,raw_status,currency,paid_minor,settlement_base_minor,paid_at,
          completed_at,last_platform_updated_at,updated_at)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,now())
         ON CONFLICT(connection_id,external_order_line_id) DO UPDATE SET
          latest_observation_id=EXCLUDED.latest_observation_id,
          external_parent_order_id=EXCLUDED.external_parent_order_id,
          normalized_status=EXCLUDED.normalized_status,
          raw_status=EXCLUDED.raw_status,
          currency=EXCLUDED.currency,
          paid_minor=EXCLUDED.paid_minor,
          settlement_base_minor=EXCLUDED.settlement_base_minor,
          paid_at=EXCLUDED.paid_at,
          completed_at=EXCLUDED.completed_at,
          last_platform_updated_at=EXCLUDED.last_platform_updated_at,
          updated_at=now()
         WHERE EXCLUDED.last_platform_updated_at >= external_orders.last_platform_updated_at",
    )
    .bind(connection_id)
    .bind(&order.external_order_line_id)
    .bind(observation_id)
    .bind(&order.external_parent_order_id)
    .bind(&order.normalized_status)
    .bind(&order.raw_status)
    .bind(&order.currency)
    .bind(order.paid_minor)
    .bind(order.settlement_base_minor)
    .bind(order.paid_at)
    .bind(order.completed_at)
    .bind(order.source_updated_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn upsert_commission_observation(
    tx: &mut Transaction<'_, Postgres>,
    connection_id: Uuid,
    raw_event_id: Uuid,
    commission: &CommissionObservation,
) -> Result<()> {
    let observation_hash = hash(commission)?;
    let candidate = Uuid::new_v4();
    let observation_id: Uuid = match sqlx::query_scalar(
        "INSERT INTO external_commission_observations
         (id,connection_id,raw_event_id,external_commission_key,external_order_line_id,
          external_beneficiary_id,beneficiary_role,phase,funding_phase,currency,gross_minor,
          platform_service_fee_minor,special_service_fee_minor,institution_share_minor,net_minor,
          raw_status,source_updated_at,normalizer_version,observation_hash,metadata)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)
         ON CONFLICT(raw_event_id,observation_hash) DO NOTHING
         RETURNING id",
    )
    .bind(candidate)
    .bind(connection_id)
    .bind(raw_event_id)
    .bind(&commission.external_commission_key)
    .bind(&commission.external_order_line_id)
    .bind(&commission.external_beneficiary_id)
    .bind(&commission.beneficiary_role)
    .bind(&commission.phase)
    .bind(&commission.funding_phase)
    .bind(&commission.currency)
    .bind(commission.gross_minor)
    .bind(commission.platform_service_fee_minor)
    .bind(commission.special_service_fee_minor)
    .bind(commission.institution_share_minor)
    .bind(commission.net_minor)
    .bind(&commission.raw_status)
    .bind(commission.source_updated_at)
    .bind(NORMALIZER_VERSION)
    .bind(&observation_hash)
    .bind(&commission.metadata)
    .fetch_optional(&mut **tx)
    .await?
    {
        Some(id) => id,
        None => {
            sqlx::query_scalar(
                "SELECT id FROM external_commission_observations
                 WHERE raw_event_id=$1 AND observation_hash=$2",
            )
            .bind(raw_event_id)
            .bind(&observation_hash)
            .fetch_one(&mut **tx)
            .await?
        }
    };

    sqlx::query(
        "INSERT INTO external_commissions
         (connection_id,external_commission_key,latest_observation_id,external_order_line_id,
          beneficiary_role,beneficiary_ref,phase,funding_phase,currency,gross_minor,
          platform_service_fee_minor,special_service_fee_minor,institution_share_minor,net_minor,
          raw_status,last_platform_updated_at,updated_at)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,now())
         ON CONFLICT(connection_id,external_commission_key) DO UPDATE SET
          latest_observation_id=EXCLUDED.latest_observation_id,
          external_order_line_id=EXCLUDED.external_order_line_id,
          beneficiary_role=EXCLUDED.beneficiary_role,
          beneficiary_ref=EXCLUDED.beneficiary_ref,
          phase=EXCLUDED.phase,
          funding_phase=EXCLUDED.funding_phase,
          currency=EXCLUDED.currency,
          gross_minor=EXCLUDED.gross_minor,
          platform_service_fee_minor=EXCLUDED.platform_service_fee_minor,
          special_service_fee_minor=EXCLUDED.special_service_fee_minor,
          institution_share_minor=EXCLUDED.institution_share_minor,
          net_minor=EXCLUDED.net_minor,
          raw_status=EXCLUDED.raw_status,
          last_platform_updated_at=EXCLUDED.last_platform_updated_at,
          updated_at=now()
         WHERE EXCLUDED.last_platform_updated_at >= external_commissions.last_platform_updated_at",
    )
    .bind(connection_id)
    .bind(&commission.external_commission_key)
    .bind(observation_id)
    .bind(&commission.external_order_line_id)
    .bind(&commission.beneficiary_role)
    .bind(&commission.external_beneficiary_id)
    .bind(&commission.phase)
    .bind(&commission.funding_phase)
    .bind(&commission.currency)
    .bind(commission.gross_minor)
    .bind(commission.platform_service_fee_minor)
    .bind(commission.special_service_fee_minor)
    .bind(commission.institution_share_minor)
    .bind(commission.net_minor)
    .bind(&commission.raw_status)
    .bind(commission.source_updated_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn hash<T: Serialize>(value: &T) -> Result<String> {
    Ok(digest(&serde_json::to_vec(value)?))
}

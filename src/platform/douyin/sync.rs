use crate::{
    platform::{
        douyin::{
            normalize_alliance_message, normalize_reconcile_page, webhook::parse_webhook,
            DouyinClient, DouyinMessageVerifier, DouyinTransport, ALLIANCE_TAGS,
            NORMALIZER_VERSION, ORDER_METHOD, ORDER_PATH, RECONCILE_STREAM, WEBHOOK_STREAM,
        },
        store::{
            insert_refund_observation, insert_settlement_observation,
            upsert_commission_observation, upsert_order_observation,
        },
        PlatformError, Result,
    },
    transaction::{configure, digest},
};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WebhookOutcome {
    pub raw_events: usize,
    pub duplicates: usize,
    pub order_observations: usize,
    pub commission_observations: usize,
    pub refund_observations: usize,
    pub settlement_observations: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileOutcome {
    pub raw_event_id: Uuid,
    pub order_observations: usize,
    pub commission_observations: usize,
    pub refund_observations: usize,
    pub settlement_observations: usize,
    pub next_cursor: Option<String>,
}

pub struct DouyinAllianceSync<T> {
    pool: PgPool,
    connection_id: Uuid,
    client: DouyinClient<T>,
}

impl<T> DouyinAllianceSync<T> {
    pub fn new(pool: PgPool, connection_id: Uuid, client: DouyinClient<T>) -> Self {
        Self {
            pool,
            connection_id,
            client,
        }
    }

    async fn ensure_connection(&self) -> Result<()> {
        let found: Option<(String, String)> =
            sqlx::query_as("SELECT platform,status FROM platform_connections WHERE id=$1")
                .bind(self.connection_id)
                .fetch_optional(&self.pool)
                .await?;
        match found {
            Some((platform, status)) if platform == "douyin" && status == "active" => Ok(()),
            Some((platform, _)) if platform != "douyin" => Err(PlatformError::invalid(
                "DouyinAllianceSync 只能使用 douyin platform connection",
            )),
            Some(_) => Err(PlatformError::invalid("抖音 platform connection 未启用")),
            None => Err(PlatformError::invalid("抖音 platform connection 不存在")),
        }
    }

    pub async fn ingest_webhook(
        &self,
        verifier: &DouyinMessageVerifier,
        app_id: &str,
        event_sign: &str,
        body: &[u8],
        received_at: DateTime<Utc>,
    ) -> Result<WebhookOutcome> {
        self.ensure_connection().await?;
        verifier.verify(app_id, event_sign, body)?;
        let messages = parse_webhook(body)?;
        let mut outcome = WebhookOutcome::default();

        for message in messages {
            let payload = json!({
                "tag": &message.tag,
                "msg_id": &message.msg_id,
                "data": &message.data,
            });
            let raw = self
                .persist_raw(
                    WEBHOOK_STREAM,
                    Some(&message.msg_id),
                    &format!("douyin.alliance.tag.{}", message.tag),
                    &payload,
                )
                .await?;
            if raw.inserted {
                outcome.raw_events += 1;
            }

            if !raw.inserted && raw.processing_status == "normalized" {
                outcome.duplicates += 1;
                continue;
            }

            if message.tag == "0" {
                self.mark_raw_normalized(raw.id).await?;
                continue;
            }
            if !ALLIANCE_TAGS.contains(&message.tag.as_str()) {
                self.reject_raw(raw.id, "unsupported_tag", "非精选联盟消息 tag")
                    .await?;
                return Err(PlatformError::invalid(format!(
                    "不支持的抖音精选联盟消息 tag：{}",
                    message.tag
                )));
            }

            let batch = match normalize_alliance_message(&message, received_at) {
                Ok(batch) => batch,
                Err(error) => {
                    self.reject_raw(raw.id, "douyin_normalize", &error.to_string())
                        .await?;
                    return Err(error);
                }
            };

            let mut tx = self.pool.begin().await?;
            configure(&mut tx).await?;
            for order in &batch.orders {
                upsert_order_observation(
                    &mut tx,
                    self.connection_id,
                    raw.id,
                    order,
                    NORMALIZER_VERSION,
                )
                .await?;
            }
            for commission in &batch.commissions {
                upsert_commission_observation(
                    &mut tx,
                    self.connection_id,
                    raw.id,
                    commission,
                    NORMALIZER_VERSION,
                )
                .await?;
            }
            for refund in &batch.refunds {
                insert_refund_observation(
                    &mut tx,
                    self.connection_id,
                    raw.id,
                    refund,
                    NORMALIZER_VERSION,
                )
                .await?;
            }
            for settlement in &batch.settlements {
                insert_settlement_observation(
                    &mut tx,
                    self.connection_id,
                    raw.id,
                    settlement,
                    NORMALIZER_VERSION,
                )
                .await?;
            }
            mark_raw_normalized_tx(&mut tx, raw.id).await?;
            tx.commit().await?;

            outcome.order_observations += batch.orders.len();
            outcome.commission_observations += batch.commissions.len();
            outcome.refund_observations += batch.refunds.len();
            outcome.settlement_observations += batch.settlements.len();
        }
        Ok(outcome)
    }

    async fn persist_raw(
        &self,
        stream: &str,
        external_event_id: Option<&str>,
        event_type: &str,
        payload: &Value,
    ) -> Result<RawState> {
        let payload_hash = digest(&serde_json::to_vec(payload)?);
        let id = Uuid::new_v4();
        let inserted: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO platform_raw_events
             (id,connection_id,stream,external_event_id,event_type,payload,payload_hash)
             VALUES($1,$2,$3,$4,$5,$6,$7)
             ON CONFLICT DO NOTHING RETURNING id",
        )
        .bind(id)
        .bind(self.connection_id)
        .bind(stream)
        .bind(external_event_id)
        .bind(event_type)
        .bind(payload)
        .bind(&payload_hash)
        .fetch_optional(&self.pool)
        .await?;
        if inserted.is_some() {
            return Ok(RawState {
                id,
                inserted: true,
                processing_status: "pending".into(),
            });
        }

        let row: (Uuid, String, String) = if let Some(external_event_id) = external_event_id {
            sqlx::query_as(
                "SELECT id,payload_hash,processing_status
                 FROM platform_raw_events
                 WHERE connection_id=$1 AND stream=$2 AND external_event_id=$3",
            )
            .bind(self.connection_id)
            .bind(stream)
            .bind(external_event_id)
            .fetch_one(&self.pool)
            .await?
        } else {
            sqlx::query_as(
                "SELECT id,payload_hash,processing_status
                 FROM platform_raw_events
                 WHERE connection_id=$1 AND stream=$2 AND event_type=$3
                   AND external_event_id IS NULL AND payload_hash=$4",
            )
            .bind(self.connection_id)
            .bind(stream)
            .bind(event_type)
            .bind(&payload_hash)
            .fetch_one(&self.pool)
            .await?
        };
        if row.1 != payload_hash {
            return Err(PlatformError::invalid(
                "同一抖音消息 msg_id 对应了不同 payload，拒绝覆盖原始事实",
            ));
        }
        Ok(RawState {
            id: row.0,
            inserted: false,
            processing_status: row.2,
        })
    }

    async fn mark_raw_normalized(&self, id: Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE platform_raw_events
             SET processing_status='normalized',normalizer_version=$2,error_code=NULL,error_detail=NULL
             WHERE id=$1",
        )
        .bind(id)
        .bind(NORMALIZER_VERSION)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn reject_raw(&self, id: Uuid, code: &str, detail: &str) -> Result<()> {
        sqlx::query(
            "UPDATE platform_raw_events
             SET processing_status='rejected',normalizer_version=$2,error_code=$3,error_detail=$4
             WHERE id=$1",
        )
        .bind(id)
        .bind(NORMALIZER_VERSION)
        .bind(code)
        .bind(detail)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

impl<T: DouyinTransport> DouyinAllianceSync<T> {
    pub async fn reconcile_page(
        &self,
        params: &Value,
        now: DateTime<Utc>,
    ) -> Result<ReconcileOutcome> {
        self.ensure_connection().await?;
        let raw = self
            .client
            .call(ORDER_PATH, ORDER_METHOD, params, now)
            .await?;
        let raw_state = self
            .persist_raw(
                RECONCILE_STREAM,
                None,
                "douyin.alliance.getOrderList.page",
                &raw,
            )
            .await?;
        let batch = match normalize_reconcile_page(&raw, now) {
            Ok(batch) => batch,
            Err(error) => {
                self.reject_raw(raw_state.id, "douyin_reconcile_normalize", &error.to_string())
                    .await?;
                return Err(error);
            }
        };
        let next_cursor = response_cursor(&raw);

        let mut tx = self.pool.begin().await?;
        configure(&mut tx).await?;
        for order in &batch.orders {
            upsert_order_observation(
                &mut tx,
                self.connection_id,
                raw_state.id,
                order,
                NORMALIZER_VERSION,
            )
            .await?;
        }
        for commission in &batch.commissions {
            upsert_commission_observation(
                &mut tx,
                self.connection_id,
                raw_state.id,
                commission,
                NORMALIZER_VERSION,
            )
            .await?;
        }
        for refund in &batch.refunds {
            insert_refund_observation(
                &mut tx,
                self.connection_id,
                raw_state.id,
                refund,
                NORMALIZER_VERSION,
            )
            .await?;
        }
        for settlement in &batch.settlements {
            insert_settlement_observation(
                &mut tx,
                self.connection_id,
                raw_state.id,
                settlement,
                NORMALIZER_VERSION,
            )
            .await?;
        }
        mark_raw_normalized_tx(&mut tx, raw_state.id).await?;
        sqlx::query(
            "INSERT INTO platform_sync_checkpoints
             (connection_id,stream,cursor,last_platform_updated_at,last_attempt_at,last_success_at,updated_at)
             VALUES($1,$2,$3,$4,$5,$5,$5)
             ON CONFLICT(connection_id,stream) DO UPDATE SET
               cursor=EXCLUDED.cursor,
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
        .bind(RECONCILE_STREAM)
        .bind(&next_cursor)
        .bind(batch.max_source_updated_at)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(ReconcileOutcome {
            raw_event_id: raw_state.id,
            order_observations: batch.orders.len(),
            commission_observations: batch.commissions.len(),
            refund_observations: batch.refunds.len(),
            settlement_observations: batch.settlements.len(),
            next_cursor,
        })
    }
}

#[derive(Debug)]
struct RawState {
    id: Uuid,
    inserted: bool,
    processing_status: String,
}

async fn mark_raw_normalized_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
) -> Result<()> {
    sqlx::query(
        "UPDATE platform_raw_events
         SET processing_status='normalized',normalizer_version=$2,error_code=NULL,error_detail=NULL
         WHERE id=$1",
    )
    .bind(id)
    .bind(NORMALIZER_VERSION)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn response_cursor(raw: &Value) -> Option<String> {
    let data = raw.get("data").unwrap_or(raw);
    for key in ["next_cursor", "cursor", "page_token"] {
        if let Some(value) = data.get(key) {
            match value {
                Value::String(v) if !v.is_empty() => return Some(v.clone()),
                Value::Number(v) => return Some(v.to_string()),
                _ => {}
            }
        }
    }
    None
}

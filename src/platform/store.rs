use crate::{
    platform::{
        normalized::{
            CommissionObservation, OrderObservation, RefundObservation, SettlementObservation,
        },
        Result,
    },
    transaction::digest,
};
use serde::Serialize;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub async fn upsert_order_observation(
    tx: &mut Transaction<'_, Postgres>,
    connection_id: Uuid,
    raw_event_id: Uuid,
    order: &OrderObservation,
    normalizer_version: i32,
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
    .bind(normalizer_version)
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

pub async fn upsert_commission_observation(
    tx: &mut Transaction<'_, Postgres>,
    connection_id: Uuid,
    raw_event_id: Uuid,
    commission: &CommissionObservation,
    normalizer_version: i32,
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
    .bind(normalizer_version)
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

pub async fn insert_refund_observation(
    tx: &mut Transaction<'_, Postgres>,
    connection_id: Uuid,
    raw_event_id: Uuid,
    refund: &RefundObservation,
    normalizer_version: i32,
) -> Result<()> {
    let observation_hash = hash(refund)?;
    sqlx::query(
        "INSERT INTO external_refund_observations
         (id,connection_id,raw_event_id,external_refund_id,external_order_line_id,refund_status,
          currency,refund_minor,commission_reversal_minor,occurred_at,source_updated_at,
          normalizer_version,observation_hash,metadata)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
         ON CONFLICT(raw_event_id,observation_hash) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(connection_id)
    .bind(raw_event_id)
    .bind(&refund.external_refund_id)
    .bind(&refund.external_order_line_id)
    .bind(&refund.refund_status)
    .bind(&refund.currency)
    .bind(refund.refund_minor)
    .bind(refund.commission_reversal_minor)
    .bind(refund.occurred_at)
    .bind(refund.source_updated_at)
    .bind(normalizer_version)
    .bind(observation_hash)
    .bind(&refund.metadata)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn insert_settlement_observation(
    tx: &mut Transaction<'_, Postgres>,
    connection_id: Uuid,
    raw_event_id: Uuid,
    settlement: &SettlementObservation,
    normalizer_version: i32,
) -> Result<()> {
    let observation_hash = hash(settlement)?;
    sqlx::query(
        "INSERT INTO external_settlement_observations
         (id,connection_id,raw_event_id,external_settlement_id,external_commission_key,
          external_order_line_id,currency,gross_minor,fee_minor,net_minor,settlement_status,
          settled_at,funded_at,statement_period,provider_reference,source_updated_at,
          normalizer_version,observation_hash,metadata)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)
         ON CONFLICT(raw_event_id,observation_hash) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(connection_id)
    .bind(raw_event_id)
    .bind(&settlement.external_settlement_id)
    .bind(&settlement.external_commission_key)
    .bind(&settlement.external_order_line_id)
    .bind(&settlement.currency)
    .bind(settlement.gross_minor)
    .bind(settlement.fee_minor)
    .bind(settlement.net_minor)
    .bind(&settlement.settlement_status)
    .bind(settlement.settled_at)
    .bind(settlement.funded_at)
    .bind(&settlement.statement_period)
    .bind(&settlement.provider_reference)
    .bind(settlement.source_updated_at)
    .bind(normalizer_version)
    .bind(observation_hash)
    .bind(&settlement.metadata)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn hash<T: Serialize>(value: &T) -> Result<String> {
    Ok(digest(&serde_json::to_vec(value)?))
}

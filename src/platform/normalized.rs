use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrderObservation {
    pub external_parent_order_id: Option<String>,
    pub external_order_line_id: String,
    pub external_product_id: Option<String>,
    pub external_promoter_id: Option<String>,
    pub external_position_id: Option<String>,
    pub merchant_ref: Option<String>,
    pub customer_ref: Option<String>,
    pub currency: String,
    pub paid_minor: Option<i64>,
    pub settlement_base_minor: Option<i64>,
    pub normalized_status: String,
    pub raw_status: String,
    pub paid_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub source_updated_at: DateTime<Utc>,
    pub normalized_payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommissionObservation {
    pub external_commission_key: String,
    pub external_order_line_id: String,
    pub external_beneficiary_id: Option<String>,
    pub beneficiary_role: String,
    pub phase: String,
    pub funding_phase: String,
    pub currency: String,
    pub gross_minor: Option<i64>,
    pub platform_service_fee_minor: Option<i64>,
    pub special_service_fee_minor: Option<i64>,
    pub institution_share_minor: Option<i64>,
    pub net_minor: Option<i64>,
    pub raw_status: String,
    pub source_updated_at: DateTime<Utc>,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedPage {
    pub orders: Vec<OrderObservation>,
    pub commissions: Vec<CommissionObservation>,
    pub has_next: bool,
    pub next_cursor: Option<String>,
    pub max_source_updated_at: Option<DateTime<Utc>>,
}

use crate::{domain::Terms, error::{Error, Result}, money::Money};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const PLATFORM: Uuid = Uuid::from_u128(1);
pub const CLEARING: Uuid = Uuid::from_u128(2);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct Actor {
    pub id: Uuid,
    pub name: String,
    pub role: String,
    pub account_id: Option<Uuid>,
    pub expires_at: DateTime<Utc>,
}
impl Actor {
    pub fn require(&self, roles: &[&str]) -> Result<()> {
        if self.role == "admin" || roles.contains(&self.role.as_str()) { Ok(()) } else { Err(Error::Forbidden) }
    }
    pub fn check_account(&self, id: Uuid) -> Result<()> {
        if self.role == "member" && self.account_id != Some(id) { Err(Error::NotFound) } else { Ok(()) }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct Account {
    pub id: Uuid, pub external_id: String, pub name: String, pub kind: String,
    pub parent_id: Option<Uuid>, pub active: bool, pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct Wallet {
    pub account_id: Uuid, pub frozen_minor: Money, pub available_minor: Money,
    pub reserved_minor: Money, pub updated_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct Rule {
    pub id: Uuid, pub version: i64, pub name: String, pub merchant_id: Option<Uuid>,
    pub priority: i32, pub min_base_minor: Money, pub max_base_minor: Option<Money>,
    pub rate_bps: i32, pub fixed_minor: Money, pub cap_minor: Option<Money>,
    pub direct_bps: i32, pub indirect_bps: i32, pub freeze_seconds: i64,
    pub effective_from: DateTime<Utc>, pub effective_until: Option<DateTime<Utc>>,
    pub active: bool, pub created_at: DateTime<Utc>,
}
impl Rule {
    pub fn terms(&self) -> Terms {
        Terms { rate_bps: self.rate_bps, fixed_minor: self.fixed_minor, cap_minor: self.cap_minor,
            direct_bps: self.direct_bps, indirect_bps: self.indirect_bps, freeze_seconds: self.freeze_seconds }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct Order {
    pub id: Uuid, pub external_id: String, pub merchant_id: Uuid,
    pub customer_external_id: Option<String>, pub currency: String,
    pub paid_minor: Money, pub commission_base_minor: Money, pub fee_pool_minor: Money,
    pub refunded_minor: Money, pub rule_id: Uuid, pub rule_snapshot: Value,
    #[serde(skip)]
    pub input_hash: String,
    pub captured_at: DateTime<Utc>, pub unlock_at: DateTime<Utc>, pub released_at: Option<DateTime<Utc>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct Allocation {
    pub order_id: Uuid, pub ordinal: i16, pub account_id: Uuid, pub slot: String,
    pub original_minor: Money, pub refunded_minor: Money,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct Payout {
    pub id: Uuid, pub external_id: String, pub account_id: Uuid, pub amount_minor: Money,
    pub destination_ref: String, pub status: String, pub requested_by: Uuid,
    pub approved_by: Option<Uuid>, pub provider_reference: Option<String>, pub evidence: Option<String>,
    pub created_at: DateTime<Utc>, pub updated_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAccount {
    pub external_id: String, pub name: String, pub kind: String, pub parent_id: Option<Uuid>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindReferral { pub customer_external_id: String, pub promoter_id: Uuid }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRule {
    pub name: String, pub merchant_id: Option<Uuid>,
    #[serde(default)] pub priority: i32,
    #[serde(default)] pub min_base_minor: Money,
    pub max_base_minor: Option<Money>, pub terms: Terms,
    pub effective_from: Option<DateTime<Utc>>, pub effective_until: Option<DateTime<Utc>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuoteInput {
    pub merchant_id: Uuid, pub customer_external_id: Option<String>,
    pub paid_minor: Money, pub commission_base_minor: Money,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureOrder {
    pub external_id: String, pub currency: String, pub merchant_id: Uuid,
    pub customer_external_id: Option<String>, pub paid_minor: Money, pub commission_base_minor: Money,
}
impl CaptureOrder {
    pub fn quote_input(&self) -> QuoteInput {
        QuoteInput { merchant_id: self.merchant_id, customer_external_id: self.customer_external_id.clone(),
            paid_minor: self.paid_minor, commission_base_minor: self.commission_base_minor }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefundInput { pub external_id: String, pub amount_minor: Money, pub reason: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestPayout { pub external_id: String, pub account_id: Uuid, pub amount_minor: Money, pub destination_ref: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayoutOutcome { pub status: String, pub provider_reference: Option<String>, pub evidence: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasonInput { pub reason: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Empty {}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateCredential {
    pub name: String, pub role: String, pub account_id: Option<Uuid>, pub expires_in_days: i64, pub secret: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimEvents { pub limit: i64, pub lease_seconds: i64 }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AckEvent { pub lease_token: Uuid }
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Page { pub limit: Option<i64>, pub offset: Option<i64>, pub account_id: Option<Uuid> }
impl Page {
    pub fn bounds(&self) -> Result<(i64, i64)> {
        let (limit, offset) = (self.limit.unwrap_or(50), self.offset.unwrap_or(0));
        if !(1..=200).contains(&limit) || !(0..=1_000_000).contains(&offset) {
            return Err(Error::invalid("分页 limit 为 1..200，offset 为 0..1000000"));
        }
        Ok((limit, offset))
    }
}
pub fn text(value: &str, max: usize, label: &str) -> Result<()> {
    if value.trim().is_empty() || value.chars().count() > max || value.chars().any(char::is_control) {
        Err(Error::invalid(format!("{label}不能为空，不能包含控制字符，最长 {max} 字符")))
    } else { Ok(()) }
}

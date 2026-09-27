use crate::{
    money::Money,
    platform::{
        normalized::{
            CommissionObservation, NormalizedBatch, OrderObservation, RefundObservation,
            SettlementObservation,
        },
        PlatformError, Result,
    },
};
use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeituanNormalizedPage {
    pub batch: NormalizedBatch,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Root {
    data: Option<Page>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    #[serde(default)]
    data_list: Vec<OrderDto>,
    scroll_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OrderDto {
    business_line: Option<i32>,
    order_id: String,
    pay_time: Option<i64>,
    pay_price: Option<String>,
    update_time: i64,
    commission_rate: Option<String>,
    profit: Option<String>,
    cpa_profit: Option<String>,
    sid: Option<String>,
    product_id: Option<String>,
    product_view_sign: Option<String>,
    product_name: Option<String>,
    order_detail: Option<Vec<OrderDetail>>,
    refund_price: Option<String>,
    refund_time: Option<String>,
    refund_profit: Option<String>,
    cpa_refund_profit: Option<String>,
    status: Option<String>,
    trade_type: Option<i32>,
    appkey: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OrderDetail {
    coupon_status: Option<String>,
    item_order_id: Option<String>,
    finish_time: Option<String>,
    basic_amount: Option<String>,
    coupon_fee: Option<String>,
    order_view_id: Option<String>,
    refund_amount: Option<String>,
    refund_fee: Option<String>,
    refund_time: Option<String>,
    settle_time: Option<String>,
    update_time: Option<String>,
}

pub fn normalize_order_page(raw: &Value) -> Result<MeituanNormalizedPage> {
    let root: Root = serde_json::from_value(raw.clone())
        .map_err(|error| PlatformError::invalid(format!("美团订单响应结构不符合预期：{error}")))?;
    let page = root.data.unwrap_or_default();
    let mut batch = NormalizedBatch::default();

    for order in &page.data_list {
        if order.order_id.trim().is_empty() {
            return Err(PlatformError::invalid("美团订单缺少 orderId"));
        }
        let parent_source_updated_at = epoch(order.update_time)?;
        let paid_at = order.pay_time.map(epoch).transpose()?;
        let details = order.order_detail.as_deref().unwrap_or_default();

        if details.is_empty() {
            normalize_parent(order, parent_source_updated_at, paid_at, &mut batch)?;
        } else {
            for (index, detail) in details.iter().enumerate() {
                normalize_detail(
                    order,
                    detail,
                    index,
                    parent_source_updated_at,
                    paid_at,
                    &mut batch,
                )?;
            }
        }
    }

    Ok(MeituanNormalizedPage {
        batch,
        next_cursor: clean(page.scroll_id.as_deref()).map(str::to_owned),
    })
}

fn normalize_parent(
    order: &OrderDto,
    source_updated_at: DateTime<Utc>,
    paid_at: Option<DateTime<Utc>>,
    batch: &mut NormalizedBatch,
) -> Result<()> {
    let line_id = order.order_id.clone();
    let raw_status = clean(order.status.as_deref()).unwrap_or("unknown");
    let refund_minor = money(order.refund_price.as_deref())?;
    let commission_reversal_minor = if order.trade_type == Some(2) {
        money(order.cpa_refund_profit.as_deref())?
    } else {
        money(order.refund_profit.as_deref())?
    };
    let paid_minor = money(order.pay_price.as_deref())?;
    let normalized_status = parent_order_status(raw_status, refund_minor);
    let product_id = clean(order.product_view_sign.as_deref())
        .or_else(|| clean(order.product_id.as_deref()))
        .map(str::to_owned);

    batch.orders.push(OrderObservation {
        external_parent_order_id: None,
        external_order_line_id: line_id.clone(),
        external_product_id: product_id,
        external_promoter_id: clean(order.appkey.as_deref()).map(str::to_owned),
        external_position_id: clean(order.sid.as_deref()).map(str::to_owned),
        merchant_ref: None,
        customer_ref: None,
        currency: "CNY".into(),
        paid_minor,
        settlement_base_minor: paid_minor,
        normalized_status: normalized_status.into(),
        raw_status: raw_status.into(),
        paid_at,
        completed_at: None,
        source_updated_at,
        normalized_payload: serde_json::to_value(order)
            .map_err(|error| PlatformError::invalid(error.to_string()))?,
    });

    let gross_minor = commission_amount(order)?;
    let (phase, funding_phase) = parent_commission_state(raw_status);
    let commission_key = commission_key(order, &line_id);
    batch.commissions.push(CommissionObservation {
        external_commission_key: commission_key.clone(),
        external_order_line_id: line_id.clone(),
        external_beneficiary_id: clean(order.appkey.as_deref()).map(str::to_owned),
        beneficiary_role: if order.trade_type == Some(2) {
            "publisher_cpa".into()
        } else {
            "publisher_cps".into()
        },
        phase: phase.into(),
        funding_phase: funding_phase.into(),
        currency: "CNY".into(),
        gross_minor,
        platform_service_fee_minor: None,
        special_service_fee_minor: None,
        institution_share_minor: None,
        net_minor: gross_minor,
        raw_status: raw_status.into(),
        source_updated_at,
        metadata: order_metadata(order),
    });

    if is_positive(refund_minor) || is_positive(commission_reversal_minor) {
        let occurred_at = optional_time(order.refund_time.as_deref())?;
        batch.refunds.push(RefundObservation {
            external_refund_id: format!(
                "{line_id}:refund:{}",
                time_identity(order.refund_time.as_deref(), source_updated_at)
            ),
            external_order_line_id: line_id.clone(),
            refund_status: "refunded".into(),
            currency: "CNY".into(),
            refund_minor,
            commission_reversal_minor,
            occurred_at,
            source_updated_at,
            metadata: order_metadata(order),
        });
    }

    if raw_status == "6" {
        batch.settlements.push(SettlementObservation {
            external_settlement_id: format!(
                "{line_id}:settlement:{}",
                source_updated_at.timestamp()
            ),
            external_commission_key: Some(commission_key),
            external_order_line_id: Some(line_id),
            currency: "CNY".into(),
            gross_minor,
            fee_minor: None,
            net_minor: gross_minor,
            settlement_status: "settled".into(),
            settled_at: Some(source_updated_at),
            funded_at: None,
            statement_period: None,
            provider_reference: Some(order.order_id.clone()),
            source_updated_at,
            metadata: order_metadata(order),
        });
    }

    update_max(batch, source_updated_at);
    Ok(())
}

fn normalize_detail(
    order: &OrderDto,
    detail: &OrderDetail,
    index: usize,
    parent_source_updated_at: DateTime<Utc>,
    paid_at: Option<DateTime<Utc>>,
    batch: &mut NormalizedBatch,
) -> Result<()> {
    let line_id = clean(detail.item_order_id.as_deref())
        .or_else(|| clean(detail.order_view_id.as_deref()))
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{}:detail:{}", order.order_id, index + 1));
    let source_updated_at =
        optional_time(detail.update_time.as_deref())?.unwrap_or(parent_source_updated_at);
    let raw_status = clean(detail.coupon_status.as_deref()).unwrap_or("unknown");
    let refund_minor = money(detail.refund_amount.as_deref())?;
    let commission_reversal_minor = money(detail.refund_fee.as_deref())?;
    let basic_minor = money(detail.basic_amount.as_deref())?;
    let completed_at = optional_time(detail.finish_time.as_deref())?;
    let normalized_status = detail_order_status(raw_status, refund_minor);
    let product_id = clean(order.product_view_sign.as_deref())
        .or_else(|| clean(order.product_id.as_deref()))
        .map(str::to_owned);

    batch.orders.push(OrderObservation {
        external_parent_order_id: Some(order.order_id.clone()),
        external_order_line_id: line_id.clone(),
        external_product_id: product_id,
        external_promoter_id: clean(order.appkey.as_deref()).map(str::to_owned),
        external_position_id: clean(order.sid.as_deref()).map(str::to_owned),
        merchant_ref: None,
        customer_ref: None,
        currency: "CNY".into(),
        paid_minor: basic_minor,
        settlement_base_minor: basic_minor,
        normalized_status: normalized_status.into(),
        raw_status: raw_status.into(),
        paid_at,
        completed_at,
        source_updated_at,
        normalized_payload: json!({
            "parent": order,
            "detail": detail,
        }),
    });

    let gross_minor = money(detail.coupon_fee.as_deref())?;
    let (phase, funding_phase) = detail_commission_state(raw_status);
    let commission_key = commission_key(order, &line_id);
    batch.commissions.push(CommissionObservation {
        external_commission_key: commission_key.clone(),
        external_order_line_id: line_id.clone(),
        external_beneficiary_id: clean(order.appkey.as_deref()).map(str::to_owned),
        beneficiary_role: if order.trade_type == Some(2) {
            "publisher_cpa".into()
        } else {
            "publisher_cps".into()
        },
        phase: phase.into(),
        funding_phase: funding_phase.into(),
        currency: "CNY".into(),
        gross_minor,
        platform_service_fee_minor: None,
        special_service_fee_minor: None,
        institution_share_minor: None,
        net_minor: gross_minor,
        raw_status: raw_status.into(),
        source_updated_at,
        metadata: detail_metadata(order, detail),
    });

    if is_positive(refund_minor) || is_positive(commission_reversal_minor) {
        let occurred_at = optional_time(detail.refund_time.as_deref())?;
        batch.refunds.push(RefundObservation {
            external_refund_id: format!(
                "{line_id}:refund:{}",
                time_identity(detail.refund_time.as_deref(), source_updated_at)
            ),
            external_order_line_id: line_id.clone(),
            refund_status: "refunded".into(),
            currency: "CNY".into(),
            refund_minor,
            commission_reversal_minor,
            occurred_at,
            source_updated_at,
            metadata: detail_metadata(order, detail),
        });
    }

    if raw_status == "3" {
        let settled_at = optional_time(detail.settle_time.as_deref())?.or(Some(source_updated_at));
        batch.settlements.push(SettlementObservation {
            external_settlement_id: format!(
                "{line_id}:settlement:{}",
                settled_at.unwrap_or(source_updated_at).timestamp()
            ),
            external_commission_key: Some(commission_key),
            external_order_line_id: Some(line_id),
            currency: "CNY".into(),
            gross_minor,
            fee_minor: None,
            net_minor: gross_minor,
            settlement_status: "settled".into(),
            settled_at,
            funded_at: None,
            statement_period: None,
            provider_reference: Some(order.order_id.clone()),
            source_updated_at,
            metadata: detail_metadata(order, detail),
        });
    }

    update_max(batch, source_updated_at);
    Ok(())
}

fn commission_amount(order: &OrderDto) -> Result<Option<i64>> {
    if order.trade_type == Some(2) {
        money(order.cpa_profit.as_deref())
    } else {
        money(order.profit.as_deref())
    }
}

fn commission_key(order: &OrderDto, line_id: &str) -> String {
    let beneficiary = clean(order.appkey.as_deref())
        .or_else(|| clean(order.sid.as_deref()))
        .unwrap_or("unknown");
    format!("{line_id}:publisher:{beneficiary}")
}

fn order_metadata(order: &OrderDto) -> Value {
    json!({
        "business_line": order.business_line,
        "commission_rate": clean(order.commission_rate.as_deref()),
        "trade_type": order.trade_type,
        "sid": clean(order.sid.as_deref()),
        "product_name": clean(order.product_name.as_deref()),
    })
}

fn detail_metadata(order: &OrderDto, detail: &OrderDetail) -> Value {
    json!({
        "parent_order_id": order.order_id,
        "business_line": order.business_line,
        "commission_rate": clean(order.commission_rate.as_deref()),
        "trade_type": order.trade_type,
        "sid": clean(order.sid.as_deref()),
        "product_name": clean(order.product_name.as_deref()),
        "settle_time": clean(detail.settle_time.as_deref()),
    })
}

fn parent_order_status(raw: &str, refund_minor: Option<i64>) -> &'static str {
    match raw {
        "2" => "paid",
        "3" | "6" => "completed",
        "4" if is_positive(refund_minor) => "refunded",
        "4" | "5" => "closed",
        _ => "unknown",
    }
}

fn detail_order_status(raw: &str, refund_minor: Option<i64>) -> &'static str {
    match raw {
        "1" => "paid",
        "2" | "3" => "completed",
        "4" if is_positive(refund_minor) => "refunded",
        "4" => "closed",
        _ => "unknown",
    }
}

fn parent_commission_state(raw: &str) -> (&'static str, &'static str) {
    match raw {
        "2" => ("estimated", "unfunded"),
        "3" => ("accrued", "unfunded"),
        "6" => ("settled", "receivable"),
        "4" | "5" => ("invalid", "reversed"),
        _ => ("estimated", "unfunded"),
    }
}

fn detail_commission_state(raw: &str) -> (&'static str, &'static str) {
    match raw {
        "1" => ("estimated", "unfunded"),
        "2" => ("accrued", "unfunded"),
        "3" => ("settled", "receivable"),
        "4" => ("invalid", "reversed"),
        _ => ("estimated", "unfunded"),
    }
}

fn money(value: Option<&str>) -> Result<Option<i64>> {
    clean(value)
        .map(Money::from_yuan)
        .transpose()
        .map(|value| value.map(|money| money.0))
        .map_err(PlatformError::invalid)
}

fn is_positive(value: Option<i64>) -> bool {
    value.is_some_and(|amount| amount > 0)
}

fn epoch(value: i64) -> Result<DateTime<Utc>> {
    if value > 10_000_000_000 {
        DateTime::from_timestamp_millis(value)
    } else {
        DateTime::from_timestamp(value, 0)
    }
    .ok_or_else(|| PlatformError::invalid(format!("无效美团时间戳：{value}")))
}

fn optional_time(value: Option<&str>) -> Result<Option<DateTime<Utc>>> {
    clean(value).map(parse_time).transpose()
}

fn parse_time(value: &str) -> Result<DateTime<Utc>> {
    if let Ok(timestamp) = value.parse::<i64>() {
        return epoch(timestamp);
    }
    if let Ok(value) = DateTime::parse_from_rfc3339(value) {
        return Ok(value.with_timezone(&Utc));
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S") {
        let china =
            FixedOffset::east_opt(8 * 3600).ok_or_else(|| PlatformError::invalid("无效时区"))?;
        return china
            .from_local_datetime(&naive)
            .single()
            .map(|value| value.with_timezone(&Utc))
            .ok_or_else(|| PlatformError::invalid(format!("无法解释美团时间：{value}")));
    }
    Err(PlatformError::invalid(format!("无效美团时间：{value}")))
}

fn clean(value: Option<&str>) -> Option<&str> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("null"))
}

fn time_identity(value: Option<&str>, fallback: DateTime<Utc>) -> String {
    clean(value)
        .map(|value| value.replace([' ', ':'], "-"))
        .unwrap_or_else(|| fallback.timestamp().to_string())
}

fn update_max(batch: &mut NormalizedBatch, source_updated_at: DateTime<Utc>) {
    batch.max_source_updated_at = Some(
        batch
            .max_source_updated_at
            .map_or(source_updated_at, |current| current.max(source_updated_at)),
    );
}

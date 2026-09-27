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
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DouyinMessage {
    pub tag: String,
    pub msg_id: String,
    pub data: Value,
}

pub fn normalize_alliance_message(
    message: &DouyinMessage,
    received_at: DateTime<Utc>,
) -> Result<NormalizedBatch> {
    let role = match message.tag.as_str() {
        "804" | "805" | "806" => "kol",
        "807" | "808" | "809" => "institution",
        _ => {
            return Err(PlatformError::invalid(format!(
                "不支持的抖音精选联盟消息 tag：{}",
                message.tag
            )))
        }
    };
    let kind = match message.tag.as_str() {
        "804" | "808" => Kind::Pay,
        "805" | "807" => Kind::Refund,
        "806" | "809" => Kind::Settlement,
        _ => unreachable!(),
    };

    let mut batch = NormalizedBatch::default();
    for record in records(&message.data)? {
        normalize_record(
            &record,
            role,
            kind,
            &message.msg_id,
            received_at,
            &mut batch,
        )?;
    }
    Ok(batch)
}

pub fn normalize_reconcile_page(raw: &Value, received_at: DateTime<Utc>) -> Result<NormalizedBatch> {
    let data = raw.get("data").unwrap_or(raw);
    let list = find_array(data, &["order_list", "orders", "list", "order_infos"])
        .ok_or_else(|| PlatformError::invalid("抖店联盟对账响应缺少订单列表"))?;

    let mut batch = NormalizedBatch::default();
    for record in list {
        let role = if first(record, &["institution_id", "inst_id", "mcn_id"]).is_some() {
            "institution"
        } else {
            "kol"
        };
        let has_refund =
            first(record, &["refund_amount", "refund_status", "after_sale_id"]).is_some();
        let has_settlement = first(
            record,
            &[
                "settle_time",
                "settlement_time",
                "settle_amount",
                "settle_commission",
            ],
        )
        .is_some();
        let kind = if has_refund {
            Kind::Refund
        } else if has_settlement {
            Kind::Settlement
        } else {
            Kind::Pay
        };
        normalize_record(record, role, kind, "reconcile", received_at, &mut batch)?;
    }
    Ok(batch)
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Pay,
    Refund,
    Settlement,
}

fn normalize_record(
    record: &Value,
    role: &str,
    kind: Kind,
    message_id: &str,
    received_at: DateTime<Utc>,
    batch: &mut NormalizedBatch,
) -> Result<()> {
    let order_id = required_id(
        record,
        &["sku_order_id", "order_id", "shop_order_id", "order_no"],
        "抖音联盟订单号",
    )?;
    let source_updated_at = time(
        first(
            record,
            &[
                "update_time",
                "update_time_ms",
                "modify_time",
                "modified_time",
                "event_time",
                "timestamp",
            ],
        ),
        received_at,
    )?;
    batch.max_source_updated_at = Some(
        batch
            .max_source_updated_at
            .map_or(source_updated_at, |v| v.max(source_updated_at)),
    );

    let raw_status = first_string(record, &["status", "order_status", "settle_status"])
        .unwrap_or_else(|| match kind {
            Kind::Pay => "paid".into(),
            Kind::Refund => "refund".into(),
            Kind::Settlement => "settled".into(),
        });
    let normalized_status = match kind {
        Kind::Pay => "paid",
        Kind::Refund => {
            let lower = raw_status.to_ascii_lowercase();
            if lower.contains("success")
                || lower.contains("refund")
                || lower == "3"
                || lower == "5"
            {
                "refunded"
            } else {
                "paid"
            }
        }
        Kind::Settlement => "completed",
    }
    .to_owned();

    let paid_minor = minor(first(
        record,
        &[
            "pay_amount",
            "pay_price",
            "order_amount",
            "total_pay_amount",
            "total_amount",
        ],
    ))?;
    let settlement_base_minor = minor(first(
        record,
        &[
            "settle_amount",
            "settlement_amount",
            "real_pay_amount",
            "pay_amount",
        ],
    ))?;
    let paid_at = optional_time(first(
        record,
        &["pay_time", "paid_time", "pay_success_time", "order_pay_time"],
    ))?;
    let completed_at = if matches!(kind, Kind::Settlement) {
        optional_time(first(record, &["settle_time", "settlement_time"]))?
    } else {
        None
    };

    batch.orders.push(OrderObservation {
        external_parent_order_id: first_string(
            record,
            &["parent_order_id", "main_order_id", "shop_order_id"],
        ),
        external_order_line_id: order_id.clone(),
        external_product_id: first_string(record, &["product_id", "item_id", "goods_id"]),
        external_promoter_id: beneficiary_id(record, role),
        external_position_id: first_string(record, &["pid", "promotion_id", "position_id"]),
        merchant_ref: first_string(record, &["shop_id", "merchant_id"]),
        customer_ref: None,
        currency: "CNY".into(),
        paid_minor,
        settlement_base_minor,
        normalized_status,
        raw_status: raw_status.clone(),
        paid_at,
        completed_at,
        source_updated_at,
        normalized_payload: record.clone(),
    });

    let commission_state = match kind {
        Kind::Pay => CommissionState {
            phase: "estimated",
            funding_phase: "unfunded",
            raw_status: &raw_status,
            source_updated_at,
        },
        Kind::Refund => {
            normalize_refund(record, role, message_id, received_at, batch)?;
            CommissionState {
                phase: "reversed",
                funding_phase: "reversed",
                raw_status: &raw_status,
                source_updated_at,
            }
        }
        Kind::Settlement => {
            normalize_settlement(record, role, message_id, received_at, batch)?;
            CommissionState {
                phase: "settled",
                funding_phase: "receivable",
                raw_status: &raw_status,
                source_updated_at,
            }
        }
    };
    normalize_commission(record, role, &order_id, commission_state, batch)?;
    Ok(())
}

struct CommissionState<'a> {
    phase: &'a str,
    funding_phase: &'a str,
    raw_status: &'a str,
    source_updated_at: DateTime<Utc>,
}

fn normalize_commission(
    record: &Value,
    role: &str,
    order_id: &str,
    state: CommissionState<'_>,
    batch: &mut NormalizedBatch,
) -> Result<()> {
    let beneficiary = beneficiary_id(record, role);
    let gross_minor = minor(first(
        record,
        &[
            "settle_commission",
            "settled_commission",
            "estimated_commission",
            "commission_amount",
            "commission",
            "ads_estimated_commission",
        ],
    ))?;
    let platform_service_fee_minor = minor(first(
        record,
        &[
            "tech_service_fee",
            "platform_service_fee",
            "service_fee",
            "technical_service_fee",
        ],
    ))?;
    let institution_share_minor = if role == "kol" {
        minor(first(
            record,
            &["institution_commission", "inst_commission", "mcn_commission"],
        ))?
    } else {
        None
    };
    let explicit_net = minor(first(
        record,
        &["net_commission", "actual_commission", "income_amount"],
    ))?;
    let net_minor = explicit_net.or_else(|| {
        gross_minor.map(|gross| {
            gross
                - platform_service_fee_minor.unwrap_or_default()
                - institution_share_minor.unwrap_or_default()
        })
    });
    let beneficiary_key = beneficiary.clone().unwrap_or_else(|| "unknown".into());

    batch.commissions.push(CommissionObservation {
        external_commission_key: format!("{order_id}:{role}:{beneficiary_key}"),
        external_order_line_id: order_id.to_owned(),
        external_beneficiary_id: beneficiary,
        beneficiary_role: role.into(),
        phase: state.phase.into(),
        funding_phase: state.funding_phase.into(),
        currency: "CNY".into(),
        gross_minor,
        platform_service_fee_minor,
        special_service_fee_minor: None,
        institution_share_minor,
        net_minor,
        raw_status: state.raw_status.into(),
        source_updated_at: state.source_updated_at,
        metadata: json!({
            "pid": first_string(record, &["pid", "promotion_id", "position_id"]),
            "source": "douyin_alliance"
        }),
    });
    Ok(())
}

fn normalize_refund(
    record: &Value,
    role: &str,
    message_id: &str,
    received_at: DateTime<Utc>,
    batch: &mut NormalizedBatch,
) -> Result<()> {
    let order_id = required_id(
        record,
        &["sku_order_id", "order_id", "shop_order_id", "order_no"],
        "抖音退款订单号",
    )?;
    let source_updated_at = time(
        first(
            record,
            &["update_time", "refund_time", "after_sale_time", "timestamp"],
        ),
        received_at,
    )?;
    let external_refund_id = first_string(
        record,
        &["refund_id", "after_sale_id", "aftersale_id", "service_id"],
    )
    .unwrap_or_else(|| format!("{order_id}:refund:{message_id}"));
    batch.refunds.push(RefundObservation {
        external_refund_id,
        external_order_line_id: order_id,
        refund_status: first_string(record, &["refund_status", "after_sale_status", "status"])
            .unwrap_or_else(|| "refund".into()),
        currency: "CNY".into(),
        refund_minor: minor(first(
            record,
            &["refund_amount", "refund_fee", "actual_refund_amount"],
        ))?,
        commission_reversal_minor: minor(first(
            record,
            &[
                "commission_refund_amount",
                "refund_commission",
                "commission_reversal_amount",
            ],
        ))?,
        occurred_at: optional_time(first(
            record,
            &["refund_time", "after_sale_time", "occurred_at"],
        ))?,
        source_updated_at,
        metadata: json!({"beneficiary_role": role}),
    });
    Ok(())
}

fn normalize_settlement(
    record: &Value,
    role: &str,
    message_id: &str,
    received_at: DateTime<Utc>,
    batch: &mut NormalizedBatch,
) -> Result<()> {
    let order_id = required_id(
        record,
        &["sku_order_id", "order_id", "shop_order_id", "order_no"],
        "抖音结算订单号",
    )?;
    let beneficiary = beneficiary_id(record, role);
    let key = format!(
        "{}:{}:{}",
        order_id,
        role,
        beneficiary.clone().unwrap_or_else(|| "unknown".into())
    );
    let source_updated_at = time(
        first(
            record,
            &["update_time", "settle_time", "settlement_time", "timestamp"],
        ),
        received_at,
    )?;
    let gross_minor = minor(first(
        record,
        &[
            "settle_commission",
            "settled_commission",
            "commission_amount",
            "commission",
        ],
    ))?;
    let fee_minor = minor(first(
        record,
        &[
            "tech_service_fee",
            "platform_service_fee",
            "service_fee",
            "technical_service_fee",
        ],
    ))?;
    let net_minor = minor(first(
        record,
        &["net_commission", "actual_commission", "income_amount"],
    ))?
    .or_else(|| gross_minor.map(|gross| gross - fee_minor.unwrap_or_default()));
    batch.settlements.push(SettlementObservation {
        external_settlement_id: first_string(
            record,
            &["settlement_id", "settle_id", "bill_id"],
        )
        .unwrap_or_else(|| format!("{order_id}:settlement:{message_id}")),
        external_commission_key: Some(key),
        external_order_line_id: Some(order_id),
        currency: "CNY".into(),
        gross_minor,
        fee_minor,
        net_minor,
        settlement_status: first_string(record, &["settle_status", "settlement_status", "status"])
            .unwrap_or_else(|| "settled".into()),
        settled_at: optional_time(first(record, &["settle_time", "settlement_time"]))?,
        funded_at: None,
        statement_period: first_string(record, &["statement_period", "bill_period"]),
        provider_reference: first_string(record, &["bill_id", "settlement_id", "settle_id"]),
        source_updated_at,
        metadata: json!({"beneficiary_role": role, "beneficiary_id": beneficiary}),
    });
    Ok(())
}

fn records(data: &Value) -> Result<Vec<Value>> {
    let data = match data {
        Value::String(raw) => serde_json::from_str(raw)
            .map_err(|e| PlatformError::invalid(format!("抖音消息 data JSON 无效：{e}")))?,
        other => other.clone(),
    };
    match data {
        Value::Array(values) => Ok(values),
        Value::Object(_) => Ok(vec![data]),
        _ => Err(PlatformError::invalid("抖音消息 data 必须是对象或数组")),
    }
}

fn find_array<'a>(value: &'a Value, names: &[&str]) -> Option<&'a Vec<Value>> {
    for name in names {
        if let Some(array) = value.get(*name).and_then(Value::as_array) {
            return Some(array);
        }
    }
    None
}

fn first<'a>(value: &'a Value, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|name| value.get(*name))
}

fn first_string(value: &Value, names: &[&str]) -> Option<String> {
    first(value, names).and_then(id_string)
}

fn required_id(value: &Value, names: &[&str], label: &str) -> Result<String> {
    first_string(value, names)
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| PlatformError::invalid(format!("{label}缺失")))
}

fn beneficiary_id(record: &Value, role: &str) -> Option<String> {
    if role == "institution" {
        first_string(record, &["institution_id", "inst_id", "mcn_id", "author_id"])
    } else {
        first_string(record, &["author_id", "kol_id", "talent_id", "达人id"])
    }
}

fn id_string(value: &Value) -> Option<String> {
    match value {
        Value::String(v) => Some(v.clone()),
        Value::Number(v) => Some(v.to_string()),
        _ => None,
    }
}

fn minor(value: Option<&Value>) -> Result<Option<i64>> {
    let Some(value) = value else { return Ok(None) };
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_u64().and_then(|v| i64::try_from(v).ok()))
            .map(Some)
            .ok_or_else(|| PlatformError::invalid("抖音金额超出 i64 范围")),
        Value::String(raw) => {
            let raw = raw.trim();
            if raw.is_empty() {
                return Ok(None);
            }
            if raw.contains('.') {
                Money::from_yuan(raw)
                    .map(|v| Some(v.0))
                    .map_err(PlatformError::invalid)
            } else {
                raw.parse::<i64>()
                    .map(Some)
                    .map_err(|_| PlatformError::invalid(format!("无效抖音金额：{raw}")))
            }
        }
        _ => Err(PlatformError::invalid("抖音金额必须是整数分或十进制字符串")),
    }
}

fn optional_time(value: Option<&Value>) -> Result<Option<DateTime<Utc>>> {
    value.map(parse_time).transpose()
}

fn time(value: Option<&Value>, fallback: DateTime<Utc>) -> Result<DateTime<Utc>> {
    value.map(parse_time).transpose().map(|v| v.unwrap_or(fallback))
}

fn parse_time(value: &Value) -> Result<DateTime<Utc>> {
    match value {
        Value::Number(number) => {
            let raw = number
                .as_i64()
                .or_else(|| number.as_u64().and_then(|v| i64::try_from(v).ok()))
                .ok_or_else(|| PlatformError::invalid("抖音时间戳超出范围"))?;
            let (seconds, nanos) = if raw.abs() >= 10_000_000_000 {
                (raw / 1000, ((raw % 1000).unsigned_abs() as u32) * 1_000_000)
            } else {
                (raw, 0)
            };
            DateTime::<Utc>::from_timestamp(seconds, nanos)
                .ok_or_else(|| PlatformError::invalid("无效抖音 Unix 时间"))
        }
        Value::String(raw) => {
            if let Ok(v) = raw.parse::<i64>() {
                return parse_time(&Value::Number(v.into()));
            }
            if let Ok(v) = DateTime::parse_from_rfc3339(raw) {
                return Ok(v.with_timezone(&Utc));
            }
            let naive = NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S")
                .map_err(|_| PlatformError::invalid(format!("无效抖音时间：{raw}")))?;
            let china =
                FixedOffset::east_opt(8 * 3600).ok_or_else(|| PlatformError::invalid("无效时区"))?;
            china
                .from_local_datetime(&naive)
                .single()
                .map(|v| v.with_timezone(&Utc))
                .ok_or_else(|| PlatformError::invalid(format!("无法解释抖音时间：{raw}")))
        }
        _ => Err(PlatformError::invalid("抖音时间必须是时间戳或字符串")),
    }
}

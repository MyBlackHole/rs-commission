use crate::{
    money::Money,
    platform::{
        normalized::{CommissionObservation, NormalizedPage, OrderObservation},
        PlatformError, Result,
    },
};
use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
struct Root {
    tbk_sc_order_details_get_response: Response,
}

#[derive(Debug, Deserialize)]
struct Response {
    data: Page,
}

#[derive(Debug, Deserialize)]
struct Page {
    #[serde(default)]
    results: Results,
    #[serde(default)]
    has_next: bool,
    position_index: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Results {
    #[serde(default)]
    publisher_order_dto: Vec<OrderDto>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct OrderDto {
    trade_id: String,
    trade_parent_id: Option<String>,
    item_id: Option<String>,
    pub_id: Option<i64>,
    adzone_id: Option<i64>,
    relation_id: Option<i64>,
    tk_order_role: Option<i32>,
    tk_status: i32,
    alipay_total_price: Option<String>,
    pay_price: Option<String>,
    pub_share_pre_fee: Option<String>,
    pub_share_fee: Option<String>,
    total_commission_fee: Option<String>,
    alimama_share_fee: Option<String>,
    tk_paid_time: Option<String>,
    tb_paid_time: Option<String>,
    tk_earning_time: Option<String>,
    modified_time: String,
}

pub fn normalize_order_page(raw: &Value) -> Result<NormalizedPage> {
    let root: Root = serde_json::from_value(raw.clone()).map_err(|e| {
        PlatformError::invalid(format!("淘宝订单响应结构不符合预期：{e}"))
    })?;
    let page = root.tbk_sc_order_details_get_response.data;
    if page.has_next && page.position_index.as_deref().is_none_or(str::is_empty) {
        return Err(PlatformError::invalid(
            "淘宝响应 has_next=true 但没有 position_index",
        ));
    }

    let mut orders = Vec::with_capacity(page.results.publisher_order_dto.len());
    let mut commissions = Vec::with_capacity(page.results.publisher_order_dto.len());
    let mut max_source_updated_at = None;

    for dto in page.results.publisher_order_dto {
        let source_updated_at = parse_time(&dto.modified_time)?;
        max_source_updated_at = Some(
            max_source_updated_at
                .map_or(source_updated_at, |v: DateTime<Utc>| v.max(source_updated_at)),
        );
        let paid_at = dto
            .tk_paid_time
            .as_deref()
            .or(dto.tb_paid_time.as_deref())
            .map(parse_time)
            .transpose()?;

        let raw_status = dto.tk_status.to_string();
        let normalized_status = match dto.tk_status {
            12 => "paid",
            13 => "closed",
            14 | 3 => "completed",
            _ => "unknown",
        }
        .to_owned();

        let paid_minor = money(dto.alipay_total_price.as_deref())?;
        let settlement_base_minor = money(dto.pay_price.as_deref())?;
        orders.push(OrderObservation {
            external_parent_order_id: dto.trade_parent_id.clone(),
            external_order_line_id: dto.trade_id.clone(),
            external_product_id: dto.item_id.clone(),
            external_promoter_id: dto.pub_id.map(|v| v.to_string()),
            external_position_id: dto.adzone_id.map(|v| v.to_string()),
            merchant_ref: None,
            customer_ref: dto.relation_id.map(|v| v.to_string()),
            currency: "CNY".into(),
            paid_minor,
            settlement_base_minor,
            normalized_status,
            raw_status: raw_status.clone(),
            paid_at,
            completed_at: None,
            source_updated_at,
            normalized_payload: serde_json::to_value(&dto)
                .map_err(|e| PlatformError::invalid(e.to_string()))?,
        });

        let phase = match dto.tk_status {
            12 => "estimated",
            13 => "invalid",
            14 | 3 => "accrued",
            _ => "estimated",
        };
        let funding_phase = match dto.tk_status {
            3 => "receivable",
            13 => "reversed",
            _ => "unfunded",
        };
        let gross_minor = if dto.tk_status == 12 {
            money(dto.pub_share_pre_fee.as_deref())?
        } else {
            money(dto.pub_share_fee.as_deref())?
                .or(money(dto.total_commission_fee.as_deref())?)
        };
        let platform_service_fee_minor = money(dto.alimama_share_fee.as_deref())?;
        let net_minor = match (gross_minor, platform_service_fee_minor) {
            (Some(gross), Some(fee)) => gross.checked_sub(fee),
            (Some(gross), None) => Some(gross),
            _ => None,
        };

        commissions.push(CommissionObservation {
            external_commission_key: format!(
                "{}:publisher:{}",
                dto.trade_id,
                dto.pub_id.map_or_else(|| "unknown".into(), |v| v.to_string())
            ),
            external_order_line_id: dto.trade_id.clone(),
            external_beneficiary_id: dto.pub_id.map(|v| v.to_string()),
            beneficiary_role: match dto.tk_order_role {
                Some(2) => "publisher_first_party",
                Some(3) => "publisher_third_party",
                _ => "publisher",
            }
            .into(),
            phase: phase.into(),
            funding_phase: funding_phase.into(),
            currency: "CNY".into(),
            gross_minor,
            platform_service_fee_minor,
            special_service_fee_minor: None,
            institution_share_minor: None,
            net_minor,
            raw_status,
            source_updated_at,
            metadata: json!({
                "relation_id": dto.relation_id,
                "tk_earning_time": dto.tk_earning_time,
            }),
        });
    }

    Ok(NormalizedPage {
        orders,
        commissions,
        has_next: page.has_next,
        next_cursor: page.position_index,
        max_source_updated_at,
    })
}

fn money(value: Option<&str>) -> Result<Option<i64>> {
    value
        .map(Money::from_yuan)
        .transpose()
        .map(|v| v.map(|money| money.0))
        .map_err(Into::into)
}

fn parse_time(value: &str) -> Result<DateTime<Utc>> {
    let naive = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
        .map_err(|_| PlatformError::invalid(format!("无效淘宝时间：{value}")))?;
    let offset =
        FixedOffset::east_opt(8 * 3600).ok_or_else(|| PlatformError::invalid("无效时区"))?;
    offset
        .from_local_datetime(&naive)
        .single()
        .map(|v| v.with_timezone(&Utc))
        .ok_or_else(|| PlatformError::invalid(format!("无法解释淘宝时间：{value}")))
}

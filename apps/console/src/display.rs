use commission_types::Money;
use serde_json::Value;

pub fn label(key: &str) -> &str {
    match key {
        "paid_minor" => "实付金额",
        "refunded_minor" => "退款金额",
        "platform_net_minor" => "平台净佣金",
        "frozen_minor" => "冻结金额",
        "available_minor" => "可用余额",
        "reserved_minor" => "提现占用",
        "debt_minor" => "待追偿欠款",
        "order_count" => "订单数量",
        "pending_payouts" => "待处理提现",
        "unknown_payouts" => "结果未知提现",
        "due_orders" => "待解冻订单",
        "outbox_pending" => "待投递事件",
        "name" => "名称",
        "kind" => "类型",
        "status" => "状态",
        "external_id" => "业务编号",
        "amount_minor" => "金额",
        "original_minor" => "原始佣金",
        "net_minor" => "净佣金",
        "delta_minor" => "变动金额",
        "created_at" => "创建时间（UTC）",
        "account_id" => "账户 UUID",
        "slot" => "分配角色",
        "bucket" => "余额分区",
        "ok" => "内部对账通过",
        "external_payment_reconciled" => "外部支付已对账",
        _ => key,
    }
}
pub fn display(key: &str, value: &Value) -> String {
    if key.ends_with("_minor") {
        if let Some(cents) = value.as_str().and_then(|v| v.parse::<i64>().ok()) {
            return format!("¥ {}", Money(cents).yuan());
        }
    }
    match value {
        Value::String(v) => v.clone(),
        Value::Null => "—".into(),
        _ => value.to_string(),
    }
}

#![allow(non_snake_case)]
//! One Rust UI for Web, desktop WebView and mobile WebView renderers.
#[cfg(any(
    all(feature = "web", feature = "desktop"),
    all(feature = "web", feature = "mobile"),
    all(feature = "desktop", feature = "mobile")
))]
compile_error!("Select exactly one renderer: web, desktop or mobile");
#[cfg(not(any(feature = "web", feature = "desktop", feature = "mobile")))]
compile_error!("A renderer feature is required");

use commission_client::{ApiClient, Operation, PreparedWrite, RESOURCES};
use commission_types::{Actor, Money, QuoteInput};
use dioxus::prelude::*;
use serde_json::Value;
use uuid::Uuid;

const STYLE: Asset = asset!("/assets/console.css");
#[derive(Clone)]
struct Session {
    client: ApiClient,
    actor: Actor,
}
type Auth = Signal<Option<Session>>;

fn main() {
    dioxus::launch(App);
}
fn default_origin() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_default()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        option_env!("COMMISSION_API_ORIGIN")
            .unwrap_or("http://127.0.0.1:8081")
            .to_owned()
    }
}
#[component]
fn App() -> Element {
    let auth = use_signal(|| None::<Session>);
    use_context_provider(|| auth);
    rsx! {
        document::Stylesheet { href: STYLE }
        if auth.read().is_some() { Shell {} } else { Login {} }
    }
}
#[component]
fn Login() -> Element {
    let mut auth = use_context::<Auth>();
    let mut origin = use_signal(default_origin);
    let mut token = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(String::new);
    rsx! {
        main { class: "login-wrap",
            section { class: "login panel",
                p { class: "eyebrow", "RS COMMISSION / 0.2" }
                h1 { "分润台" }
                p { class: "muted", "Rust 跨平台抽佣与结算工作台" }
                form { onsubmit: move |event| {
                    event.prevent_default();
                    if busy() { return; }
                    let client = match ApiClient::new(&origin(), &token()) {
                        Ok(c) => c, Err(e) => { error.set(e.to_string()); return; }
                    };
                    busy.set(true); error.set(String::new());
                    spawn(async move {
                        match client.me().await {
                            Ok(actor) => { token.set(String::new()); auth.set(Some(Session { client, actor })); }
                            Err(e) => error.set(e.to_string()),
                        }
                        busy.set(false);
                    });
                },
                    label { "服务地址" }
                    input { value: origin(), disabled: busy() || cfg!(target_arch = "wasm32"),
                        oninput: move |e| origin.set(e.value()), autocomplete: "url" }
                    label { "访问令牌" }
                    input { r#type: "password", value: token(), disabled: busy(),
                        oninput: move |e| token.set(e.value()), autocomplete: "off", required: true }
                    button { r#type: "submit", disabled: busy(), if busy() { "验证身份中…" } else { "安全登录" } }
                }
                if !error().is_empty() { p { class: "error", role: "alert", "{error}" } }
                p { class: "notice", "令牌仅保存在本次运行内存中，不写入浏览器存储。非本机连接必须使用 HTTPS。" }
            }
        }
    }
}
#[component]
fn Shell() -> Element {
    let mut auth = use_context::<Auth>();
    let Some(session) = auth.read().clone() else {
        return rsx! {};
    };
    let mut view = use_signal(|| "dashboard".to_owned());
    rsx! {
        div { class: "shell",
            aside {
                div { class: "brand", h1 { "分润台" } small { "RUST · MULTIPLATFORM" } }
                nav {
                    for &(resource, title) in RESOURCES {
                        if session.actor.role != "member" || ["dashboard", "accounts", "commissions", "wallets", "payouts", "ledger"].contains(&resource) {
                            button { class: if view() == resource { "nav active" } else { "nav" },
                                onclick: move |_| view.set(resource.to_owned()), "{title}" }
                        }
                    }
                    if session.actor.role != "member" {
                        button { class: if view() == "quote" { "nav active" } else { "nav" },
                            onclick: move |_| view.set("quote".into()), "佣金试算" }
                    }
                }
                p { class: "sidebar-note", "单平台 · CNY\n结算以服务器账本为准" }
            }
            main { class: "workspace",
                header {
                    div { strong { "{session.actor.name}" } span { class: "tag", "{session.actor.role}" } }
                    button { class: "secondary", onclick: move |_| auth.set(None), "退出并清除会话" }
                }
                p { class: "notice", "外部人工转账模式：登记执行不会自动付款。未确认的写请求请勿关闭页面或退出；先记录幂等键和业务编号并核验。" }
                if view() == "quote" { QuotePanel {} }
                else { ReadPanel { key: "{view}", resource: view() } }
                Operations {}
            }
        }
    }
}
#[component]
fn ReadPanel(resource: String) -> Element {
    let mut auth = use_context::<Auth>();
    let client = auth
        .read()
        .as_ref()
        .expect("authenticated component")
        .client
        .clone();
    let title = RESOURCES
        .iter()
        .find(|(name, _)| *name == resource)
        .map_or("业务数据", |(_, title)| *title);
    let mut offset = use_signal(|| 0_u32);
    let mut result = use_resource(move || {
        let client = client.clone();
        let resource = resource.clone();
        let offset = offset();
        async move { client.resource(&resource, offset).await }
    });
    use_effect(move || {
        if let Some(Err(error)) = result.read().as_ref() {
            if error.unauthorized() {
                auth.set(None);
            }
        }
    });
    let data = result.read().clone();
    let has_more = data
        .as_ref()
        .and_then(|v| v.as_ref().ok())
        .and_then(|v| v.get("has_more"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    rsx! {
        section { class: "panel",
            div { class: "section-head", h2 { "{title}" } button { class: "secondary", onclick: move |_| result.restart(), "刷新" } }
            match data {
                None => rsx! { p { class: "muted", "正在读取服务器数据…" } },
                Some(Err(error)) => rsx! { p { class: "error", role: "alert", "{error}" } },
                Some(Ok(value)) => rsx! { DataView { value } },
            }
            div { class: "pagination",
                button { class: "secondary", disabled: offset() == 0, onclick: move |_| offset.set(offset().saturating_sub(50)), "上一页" }
                span { "偏移 {offset} · 每页 50 条" }
                button { class: "secondary", disabled: !has_more || offset() >= 1_000_000, onclick: move |_| offset.set(offset() + 50), "下一页" }
            }
        }
    }
}
fn label(key: &str) -> &str {
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
fn display(key: &str, value: &Value) -> String {
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
#[component]
fn DataView(value: Value) -> Element {
    let raw = serde_json::to_string_pretty(&value).unwrap_or_default();
    if let Some(items) = value.get("items").and_then(Value::as_array) {
        let keys: Vec<String> = items
            .first()
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        rsx! {
            if items.is_empty() { p { class: "muted", "暂无记录" } }
            else { div { class: "table-scroll", table {
                thead { tr { for key in &keys { th { "{label(key)}" } } } }
                tbody { for item in items { tr { for key in &keys { td { "{display(key, &item[key])}" } } } } }
            } } }
            details { summary { "查看原始 JSON" } pre { "{raw}" } }
        }
    } else {
        let fields: Vec<(String, String)> = value
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (label(k).to_owned(), display(k, v)))
                    .collect()
            })
            .unwrap_or_default();
        rsx! {
            div { class: "metrics", for (name, content) in fields { article { class: "metric", span { "{name}" } strong { "{content}" } } } }
            details { summary { "查看原始 JSON" } pre { "{raw}" } }
        }
    }
}
#[component]
fn QuotePanel() -> Element {
    let auth = use_context::<Auth>();
    let mut merchant = use_signal(String::new);
    let mut customer = use_signal(String::new);
    let mut paid = use_signal(|| "100.00".to_owned());
    let mut base = use_signal(|| "100.00".to_owned());
    let mut busy = use_signal(|| false);
    let mut output = use_signal(String::new);
    rsx! {
        section { class: "panel",
            h2 { "佣金试算" } p { class: "muted", "仅查询服务器规则，不入账、不冻结资金。金额输入单位：元。" }
            form { onsubmit: move |e| {
                e.prevent_default(); if busy() { return; }
                let input = (|| -> std::result::Result<QuoteInput, String> {
                    Ok(QuoteInput { merchant_id: merchant().parse::<Uuid>().map_err(|_| "商家 UUID 无效")?,
                        customer_external_id: if customer().is_empty() { None } else { Some(customer()) },
                        paid_minor: Money::from_yuan(&paid()).map_err(str::to_owned)?,
                        commission_base_minor: Money::from_yuan(&base()).map_err(str::to_owned)? })
                })();
                let input = match input { Ok(v) => v, Err(e) => { output.set(e); return; } };
                let client = auth.read().as_ref().expect("authenticated").client.clone();
                busy.set(true);
                spawn(async move {
                    output.set(match client.quote(&input).await { Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_default(), Err(e) => e.to_string() });
                    busy.set(false);
                });
            },
                div { class: "form-grid",
                    label { "商家 UUID" input { value: merchant(), oninput: move |e| merchant.set(e.value()), required: true } }
                    label { "客户业务编号（可选）" input { value: customer(), oninput: move |e| customer.set(e.value()) } }
                    label { "实付金额（元）" input { value: paid(), oninput: move |e| paid.set(e.value()), inputmode: "decimal" } }
                    label { "计佣基数（元）" input { value: base(), oninput: move |e| base.set(e.value()), inputmode: "decimal" } }
                }
                button { r#type: "submit", disabled: busy(), "向服务器试算" }
            }
            pre { "{output}" }
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum WritePhase {
    Editing,
    Prepared,
    Sending,
    Unknown,
    Succeeded,
    Rejected,
}
#[component]
fn Operations() -> Element {
    let auth = use_context::<Auth>();
    let session = auth.read().as_ref().expect("authenticated").clone();
    let allowed: Vec<Operation> = Operation::ALL
        .into_iter()
        .filter(|op| op.allowed(&session.actor))
        .collect();
    let initial = allowed.first().copied().unwrap_or(Operation::CreateAccount);
    let mut operation = use_signal(|| initial);
    let mut target = use_signal(String::new);
    let mut body = use_signal(|| initial.example().to_owned());
    let mut pending = use_signal(|| None::<PreparedWrite>);
    let mut phase = use_signal(|| WritePhase::Editing);
    let mut confirmed = use_signal(|| false);
    let mut message = use_signal(String::new);
    if allowed.is_empty() {
        return rsx! {};
    }
    let editing = phase() == WritePhase::Editing;
    let request_info = pending
        .read()
        .as_ref()
        .map(|p| format!("POST /api/v1/{}\nIdempotency-Key: {}", p.path(), p.key()))
        .unwrap_or_default();
    rsx! {
        section { class: "panel operations",
            h2 { "业务操作" }
            p { class: "muted", "当前为结构化操作台。请求按前后端共享 Rust 类型校验；金额字段必须是以分为单位的字符串。所有授权仍由后端判定。" }
            label { "操作类型" }
            select { disabled: !editing, value: operation().label(), oninput: move |e| {
                if let Some(op) = Operation::ALL.into_iter().find(|op| op.label() == e.value()) {
                    operation.set(op); body.set(op.example().to_owned()); target.set(String::new()); message.set(String::new());
                }
            }, for op in allowed { option { value: op.label(), "{op.label()}" } } }
            if operation().needs_id() {
                label { "目标记录 UUID" }
                input { disabled: !editing, value: target(), oninput: move |e| target.set(e.value()) }
            }
            label { "请求 JSON" }
            textarea { disabled: !editing, rows: 9, value: body(), oninput: move |e| body.set(e.value()), spellcheck: "false" }
            if editing {
                button { onclick: move |_| {
                    let id = if operation().needs_id() {
                        match target().parse::<Uuid>() { Ok(id) => Some(id), Err(_) => { message.set("目标 UUID 无效".into()); return; } }
                    } else { None };
                    let client = auth.read().as_ref().expect("authenticated").client.clone();
                    match client.prepare(operation(), id, &body()) {
                        Ok(write) => { pending.set(Some(write)); phase.set(WritePhase::Prepared); confirmed.set(false); message.set(String::new()); }
                        Err(e) => message.set(e.to_string()),
                    }
                }, "校验并准备请求" }
            } else {
                pre { class: "request-info", "{request_info}" }
                label { class: "confirm",
                    input { r#type: "checkbox", checked: confirmed(), disabled: phase() == WritePhase::Sending,
                        onchange: move |e| confirmed.set(e.checked()) }
                    "我已核对业务编号、金额和收款目标；此操作可能改变账本。"
                }
                if phase() == WritePhase::Unknown { p { class: "error", "结果未知。只能使用原请求核验/重试，不得更换幂等键重新申请。" } }
                div { class: "actions",
                    button { disabled: !confirmed() || !matches!(phase(), WritePhase::Prepared | WritePhase::Unknown), onclick: move |_| {
                        if !confirmed() || !matches!(phase(), WritePhase::Prepared | WritePhase::Unknown) { return; }
                        let write = pending.read().clone().expect("prepared request");
                        let client = auth.read().as_ref().expect("authenticated").client.clone();
                        phase.set(WritePhase::Sending);
                        spawn(async move {
                            match client.execute(&write).await {
                                Ok(value) => { message.set(serde_json::to_string_pretty(&value).unwrap_or_default()); phase.set(WritePhase::Succeeded); }
                                Err(error) => { phase.set(if error.outcome_unknown() { WritePhase::Unknown } else { WritePhase::Rejected }); message.set(error.to_string()); }
                            }
                        });
                    }, if phase() == WritePhase::Unknown { "以原幂等键重试" } else if phase() == WritePhase::Sending { "提交中…" } else { "确认提交" } }
                    button { class: "secondary", disabled: matches!(phase(), WritePhase::Sending | WritePhase::Unknown), onclick: move |_| {
                        if matches!(phase(), WritePhase::Sending | WritePhase::Unknown) { return; }
                        pending.set(None); phase.set(WritePhase::Editing); confirmed.set(false); message.set(String::new());
                    }, "返回编辑 / 新操作" }
                }
            }
            if !message().is_empty() { pre { role: "status", "{message}" } }
        }
    }
}

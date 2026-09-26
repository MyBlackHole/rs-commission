use crate::{app::ClientStore, display::display, platform::Write};
use commission_client::{bridge::WritePhase, Operation};
use commission_types::{Money, RefundInput};
use leptos::prelude::*;
use serde_json::Value;
use uuid::Uuid;
use wasm_bindgen_futures::spawn_local;

#[component]
pub fn OrdersPanel(
    client: ClientStore,
    phase: RwSignal<WritePhase>,
    can_refund: bool,
) -> impl IntoView {
    let offset = RwSignal::new(0_u32);
    let list = RwSignal::new(None::<Result<Value, String>>);
    let selected = RwSignal::new(None::<Uuid>);
    let detail = RwSignal::new(None::<Result<Value, String>>);
    let list_reload = RwSignal::new(0_u64);
    let detail_reload = RwSignal::new(0_u64);
    let list_generation = RwSignal::new(0_u64);
    let detail_generation = RwSignal::new(0_u64);

    Effect::new(move |_| {
        let at = offset.get();
        let _ = list_reload.get();
        let version = list_generation.get_untracked().wrapping_add(1);
        list_generation.set(version);
        list.set(None);
        let client = client.get_value();
        spawn_local(async move {
            let value = client
                .resource("orders", at)
                .await
                .map_err(|e| e.to_string());
            if list_generation.try_get_untracked() == Some(version) {
                list.set(Some(value));
            }
        });
    });

    Effect::new(move |_| {
        let order_id = selected.get();
        let _ = detail_reload.get();
        let version = detail_generation.get_untracked().wrapping_add(1);
        detail_generation.set(version);
        detail.set(None);
        let Some(order_id) = order_id else {
            return;
        };
        let client = client.get_value();
        spawn_local(async move {
            let value = client.order(order_id).await.map_err(|e| e.to_string());
            if detail_generation.try_get_untracked() == Some(version) {
                detail.set(Some(value));
            }
        });
    });

    view! {
        <section class="panel">
            <div class="section-head">
                <div>
                    <h2>"订单管理"</h2>
                    <p class="muted">"选择订单查看计佣快照、分配和退款历史。"</p>
                </div>
                <button class="secondary" on:click=move |_| list_reload.update(|v| *v = v.wrapping_add(1))>"刷新列表"</button>
            </div>
            {move || match list.get() {
                None => view! { <p class="muted">"正在读取订单…"</p> }.into_any(),
                Some(Err(error)) => view! { <p class="error" role="alert">{error}</p> }.into_any(),
                Some(Ok(value)) => order_list(value, selected),
            }}
            <div class="pagination">
                <button class="secondary" disabled=move || offset.get() == 0
                    on:click=move |_| offset.update(|v| *v = v.saturating_sub(50))>"上一页"</button>
                <span>{move || format!("偏移 {} · 每页 50 条", offset.get())}</span>
                <button class="secondary"
                    disabled=move || !list.get().and_then(|r| r.ok()).and_then(|v| v.get("has_more").and_then(Value::as_bool)).unwrap_or(false)
                    on:click=move |_| offset.update(|v| *v += 50)>"下一页"</button>
            </div>
        </section>
        {move || selected.get().map(|order_id| view! {
            <section class="panel order-detail">
                <div class="section-head">
                    <h2>"订单详情"</h2>
                    <button class="secondary" disabled=move || phase.get() != WritePhase::Editing
                        on:click=move |_| selected.set(None)>"关闭详情"</button>
                </div>
                {move || match detail.get() {
                    None => view! { <p class="muted">"正在读取订单详情…"</p> }.into_any(),
                    Some(Err(error)) => view! { <p class="error" role="alert">{error}</p> }.into_any(),
                    Some(Ok(value)) => view! {
                        <OrderDetail
                            client
                            order_id
                            value
                            phase
                            can_refund
                            on_refresh=move |_| {
                                detail_reload.update(|v| *v = v.wrapping_add(1));
                                list_reload.update(|v| *v = v.wrapping_add(1));
                            }
                        />
                    }.into_any(),
                }}
            </section>
        })}
    }
}

fn order_list(value: Value, selected: RwSignal<Option<Uuid>>) -> AnyView {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if items.is_empty() {
        return view! { <p class="muted">"暂无订单"</p> }.into_any();
    }
    let rows = items
        .into_iter()
        .map(|item| {
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .and_then(|v| v.parse::<Uuid>().ok());
            let status = if item.get("released_at").is_some_and(|v| !v.is_null()) {
                "已解冻"
            } else {
                "冻结中"
            };
            view! {
                <tr>
                    <td>{display("external_id", &item["external_id"])}</td>
                    <td>{display("paid_minor", &item["paid_minor"])}</td>
                    <td>{display("refunded_minor", &item["refunded_minor"])}</td>
                    <td>{status}</td>
                    <td>{display("captured_at", &item["captured_at"])}</td>
                    <td><button class="secondary" disabled=id.is_none()
                        on:click=move |_| if let Some(id) = id { selected.set(Some(id)); }>"查看详情"</button></td>
                </tr>
            }
        })
        .collect_view();
    view! {
        <div class="table-scroll"><table>
            <thead><tr><th>"订单号"</th><th>"实付金额"</th><th>"已退款"</th><th>"状态"</th><th>"入账时间"</th><th>"操作"</th></tr></thead>
            <tbody>{rows}</tbody>
        </table></div>
    }
    .into_any()
}

#[component]
fn OrderDetail(
    client: ClientStore,
    order_id: Uuid,
    value: Value,
    phase: RwSignal<WritePhase>,
    can_refund: bool,
    on_refresh: Callback<()>,
) -> impl IntoView {
    let order = value.get("order").cloned().unwrap_or(Value::Null);
    let allocations = value
        .get("allocations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let refunds = value
        .get("refunds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let has_refunds = !refunds.is_empty();
    let remaining = minor(&order["paid_minor"]).saturating_sub(minor(&order["refunded_minor"]));
    let cards = [
        ("订单号", display("external_id", &order["external_id"])),
        ("实付金额", display("paid_minor", &order["paid_minor"])),
        ("已退款", display("refunded_minor", &order["refunded_minor"])),
        ("剩余可退", format!("¥ {}", Money(remaining).yuan())),
        ("佣金池", display("fee_pool_minor", &order["fee_pool_minor"])),
        ("解冻时间", display("unlock_at", &order["unlock_at"])),
    ]
    .into_iter()
    .map(|(name, content)| view! {
        <article class="metric"><span>{name}</span><strong>{content}</strong></article>
    })
    .collect_view();
    let allocation_rows = allocations
        .into_iter()
        .map(|a| {
            let original = minor(&a["original_minor"]);
            let refunded = minor(&a["refunded_minor"]);
            view! { <tr>
                <td>{display("slot", &a["slot"])}</td><td>{display("account_id", &a["account_id"])}</td>
                <td>{format!("¥ {}", Money(original).yuan())}</td><td>{format!("¥ {}", Money(refunded).yuan())}</td>
                <td>{format!("¥ {}", Money(original.saturating_sub(refunded)).yuan())}</td>
            </tr> }
        })
        .collect_view();
    let refund_rows = refunds
        .into_iter()
        .map(|r| view! { <tr>
            <td>{display("external_id", &r["external_id"])}</td><td>{display("amount_minor", &r["amount_minor"])}</td>
            <td>{display("reason", &r["reason"])}</td><td>{display("created_at", &r["created_at"])}</td>
        </tr> })
        .collect_view();
    view! {
        <div class="metrics order-summary">{cards}</div>
        <h3>"分配明细"</h3>
        <div class="table-scroll"><table><thead><tr><th>"角色"</th><th>"账户"</th><th>"原始金额"</th><th>"已退回"</th><th>"净额"</th></tr></thead><tbody>{allocation_rows}</tbody></table></div>
        <h3>"退款历史"</h3>
        <div class="table-scroll"><table><thead><tr><th>"退款号"</th><th>"金额"</th><th>"原因"</th><th>"时间"</th></tr></thead>
            <tbody>
                {if has_refunds {
                    view! { {refund_rows} }.into_any()
                } else {
                    view! { <tr><td colspan="4">"暂无退款"</td></tr> }.into_any()
                }}
            </tbody>
        </table></div>
        <details><summary>"规则快照"</summary><pre>{serde_json::to_string_pretty(&order["rule_snapshot"]).unwrap_or_default()}</pre></details>
        {can_refund.then(|| view! { <RefundForm client order_id remaining phase on_refresh/> })}
    }
}

#[component]
fn RefundForm(
    client: ClientStore,
    order_id: Uuid,
    remaining: i64,
    phase: RwSignal<WritePhase>,
    on_refresh: Callback<()>,
) -> impl IntoView {
    let external_id = RwSignal::new(String::new());
    let amount = RwSignal::new(String::new());
    let reason = RwSignal::new(String::new());
    let confirmed = RwSignal::new(false);
    let pending = RwSignal::new_local(None::<Write>);
    let message = RwSignal::new(String::new());
    view! {
        <div class="refund-box">
            <h3>"登记退款并退佣"</h3>
            <p class="muted">"这里登记已经由可信上游核实的退款事实，不会主动调用支付渠道退款。"</p>
            <div class="form-grid">
                <label>"退款业务号"<input disabled=move || phase.get() != WritePhase::Editing
                    prop:value=move || external_id.get() on:input=move |e| external_id.set(event_target_value(&e)) placeholder="refund-001"/></label>
                <label>"退款金额（元）"<input disabled=move || phase.get() != WritePhase::Editing inputmode="decimal"
                    prop:value=move || amount.get() on:input=move |e| amount.set(event_target_value(&e)) placeholder="0.00"/></label>
            </div>
            <label>"退款依据 / 原因"<textarea disabled=move || phase.get() != WritePhase::Editing rows="3"
                prop:value=move || reason.get() on:input=move |e| reason.set(event_target_value(&e))></textarea></label>
            <button hidden=move || phase.get() != WritePhase::Editing on:click=move |_| {
                if phase.get_untracked() != WritePhase::Editing { return; }
                let business_id = external_id.get_untracked();
                let reason_text = reason.get_untracked();
                if business_id.trim().is_empty() || reason_text.trim().is_empty() {
                    message.set("退款业务号和退款依据不能为空".into());
                    return;
                }
                let money = match Money::from_yuan(&amount.get_untracked()) {
                    Ok(v) if v.0 > 0 && v.0 <= remaining => v,
                    Ok(_) => {
                        message.set(format!("退款金额必须大于 0 且不超过 ¥ {}", Money(remaining).yuan()));
                        return;
                    }
                    Err(e) => { message.set(e.into()); return; }
                };
                let body = match serde_json::to_string(&RefundInput {
                    external_id: business_id,
                    amount_minor: money,
                    reason: reason_text,
                }) {
                    Ok(v) => v,
                    Err(_) => { message.set("无法编码退款请求".into()); return; }
                };
                let client = client.get_value();
                phase.set(WritePhase::Preparing);
                message.set(String::new());
                spawn_local(async move {
                    match client.prepare(Operation::Refund, Some(order_id), &body).await {
                        Ok(write) => {
                            pending.set(Some(write));
                            confirmed.set(false);
                            phase.set(WritePhase::Prepared);
                        }
                        Err(e) => {
                            phase.set(WritePhase::Editing);
                            message.set(e.to_string());
                        }
                    }
                });
            }>"校验退款请求"</button>
            <div hidden=move || pending.get().is_none()>
                <pre class="request-info">{move || pending.get().map(|w| format!("POST /api/v1/{}\nIdempotency-Key: {}", w.path(), w.key())).unwrap_or_default()}</pre>
                <label class="confirm"><input type="checkbox" prop:checked=move || confirmed.get()
                    disabled=move || phase.get() == WritePhase::Sending
                    on:change=move |e| confirmed.set(event_target_checked(&e))/>
                    "我已核对退款流水、退款金额与原订单；确认写入退款事实并冲正佣金。"</label>
                <p class="error" hidden=move || phase.get() != WritePhase::Unknown>"结果未知：不得换退款号或幂等键，只能核验/重试原请求。"</p>
                <div class="actions">
                    <button hidden=move || phase.get() == WritePhase::Succeeded
                        disabled=move || !confirmed.get() || !phase.get().may_send()
                        on:click=move |_| {
                            if !confirmed.get_untracked() || !phase.get_untracked().may_send() { return; }
                            let Some(write) = pending.get_untracked() else { return; };
                            let client = client.get_value();
                            phase.set(WritePhase::Sending);
                            spawn_local(async move {
                                match client.execute(&write).await {
                                    Ok(value) => {
                                        message.set(serde_json::to_string_pretty(&value).unwrap_or_default());
                                        phase.set(WritePhase::Succeeded);
                                    }
                                    Err(e) => {
                                        phase.set(if e.outcome_unknown() { WritePhase::Unknown } else { WritePhase::Rejected });
                                        message.set(e.to_string());
                                    }
                                }
                            });
                        }>{move || if phase.get() == WritePhase::Unknown { "以原幂等键重试" } else { "确认退款" }}</button>
                    <button class="secondary" hidden=move || phase.get() == WritePhase::Succeeded
                        disabled=move || !phase.get().may_edit()
                        on:click=move |_| {
                            if !phase.get_untracked().may_edit() { return; }
                            let Some(write) = pending.get_untracked() else { return; };
                            let client = client.get_value();
                            phase.set(WritePhase::Preparing);
                            spawn_local(async move {
                                match client.discard(&write).await {
                                    Ok(()) => {
                                        pending.set(None);
                                        confirmed.set(false);
                                        phase.set(WritePhase::Editing);
                                        message.set(String::new());
                                    }
                                    Err(e) => {
                                        phase.set(WritePhase::Rejected);
                                        message.set(e.to_string());
                                    }
                                }
                            });
                        }>"返回修改"</button>
                    <button hidden=move || phase.get() != WritePhase::Succeeded on:click=move |_| {
                        let Some(write) = pending.get_untracked() else { return; };
                        let client = client.get_value();
                        spawn_local(async move {
                            match client.discard(&write).await {
                                Ok(()) => {
                                    pending.set(None);
                                    confirmed.set(false);
                                    external_id.set(String::new());
                                    amount.set(String::new());
                                    reason.set(String::new());
                                    phase.set(WritePhase::Editing);
                                    message.set(String::new());
                                    on_refresh.run(());
                                }
                                Err(e) => message.set(format!("退款已成功，但本地请求清理失败：{e}。请重新登录后继续。")),
                            }
                        });
                    }>"完成并刷新订单"</button>
                </div>
            </div>
            <pre role="status">{move || message.get()}</pre>
        </div>
    }
}

fn minor(value: &Value) -> i64 {
    value
        .as_str()
        .and_then(|v| v.parse().ok())
        .unwrap_or_default()
}

use crate::{app::ClientStore, display::display, platform::Write};
use commission_client::{bridge::WritePhase, Operation};
use commission_types::{Money, PayoutOutcome, ReasonInput, RequestPayout};
use leptos::prelude::*;
use serde_json::Value;
use uuid::Uuid;
use wasm_bindgen_futures::spawn_local;

#[component]
pub fn PayoutsPanel(
    client: ClientStore,
    phase: RwSignal<WritePhase>,
    can_request: bool,
    fixed_account: Option<Uuid>,
    can_manage: bool,
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
                .resource("payouts", at)
                .await
                .map_err(|e| e.to_string());
            if list_generation.try_get_untracked() == Some(version) {
                list.set(Some(value));
            }
        });
    });

    Effect::new(move |_| {
        let payout_id = selected.get();
        let _ = detail_reload.get();
        let version = detail_generation.get_untracked().wrapping_add(1);
        detail_generation.set(version);
        detail.set(None);
        let Some(payout_id) = payout_id else {
            return;
        };
        let client = client.get_value();
        spawn_local(async move {
            let value = client.payout(payout_id).await.map_err(|e| e.to_string());
            if detail_generation.try_get_untracked() == Some(version) {
                detail.set(Some(value));
            }
        });
    });

    view! {
        {can_request.then(|| view! {
            <PayoutRequest
                client
                phase
                fixed_account
                on_refresh=Callback::new(move |_| {
                    offset.set(0);
                    list_reload.update(|v| *v = v.wrapping_add(1));
                })
            />
        })}
        <section class="panel">
            <div class="section-head">
                <div>
                    <h2>"提现结算"</h2>
                    <p class="muted">"查看提现状态；财务操作必须沿服务器状态机推进，执行中/未知不能直接驳回。"</p>
                </div>
                <button class="secondary" disabled=move || phase.get() != WritePhase::Editing
                    on:click=move |_| list_reload.update(|v| *v = v.wrapping_add(1))>"刷新列表"</button>
            </div>
            {move || match list.get() {
                None => view! { <p class="muted">"正在读取提现记录…"</p> }.into_any(),
                Some(Err(error)) => view! { <p class="error" role="alert">{error}</p> }.into_any(),
                Some(Ok(value)) => payout_list(value, selected),
            }}
            <div class="pagination">
                <button class="secondary" disabled=move || offset.get() == 0 || phase.get() != WritePhase::Editing
                    on:click=move |_| offset.update(|v| *v = v.saturating_sub(50))>"上一页"</button>
                <span>{move || format!("偏移 {} · 每页 50 条", offset.get())}</span>
                <button class="secondary"
                    disabled=move || phase.get() != WritePhase::Editing || !list.get().and_then(|r| r.ok()).and_then(|v| v.get("has_more").and_then(Value::as_bool)).unwrap_or(false)
                    on:click=move |_| offset.update(|v| *v += 50)>"下一页"</button>
            </div>
        </section>
        {move || selected.get().map(|payout_id| view! {
            <section class="panel payout-detail">
                <div class="section-head">
                    <h2>"提现详情"</h2>
                    <button class="secondary" disabled=move || phase.get() != WritePhase::Editing
                        on:click=move |_| selected.set(None)>"关闭详情"</button>
                </div>
                {move || match detail.get() {
                    None => view! { <p class="muted">"正在读取提现详情…"</p> }.into_any(),
                    Some(Err(error)) => view! { <p class="error" role="alert">{error}</p> }.into_any(),
                    Some(Ok(value)) => view! {
                        <PayoutDetail
                            client
                            payout_id
                            value
                            phase
                            can_manage
                            on_refresh=Callback::new(move |_| {
                                detail_reload.update(|v| *v = v.wrapping_add(1));
                                list_reload.update(|v| *v = v.wrapping_add(1));
                            })
                        />
                    }.into_any(),
                }}
            </section>
        })}
    }
}

#[component]
fn PayoutRequest(
    client: ClientStore,
    phase: RwSignal<WritePhase>,
    fixed_account: Option<Uuid>,
    on_refresh: Callback<()>,
) -> impl IntoView {
    let external_id = RwSignal::new(String::new());
    let account_id = RwSignal::new(fixed_account.map(|id| id.to_string()).unwrap_or_default());
    let amount = RwSignal::new("100.00".to_owned());
    let destination_ref = RwSignal::new(String::new());
    let confirmed = RwSignal::new(false);
    let pending = RwSignal::new_local(None::<Write>);
    let message = RwSignal::new(String::new());

    view! {
        <section class="panel payout-request">
            <div class="section-head"><div>
                <h2>"申请提现"</h2>
                <p class="muted">"申请成功会立即占用可用余额；这里只登记受控收款目标引用，不填写银行卡等敏感明文。"</p>
            </div></div>
            <div class="form-grid">
                <label>"提现业务号"
                    <input disabled=move || phase.get() != WritePhase::Editing
                        prop:value=move || external_id.get()
                        on:input=move |e| external_id.set(event_target_value(&e))
                        autocomplete="off" placeholder="例如 payout-20260927-001"/>
                </label>
                <label>"受益账户 UUID"
                    <input disabled=move || phase.get() != WritePhase::Editing || fixed_account.is_some()
                        prop:value=move || account_id.get()
                        on:input=move |e| account_id.set(event_target_value(&e))
                        autocomplete="off"/>
                </label>
                <label>"提现金额（元）"
                    <input disabled=move || phase.get() != WritePhase::Editing
                        prop:value=move || amount.get()
                        on:input=move |e| amount.set(event_target_value(&e))
                        inputmode="decimal" autocomplete="off"/>
                </label>
                <label>"收款目标引用"
                    <input disabled=move || phase.get() != WritePhase::Editing
                        prop:value=move || destination_ref.get()
                        on:input=move |e| destination_ref.set(event_target_value(&e))
                        autocomplete="off" placeholder="例如 verified-payee-001"/>
                </label>
            </div>
            {fixed_account.map(|id| view! {
                <p class="notice">{format!("member 身份已锁定绑定账户：{id}")}</p>
            })}
            <button hidden=move || phase.get() != WritePhase::Editing
                on:click=move |_| {
                    if phase.get_untracked() != WritePhase::Editing { return; }
                    let external = external_id.get_untracked();
                    if external.trim().is_empty() {
                        message.set("提现业务号不能为空".into());
                        return;
                    }
                    let account = match fixed_account {
                        Some(id) => id,
                        None => match account_id.get_untracked().parse::<Uuid>() {
                            Ok(id) => id,
                            Err(_) => {
                                message.set("受益账户 UUID 无效".into());
                                return;
                            }
                        },
                    };
                    let amount_minor = match Money::from_yuan(&amount.get_untracked()) {
                        Ok(value) if value.0 > 0 => value,
                        Ok(_) => {
                            message.set("提现金额必须大于 0".into());
                            return;
                        }
                        Err(error) => {
                            message.set(error.into());
                            return;
                        }
                    };
                    let destination = destination_ref.get_untracked();
                    if destination.trim().is_empty() {
                        message.set("收款目标引用不能为空".into());
                        return;
                    }
                    let body = match serde_json::to_string(&RequestPayout {
                        external_id: external.trim().to_owned(),
                        account_id: account,
                        amount_minor,
                        destination_ref: destination.trim().to_owned(),
                    }) {
                        Ok(body) => body,
                        Err(_) => {
                            message.set("无法编码提现申请".into());
                            return;
                        }
                    };
                    message.set(String::new());
                    phase.set(WritePhase::Preparing);
                    let client = client.get_value();
                    spawn_local(async move {
                        match client.prepare(Operation::RequestPayout, None, &body).await {
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
                }>"校验提现申请"</button>
            <div hidden=move || pending.get().is_none()>
                <pre class="request-info">{move || pending.get().map(|w| format!(
                    "POST /api/v1/{}\nIdempotency-Key: {}",
                    w.path(),
                    w.key()
                )).unwrap_or_default()}</pre>
                <label class="confirm">
                    <input type="checkbox" prop:checked=move || confirmed.get()
                        disabled=move || phase.get() == WritePhase::Sending
                        on:change=move |e| confirmed.set(event_target_checked(&e))/>
                    "我已核对提现业务号、账户、金额和收款目标；确认申请后立即占用可用余额。"
                </label>
                <p class="error" hidden=move || phase.get() != WritePhase::Unknown>
                    "结果未知：不得创建新的提现申请，只能使用原幂等键继续核验/重试。"
                </p>
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
                                        phase.set(if e.outcome_unknown() {
                                            WritePhase::Unknown
                                        } else {
                                            WritePhase::Rejected
                                        });
                                        message.set(e.to_string());
                                    }
                                }
                            });
                        }>{move || if phase.get() == WritePhase::Unknown {
                            "以原幂等键重试"
                        } else {
                            "确认申请提现"
                        }}</button>
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
                    <button hidden=move || phase.get() != WritePhase::Succeeded
                        on:click=move |_| {
                            let Some(write) = pending.get_untracked() else { return; };
                            let client = client.get_value();
                            spawn_local(async move {
                                match client.discard(&write).await {
                                    Ok(()) => {
                                        pending.set(None);
                                        confirmed.set(false);
                                        external_id.set(String::new());
                                        if fixed_account.is_none() {
                                            account_id.set(String::new());
                                        }
                                        amount.set("100.00".into());
                                        destination_ref.set(String::new());
                                        message.set(String::new());
                                        phase.set(WritePhase::Editing);
                                        on_refresh.run(());
                                    }
                                    Err(e) => message.set(format!(
                                        "申请已成功，但本地请求清理失败：{e}。请重新登录后继续。"
                                    )),
                                }
                            });
                        }>"完成并刷新提现列表"</button>
                </div>
            </div>
            <pre role="status">{move || message.get()}</pre>
        </section>
    }
}

fn payout_list(value: Value, selected: RwSignal<Option<Uuid>>) -> AnyView {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if items.is_empty() {
        return view! { <p class="muted">"暂无提现记录"</p> }.into_any();
    }
    let rows = items
        .into_iter()
        .map(|item| {
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .and_then(|v| v.parse::<Uuid>().ok());
            view! {
                <tr>
                    <td>{display("external_id", &item["external_id"])}</td>
                    <td>{display("amount_minor", &item["amount_minor"])}</td>
                    <td><span class="status-chip">{status_label(&item["status"])}</span></td>
                    <td>{display("destination_ref", &item["destination_ref"])}</td>
                    <td>{display("updated_at", &item["updated_at"])}</td>
                    <td><button class="secondary" disabled=id.is_none()
                        on:click=move |_| if let Some(id) = id { selected.set(Some(id)); }>"查看详情"</button></td>
                </tr>
            }
        })
        .collect_view();
    view! {
        <div class="table-scroll"><table>
            <thead><tr><th>"提现号"</th><th>"金额"</th><th>"状态"</th><th>"收款目标"</th><th>"更新时间"</th><th>"操作"</th></tr></thead>
            <tbody>{rows}</tbody>
        </table></div>
    }
    .into_any()
}

#[component]
fn PayoutDetail(
    client: ClientStore,
    payout_id: Uuid,
    value: Value,
    phase: RwSignal<WritePhase>,
    can_manage: bool,
    on_refresh: Callback<()>,
) -> impl IntoView {
    let status = value["status"].as_str().unwrap_or_default().to_owned();
    let cards = [
        ("提现号", display("external_id", &value["external_id"])),
        ("金额", display("amount_minor", &value["amount_minor"])),
        ("状态", status_label(&value["status"]).to_owned()),
        (
            "收款目标",
            display("destination_ref", &value["destination_ref"]),
        ),
        (
            "外部流水",
            display("provider_reference", &value["provider_reference"]),
        ),
        ("更新时间", display("updated_at", &value["updated_at"])),
    ]
    .into_iter()
    .map(|(name, content)| {
        view! {
            <article class="metric"><span>{name}</span><strong>{content}</strong></article>
        }
    })
    .collect_view();
    view! {
        <div class="metrics payout-summary">{cards}</div>
        <details open>
            <summary>"审核 / 执行依据"</summary>
            <pre>{display("evidence", &value["evidence"])}</pre>
        </details>
        {if can_manage {
            match status.as_str() {
                "requested" => view! {
                    <div class="payout-actions">
                        <PayoutAction client payout_id phase operation=Operation::ApprovePayout
                            title="通过审核" note="审核凭据必须与申请凭据不同。" on_refresh/>
                        <PayoutAction client payout_id phase operation=Operation::RejectPayout
                            title="驳回提现" note="驳回会释放已占用余额。" on_refresh/>
                    </div>
                }.into_any(),
                "approved" => view! {
                    <div class="payout-actions">
                        <PayoutAction client payout_id phase operation=Operation::ProcessPayout
                            title="登记开始执行" note="只登记人工外部转账开始，不自动付款。" on_refresh/>
                        <PayoutAction client payout_id phase operation=Operation::RejectPayout
                            title="驳回提现" note="尚未执行时可驳回并释放占用。" on_refresh/>
                    </div>
                }.into_any(),
                "processing" | "unknown" => view! {
                    <div class="payout-actions">
                        <PayoutAction client payout_id phase operation=Operation::PayoutOutcome
                            title="核验外部结果" note="unknown 保留占用；成功必须填写唯一外部流水。" on_refresh/>
                    </div>
                }.into_any(),
                _ => view! { <p class="notice">"该提现已进入终态，不允许再次执行资金操作。"</p> }.into_any(),
            }
        } else {
            view! { <p class="muted">"当前身份可查看该记录，但不能执行财务审核或结果核验。"</p> }.into_any()
        }}
    }
}

#[component]
fn PayoutAction(
    client: ClientStore,
    payout_id: Uuid,
    phase: RwSignal<WritePhase>,
    operation: Operation,
    title: &'static str,
    note: &'static str,
    on_refresh: Callback<()>,
) -> impl IntoView {
    let reason = RwSignal::new(String::new());
    let outcome = RwSignal::new("unknown".to_owned());
    let reference = RwSignal::new(String::new());
    let evidence = RwSignal::new(String::new());
    let confirmed = RwSignal::new(false);
    let pending = RwSignal::new_local(None::<Write>);
    let message = RwSignal::new(String::new());
    let needs_reason = matches!(
        operation,
        Operation::ProcessPayout | Operation::RejectPayout
    );
    let needs_outcome = operation == Operation::PayoutOutcome;

    view! {
        <div class="payout-action">
            <h3>{title}</h3>
            <p class="muted">{note}</p>
            {needs_reason.then(|| view! {
                <label>"操作依据"<textarea rows="3" disabled=move || phase.get() != WritePhase::Editing
                    prop:value=move || reason.get() on:input=move |e| reason.set(event_target_value(&e))></textarea></label>
            })}
            {needs_outcome.then(|| view! {
                <label>"核验结果"
                    <select disabled=move || phase.get() != WritePhase::Editing
                        prop:value=move || outcome.get()
                        on:change=move |e| outcome.set(event_target_value(&e))>
                        <option value="unknown">"结果未知"</option>
                        <option value="succeeded">"确认成功"</option>
                        <option value="failed">"明确失败"</option>
                    </select>
                </label>
                <label>"外部流水号（成功时必填）"<input disabled=move || phase.get() != WritePhase::Editing
                    prop:value=move || reference.get() on:input=move |e| reference.set(event_target_value(&e))/></label>
                <label>"核验凭据 / 说明"<textarea rows="4" disabled=move || phase.get() != WritePhase::Editing
                    prop:value=move || evidence.get() on:input=move |e| evidence.set(event_target_value(&e))></textarea></label>
            })}
            <button hidden=move || phase.get() != WritePhase::Editing
                on:click=move |_| {
                    if phase.get_untracked() != WritePhase::Editing { return; }
                    let body = match operation {
                        Operation::ApprovePayout => "{}".to_owned(),
                        Operation::ProcessPayout | Operation::RejectPayout => {
                            let value = reason.get_untracked();
                            if value.trim().is_empty() {
                                message.set("操作依据不能为空".into());
                                return;
                            }
                            match serde_json::to_string(&ReasonInput { reason: value }) {
                                Ok(v) => v,
                                Err(_) => { message.set("无法编码请求".into()); return; }
                            }
                        }
                        Operation::PayoutOutcome => {
                            let status = outcome.get_untracked();
                            let evidence_value = evidence.get_untracked();
                            let reference_value = reference.get_untracked();
                            if evidence_value.trim().is_empty() {
                                message.set("核验凭据或说明不能为空".into());
                                return;
                            }
                            if status == "succeeded" && reference_value.trim().is_empty() {
                                message.set("确认成功必须填写唯一外部流水号".into());
                                return;
                            }
                            let provider_reference = if reference_value.trim().is_empty() { None } else { Some(reference_value) };
                            match serde_json::to_string(&PayoutOutcome {
                                status,
                                provider_reference,
                                evidence: evidence_value,
                            }) {
                                Ok(v) => v,
                                Err(_) => { message.set("无法编码核验请求".into()); return; }
                            }
                        }
                        _ => { message.set("不支持的提现操作".into()); return; }
                    };
                    phase.set(WritePhase::Preparing);
                    message.set(String::new());
                    let client = client.get_value();
                    spawn_local(async move {
                        match client.prepare(operation, Some(payout_id), &body).await {
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
                }>{format!("准备{title}")}</button>
            <div hidden=move || pending.get().is_none()>
                <pre class="request-info">{move || pending.get().map(|w| format!("POST /api/v1/{}\nIdempotency-Key: {}", w.path(), w.key())).unwrap_or_default()}</pre>
                <label class="confirm"><input type="checkbox" prop:checked=move || confirmed.get()
                    disabled=move || phase.get() == WritePhase::Sending
                    on:change=move |e| confirmed.set(event_target_checked(&e))/>
                    "我已核对提现记录、金额、收款目标与外部凭据；确认推进提现状态。"</label>
                <p class="error" hidden=move || phase.get() != WritePhase::Unknown>
                    "结果未知：不得创建替代操作，只能使用原幂等键继续核验/重试。"
                </p>
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
                        }>{move || if phase.get() == WritePhase::Unknown { "以原幂等键重试" } else { "确认提交" }}</button>
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
                    <button hidden=move || phase.get() != WritePhase::Succeeded
                        on:click=move |_| {
                            let Some(write) = pending.get_untracked() else { return; };
                            let client = client.get_value();
                            spawn_local(async move {
                                match client.discard(&write).await {
                                    Ok(()) => {
                                        pending.set(None);
                                        confirmed.set(false);
                                        phase.set(WritePhase::Editing);
                                        message.set(String::new());
                                        on_refresh.run(());
                                    }
                                    Err(e) => message.set(format!("操作已成功，但本地请求清理失败：{e}。请重新登录后继续。")),
                                }
                            });
                        }>"完成并刷新提现"</button>
                </div>
            </div>
            <pre role="status">{move || message.get()}</pre>
        </div>
    }
}

fn status_label(value: &Value) -> &'static str {
    match value.as_str().unwrap_or_default() {
        "requested" => "待审核",
        "approved" => "已审核",
        "processing" => "执行中",
        "unknown" => "结果未知",
        "succeeded" => "已成功",
        "failed" => "明确失败",
        "rejected" => "已驳回",
        _ => "未知状态",
    }
}

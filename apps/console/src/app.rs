//! Shared Leptos CSR UI: identical components for browser and Tauri WebViews.
use crate::{
    display::{display, label},
    platform::{self, Client, Write},
};
use commission_client::{bridge::WritePhase, Operation, RESOURCES};
use commission_types::{Actor, Money, QuoteInput};
use leptos::{prelude::*, reactive::owner::LocalStorage};
use serde_json::Value;
use uuid::Uuid;
use wasm_bindgen_futures::spawn_local;

#[derive(Clone)]
struct Session {
    client: Client,
    actor: Actor,
}
type Auth = RwSignal<Option<Session>, LocalStorage>;
type ClientStore = StoredValue<Client, LocalStorage>;

#[component]
pub fn App() -> impl IntoView {
    let auth: Auth = RwSignal::new_local(None);
    view! { {move || match auth.get() {
        Some(session) => view! { <Shell session auth/> }.into_any(),
        None => view! { <Login auth/> }.into_any(),
    }} }
}

#[component]
fn Login(auth: Auth) -> impl IntoView {
    let origin = RwSignal::new(platform::default_origin());
    let token = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    view! {
        <main class="login-wrap"><section class="login panel">
            <p class="eyebrow">"RS COMMISSION / 0.3"</p><h1>"分润台"</h1>
            <p class="muted">"Leptos + Tauri · Rust 跨平台抽佣工作台"</p>
            <form on:submit=move |event| {
                event.prevent_default();
                if busy.get_untracked() { return; }
                let endpoint = origin.get_untracked();
                let secret = token.get_untracked();
                busy.set(true); error.set(String::new()); token.set(String::new());
                spawn_local(async move {
                    let result = Client::login(&endpoint, &secret).await;
                    if busy.is_disposed() { return; }
                    busy.set(false);
                    match result {
                        Ok((client, actor)) => auth.set(Some(Session { client, actor })),
                        Err(e) => error.set(e.to_string()),
                    }
                });
            }>
                <label for="api-origin">"服务地址"</label>
                <input id="api-origin" prop:value=move || origin.get() disabled=move || busy.get() || !cfg!(feature = "tauri")
                    on:input=move |e| origin.set(event_target_value(&e)) autocomplete="url"/>
                <label for="access-token">"访问令牌"</label>
                <input id="access-token" type="password" prop:value=move || token.get() disabled=move || busy.get()
                    on:input=move |e| token.set(event_target_value(&e)) autocomplete="off" required/>
                <button type="submit" disabled=move || busy.get()>{move || if busy.get() { "验证身份中…" } else { "安全登录" }}</button>
            </form>
            <p class="error" role="alert">{move || error.get()}</p>
            <p class="notice">"令牌只存内存，不写入浏览器存储。非本机连接必须使用 HTTPS。"</p>
        </section></main>
    }
}

#[component]
fn Shell(session: Session, auth: Auth) -> impl IntoView {
    let client = StoredValue::new_local(session.client);
    let view = RwSignal::new("dashboard".to_owned());
    let offset = RwSignal::new(0_u32);
    let phase = RwSignal::new(WritePhase::Editing);
    let error = RwSignal::new(String::new());
    let logging_out = RwSignal::new(false);
    let is_member = session.actor.role == "member";
    let links = RESOURCES.iter().filter(|(r, _)| !is_member || ["dashboard", "accounts", "commissions", "wallets", "payouts", "ledger"].contains(r))
        .map(|&(resource, title)| view! {
            <button class=move || if view.get() == resource { "nav active" } else { "nav" }
                on:click=move |_| { offset.set(0); view.set(resource.to_owned()); }>{title}</button>
        }).collect_view();
    view! {
        <div class="shell"><aside>
            <div class="brand"><h1>"分润台"</h1><small>"LEPTOS · TAURI · RUST"</small></div>
            <nav>{links}{(!is_member).then(|| view! {
                <button class=move || if view.get() == "quote" { "nav active" } else { "nav" }
                    on:click=move |_| view.set("quote".into())>"佣金试算"</button>
            })}</nav>
            <p class="sidebar-note">"单平台 · CNY / 结算以服务器账本为准"</p>
        </aside><main class="workspace">
            <header><div><strong>{session.actor.name.clone()}</strong><span class="tag">{session.actor.role.clone()}</span></div>
                <button class="secondary" disabled=move || phase.get().unresolved() || logging_out.get()
                    on:click=move |_| {
                        if phase.get_untracked().unresolved() || logging_out.get_untracked() { return; }
                        logging_out.set(true);
                        let client = client.get_value();
                        spawn_local(async move {
                            match client.logout().await {
                                Ok(()) => { auth.set(None); }
                                Err(e) => { error.set(e.to_string()); logging_out.set(false); }
                            }
                        });
                    }>"退出并清除会话"</button>
            </header>
            <p class="error" role="alert">{move || error.get()}</p>
            <p class="notice">"外部人工转账模式：登记执行不会自动付款。结果未知时不能退出或另建请求；请保留幂等键核验，勿关闭程序。"</p>
            <Show when=move || view.get() == "quote"
                fallback=move || view! { <ReadPanel client resource=view offset/> }>
                <QuotePanel client/>
            </Show>
            <Operations client actor=session.actor phase/>
        </main></div>
    }
}

#[component]
fn ReadPanel(
    client: ClientStore,
    resource: RwSignal<String>,
    offset: RwSignal<u32>,
) -> impl IntoView {
    let result = RwSignal::new(None::<std::result::Result<Value, String>>);
    let reload = RwSignal::new(0_u64);
    let generation = RwSignal::new(0_u64);
    Effect::new(move |_| {
        let name = resource.get();
        let at = offset.get();
        let _ = reload.get();
        let version = generation.get_untracked().wrapping_add(1);
        generation.set(version);
        result.set(None);
        let client = client.get_value();
        spawn_local(async move {
            let value = client.resource(&name, at).await.map_err(|e| e.to_string());
            // An old request must never overwrite a newer page or a new session.
            if generation.try_get_untracked() == Some(version) {
                result.set(Some(value));
            }
        });
    });
    view! {
        <section class="panel">
            <div class="section-head"><h2>{move || RESOURCES.iter().find(|(r,_)| *r == resource.get()).map_or("业务数据", |(_,t)| *t)}</h2>
                <button class="secondary" on:click=move |_| reload.update(|v| *v = v.wrapping_add(1))>"刷新"</button></div>
            {move || match result.get() {
                None => view! { <p class="muted">"正在读取服务器数据…"</p> }.into_any(),
                Some(Err(error)) => view! { <p class="error" role="alert">{error}</p> }.into_any(),
                Some(Ok(value)) => view! { <DataView value/> }.into_any(),
            }}
            <div class="pagination">
                <button class="secondary" disabled=move || offset.get() == 0 on:click=move |_| offset.update(|v| *v = v.saturating_sub(50))>"上一页"</button>
                <span>{move || format!("偏移 {} · 每页 50 条", offset.get())}</span>
                <button class="secondary" disabled={move || offset.get() >= 1_000_000 || !result.get().and_then(|r| r.ok()).and_then(|v| v.get("has_more").and_then(Value::as_bool)).unwrap_or(false)}
                    on:click=move |_| offset.update(|v| *v += 50)>"下一页"</button>
            </div>
        </section>
    }
}

#[component]
fn DataView(value: Value) -> impl IntoView {
    let raw = serde_json::to_string_pretty(&value).unwrap_or_default();
    let content = if let Some(items) = value.get("items").and_then(Value::as_array) {
        let keys: Vec<String> = items
            .first()
            .and_then(Value::as_object)
            .map(|v| v.keys().cloned().collect())
            .unwrap_or_default();
        if items.is_empty() {
            view! { <p class="muted">"暂无记录"</p> }.into_any()
        } else {
            let headings = keys
                .iter()
                .map(|k| view! { <th>{label(k).to_owned()}</th> })
                .collect_view();
            let rows = items
                .iter()
                .map(|item| {
                    let cells = keys
                        .iter()
                        .map(|k| view! { <td>{display(k, &item[k])}</td> })
                        .collect_view();
                    view! { <tr>{cells}</tr> }
                })
                .collect_view();
            view! { <div class="table-scroll"><table><thead><tr>{headings}</tr></thead><tbody>{rows}</tbody></table></div> }.into_any()
        }
    } else {
        let fields = value.as_object().map(|m| m.iter().map(|(k,v)| {
            view! { <article class="metric"><span>{label(k).to_owned()}</span><strong>{display(k,v)}</strong></article> }
        }).collect_view());
        view! { <div class="metrics">{fields}</div> }.into_any()
    };
    view! { {content}<details><summary>"查看原始 JSON"</summary><pre>{raw}</pre></details> }
}

#[component]
fn QuotePanel(client: ClientStore) -> impl IntoView {
    let merchant = RwSignal::new(String::new());
    let customer = RwSignal::new(String::new());
    let paid = RwSignal::new("100.00".to_owned());
    let base = RwSignal::new("100.00".to_owned());
    let busy = RwSignal::new(false);
    let output = RwSignal::new(String::new());
    view! {
        <section class="panel"><h2>"佣金试算"</h2><p class="muted">"仅查询服务器规则，不入账。金额单位：元。"</p>
            <form on:submit=move |e| {
                e.prevent_default(); if busy.get_untracked() { return; }
                let input = (|| -> std::result::Result<QuoteInput, String> {
                    Ok(QuoteInput {
                        merchant_id: merchant.get_untracked().parse::<Uuid>().map_err(|_| "商家 UUID 无效")?,
                        customer_external_id: if customer.get_untracked().is_empty() { None } else { Some(customer.get_untracked()) },
                        paid_minor: Money::from_yuan(&paid.get_untracked()).map_err(str::to_owned)?,
                        commission_base_minor: Money::from_yuan(&base.get_untracked()).map_err(str::to_owned)?,
                    })
                })();
                let input = match input { Ok(v) => v, Err(e) => { output.set(e); return; } };
                busy.set(true); let client = client.get_value();
                spawn_local(async move {
                    let result = client.quote(&input).await;
                    if output.is_disposed() { return; }
                    output.set(match result { Ok(v) => serde_json::to_string_pretty(&v).unwrap_or_default(), Err(e) => e.to_string() });
                    busy.set(false);
                });
            }>
                <div class="form-grid">
                    <label>"商家 UUID"<input prop:value=move || merchant.get() on:input=move |e| merchant.set(event_target_value(&e)) required/></label>
                    <label>"客户业务编号（可选）"<input prop:value=move || customer.get() on:input=move |e| customer.set(event_target_value(&e))/></label>
                    <label>"实付金额（元）"<input prop:value=move || paid.get() on:input=move |e| paid.set(event_target_value(&e)) inputmode="decimal"/></label>
                    <label>"计佣基数（元）"<input prop:value=move || base.get() on:input=move |e| base.set(event_target_value(&e)) inputmode="decimal"/></label>
                </div>
                <button type="submit" disabled=move || busy.get()>"向服务器试算"</button>
            </form><pre role="status">{move || output.get()}</pre>
        </section>
    }
}

#[component]
fn Operations(client: ClientStore, actor: Actor, phase: RwSignal<WritePhase>) -> impl IntoView {
    let allowed: Vec<_> = Operation::ALL
        .into_iter()
        .filter(|op| op.allowed(&actor))
        .collect();
    let Some(first) = allowed.first().copied() else {
        return view! { <p>"当前身份无写入权限"</p> }.into_any();
    };
    let operation = RwSignal::new(first);
    let body = RwSignal::new(first.example().to_owned());
    let target = RwSignal::new(String::new());
    let message = RwSignal::new(String::new());
    let confirmed = RwSignal::new(false);
    let pending = RwSignal::new_local(None::<Write>);
    let options = allowed
        .into_iter()
        .map(|op| view! { <option value=op.label()>{op.label()}</option> })
        .collect_view();
    view! {
        <section class="panel operations"><h2>"业务操作"</h2>
            <p class="muted">"共享 Rust 类型校验。金额必须是以分为单位的字符串；所有授权由后端最终判定。"</p>
            <label>"操作类型"</label>
            <select disabled=move || phase.get() != WritePhase::Editing prop:value=move || operation.get().label()
                on:change=move |e| {
                    if let Some(op) = Operation::ALL.into_iter().find(|op| op.label() == event_target_value(&e)) {
                        operation.set(op); body.set(op.example().into()); target.set(String::new()); message.set(String::new());
                    }
                }>{options}</select>
            {move || operation.get().needs_id().then(|| view! {
                <label>"目标记录 UUID"</label><input disabled=move || phase.get() != WritePhase::Editing
                    prop:value=move || target.get() on:input=move |e| target.set(event_target_value(&e))/>
            })}
            <label>"请求 JSON"</label><textarea disabled=move || phase.get() != WritePhase::Editing rows="9" spellcheck="false"
                prop:value=move || body.get() on:input=move |e| body.set(event_target_value(&e))></textarea>
            <button hidden=move || phase.get() != WritePhase::Editing on:click=move |_| {
                if phase.get_untracked() != WritePhase::Editing { return; }
                let op = operation.get_untracked();
                let id = if op.needs_id() { match target.get_untracked().parse::<Uuid>() {
                    Ok(v) => Some(v), Err(_) => { message.set("目标 UUID 无效".into()); return; }
                }} else { None };
                let json = body.get_untracked(); let client = client.get_value();
                phase.set(WritePhase::Preparing);
                spawn_local(async move {
                    match client.prepare(op, id, &json).await {
                        Ok(write) => { pending.set(Some(write)); phase.set(WritePhase::Prepared); confirmed.set(false); message.set(String::new()); }
                        Err(e) => { phase.set(WritePhase::Editing); message.set(e.to_string()); }
                    }
                });
            }>"校验并准备请求"</button>
            <div hidden=move || pending.get().is_none()>
                <pre class="request-info">{move || pending.get().map(|w| format!("POST /api/v1/{}\nIdempotency-Key: {}", w.path(), w.key())).unwrap_or_default()}</pre>
                <label class="confirm"><input type="checkbox" prop:checked=move || confirmed.get() disabled=move || phase.get() == WritePhase::Sending
                    on:change=move |e| confirmed.set(event_target_checked(&e))/>
                    "我已核对业务编号、金额和收款目标；此操作可能改变账本。"</label>
                <p class="error" hidden=move || phase.get() != WritePhase::Unknown>"结果未知。只能使用原请求核验/重试，不得更换幂等键。"</p>
                <div class="actions"><button disabled=move || !confirmed.get() || !phase.get().may_send()
                    on:click=move |_| {
                        if !confirmed.get_untracked() || !phase.get_untracked().may_send() { return; }
                        let Some(write) = pending.get_untracked() else { return; };
                        let client = client.get_value(); phase.set(WritePhase::Sending);
                        spawn_local(async move {
                            match client.execute(&write).await {
                                Ok(value) => { message.set(serde_json::to_string_pretty(&value).unwrap_or_default()); phase.set(WritePhase::Succeeded); }
                                Err(e) => { phase.set(if e.outcome_unknown() { WritePhase::Unknown } else { WritePhase::Rejected }); message.set(e.to_string()); }
                            }
                        });
                    }>{move || match phase.get() { WritePhase::Unknown => "以原幂等键重试", WritePhase::Sending => "提交中…", _ => "确认提交" }}</button>
                    <button class="secondary" disabled=move || !phase.get().may_edit()
                        on:click=move |_| {
                            if !phase.get_untracked().may_edit() { return; }
                            let Some(write) = pending.get_untracked() else { return; };
                            let client = client.get_value(); phase.set(WritePhase::Preparing);
                            spawn_local(async move {
                                match client.discard(&write).await {
                                    Ok(()) => { pending.set(None); phase.set(WritePhase::Editing); confirmed.set(false); message.set(String::new()); }
                                    Err(e) => { phase.set(WritePhase::Unknown); message.set(e.to_string()); }
                                }
                            });
                        }>"返回编辑 / 新操作"</button>
                </div>
            </div>
            <pre role="status">{move || message.get()}</pre>
        </section>
    }.into_any()
}

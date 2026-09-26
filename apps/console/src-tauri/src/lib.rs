//! Thin OS host. No commission calculation, SQL access or arbitrary HTTP proxy.
use commission_client::{
    bridge::{NativeSessionInfo, WriteReceipt},
    native::NativeBridge,
    Operation, Result,
};
use commission_types::QuoteInput;
use serde_json::Value;
use tauri::State;
use uuid::Uuid;

#[tauri::command]
async fn session_login(
    state: State<'_, NativeBridge>,
    origin: String,
    token: String,
) -> Result<NativeSessionInfo> {
    state.login(&origin, &token).await
}
#[tauri::command]
fn session_logout(state: State<'_, NativeBridge>, session: Uuid) -> Result<()> {
    state.logout(session)
}
#[tauri::command]
async fn read_resource(
    state: State<'_, NativeBridge>,
    session: Uuid,
    resource: String,
    offset: u32,
) -> Result<Value> {
    state.resource(session, &resource, offset).await
}
#[tauri::command]
async fn quote_commission(
    state: State<'_, NativeBridge>,
    session: Uuid,
    input: QuoteInput,
) -> Result<Value> {
    state.quote(session, &input).await
}
#[tauri::command]
async fn order_detail(state: State<'_, NativeBridge>, session: Uuid, order: Uuid) -> Result<Value> {
    state.order(session, order).await
}
#[tauri::command]
async fn payout_detail(state: State<'_, NativeBridge>, session: Uuid, payout: Uuid) -> Result<Value> {
    state.payout(session, payout).await
}
#[tauri::command]
fn prepare_write(
    state: State<'_, NativeBridge>,
    session: Uuid,
    operation: Operation,
    target: Option<Uuid>,
    body: String,
) -> Result<WriteReceipt> {
    state.prepare(session, operation, target, &body)
}
#[tauri::command]
async fn execute_write(
    state: State<'_, NativeBridge>,
    session: Uuid,
    request: Uuid,
) -> Result<Value> {
    state.execute(session, request).await
}
#[tauri::command]
fn discard_write(state: State<'_, NativeBridge>, session: Uuid, request: Uuid) -> Result<()> {
    state.discard(session, request)
}
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(NativeBridge::default())
        .invoke_handler(tauri::generate_handler![
            session_login,
            session_logout,
            read_resource,
            quote_commission,
            order_detail,
            payout_detail,
            prepare_write,
            execute_write,
            discard_write
        ])
        .run(tauri::generate_context!())
        .expect("Unable to start the commission client");
}

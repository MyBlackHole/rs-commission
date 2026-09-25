use crate::{auth, error::{Error, Result}, model::*, service::{catalog, orders, outbox, payouts, queries}, AppState};
use axum::{
    extract::{DefaultBodyLimit, FromRequest, Path, Query, Request, State},
    http::{header, HeaderMap, HeaderValue},
    middleware,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tower_http::{set_header::SetResponseHeaderLayer, trace::TraceLayer};
use uuid::Uuid;

pub struct ApiJson<T>(pub T);
impl<S, T> FromRequest<S> for ApiJson<T>
where S: Send + Sync, T: DeserializeOwned + Send {
    type Rejection = Error;
    async fn from_request(req: Request, state: &S) -> Result<Self> {
        Json::<T>::from_request(req, state).await.map(|Json(v)| Self(v))
            .map_err(|_| Error::invalid("JSON 格式或字段类型不正确；金额须为以分为单位的整数字符串"))
    }
}

fn key(headers: &HeaderMap) -> Result<&str> {
    headers.get("idempotency-key").and_then(|v| v.to_str().ok())
        .ok_or_else(|| Error::invalid("写操作必须携带 Idempotency-Key 请求头"))
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/me", get(me))
        .route("/dashboard", get(dashboard))
        .route("/accounts", get(list_accounts).post(create_account))
        .route("/accounts/{id}/wallet", get(wallet))
        .route("/wallets", get(list_wallets))
        .route("/referrals", get(list_referrals).post(bind_referral))
        .route("/rules", get(list_rules).post(create_rule))
        .route("/rules/{id}/disable", post(disable_rule))
        .route("/quotes", post(quote))
        .route("/orders", get(list_orders).post(capture))
        .route("/orders/{id}", get(order))
        .route("/orders/{id}/refunds", post(refund))
        .route("/orders/{id}/release", post(release))
        .route("/commissions", get(list_commissions))
        .route("/ledger", get(list_ledger))
        .route("/payouts", get(list_payouts).post(request_payout))
        .route("/payouts/{id}/approve", post(approve_payout))
        .route("/payouts/{id}/processing", post(process_payout))
        .route("/payouts/{id}/reject", post(reject_payout))
        .route("/payouts/{id}/outcome", post(payout_outcome))
        .route("/reconciliation", get(reconciliation))
        .route("/audit", get(list_audit))
        .route("/credentials", get(list_credentials).post(create_credential))
        .route("/credentials/{id}/revoke", post(revoke_credential))
        .route("/outbox", get(list_outbox))
        .route("/outbox/claim", post(claim_outbox))
        .route("/outbox/{id}/ack", post(ack_outbox))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::authenticate));
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(javascript))
        .route("/app.css", get(stylesheet))
        .route("/health/live", get(|| async { Json(json!({"status":"alive"})) }))
        .route("/health/ready", get(ready))
        .nest("/api/v1", api)
        .fallback(|| async { Error::NotFound })
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(SetResponseHeaderLayer::overriding(header::CACHE_CONTROL, HeaderValue::from_static("no-store")))
        .layer(SetResponseHeaderLayer::overriding(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")))
        .layer(SetResponseHeaderLayer::overriding(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer")))
        .layer(SetResponseHeaderLayer::overriding(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY")))
        .layer(SetResponseHeaderLayer::overriding(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'")))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn index() -> Html<&'static str> { Html(include_str!("../../web/index.html")) }
async fn javascript() -> Response { ([(header::CONTENT_TYPE, "application/javascript; charset=utf-8")], include_str!("../../web/app.js")).into_response() }
async fn stylesheet() -> Response { ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], include_str!("../../web/app.css")).into_response() }
async fn ready(State(s): State<AppState>) -> Result<Json<Value>> {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _sqlx_migrations WHERE success").fetch_one(&s.pool).await?;
    Ok(Json(json!({"status":"ready"})))
}
async fn me(Extension(a): Extension<Actor>) -> Json<Actor> { Json(a) }
async fn dashboard(State(s): State<AppState>, Extension(a): Extension<Actor>) -> Result<Json<Value>> { Ok(Json(queries::dashboard(&s.pool,&a).await?)) }
async fn wallet(State(s): State<AppState>, Extension(a): Extension<Actor>, Path(id): Path<Uuid>) -> Result<Json<Value>> { Ok(Json(queries::wallet(&s.pool,&a,id).await?)) }
async fn order(State(s): State<AppState>, Extension(a): Extension<Actor>, Path(id): Path<Uuid>) -> Result<Json<Value>> { Ok(Json(queries::order(&s.pool,&a,id).await?)) }
async fn reconciliation(State(s): State<AppState>, Extension(a): Extension<Actor>) -> Result<Json<Value>> { Ok(Json(queries::reconcile(&s.pool,&a).await?)) }

macro_rules! list_handler {
    ($name:ident, $module:ident, $function:ident) => {
        async fn $name(State(s): State<AppState>, Extension(a): Extension<Actor>, Query(p): Query<Page>) -> Result<Json<Value>> {
            Ok(Json($module::$function(&s.pool, &a, p).await?))
        }
    };
}
list_handler!(list_accounts, queries, accounts);
list_handler!(list_wallets, queries, wallets);
list_handler!(list_referrals, queries, referrals);
list_handler!(list_rules, queries, rules);
list_handler!(list_orders, queries, orders);
list_handler!(list_commissions, queries, commissions);
list_handler!(list_ledger, queries, ledger);
list_handler!(list_payouts, queries, payouts);
list_handler!(list_audit, queries, audit);
list_handler!(list_credentials, queries, credentials);
list_handler!(list_outbox, outbox, list);

macro_rules! create_handler {
    ($name:ident, $module:ident, $function:ident, $input:ty) => {
        async fn $name(State(s): State<AppState>, Extension(a): Extension<Actor>, h: HeaderMap, ApiJson(input): ApiJson<$input>) -> Result<Json<Value>> {
            Ok(Json($module::$function(&s.pool, &a, key(&h)?, input).await?))
        }
    };
}
create_handler!(create_account, catalog, create_account, CreateAccount);
create_handler!(bind_referral, catalog, bind_referral, BindReferral);
create_handler!(create_rule, catalog, create_rule, CreateRule);
create_handler!(capture, orders, capture, CaptureOrder);
create_handler!(request_payout, payouts, request, RequestPayout);
create_handler!(create_credential, catalog, create_credential, CreateCredential);

macro_rules! action_handler {
    ($name:ident, $module:ident, $function:ident) => {
        async fn $name(State(s): State<AppState>, Extension(a): Extension<Actor>, Path(id): Path<Uuid>, h: HeaderMap, ApiJson(_): ApiJson<Empty>) -> Result<Json<Value>> {
            Ok(Json($module::$function(&s.pool, &a, key(&h)?, id).await?))
        }
    };
}
action_handler!(disable_rule, catalog, disable_rule);
action_handler!(revoke_credential, catalog, revoke_credential);
action_handler!(release, orders, release);
action_handler!(approve_payout, payouts, approve);

macro_rules! payload_action_handler {
    ($name:ident, $module:ident, $function:ident, $input:ty) => {
        async fn $name(State(s): State<AppState>, Extension(a): Extension<Actor>, Path(id): Path<Uuid>, h: HeaderMap, ApiJson(input): ApiJson<$input>) -> Result<Json<Value>> {
            Ok(Json($module::$function(&s.pool, &a, key(&h)?, id, input).await?))
        }
    };
}
payload_action_handler!(refund, orders, refund, RefundInput);
payload_action_handler!(process_payout, payouts, processing, ReasonInput);
payload_action_handler!(reject_payout, payouts, reject, ReasonInput);
payload_action_handler!(payout_outcome, payouts, outcome, PayoutOutcome);

async fn quote(State(s): State<AppState>, Extension(a): Extension<Actor>, ApiJson(input): ApiJson<QuoteInput>) -> Result<Json<Value>> {
    Ok(Json(catalog::quote(&s.pool,&a,input).await?))
}
async fn claim_outbox(State(s): State<AppState>, Extension(a): Extension<Actor>, ApiJson(input): ApiJson<ClaimEvents>) -> Result<Json<Value>> {
    Ok(Json(outbox::claim(&s.pool,&a,input).await?))
}
async fn ack_outbox(State(s): State<AppState>, Extension(a): Extension<Actor>, Path(id): Path<Uuid>, ApiJson(input): ApiJson<AckEvent>) -> Result<Json<Value>> {
    Ok(Json(outbox::ack(&s.pool,&a,id,input).await?))
}

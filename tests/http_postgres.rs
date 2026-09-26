//! Native Rust integration tests against an isolated REAL PostgreSQL database.
//! Run: DATABASE_URL=postgres://... cargo test --test http_postgres
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use commission::{auth, http, service::orders, AppState};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn call(
    app: &Router,
    token: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    key: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("/api/v1{path}"))
        .header("Authorization", format!("Bearer {token}"));
    if let Some(key) = key {
        request = request.header("Idempotency-Key", key);
    }
    let body = if let Some(body) = body {
        request = request.header("Content-Type", "application/json");
        Body::from(serde_json::to_vec(&body).unwrap())
    } else {
        Body::empty()
    };
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({"text":String::from_utf8_lossy(&bytes)}));
    (status, json)
}

async fn ok(app: &Router, token: &str, path: &str, body: Value) -> Value {
    let key = Uuid::new_v4().to_string();
    let (status, response) = call(app, token, "POST", path, Some(body), Some(&key)).await;
    assert_eq!(status, StatusCode::OK, "{path}: {response}");
    response
}
async fn get(app: &Router, token: &str, path: &str) -> Value {
    let (status, response) = call(app, token, "GET", path, None, None).await;
    assert_eq!(status, StatusCode::OK, "{path}: {response}");
    response
}
fn id(value: &Value) -> Uuid {
    Uuid::parse_str(value["id"].as_str().unwrap()).unwrap()
}

struct Fixture {
    app: Router,
    pool: PgPool,
    admin: String,
    operator: String,
    finance: String,
    merchant: Uuid,
    direct: Uuid,
    indirect: Uuid,
}
impl Fixture {
    async fn new(pool: PgPool, freeze_seconds: i64) -> Self {
        let admin = auth::bootstrap(&pool).await.unwrap()["secret"]
            .as_str()
            .unwrap()
            .to_owned();
        let app = http::router(AppState { pool: pool.clone() });
        let operator = Self::credential(&app, &admin, "operator", None).await;
        let finance = Self::credential(&app, &admin, "finance", None).await;
        let merchant = id(&ok(
            &app,
            &operator,
            "/accounts",
            json!({"external_id":"shop","name":"测试商家","kind":"merchant","parent_id":null}),
        )
        .await);
        let indirect = id(&ok(
            &app,
            &operator,
            "/accounts",
            json!({"external_id":"p2","name":"上级推广员","kind":"promoter","parent_id":null}),
        )
        .await);
        let direct = id(&ok(
            &app,
            &operator,
            "/accounts",
            json!({"external_id":"p1","name":"直接推广员","kind":"promoter","parent_id":indirect}),
        )
        .await);
        ok(
            &app,
            &operator,
            "/referrals",
            json!({"customer_external_id":"buyer-1","promoter_id":direct}),
        )
        .await;
        ok(
            &app,
            &operator,
            "/rules",
            Self::rule(None, 1000, freeze_seconds),
        )
        .await;
        Self {
            app,
            pool,
            admin,
            operator,
            finance,
            merchant,
            direct,
            indirect,
        }
    }
    async fn credential(app: &Router, admin: &str, role: &str, account: Option<Uuid>) -> String {
        let secret = auth::generate_secret();
        ok(app,admin,"/credentials",json!({"name":role,"role":role,"account_id":account,"expires_in_days":30,"secret":secret})).await;
        secret
    }
    fn rule(merchant: Option<Uuid>, rate: i32, freeze: i64) -> Value {
        json!({"name":"测试规则","merchant_id":merchant,"priority":0,"min_base_minor":"0","max_base_minor":null,
            "effective_from":null,"effective_until":null,"terms":{"rate_bps":rate,"fixed_minor":"0","cap_minor":null,
                "direct_bps":3000,"indirect_bps":1000,"freeze_seconds":freeze}})
    }
    fn order_input(&self, external: &str) -> Value {
        json!({"external_id":external,"merchant_id":self.merchant,"customer_external_id":"buyer-1",
            "currency":"CNY","paid_minor":"10000","commission_base_minor":"10000"})
    }
    async fn capture(&self, external: &str) -> Uuid {
        let response = ok(
            &self.app,
            &self.operator,
            "/orders",
            self.order_input(external),
        )
        .await;
        id(&response["order"])
    }
    async fn release(&self, id: Uuid) {
        ok(
            &self.app,
            &self.operator,
            &format!("/orders/{id}/release"),
            json!({}),
        )
        .await;
    }
    async fn wallet(&self, account: Uuid) -> Value {
        get(
            &self.app,
            &self.admin,
            &format!("/accounts/{account}/wallet"),
        )
        .await
    }
    async fn refund(&self, order: Uuid, external: &str, amount: &str) -> Value {
        ok(
            &self.app,
            &self.operator,
            &format!("/orders/{order}/refunds"),
            json!({"external_id":external,"amount_minor":amount,"reason":"已核实退款"}),
        )
        .await
    }
    async fn payout(&self, account: Uuid, external: &str, amount: &str) -> Uuid {
        id(&ok(&self.app,&self.operator,"/payouts",json!({"external_id":external,"account_id":account,"amount_minor":amount,"destination_ref":"verified-payee-1"})).await)
    }
    async fn approve(&self, payout: Uuid) {
        ok(
            &self.app,
            &self.finance,
            &format!("/payouts/{payout}/approve"),
            json!({}),
        )
        .await;
    }
    async fn process(&self, payout: Uuid) {
        ok(
            &self.app,
            &self.finance,
            &format!("/payouts/{payout}/processing"),
            json!({"reason":"开始外部人工转账"}),
        )
        .await;
    }
    async fn outcome(&self, payout: Uuid, status: &str) -> Value {
        ok(&self.app,&self.finance,&format!("/payouts/{payout}/outcome"),json!({"status":status,
            "provider_reference":if status=="succeeded" { Some(format!("bank-{payout}")) } else { None },"evidence":"已核验渠道最终结果"})).await
    }
    async fn consistent(&self) {
        let result = get(&self.app, &self.finance, "/reconciliation").await;
        assert_eq!(result["ok"], true, "{result}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn full_lifecycle_keeps_history_and_records_post_payout_debt(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    let order = f.capture("order-life").await;
    assert_eq!(f.wallet(f.merchant).await["frozen_minor"], "9000");
    assert_eq!(f.wallet(f.direct).await["frozen_minor"], "300");
    assert_eq!(f.wallet(f.indirect).await["frozen_minor"], "100");
    f.release(order).await;
    let payout = f.payout(f.direct, "pay-1", "300").await;
    f.approve(payout).await;
    f.process(payout).await;
    f.outcome(payout, "succeeded").await;
    f.refund(order, "refund-half", "5000").await;
    assert_eq!(f.wallet(f.direct).await["available_minor"], "-150");
    f.refund(order, "refund-rest", "5000").await;
    assert_eq!(f.wallet(f.direct).await["available_minor"], "-300");
    assert_eq!(f.wallet(f.direct).await["reserved_minor"], "0");
    let details = get(&f.app, &f.admin, &format!("/orders/{order}")).await;
    assert_eq!(details["order"]["paid_minor"], "10000");
    assert_eq!(details["allocations"][0]["original_minor"], "300");
    assert_eq!(details["refunds"].as_array().unwrap().len(), 2);
    f.consistent().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn partial_refund_before_release_only_thaws_remaining_funds(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    let order = f.capture("order-partial").await;
    f.refund(order, "refund-before-thaw", "2500").await;
    assert_eq!(f.wallet(f.direct).await["frozen_minor"], "225");
    f.release(order).await;
    assert_eq!(f.wallet(f.direct).await["available_minor"], "225");
    assert_eq!(f.wallet(f.direct).await["frozen_minor"], "0");
    f.consistent().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn repeated_idempotency_key_has_one_financial_effect(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    let input = f.order_input("same-order");
    let key = "same-key-12345";
    let first = call(
        &f.app,
        &f.operator,
        "POST",
        "/orders",
        Some(input.clone()),
        Some(key),
    )
    .await;
    let second = call(
        &f.app,
        &f.operator,
        "POST",
        "/orders",
        Some(input),
        Some(key),
    )
    .await;
    assert_eq!(first, second);
    assert_eq!(first.0, StatusCode::OK);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM journals WHERE kind='capture'")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    f.consistent().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_same_key_is_deduplicated(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    let mut tasks = Vec::new();
    for _ in 0..12 {
        let app = f.app.clone();
        let token = f.operator.clone();
        let input = f.order_input("concurrent-order");
        tasks.push(tokio::spawn(async move {
            call(
                &app,
                &token,
                "POST",
                "/orders",
                Some(input),
                Some("concurrent-key-1"),
            )
            .await
        }));
    }
    let mut responses = Vec::new();
    for task in tasks {
        let (status, body) = task.await.unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
        responses.push(body);
    }
    assert!(responses.iter().all(|r| r == &responses[0]));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM orders")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    f.consistent().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn key_reuse_with_different_payload_is_rejected(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    let first = call(
        &f.app,
        &f.operator,
        "POST",
        "/orders",
        Some(f.order_input("A")),
        Some("same-key-999"),
    )
    .await;
    assert_eq!(first.0, StatusCode::OK);
    let second = call(
        &f.app,
        &f.operator,
        "POST",
        "/orders",
        Some(f.order_input("B")),
        Some("same-key-999"),
    )
    .await;
    assert_eq!(second.0, StatusCode::CONFLICT);
}

#[sqlx::test(migrations = "./migrations")]
async fn duplicate_external_order_and_over_refund_are_rejected(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    let order = f.capture("unique-order").await;
    let duplicate = call(
        &f.app,
        &f.operator,
        "POST",
        "/orders",
        Some(f.order_input("unique-order")),
        Some("new-key-123456"),
    )
    .await;
    assert_eq!(duplicate.0, StatusCode::CONFLICT);
    let refund = call(
        &f.app,
        &f.operator,
        "POST",
        &format!("/orders/{order}/refunds"),
        Some(json!({"external_id":"too-much","amount_minor":"10001","reason":"test"})),
        Some("refund-too-much"),
    )
    .await;
    assert_eq!(refund.0, StatusCode::CONFLICT);
    f.consistent().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn new_rule_does_not_reprice_historical_refunds(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    let old = f.capture("old-rule").await;
    ok(
        &f.app,
        &f.operator,
        "/rules",
        Fixture::rule(Some(f.merchant), 2000, 0),
    )
    .await;
    let new = f.capture("new-rule").await;
    let old_detail = get(&f.app, &f.admin, &format!("/orders/{old}")).await;
    let new_detail = get(&f.app, &f.admin, &format!("/orders/{new}")).await;
    assert_eq!(old_detail["order"]["fee_pool_minor"], "1000");
    assert_eq!(new_detail["order"]["fee_pool_minor"], "2000");
    let refund = f.refund(old, "old-rule-refund", "10000").await;
    assert_eq!(refund["allocation_deltas"][0]["delta_minor"], "300");
    f.consistent().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn merchant_scope_overrides_high_priority_global_rule(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    let mut global = Fixture::rule(None, 5000, 0);
    global["priority"] = json!(9999);
    ok(&f.app, &f.operator, "/rules", global).await;
    ok(
        &f.app,
        &f.operator,
        "/rules",
        Fixture::rule(Some(f.merchant), 200, 0),
    )
    .await;
    let order = f.capture("merchant-specific").await;
    let result = get(&f.app, &f.admin, &format!("/orders/{order}")).await;
    assert_eq!(result["order"]["fee_pool_minor"], "200");
}

#[sqlx::test(migrations = "./migrations")]
async fn freeze_period_cannot_be_bypassed(pool: PgPool) {
    let f = Fixture::new(pool, 86400).await;
    let order = f.capture("frozen-day").await;
    let response = call(
        &f.app,
        &f.operator,
        "POST",
        &format!("/orders/{order}/release"),
        Some(json!({})),
        Some("early-release-key"),
    )
    .await;
    assert_eq!(response.0, StatusCode::CONFLICT);
    assert!(!orders::release_one_due(&f.pool).await.unwrap());
    assert_eq!(f.wallet(f.merchant).await["available_minor"], "0");
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_release_and_refund_remain_consistent(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    let order = f.capture("release-refund-race").await;
    let release_path = format!("/orders/{order}/release");
    let refund_path = format!("/orders/{order}/refunds");
    let (a, b) = tokio::join!(
        call(
            &f.app,
            &f.operator,
            "POST",
            &release_path,
            Some(json!({})),
            Some("race-release-key")
        ),
        call(
            &f.app,
            &f.operator,
            "POST",
            &refund_path,
            Some(json!({"external_id":"race-refund","amount_minor":"5000","reason":"test"})),
            Some("race-refund-key")
        )
    );
    assert_eq!(a.0, StatusCode::OK, "{}", a.1);
    assert_eq!(b.0, StatusCode::OK, "{}", b.1);
    assert_eq!(f.wallet(f.direct).await["available_minor"], "150");
    assert_eq!(f.wallet(f.direct).await["frozen_minor"], "0");
    f.consistent().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn simultaneous_payout_requests_cannot_overspend(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    let order = f.capture("overspend").await;
    f.release(order).await;
    let input = |external: &str| json!({"external_id":external,"account_id":f.direct,"amount_minor":"200","destination_ref":"payee"});
    let (a, b) = tokio::join!(
        call(
            &f.app,
            &f.operator,
            "POST",
            "/payouts",
            Some(input("payout-a")),
            Some("withdraw-a-key")
        ),
        call(
            &f.app,
            &f.operator,
            "POST",
            "/payouts",
            Some(input("payout-b")),
            Some("withdraw-b-key")
        )
    );
    assert!(
        (a.0 == StatusCode::OK && b.0 == StatusCode::CONFLICT)
            || (b.0 == StatusCode::OK && a.0 == StatusCode::CONFLICT)
    );
    assert_eq!(f.wallet(f.direct).await["reserved_minor"], "200");
    assert_eq!(f.wallet(f.direct).await["available_minor"], "100");
    f.consistent().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn unknown_result_keeps_reserved_and_cannot_be_rejected(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    let order = f.capture("unknown").await;
    f.release(order).await;
    let payout = f.payout(f.direct, "unknown-pay", "300").await;
    f.approve(payout).await;
    f.process(payout).await;
    f.outcome(payout, "unknown").await;
    assert_eq!(f.wallet(f.direct).await["reserved_minor"], "300");
    let rejected = call(
        &f.app,
        &f.finance,
        "POST",
        &format!("/payouts/{payout}/reject"),
        Some(json!({"reason":"timeout"})),
        Some("reject-unknown-key"),
    )
    .await;
    assert_eq!(rejected.0, StatusCode::CONFLICT);
    f.outcome(payout, "failed").await;
    assert_eq!(f.wallet(f.direct).await["available_minor"], "300");
    assert_eq!(f.wallet(f.direct).await["reserved_minor"], "0");
    f.consistent().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn refund_after_reservation_blocks_approval_and_release_offsets_debt(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    let order = f.capture("reserve-refund").await;
    f.release(order).await;
    let payout = f.payout(f.direct, "reserve-pay", "300").await;
    f.refund(order, "refund-after-reserve", "5000").await;
    assert_eq!(f.wallet(f.direct).await["available_minor"], "-150");
    let approve = call(
        &f.app,
        &f.finance,
        "POST",
        &format!("/payouts/{payout}/approve"),
        Some(json!({})),
        Some("debt-approve-key"),
    )
    .await;
    assert_eq!(approve.0, StatusCode::CONFLICT);
    ok(
        &f.app,
        &f.finance,
        &format!("/payouts/{payout}/reject"),
        json!({"reason":"退款后重新申请"}),
    )
    .await;
    assert_eq!(f.wallet(f.direct).await["available_minor"], "150");
    f.consistent().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn requester_cannot_approve_own_payout_even_as_admin(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    let order = f.capture("maker-checker").await;
    f.release(order).await;
    let payout=id(&ok(&f.app,&f.admin,"/payouts",json!({"external_id":"admin-pay","account_id":f.direct,"amount_minor":"100","destination_ref":"payee"})).await);
    let response = call(
        &f.app,
        &f.admin,
        "POST",
        &format!("/payouts/{payout}/approve"),
        Some(json!({})),
        Some("self-approve-key"),
    )
    .await;
    assert_eq!(response.0, StatusCode::CONFLICT);
    f.approve(payout).await;
}

#[sqlx::test(migrations = "./migrations")]
async fn members_are_scoped_and_auditors_cannot_write(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    let member = Fixture::credential(&f.app, &f.admin, "member", Some(f.direct)).await;
    let auditor = Fixture::credential(&f.app, &f.admin, "auditor", None).await;
    let wallets = get(&f.app, &member, "/wallets").await;
    assert_eq!(wallets["items"].as_array().unwrap().len(), 1);
    assert_eq!(wallets["items"][0]["account_id"], f.direct.to_string());
    assert_eq!(
        call(
            &f.app,
            &member,
            "GET",
            &format!("/accounts/{}/wallet", f.merchant),
            None,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&f.app, &member, "GET", "/orders", None, None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &f.app,
            &auditor,
            "POST",
            "/orders",
            Some(f.order_input("audit-write")),
            Some("auditor-write-key")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn integer_money_strings_and_idempotency_headers_are_required(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    let mut numeric = f.order_input("numeric");
    numeric["paid_minor"] = json!(10000);
    assert_eq!(
        call(
            &f.app,
            &f.operator,
            "POST",
            "/orders",
            Some(numeric),
            Some("numeric-test-key")
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &f.app,
            &f.operator,
            "POST",
            "/orders",
            Some(f.order_input("missing-key")),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&f.app, "bad", "GET", "/wallets", None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn ledger_is_append_only_and_unbalanced_commit_rolls_back(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    f.capture("immutable").await;
    assert!(sqlx::query("UPDATE ledger_entries SET delta_minor=1")
        .execute(&f.pool)
        .await
        .is_err());
    assert!(sqlx::query("DELETE FROM journals")
        .execute(&f.pool)
        .await
        .is_err());
    let mut tx = f.pool.begin().await.unwrap();
    let journal = Uuid::new_v4();
    sqlx::query("INSERT INTO journals(id,event_key,kind) VALUES($1,'bad-journal','capture')")
        .bind(journal)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO ledger_entries(journal_id,line_no,account_id,bucket,delta_minor) VALUES($1,0,$2,'available',1)")
        .bind(journal).bind(f.direct).execute(&mut *tx).await.unwrap();
    assert!(tx.commit().await.is_err());
    assert_eq!(f.wallet(f.direct).await["available_minor"], "0");
    f.consistent().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn outbox_claim_lease_and_stale_ack_behavior(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    let first = ok(
        &f.app,
        &f.admin,
        "/outbox/claim",
        json!({"limit":100,"lease_seconds":60}),
    )
    .await;
    assert!(!first["items"].as_array().unwrap().is_empty());
    let second = ok(
        &f.app,
        &f.admin,
        "/outbox/claim",
        json!({"limit":100,"lease_seconds":60}),
    )
    .await;
    assert!(second["items"].as_array().unwrap().is_empty());
    let event = id(&first["items"][0]);
    sqlx::query("UPDATE outbox SET lease_until=now()-INTERVAL '1 second' WHERE id=$1")
        .bind(event)
        .execute(&f.pool)
        .await
        .unwrap();
    let reclaimed = ok(
        &f.app,
        &f.admin,
        "/outbox/claim",
        json!({"limit":100,"lease_seconds":60}),
    )
    .await;
    let stale = call(
        &f.app,
        &f.admin,
        "POST",
        &format!("/outbox/{event}/ack"),
        Some(json!({"lease_token":first["lease_token"]})),
        None,
    )
    .await;
    assert_eq!(stale.0, StatusCode::CONFLICT);
    ok(
        &f.app,
        &f.admin,
        &format!("/outbox/{event}/ack"),
        json!({"lease_token":reclaimed["lease_token"]}),
    )
    .await;
    ok(
        &f.app,
        &f.admin,
        &format!("/outbox/{event}/ack"),
        json!({"lease_token":reclaimed["lease_token"]}),
    )
    .await;
}

#[sqlx::test(migrations = "./migrations")]
async fn credentials_never_store_plaintext_in_response_audit_or_outbox(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    let rows = get(&f.app, &f.admin, "/credentials").await;
    let text = rows.to_string();
    assert!(!text.contains("token_hash"));
    assert!(!text.contains(&f.admin));
    assert!(!text.contains(&f.operator));
    for secret in [&f.admin, &f.operator, &f.finance] {
        let search = format!("%{secret}%");
        let count:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM idempotency WHERE response::TEXT LIKE $1)+(SELECT count(*) FROM audit_events WHERE detail::TEXT LIKE $1)+(SELECT count(*) FROM outbox WHERE payload::TEXT LIKE $1)")
            .bind(search).fetch_one(&f.pool).await.unwrap();
        assert_eq!(count, 0);
    }
    assert!(auth::bootstrap(&f.pool).await.is_err());
}

#[sqlx::test(migrations = "./migrations")]
async fn cannot_rebind_customer_after_first_order(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    f.capture("first-order").await;
    let response = call(
        &f.app,
        &f.operator,
        "POST",
        "/referrals",
        Some(json!({"customer_external_id":"buyer-1","promoter_id":f.indirect})),
        Some("rebind-customer-key"),
    )
    .await;
    assert_eq!(response.0, StatusCode::CONFLICT);
}

#[sqlx::test(migrations = "./migrations")]
async fn repeat_worker_release_has_no_duplicate_postings(pool: PgPool) {
    let f = Fixture::new(pool, 0).await;
    f.capture("worker-order").await;
    assert!(orders::release_one_due(&f.pool).await.unwrap());
    assert!(!orders::release_one_due(&f.pool).await.unwrap());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM journals WHERE kind='release'")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    f.consistent().await;
}

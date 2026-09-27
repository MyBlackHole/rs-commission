//! Douyin alliance webhook/API connector tests without touching financial ledger.
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use commission::platform::{
    douyin::{
        DouyinAllianceSync, DouyinApiSigner, DouyinClient, DouyinCredentials,
        DouyinMessageVerifier, DouyinTransport, ORDER_METHOD,
    },
    Result,
};
use hmac::{Hmac, Mac};
use md5::{Digest as Md5Digest, Md5};
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::PgPool;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};
use uuid::Uuid;

#[derive(Clone, Default)]
struct FixtureTransport {
    pages: Arc<Mutex<VecDeque<Value>>>,
    requests: Arc<Mutex<Vec<(String, String, BTreeMap<String, String>, String)>>>,
}

impl FixtureTransport {
    fn with_pages(pages: Vec<Value>) -> Self {
        Self {
            pages: Arc::new(Mutex::new(pages.into())),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl DouyinTransport for FixtureTransport {
    async fn execute(
        &self,
        endpoint: &str,
        path: &str,
        common: &BTreeMap<String, String>,
        body: &str,
    ) -> Result<Value> {
        self.requests.lock().unwrap().push((
            endpoint.into(),
            path.into(),
            common.clone(),
            body.into(),
        ));
        self.pages
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| commission::platform::PlatformError::invalid("fixture page exhausted"))
    }
}

fn at(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn credentials() -> DouyinCredentials {
    DouyinCredentials::new("test-app", "test-secret", "test-token").unwrap()
}

fn verifier() -> DouyinMessageVerifier {
    DouyinMessageVerifier::new("test-app", "test-secret").unwrap()
}

fn sign_message(body: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(b"test-app");
    hasher.update(body);
    hasher.update(b"test-secret");
    hex::encode(hasher.finalize())
}

async fn connection(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO platform_connections
         (id,platform,external_account_id,display_name,connection_type,credential_ref)
         VALUES($1,'douyin',$2,'抖音精选联盟测试账号','oauth',$3)",
    )
    .bind(id)
    .bind(format!("douyin-{id}"))
    .bind(format!("secret://douyin/{id}"))
    .execute(pool)
    .await
    .unwrap();
    id
}

#[test]
fn douyin_api_signature_matches_official_pattern() {
    let signer = DouyinApiSigner::new(credentials());
    let params = json!({
        "start_time": 1770000000_i64,
        "end_time": 1770001200_i64,
        "page": 1,
        "page_size": 100
    });
    let (common, body) = signer
        .signed_params(ORDER_METHOD, &params, at("2026-09-27T08:00:00Z"))
        .unwrap();
    assert_eq!(
        body,
        r#"{"end_time":1770001200,"page":1,"page_size":100,"start_time":1770000000}"#
    );
    assert_eq!(common["method"], "alliance.getOrderList");
    assert_eq!(common["timestamp"], "2026-09-27 16:00:00");
    assert_eq!(common["sign_method"], "hmac-sha256");
    assert_eq!(
        common["sign"],
        "3db372b7587b71e2bbe4d51aab671e551e2a133944ce6cda5aac8b2bf120eccf"
    );
    assert!(!common.values().any(|value| value == "test-secret"));

    // An independent recomputation guards against accidentally signing access_token/sign_method.
    let pattern = format!(
        "test-secretapp_keytest-appmethodalliance.getOrderListparam_json{}timestamp2026-09-27 16:00:00v2test-secret",
        body
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(b"test-secret").unwrap();
    mac.update(pattern.as_bytes());
    assert_eq!(hex::encode(mac.finalize().into_bytes()), common["sign"]);
}

#[sqlx::test(migrations = "./migrations")]
async fn webhook_is_verified_idempotent_and_out_of_order_safe(pool: PgPool) {
    let connection_id = connection(&pool).await;
    let client = DouyinClient::new(
        DouyinApiSigner::new(credentials()),
        FixtureTransport::default(),
    );
    let sync = DouyinAllianceSync::new(pool.clone(), connection_id, client);
    let verifier = verifier();

    let settle = serde_json::to_vec(&json!([{
        "tag": "806",
        "msg_id": "settle-1",
        "data": {
            "order_id": "DOU-ORDER-1",
            "author_id": "KOL-1",
            "pay_amount": 10000,
            "settle_amount": 10000,
            "settle_commission": 1000,
            "tech_service_fee": 100,
            "net_commission": 900,
            "status": "SETTLED",
            "pay_time": 1790495700_i64,
            "settle_time": 1790496000_i64,
            "update_time": 1790496000_i64
        }
    }]))
    .unwrap();
    let settled = sync
        .ingest_webhook(
            &verifier,
            "test-app",
            &sign_message(&settle),
            &settle,
            at("2026-09-27T08:01:00Z"),
        )
        .await
        .unwrap();
    assert_eq!(settled.raw_events, 1);
    assert_eq!(settled.settlement_observations, 1);

    let pay_older = serde_json::to_vec(&json!([{
        "tag": "804",
        "msg_id": "pay-1",
        "data": {
            "order_id": "DOU-ORDER-1",
            "author_id": "KOL-1",
            "pay_amount": 10000,
            "estimated_commission": 1000,
            "tech_service_fee": 100,
            "status": "PAID",
            "pay_time": 1790495700_i64,
            "update_time": 1790495700_i64
        }
    }]))
    .unwrap();
    sync.ingest_webhook(
        &verifier,
        "test-app",
        &sign_message(&pay_older),
        &pay_older,
        at("2026-09-27T08:02:00Z"),
    )
    .await
    .unwrap();

    let current: (String, String, Option<i64>) = sqlx::query_as(
        "SELECT phase,funding_phase,net_minor
         FROM external_commissions
         WHERE connection_id=$1
           AND external_commission_key='DOU-ORDER-1:kol:KOL-1'",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(current, ("settled".into(), "receivable".into(), Some(900)));

    let duplicate = sync
        .ingest_webhook(
            &verifier,
            "test-app",
            &sign_message(&pay_older),
            &pay_older,
            at("2026-09-27T08:03:00Z"),
        )
        .await
        .unwrap();
    assert_eq!(duplicate.duplicates, 1);

    let changed = serde_json::to_vec(&json!([{
        "tag": "804",
        "msg_id": "pay-1",
        "data": {"order_id":"DOU-ORDER-CHANGED","author_id":"KOL-1","update_time":1790495701_i64}
    }]))
    .unwrap();
    assert!(sync
        .ingest_webhook(
            &verifier,
            "test-app",
            &sign_message(&changed),
            &changed,
            at("2026-09-27T08:04:00Z"),
        )
        .await
        .is_err());

    let raw_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM platform_raw_events WHERE connection_id=$1")
            .bind(connection_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(raw_count, 2);

    let ledger_count: i64 = sqlx::query_scalar("SELECT count(*) FROM ledger_entries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(ledger_count, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn refund_and_handshake_are_persisted_without_financial_posting(pool: PgPool) {
    let connection_id = connection(&pool).await;
    let client = DouyinClient::new(
        DouyinApiSigner::new(credentials()),
        FixtureTransport::default(),
    );
    let sync = DouyinAllianceSync::new(pool.clone(), connection_id, client);
    let verifier = verifier();

    let handshake = br#"[{"tag":"0","msg_id":"0","data":"2026-09-27T16:00:00+08:00"}]"#;
    let result = sync
        .ingest_webhook(
            &verifier,
            "test-app",
            &sign_message(handshake),
            handshake,
            at("2026-09-27T08:00:00Z"),
        )
        .await
        .unwrap();
    assert_eq!(result.raw_events, 1);
    assert_eq!(result.order_observations, 0);

    let refund = serde_json::to_vec(&json!([{
        "tag": 805,
        "msg_id": 9001,
        "data": {
            "order_id":"DOU-REFUND-1",
            "author_id":"KOL-9",
            "refund_id":"RF-1",
            "refund_status":"SUCCESS",
            "refund_amount":5000,
            "commission_refund_amount":500,
            "commission_amount":1000,
            "update_time":1790496300_i64,
            "refund_time":1790496300_i64
        }
    }]))
    .unwrap();
    let outcome = sync
        .ingest_webhook(
            &verifier,
            "test-app",
            &sign_message(&refund),
            &refund,
            at("2026-09-27T08:05:00Z"),
        )
        .await
        .unwrap();
    assert_eq!(outcome.refund_observations, 1);

    let refund_row: (Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT refund_minor,commission_reversal_minor
         FROM external_refund_observations
         WHERE connection_id=$1 AND external_refund_id='RF-1'",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(refund_row, (Some(5000), Some(500)));

    let phase: String = sqlx::query_scalar(
        "SELECT phase FROM external_commissions
         WHERE connection_id=$1 AND external_commission_key='DOU-REFUND-1:kol:KOL-9'",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(phase, "reversed");

    let ledger_count: i64 = sqlx::query_scalar("SELECT count(*) FROM ledger_entries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(ledger_count, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn api_reconciliation_uses_same_projection_and_checkpoint(pool: PgPool) {
    let connection_id = connection(&pool).await;
    let response = json!({
        "code": 10000,
        "msg": "success",
        "data": {
            "next_cursor": "cursor-2",
            "order_list": [{
                "order_id": "DOU-API-1",
                "author_id": "KOL-API",
                "product_id": "P-1",
                "pid": "PID-1",
                "pay_amount": 20000,
                "settle_amount": 18000,
                "commission_amount": 2000,
                "tech_service_fee": 200,
                "net_commission": 1800,
                "status": "SETTLED",
                "pay_time": 1790495700_i64,
                "settle_time": 1790496000_i64,
                "update_time": 1790496000_i64
            }]
        }
    });
    let transport = FixtureTransport::with_pages(vec![response]);
    let client = DouyinClient::new(DouyinApiSigner::new(credentials()), transport.clone());
    let sync = DouyinAllianceSync::new(pool.clone(), connection_id, client);

    let params = json!({"start_time":1790494800_i64,"end_time":1790496600_i64,"page_size":100});
    let outcome = sync
        .reconcile_page(&params, at("2026-09-27T08:10:00Z"))
        .await
        .unwrap();
    assert_eq!(outcome.next_cursor.as_deref(), Some("cursor-2"));
    assert_eq!(outcome.order_observations, 1);
    assert_eq!(outcome.commission_observations, 1);
    assert_eq!(outcome.settlement_observations, 1);

    let request = transport.requests.lock().unwrap()[0].clone();
    assert_eq!(request.1, "/alliance/getOrderList");
    assert_eq!(request.2["method"], "alliance.getOrderList");
    assert_eq!(request.2["sign_method"], "hmac-sha256");
    assert_eq!(
        request.3,
        r#"{"end_time":1790496600,"page_size":100,"start_time":1790494800}"#
    );

    let checkpoint: (Option<String>, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT cursor,last_platform_updated_at
         FROM platform_sync_checkpoints
         WHERE connection_id=$1 AND stream='douyin_alliance_reconcile'",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(checkpoint.0.as_deref(), Some("cursor-2"));
    assert_eq!(
        checkpoint.1,
        Some(DateTime::from_timestamp(1790496000, 0).unwrap())
    );

    let ledger_count: i64 = sqlx::query_scalar("SELECT count(*) FROM ledger_entries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(ledger_count, 0);
}

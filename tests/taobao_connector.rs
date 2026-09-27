//! Taobao connector contract and PostgreSQL synchronization tests.
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use commission::platform::{
    taobao::{
        TaobaoClient, TaobaoCredentials, TaobaoOrderQuery, TaobaoOrderSync, TaobaoSigner,
        TaobaoTransport,
    },
    Result,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};
use uuid::Uuid;

#[derive(Clone, Default)]
struct FixtureTransport {
    pages: Arc<Mutex<VecDeque<Value>>>,
    requests: Arc<Mutex<Vec<BTreeMap<String, String>>>>,
}

impl FixtureTransport {
    fn with_pages(pages: Vec<Value>) -> Self {
        Self {
            pages: Arc::new(Mutex::new(pages.into())),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn requests(&self) -> Vec<BTreeMap<String, String>> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl TaobaoTransport for FixtureTransport {
    async fn execute(&self, _endpoint: &str, params: &BTreeMap<String, String>) -> Result<Value> {
        self.requests.lock().unwrap().push(params.clone());
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

fn signer() -> TaobaoSigner {
    TaobaoSigner::new(TaobaoCredentials::new("test-app", "test-secret", "test-session").unwrap())
}

#[test]
fn taobao_signing_uses_stable_ascii_order_and_hmac_sha256() {
    let query = TaobaoOrderQuery {
        start: at("2026-09-27T07:40:00Z"),
        end: at("2026-09-27T08:00:00Z"),
        position_index: None,
    };
    let params = signer()
        .signed_order_params(&query, at("2026-09-27T08:00:00Z"))
        .unwrap();

    assert_eq!(params["method"], "taobao.tbk.sc.order.details.get");
    assert_eq!(params["query_type"], "4");
    assert_eq!(params["page_size"], "100");
    assert_eq!(params["start_time"], "2026-09-27 15:40:00");
    assert_eq!(params["end_time"], "2026-09-27 16:00:00");
    assert_eq!(params["timestamp"], "2026-09-27 16:00:00");
    assert_eq!(params["sign_method"], "hmac-sha256");
    assert_eq!(
        params["sign"],
        "EBF2FD2D462AE7B3E14503FBDF915639C114A528A2394A58E693F4191D667F54"
    );
    assert!(!params.values().any(|value| value == "test-secret"));
}

fn page(
    status: i32,
    modified_time: &str,
    has_next: bool,
    cursor: Option<&str>,
    pre_fee: &str,
    settle_fee: &str,
) -> Value {
    json!({
        "tbk_sc_order_details_get_response": {
            "data": {
                "results": {
                    "publisher_order_dto": [{
                        "trade_id": "TB-SUB-001",
                        "trade_parent_id": "TB-PARENT-001",
                        "item_id": "590141576510",
                        "pub_id": 98836808,
                        "adzone_id": 11,
                        "relation_id": 2323,
                        "tk_order_role": 2,
                        "tk_status": status,
                        "alipay_total_price": "100.00",
                        "pay_price": "80.00",
                        "pub_share_pre_fee": pre_fee,
                        "pub_share_fee": settle_fee,
                        "total_commission_fee": settle_fee,
                        "alimama_share_fee": "1.00",
                        "tk_paid_time": "2026-09-27 15:45:00",
                        "tb_paid_time": "2026-09-27 15:45:00",
                        "tk_earning_time": if status == 3 { Some("2026-09-27 15:58:00") } else { None },
                        "modified_time": modified_time
                    }]
                },
                "has_next": has_next,
                "position_index": cursor,
                "page_no": 1,
                "page_size": 100
            }
        }
    })
}

#[sqlx::test(migrations = "./migrations")]
async fn taobao_sync_resumes_cursor_projects_latest_state_and_never_posts_ledger(pool: PgPool) {
    let connection_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO platform_connections
         (id,platform,external_account_id,display_name,connection_type,credential_ref)
         VALUES($1,'taobao','publisher-98836808','淘宝测试推广账号','oauth','secret://taobao/test')",
    )
    .bind(connection_id)
    .execute(&pool)
    .await
    .unwrap();

    let transport = FixtureTransport::with_pages(vec![
        page(
            12,
            "2026-09-27 15:50:00",
            true,
            Some("cursor-2"),
            "10.00",
            "0",
        ),
        page(3, "2026-09-27 15:55:00", false, None, "10.00", "9.00"),
    ]);
    let client = TaobaoClient::new(signer(), transport.clone());
    let sync = TaobaoOrderSync::new(pool.clone(), connection_id, client);
    let now = at("2026-09-27T08:00:00Z");

    let first = sync.sync_next(now).await.unwrap();
    assert!(first.has_next);
    assert_eq!(first.next_cursor.as_deref(), Some("cursor-2"));
    assert_eq!(first.order_observations, 1);
    assert_eq!(first.commission_observations, 1);

    let first_checkpoint: (Option<String>, DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
        "SELECT cursor,window_start,window_end
         FROM platform_sync_checkpoints
         WHERE connection_id=$1 AND stream='taobao_order_updated'",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(first_checkpoint.0.as_deref(), Some("cursor-2"));

    let second = sync.sync_next(now).await.unwrap();
    assert!(!second.has_next);
    assert_eq!(second.next_cursor, None);
    assert_eq!(second.window_start, first.window_start);
    assert_eq!(second.window_end, first.window_end);

    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert!(!requests[0].contains_key("position_index"));
    assert_eq!(requests[1]["position_index"], "cursor-2");
    assert_eq!(requests[0]["start_time"], requests[1]["start_time"]);
    assert_eq!(requests[0]["end_time"], requests[1]["end_time"]);

    let raw_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM platform_raw_events WHERE connection_id=$1")
            .bind(connection_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(raw_count, 2);

    let order_observations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM external_order_observations WHERE connection_id=$1",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(order_observations, 2);

    let order: (String, String, Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT normalized_status,raw_status,paid_minor,settlement_base_minor
         FROM external_orders
         WHERE connection_id=$1 AND external_order_line_id='TB-SUB-001'",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        order,
        ("completed".into(), "3".into(), Some(10_000), Some(8_000))
    );

    let commission: (String, String, Option<i64>, Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT phase,funding_phase,gross_minor,platform_service_fee_minor,net_minor
             FROM external_commissions
             WHERE connection_id=$1
               AND external_commission_key='TB-SUB-001:publisher:98836808'",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        commission,
        (
            "accrued".into(),
            "receivable".into(),
            Some(900),
            Some(100),
            Some(800)
        )
    );

    let checkpoint_cursor: Option<String> = sqlx::query_scalar(
        "SELECT cursor FROM platform_sync_checkpoints
         WHERE connection_id=$1 AND stream='taobao_order_updated'",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(checkpoint_cursor, None);

    let ledger_count: i64 = sqlx::query_scalar("SELECT count(*) FROM ledger_entries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(ledger_count, 0);
}

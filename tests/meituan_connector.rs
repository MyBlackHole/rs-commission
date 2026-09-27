use async_trait::async_trait;
use chrono::{DateTime, Utc};
use commission::platform::{
    meituan::{
        MeituanClient, MeituanCredentials, MeituanOrderSync, MeituanSigner, MeituanTransport,
    },
    PlatformError, Result,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};
use uuid::Uuid;

type RecordedRequest = (String, String, BTreeMap<String, String>, String);

#[derive(Clone, Default)]
struct FixtureTransport {
    pages: Arc<Mutex<VecDeque<Value>>>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl FixtureTransport {
    fn new(pages: Vec<Value>) -> Self {
        Self {
            pages: Arc::new(Mutex::new(pages.into())),
            requests: Arc::default(),
        }
    }

    fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl MeituanTransport for FixtureTransport {
    async fn execute(
        &self,
        endpoint: &str,
        path: &str,
        headers: &BTreeMap<String, String>,
        body: &str,
    ) -> Result<Value> {
        self.requests.lock().unwrap().push((
            endpoint.into(),
            path.into(),
            headers.clone(),
            body.into(),
        ));
        self.pages
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| PlatformError::Transport("测试响应已耗尽".into()))
    }
}

#[derive(Clone)]
struct CheckpointAdvancingTransport {
    pool: PgPool,
    connection_id: Uuid,
    response: Value,
}

#[async_trait]
impl MeituanTransport for CheckpointAdvancingTransport {
    async fn execute(
        &self,
        _endpoint: &str,
        _path: &str,
        _headers: &BTreeMap<String, String>,
        _body: &str,
    ) -> Result<Value> {
        sqlx::query(
            "INSERT INTO platform_sync_checkpoints
             (connection_id,stream,cursor,window_start,window_end,last_platform_updated_at,
              last_attempt_at,last_success_at)
             VALUES($1,'meituan_union_order_updated','other-worker',
                    '2026-09-27T07:30:00Z','2026-09-27T08:00:00Z',
                    '2026-09-27T07:55:00Z',now(),now())
             ON CONFLICT(connection_id,stream) DO UPDATE SET
                cursor=EXCLUDED.cursor,
                window_start=EXCLUDED.window_start,
                window_end=EXCLUDED.window_end,
                last_platform_updated_at=EXCLUDED.last_platform_updated_at,
                last_attempt_at=EXCLUDED.last_attempt_at,
                last_success_at=EXCLUDED.last_success_at,
                updated_at=now()",
        )
        .bind(self.connection_id)
        .execute(&self.pool)
        .await?;
        Ok(self.response.clone())
    }
}

fn at(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn signer() -> MeituanSigner {
    MeituanSigner::new(MeituanCredentials::new("test-app", "test-secret").unwrap())
}

async fn connection(pool: &PgPool, external: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO platform_connections
         (id,platform,external_account_id,display_name,connection_type,credential_ref)
         VALUES($1,'meituan',$2,'美团联盟测试','app_credentials',$3)",
    )
    .bind(id)
    .bind(external)
    .bind(format!("secret://meituan/{id}"))
    .execute(pool)
    .await
    .unwrap();
    id
}

#[test]
fn meituan_signature_vector_is_stable() {
    let body = r#"{"endTime":1790496000,"limit":100,"page":1,"queryTimeType":2,"searchType":2,"startTime":1790494200}"#;
    let headers = signer().signed_headers(body, 1_790_496_000_000).unwrap();

    assert_eq!(
        headers.get("Content-MD5").map(String::as_str),
        Some("fZoSpUUECIApXyXOe+dCoA==")
    );
    assert_eq!(
        headers.get("S-Ca-Signature").map(String::as_str),
        Some("6j9AV9DjVmflCYG7aSJnouG6Ts6vXGUr8uqZvdFfl08=")
    );
    assert_eq!(
        headers.get("S-Ca-Signature-Headers").map(String::as_str),
        Some("S-Ca-App,S-Ca-Timestamp")
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn meituan_scroll_sync_uses_child_facts_and_never_posts_ledger(pool: PgPool) {
    let connection_id = connection(&pool, "publisher-main").await;
    let first_page = json!({
        "code": 0,
        "message": "成功",
        "data": {
            "scrollId": "cursor-2",
            "dataList": [{
                "businessLine": 1,
                "orderId": "MT-PARENT-1",
                "payTime": 1790494800_i64,
                "payPrice": "100.00",
                "updateTime": 1790495400_i64,
                "commissionRate": "300",
                "profit": "3.00",
                "cpaProfit": "0",
                "sid": "channel-a",
                "productId": "sku-a",
                "productName": "测试商品",
                "orderDetail": null,
                "refundPrice": null,
                "refundTime": "null",
                "refundProfit": "null",
                "cpaRefundProfit": "0",
                "status": "2",
                "tradeType": 1,
                "appkey": "test-app"
            }]
        }
    });
    let second_page = json!({
        "code": 0,
        "message": "成功",
        "data": {
            "scrollId": null,
            "dataList": [{
                "businessLine": 2,
                "orderId": "MT-PARENT-DETAIL",
                "payTime": 1790494800_i64,
                "payPrice": "110.00",
                "updateTime": 1790495900_i64,
                "commissionRate": "300",
                "profit": "3.00",
                "cpaProfit": "0",
                "sid": "channel-detail",
                "productViewSign": "product-sign",
                "productName": "混合子订单",
                "orderDetail": [{
                    "couponStatus": "1",
                    "itemOrderId": "MT-ITEM-1",
                    "finishTime": null,
                    "basicAmount": "40.00",
                    "couponFee": "1.20",
                    "refundAmount": null,
                    "refundFee": null,
                    "refundTime": null,
                    "settleTime": null,
                    "updateTime": "1790495700"
                }, {
                    "couponStatus": "3",
                    "itemOrderId": "MT-ITEM-2",
                    "finishTime": "1790495750",
                    "basicAmount": "60.00",
                    "couponFee": "1.80",
                    "refundAmount": null,
                    "refundFee": null,
                    "refundTime": null,
                    "settleTime": "1790495800",
                    "updateTime": "1790495800"
                }, {
                    "couponStatus": "4",
                    "itemOrderId": "MT-ITEM-3",
                    "finishTime": null,
                    "basicAmount": "10.00",
                    "couponFee": "0.00",
                    "refundAmount": "10.00",
                    "refundFee": "0.30",
                    "refundTime": "1790495850",
                    "settleTime": null,
                    "updateTime": "1790495900"
                }],
                "refundPrice": "10.00",
                "refundTime": "1790495850",
                "refundProfit": "0.30",
                "cpaRefundProfit": "0",
                "status": "6",
                "tradeType": 1,
                "appkey": "test-app"
            }]
        }
    });

    let transport = FixtureTransport::new(vec![first_page, second_page]);
    let sync = MeituanOrderSync::new(
        pool.clone(),
        connection_id,
        MeituanClient::new(signer(), transport.clone()),
    );
    let now = at("2026-09-27T08:00:00Z");

    let first = sync.sync_next(now).await.unwrap();
    assert_eq!(first.next_cursor.as_deref(), Some("cursor-2"));
    assert_eq!(first.order_observations, 1);
    assert_eq!(first.commission_observations, 1);

    let second = sync.sync_next(now).await.unwrap();
    assert_eq!(second.next_cursor, None);
    assert_eq!(second.order_observations, 3);
    assert_eq!(second.commission_observations, 3);
    assert_eq!(second.refund_observations, 1);
    assert_eq!(second.settlement_observations, 1);
    assert_eq!(second.window_start, first.window_start);
    assert_eq!(second.window_end, first.window_end);

    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].1, "/cps_open/common/api/v1/query_order");
    let first_body: Value = serde_json::from_str(&requests[0].3).unwrap();
    let second_body: Value = serde_json::from_str(&requests[1].3).unwrap();
    assert_eq!(first_body["queryTimeType"], 2);
    assert_eq!(first_body["searchType"], 2);
    assert_eq!(first_body["page"], 1);
    assert_eq!(first_body["limit"], 100);
    assert!(first_body.get("scrollId").is_none());
    assert_eq!(second_body["scrollId"], "cursor-2");
    assert_eq!(
        requests[0].2.get("S-Ca-App").map(String::as_str),
        Some("test-app")
    );
    assert!(requests[0].2.contains_key("Content-MD5"));
    assert!(requests[0].2.contains_key("S-Ca-Signature"));

    let counts: (i64, i64, i64, i64) = (
        sqlx::query_scalar("SELECT count(*) FROM external_order_observations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        sqlx::query_scalar("SELECT count(*) FROM external_commission_observations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        sqlx::query_scalar("SELECT count(*) FROM external_refund_observations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        sqlx::query_scalar("SELECT count(*) FROM external_settlement_observations")
            .fetch_one(&pool)
            .await
            .unwrap(),
    );
    assert_eq!(counts, (4, 4, 1, 1));

    let settled: (String, String, Option<i64>) = sqlx::query_as(
        "SELECT phase,funding_phase,gross_minor
         FROM external_commissions
         WHERE connection_id=$1
           AND external_commission_key='MT-ITEM-2:publisher:test-app'",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(settled, ("settled".into(), "receivable".into(), Some(180)));

    let refunded_status: String = sqlx::query_scalar(
        "SELECT normalized_status FROM external_orders
         WHERE connection_id=$1 AND external_order_line_id='MT-ITEM-3'",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(refunded_status, "refunded");

    let refund: (Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT refund_minor,commission_reversal_minor
         FROM external_refund_observations
         WHERE connection_id=$1 AND external_order_line_id='MT-ITEM-3'",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(refund, (Some(1000), Some(30)));

    let funded_at: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT funded_at FROM external_settlement_observations
         WHERE connection_id=$1 AND external_order_line_id='MT-ITEM-2'",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(funded_at, None);

    let checkpoint: (Option<String>, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT cursor,last_platform_updated_at
         FROM platform_sync_checkpoints
         WHERE connection_id=$1 AND stream='meituan_union_order_updated'",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(checkpoint.0, None);
    assert_eq!(checkpoint.1, Some(at("2026-09-27T07:58:20Z")));

    let ledger_count: i64 = sqlx::query_scalar("SELECT count(*) FROM ledger_entries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(ledger_count, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn stale_meituan_worker_keeps_raw_page_but_cannot_overwrite_checkpoint(pool: PgPool) {
    let connection_id = connection(&pool, "publisher-race").await;
    let response = json!({
        "code": 0,
        "message": "成功",
        "data": {
            "scrollId": null,
            "dataList": [{
                "businessLine": 1,
                "orderId": "MT-RACE-1",
                "payTime": 1790494800_i64,
                "payPrice": "10.00",
                "updateTime": 1790495700_i64,
                "commissionRate": "300",
                "profit": "0.30",
                "cpaProfit": "0",
                "status": "2",
                "tradeType": 1,
                "appkey": "test-app"
            }]
        }
    });
    let transport = CheckpointAdvancingTransport {
        pool: pool.clone(),
        connection_id,
        response,
    };
    let sync = MeituanOrderSync::new(
        pool.clone(),
        connection_id,
        MeituanClient::new(signer(), transport),
    );

    let error = sync
        .sync_next(at("2026-09-27T08:00:00Z"))
        .await
        .unwrap_err();
    assert!(matches!(error, PlatformError::ConcurrentSync));

    let raw: (i64, String) = sqlx::query_as(
        "SELECT count(*),min(processing_status)
         FROM platform_raw_events
         WHERE connection_id=$1 AND stream='meituan_union_order_updated'",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(raw, (1, "pending".into()));

    let observations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM external_order_observations WHERE connection_id=$1",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(observations, 0);

    let cursor: Option<String> = sqlx::query_scalar(
        "SELECT cursor FROM platform_sync_checkpoints
         WHERE connection_id=$1 AND stream='meituan_union_order_updated'",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(cursor.as_deref(), Some("other-worker"));

    let ledger_count: i64 = sqlx::query_scalar("SELECT count(*) FROM ledger_entries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(ledger_count, 0);
}

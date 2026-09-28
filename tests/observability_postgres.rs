use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use commission::{http, observability, AppState};
use http_body_util::BodyExt;
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const ALERT_RULES: &str = include_str!("../deploy/prometheus-alerts.yml");
const ALERT_METRIC_CONTRACT: &[(&str, &str)] = &[
    (
        "commission_http_requests_total",
        "commission_http_requests_total",
    ),
    (
        "commission_http_request_duration_seconds_bucket",
        "commission_http_request_duration_seconds",
    ),
    (
        "commission_release_worker_enabled",
        "commission_release_worker_enabled",
    ),
    (
        "commission_release_worker_last_tick_age_seconds",
        "commission_release_worker_last_tick_age_seconds",
    ),
    (
        "commission_release_worker_errors_total",
        "commission_release_worker_errors_total",
    ),
    (
        "commission_platform_sync_worker_enabled",
        "commission_platform_sync_worker_enabled",
    ),
    (
        "commission_platform_sync_worker_last_cycle_age_seconds",
        "commission_platform_sync_worker_last_cycle_age_seconds",
    ),
    (
        "commission_platform_sync_worker_interval_seconds",
        "commission_platform_sync_worker_interval_seconds",
    ),
    (
        "commission_platform_sync_worker_errors_total",
        "commission_platform_sync_worker_errors_total",
    ),
    (
        "commission_platform_sync_attempts_total",
        "commission_platform_sync_attempts_total",
    ),
    (
        "commission_outbox_oldest_pending_age_seconds",
        "commission_outbox_oldest_pending_age_seconds",
    ),
    ("commission_payouts_unknown", "commission_payouts_unknown"),
    (
        "commission_platform_raw_events",
        "commission_platform_raw_events",
    ),
    (
        "commission_platform_raw_event_oldest_age_seconds",
        "commission_platform_raw_event_oldest_age_seconds",
    ),
    (
        "commission_platform_active_connections",
        "commission_platform_active_connections",
    ),
    (
        "commission_platform_connections_with_success",
        "commission_platform_connections_with_success",
    ),
    (
        "commission_db_pool_idle_connections",
        "commission_db_pool_idle_connections",
    ),
    (
        "commission_http_requests_in_flight",
        "commission_http_requests_in_flight",
    ),
];

async fn text(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn get(app: &Router, path: &str, request_id: Option<&str>) -> axum::response::Response {
    let mut request = Request::builder().uri(path);
    if let Some(request_id) = request_id {
        request = request.header("x-request-id", request_id);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn metrics_surface_request_and_operational_signals(pool: PgPool) {
    let connection_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO platform_connections
         (id,platform,external_account_id,display_name,connection_type)
         VALUES($1,'taobao','publisher-observe','可观测测试','app_credentials')",
    )
    .bind(connection_id)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO platform_raw_events
         (id,connection_id,stream,event_type,payload,payload_hash)
         VALUES($1,$2,'taobao_order_updated','fixture',$3,$4)",
    )
    .bind(Uuid::new_v4())
    .bind(connection_id)
    .bind(json!({"fixture": true}))
    .bind("0".repeat(64))
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO platform_sync_checkpoints
         (connection_id,stream,last_attempt_at,last_success_at)
         VALUES($1,'taobao_order_updated',now() - interval '120 seconds',
                now() - interval '120 seconds')",
    )
    .bind(connection_id)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO outbox(id,topic,payload)
         VALUES($1,'observe.fixture',$2)",
    )
    .bind(Uuid::new_v4())
    .bind(json!({"fixture": true}))
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "UPDATE wallets
         SET available_minor = -123
         WHERE account_id = '00000000-0000-0000-0000-000000000002'",
    )
    .execute(&pool)
    .await
    .unwrap();

    observability::configure_workers(true, true, 60);
    observability::platform_sync_observe(
        "untrusted-platform-id",
        false,
        std::time::Duration::from_millis(250),
    );

    let app = http::router(AppState { pool: pool.clone() });

    let supplied = Uuid::new_v4();
    let live = get(&app, "/health/live", Some(&supplied.to_string())).await;
    assert_eq!(live.status(), StatusCode::OK);
    assert_eq!(
        live.headers()
            .get("x-request-id")
            .unwrap()
            .to_str()
            .unwrap(),
        supplied.to_string()
    );

    let generated = get(&app, "/health/live", Some("not-a-uuid")).await;
    assert_eq!(generated.status(), StatusCode::OK);
    let generated_id = generated
        .headers()
        .get("x-request-id")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(Uuid::parse_str(generated_id).is_ok());
    assert_ne!(generated_id, "not-a-uuid");

    let public_metrics = get(&app, "/metrics", None).await;
    assert_eq!(public_metrics.status(), StatusCode::NOT_FOUND);

    let metrics_app = observability::router(AppState { pool });
    let metrics = get(&metrics_app, "/metrics", None).await;
    assert_eq!(metrics.status(), StatusCode::OK);
    assert!(metrics
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("text/plain; version=0.0.4"));
    let body = text(metrics).await;

    assert!(ALERT_RULES.contains("up{job=\"commission\"}"));
    for &(rule_metric, exposition_metric) in ALERT_METRIC_CONTRACT {
        assert!(
            ALERT_RULES.contains(rule_metric),
            "alert rules no longer reference contracted metric {rule_metric}"
        );
        let type_marker = format!("# TYPE {exposition_metric} ");
        assert!(
            body.contains(&type_marker),
            "metrics exposition is missing contracted metric {exposition_metric}"
        );
    }

    assert!(body.contains("# TYPE commission_http_requests_total counter"));
    assert!(body.contains(
        "commission_http_requests_total{method=\"GET\",route=\"/health/live\",status=\"200\"}"
    ));
    assert!(body.contains("commission_db_pool_connections "));
    assert!(body.contains("commission_release_worker_enabled 1.000000"));
    assert!(body.contains("commission_platform_sync_worker_enabled 1.000000"));
    assert!(body.contains("commission_platform_sync_worker_interval_seconds 60.000000"));
    assert!(body.contains(
        "commission_platform_sync_attempts_total{platform=\"other\",outcome=\"error\"} 1.000000"
    ));
    assert!(!body.contains("untrusted-platform-id"));
    assert!(body.contains("commission_outbox_pending 1.000000"));
    assert!(body.contains("commission_wallets_negative_available 1.000000"));
    assert!(body.contains(
        "commission_platform_raw_events{platform=\"taobao\",status=\"pending\"} 1.000000"
    ));
    assert!(body.contains("commission_platform_active_connections{platform=\"taobao\"} 1.000000"));
    assert!(
        body.contains("commission_platform_connections_with_success{platform=\"taobao\"} 1.000000")
    );
    assert!(!body.contains(&supplied.to_string()));
}

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use commission::{auth, http, AppState};
use http_body_util::BodyExt;
use md5::{Digest, Md5};
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn get(app: &Router, token: &str, path: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

fn sign(app_id: &str, secret: &str, body: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(app_id.as_bytes());
    hasher.update(body);
    hasher.update(secret.as_bytes());
    hex::encode(hasher.finalize())
}
#[sqlx::test(migrations = "./migrations")]
async fn douyin_webhook_is_public_but_signature_verified_and_durable(pool: PgPool) {
    let connection_id = Uuid::new_v4();
    let env_name = format!("RS_COMMISSION_TEST_DOUYIN_{}", connection_id.simple());
    let credential_ref = format!("env:{env_name}");
    std::env::set_var(
        &env_name,
        r#"{"app_key":"runtime-app","app_secret":"runtime-secret","access_token":"unused-test-token"}"#,
    );

    sqlx::query(
        "INSERT INTO platform_connections
         (id,platform,external_account_id,display_name,connection_type,credential_ref)
         VALUES($1,'douyin','runtime-shop','运行时抖音','app_credentials',$2)",
    )
    .bind(connection_id)
    .bind(&credential_ref)
    .execute(&pool)
    .await
    .unwrap();

    let app = http::router(AppState { pool: pool.clone() });
    let body = br#"[{"tag":"0","msg_id":"0","data":"2026-09-28T10:00:00+08:00"}]"#;
    let event_sign = sign("runtime-app", "runtime-secret", body);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/webhooks/douyin/{connection_id}"))
                .header("content-type", "application/json")
                .header("app-id", "runtime-app")
                .header("event-sign", event_sign)
                .body(Body::from(body.as_slice()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_json(response).await,
        serde_json::json!({"code":0,"msg":"success"})
    );

    let raw: (i64, String) = sqlx::query_as(
        "SELECT count(*)::bigint,min(processing_status)
         FROM platform_raw_events
         WHERE connection_id=$1 AND stream='douyin_alliance_webhook'",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(raw, (1, "normalized".into()));

    let rejected = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/webhooks/douyin/{connection_id}"))
                .header("content-type", "application/json")
                .header("app-id", "runtime-app")
                .header("event-sign", "00000000000000000000000000000000")
                .body(Body::from(body.as_slice()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);

    std::env::remove_var(env_name);
}

#[sqlx::test(migrations = "./migrations")]
async fn connection_status_requires_auth_and_never_exposes_secret_reference(pool: PgPool) {
    let credential_ref = "env:SHOULD_NOT_LEAK";
    sqlx::query(
        "INSERT INTO platform_connections
         (id,platform,external_account_id,display_name,connection_type,credential_ref)
         VALUES($1,'taobao','publisher-runtime','运行时淘宝','app_credentials',$2)",
    )
    .bind(Uuid::new_v4())
    .bind(credential_ref)
    .execute(&pool)
    .await
    .unwrap();

    let admin = auth::bootstrap(&pool).await.unwrap()["secret"]
        .as_str()
        .unwrap()
        .to_owned();
    let app = http::router(AppState { pool });

    let unauthenticated = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/platform/connections")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let response = get(&app, &admin, "/api/v1/platform/connections").await;
    assert_eq!(response.status(), StatusCode::OK);
    let value = body_json(response).await;
    assert_eq!(value.as_array().unwrap().len(), 1);
    let encoded = serde_json::to_string(&value).unwrap();
    assert!(!encoded.contains("credential_ref"));
    assert!(!encoded.contains("SHOULD_NOT_LEAK"));
}


#[sqlx::test(migrations = "./migrations")]
async fn pull_sync_configuration_failure_is_visible_in_checkpoint(pool: PgPool) {
    let connection_id = Uuid::new_v4();
    let missing = format!("env:RS_COMMISSION_MISSING_{}", connection_id.simple());
    sqlx::query(
        "INSERT INTO platform_connections
         (id,platform,external_account_id,display_name,connection_type,credential_ref)
         VALUES($1,'taobao','publisher-missing','缺失凭据','app_credentials',$2)",
    )
    .bind(connection_id)
    .bind(&missing)
    .execute(&pool)
    .await
    .unwrap();

    let error = commission::platform::runtime::sync_pull_connection(
        &pool,
        connection_id,
        chrono::Utc::now(),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("未配置"));

    let checkpoint: (Option<chrono::DateTime<chrono::Utc>>, Option<String>) = sqlx::query_as(
        "SELECT last_attempt_at,last_error
         FROM platform_sync_checkpoints
         WHERE connection_id=$1 AND stream='taobao_order_updated'",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert!(checkpoint.0.is_some());
    assert!(checkpoint
        .1
        .as_deref()
        .is_some_and(|message| message.contains("未配置")));
}


#[sqlx::test(migrations = "./migrations")]
async fn connection_management_is_idempotent_and_revocation_is_terminal(pool: PgPool) {
    let admin = auth::bootstrap(&pool).await.unwrap()["secret"]
        .as_str()
        .unwrap()
        .to_owned();
    let app = http::router(AppState { pool: pool.clone() });

    let create_body = r#"{
      "platform":"meituan",
      "external_account_id":"meituan-runtime",
      "display_name":"运行时美团",
      "connection_type":"app_credentials",
      "credential_ref":"env:MEITUAN_RUNTIME",
      "settlement_owner_account_id":null
    }"#;

    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/platform/connections")
                .header("authorization", format!("Bearer {admin}"))
                .header("content-type", "application/json")
                .header("idempotency-key", "platform-create-runtime-001")
                .body(Body::from(create_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let created = body_json(create).await;
    let id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
    assert_eq!(created["credential_configured"], true);
    assert!(serde_json::to_string(&created)
        .unwrap()
        .find("MEITUAN_RUNTIME")
        .is_none());

    let replay = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/platform/connections")
                .header("authorization", format!("Bearer {admin}"))
                .header("content-type", "application/json")
                .header("idempotency-key", "platform-create-runtime-001")
                .body(Body::from(create_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(body_json(replay).await["id"], created["id"]);

    for (key, status) in [
        ("platform-status-suspend-001", "suspended"),
        ("platform-status-revoke-001", "revoked"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/platform/connections/{id}/status"))
                    .header("authorization", format!("Bearer {admin}"))
                    .header("content-type", "application/json")
                    .header("idempotency-key", key)
                    .body(Body::from(format!(r#"{{"status":"{status}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["status"], status);
    }

    let reactivate = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/platform/connections/{id}/status"))
                .header("authorization", format!("Bearer {admin}"))
                .header("content-type", "application/json")
                .header("idempotency-key", "platform-status-reactivate-001")
                .body(Body::from(r#"{"status":"active"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reactivate.status(), StatusCode::CONFLICT);

    let stored_ref: String =
        sqlx::query_scalar("SELECT credential_ref FROM platform_connections WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored_ref, "env:MEITUAN_RUNTIME");
}

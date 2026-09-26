#![cfg(not(target_arch = "wasm32"))]
use commission_client::{native::NativeBridge, Operation};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};
use uuid::Uuid;

async fn server(
    responses: Vec<(&'static str, &'static str)>,
) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut seen = Vec::new();
        for (status, body) in responses {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut request = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                assert!(stream.read_line(&mut line).await.unwrap() > 0);
                if let Some(n) = line.to_lowercase().strip_prefix("content-length:") {
                    length = n.trim().parse::<usize>().unwrap();
                }
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
                assert!(request.len() < 16_384);
            }
            assert!(length < 65_536);
            let mut input = vec![0; length];
            stream.read_exact(&mut input).await.unwrap();
            request.push_str(std::str::from_utf8(&input).unwrap());
            seen.push(request);
            let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            stream
                .get_mut()
                .write_all(response.as_bytes())
                .await
                .unwrap();
            stream.get_mut().shutdown().await.unwrap();
        }
        seen
    });
    (origin, task)
}

#[tokio::test]
async fn native_bridge_keeps_tokens_and_retry_material_out_of_webview() {
    let actor = r#"{"id":"11111111-1111-4111-8111-111111111111","name":"test","role":"admin","account_id":null,"expires_at":"2099-01-01T00:00:00Z"}"#;
    let (address, seen) = server(vec![
        ("200 OK", actor),
        (
            "503 Service Unavailable",
            r#"{"error":{"code":"busy","message":"retry"}}"#,
        ),
        ("200 OK", r#"{"done":true}"#),
    ])
    .await;
    let host = NativeBridge::default();
    let session = host.login(&address, "native-secret").await.unwrap();
    assert!(!serde_json::to_string(&session)
        .unwrap()
        .contains("native-secret"));
    assert!(host
        .prepare(Uuid::new_v4(), Operation::CreateAccount, None, "{}")
        .is_err());
    let write = host
        .prepare(session.id, Operation::Release, Some(Uuid::nil()), "{}")
        .unwrap();
    assert!(host
        .prepare(session.id, Operation::Release, Some(Uuid::nil()), "{}")
        .is_err());
    assert!(host
        .execute(session.id, write.id)
        .await
        .unwrap_err()
        .outcome_unknown());
    assert!(host.discard(session.id, write.id).is_err());
    assert!(host.logout(session.id).is_err());
    assert_eq!(
        host.execute(session.id, write.id).await.unwrap()["done"],
        true
    );
    // A lost IPC result must replay the cached outcome, not issue a third transfer.
    assert_eq!(
        host.execute(session.id, write.id).await.unwrap()["done"],
        true
    );
    let requests = seen.await.unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1], requests[2]);
    assert!(requests[1]
        .to_lowercase()
        .contains(&format!("idempotency-key: {}", write.key)));
    let receipt = serde_json::to_string(&write).unwrap();
    assert!(!receipt.contains("native-secret") && !receipt.contains("body"));
    host.discard(session.id, write.id).unwrap();
    host.logout(session.id).unwrap();
    assert!(host.resource(session.id, "dashboard", 0).await.is_err());
}

#[tokio::test]
async fn native_bridge_checks_role_and_resource_allowlist() {
    let actor = r#"{"id":"11111111-1111-4111-8111-111111111111","name":"member","role":"member","account_id":"11111111-1111-4111-8111-111111111111","expires_at":"2099-01-01T00:00:00Z"}"#;
    let (address, seen) = server(vec![("200 OK", actor)]).await;
    let host = NativeBridge::default();
    let session = host.login(&address, "member-secret").await.unwrap();
    assert!(host
        .prepare(session.id, Operation::Release, Some(Uuid::nil()), "{}")
        .is_err());
    assert!(host.resource(session.id, "unknown", 0).await.is_err());
    assert!(host
        .resource(session.id, "accounts", 1_000_001)
        .await
        .is_err());
    host.logout(session.id).unwrap();
    assert_eq!(seen.await.unwrap().len(), 1);
}

#[tokio::test]
async fn native_bridge_reads_order_detail_through_the_fixed_api_surface() {
    let actor = r#"{"id":"11111111-1111-4111-8111-111111111111","name":"admin","role":"admin","account_id":null,"expires_at":"2099-01-01T00:00:00Z"}"#;
    let order_id = Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap();
    let detail = r#"{"order":{"id":"22222222-2222-4222-8222-222222222222","external_id":"order-001"},"allocations":[],"refunds":[]}"#;
    let (address, seen) = server(vec![("200 OK", actor), ("200 OK", detail)]).await;
    let host = NativeBridge::default();
    let session = host.login(&address, "native-secret").await.unwrap();
    assert!(!serde_json::to_string(&session).unwrap().contains("native-secret"));
    let value = host.order(session.id, order_id).await.unwrap();
    assert_eq!(value["order"]["external_id"], "order-001");
    let requests = seen.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].starts_with(&format!("GET /api/v1/orders/{order_id} HTTP/1.1")));
}

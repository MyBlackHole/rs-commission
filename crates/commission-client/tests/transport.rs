#![cfg(not(target_arch = "wasm32"))]
//! Exercise real loopback HTTP rather than substituting the SDK transport.
use commission_client::{ApiClient, ClientError, Operation};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use uuid::Uuid;

async fn server(responses: Vec<String>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let mut requests = Vec::new();
        for response in responses {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut request = String::new();
            let mut content_length = 0;
            loop {
                let mut line = String::new();
                assert!(stream.read_line(&mut line).await.unwrap() > 0);
                if let Some(length) = line.to_lowercase().strip_prefix("content-length:") {
                    content_length = length.trim().parse::<usize>().unwrap();
                }
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
                assert!(request.len() < 16_384);
            }
            let mut body = vec![0; content_length];
            stream.read_exact(&mut body).await.unwrap();
            request.push_str(std::str::from_utf8(&body).unwrap());
            requests.push(request);
            stream
                .get_mut()
                .write_all(response.as_bytes())
                .await
                .unwrap();
            stream.get_mut().shutdown().await.unwrap();
        }
        requests
    });
    (address, handle)
}
fn response(status: &str, body: &str) -> String {
    format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
}

#[tokio::test]
async fn explicit_retry_reuses_identical_key_body_and_credentials() {
    let (address, seen) = server(vec![
        response(
            "503 Service Unavailable",
            r#"{"error":{"code":"retryable","message":"busy"}}"#,
        ),
        response("200 OK", r#"{"done":true}"#),
    ])
    .await;
    let client = ApiClient::new(&address, "test-bearer").unwrap();
    let write = client
        .prepare(Operation::Release, Some(Uuid::nil()), "{}")
        .unwrap();
    let first = client.execute(&write).await.unwrap_err();
    assert!(first.outcome_unknown());
    assert_eq!(client.execute(&write).await.unwrap()["done"], true);
    let requests = seen.await.unwrap();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        let lower = request.to_lowercase();
        assert!(lower.contains("authorization: bearer test-bearer\r\n"));
        assert!(lower.contains(&format!("idempotency-key: {}\r\n", write.key())));
        assert!(request.ends_with("\r\n\r\n{}"));
        assert!(request
            .starts_with("POST /api/v1/orders/00000000-0000-0000-0000-000000000000/release "));
    }
}

#[tokio::test]
async fn structured_unauthorized_is_distinguishable() {
    let (address, seen) = server(vec![response(
        "401 Unauthorized",
        r#"{"error":{"code":"unauthorized","message":"expired"}}"#,
    )])
    .await;
    let client = ApiClient::new(&address, "expired").unwrap();
    let error = client.me().await.unwrap_err();
    assert!(error.unauthorized());
    assert!(!error.outcome_unknown());
    seen.await.unwrap();
}

#[tokio::test]
async fn prepared_write_cannot_be_sent_by_another_session() {
    let first = ApiClient::new("https://example.invalid", "first").unwrap();
    let second = ApiClient::new("https://example.invalid", "second").unwrap();
    let write = first
        .prepare(Operation::Release, Some(Uuid::nil()), "{}")
        .unwrap();
    assert!(matches!(
        second.execute(&write).await,
        Err(ClientError::Invalid(_))
    ));
}

use crate::{error::Result, AppState};
use axum::{
    extract::{MatchedPath, Request, State},
    http::{HeaderName, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        LazyLock, Mutex,
    },
    time::{Duration, Instant},
};
use tracing::Instrument;
use uuid::Uuid;

const X_REQUEST_ID: &str = "x-request-id";
const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";
const HTTP_DURATION_BUCKETS: [(&str, f64); 10] = [
    ("0.005", 0.005),
    ("0.010", 0.010),
    ("0.025", 0.025),
    ("0.050", 0.050),
    ("0.100", 0.100),
    ("0.250", 0.250),
    ("0.500", 0.500),
    ("1.000", 1.000),
    ("2.500", 2.500),
    ("5.000", 5.000),
];

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct HttpKey {
    method: &'static str,
    route: String,
    status: u16,
}

#[derive(Clone, Debug, Default)]
struct HttpStat {
    count: u64,
    duration_micros: u128,
    duration_buckets: [u64; HTTP_DURATION_BUCKETS.len()],
}

struct RuntimeMetrics {
    started_at: Instant,
    in_flight: AtomicU64,
    release_worker_runs: AtomicU64,
    release_worker_released: AtomicU64,
    release_worker_errors: AtomicU64,
    http: Mutex<BTreeMap<HttpKey, HttpStat>>,
}

impl RuntimeMetrics {
    fn new() -> Self {
        Self {
            started_at: Instant::now(),
            in_flight: AtomicU64::new(0),
            release_worker_runs: AtomicU64::new(0),
            release_worker_released: AtomicU64::new(0),
            release_worker_errors: AtomicU64::new(0),
            http: Mutex::new(BTreeMap::new()),
        }
    }

    fn record_http(&self, method: &'static str, route: String, status: StatusCode, elapsed: Duration) {
        let key = HttpKey {
            method,
            route,
            status: status.as_u16(),
        };
        let mut http = self.http.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let stat = http.entry(key).or_default();
        stat.count = stat.count.saturating_add(1);
        stat.duration_micros = stat.duration_micros.saturating_add(elapsed.as_micros());
        let elapsed_seconds = elapsed.as_secs_f64();
        for (index, (_, upper_bound)) in HTTP_DURATION_BUCKETS.iter().enumerate() {
            if elapsed_seconds <= *upper_bound {
                stat.duration_buckets[index] = stat.duration_buckets[index].saturating_add(1);
            }
        }
    }

    fn http_snapshot(&self) -> BTreeMap<HttpKey, HttpStat> {
        self.http
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

static METRICS: LazyLock<RuntimeMetrics> = LazyLock::new(RuntimeMetrics::new);

struct InFlightGuard;

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        METRICS.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

pub async fn observe_http(mut request: Request, next: Next) -> Response {
    let request_id = request
        .headers()
        .get(X_REQUEST_ID)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .unwrap_or_else(Uuid::new_v4);
    let method = method_label(request.method());
    let route = route_label(&request);
    let started = Instant::now();
    METRICS.in_flight.fetch_add(1, Ordering::Relaxed);
    let _in_flight = InFlightGuard;

    let span = tracing::info_span!(
        "http.request",
        request_id = %request_id,
        method = method,
        route = %route
    );
    let mut response = next.run(request).instrument(span.clone()).await;
    let status = response.status();
    let elapsed = started.elapsed();
    METRICS.record_http(method, route.clone(), status, elapsed);

    if let Ok(value) = HeaderValue::from_str(&request_id.to_string()) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(X_REQUEST_ID), value);
    }

    span.in_scope(|| {
        let latency_ms = elapsed.as_secs_f64() * 1000.0;
        if status.is_server_error() {
            tracing::error!(status = status.as_u16(), latency_ms, "http request completed");
        } else {
            tracing::info!(status = status.as_u16(), latency_ms, "http request completed");
        }
    });
    response
}

fn method_label(method: &Method) -> &'static str {
    match method.as_str() {
        "GET" => "GET",
        "POST" => "POST",
        "PUT" => "PUT",
        "PATCH" => "PATCH",
        "DELETE" => "DELETE",
        "HEAD" => "HEAD",
        "OPTIONS" => "OPTIONS",
        _ => "OTHER",
    }
}

fn route_label(request: &Request) -> String {
    if let Some(path) = request.extensions().get::<MatchedPath>() {
        return path.as_str().to_owned();
    }
    match request.uri().path() {
        "/" => "/".to_owned(),
        "/health/live" => "/health/live".to_owned(),
        "/health/ready" => "/health/ready".to_owned(),
        "/metrics" => "/metrics".to_owned(),
        path if path.starts_with("/api/v1/") => "__unmatched_api__".to_owned(),
        _ => "__unmatched__".to_owned(),
    }
}

pub fn release_worker_tick() {
    METRICS.release_worker_runs.fetch_add(1, Ordering::Relaxed);
}

pub fn release_worker_released() {
    METRICS
        .release_worker_released
        .fetch_add(1, Ordering::Relaxed);
}

pub fn release_worker_error() {
    METRICS
        .release_worker_errors
        .fetch_add(1, Ordering::Relaxed);
}

#[derive(Debug)]
struct OperationalSnapshot {
    outbox_pending: i64,
    outbox_oldest_age_seconds: f64,
    payouts_processing: i64,
    payouts_unknown: i64,
    negative_available_wallets: i64,
    platform_raw: Vec<(String, String, i64, f64)>,
    platform_sync: Vec<(String, i64, i64, f64)>,
}

impl OperationalSnapshot {
    async fn load(pool: &sqlx::PgPool) -> Result<Self> {
        let (outbox_pending, outbox_oldest_age_seconds): (i64, Option<f64>) = sqlx::query_as(
            "SELECT count(*)::bigint,
                    CASE WHEN count(*) = 0 THEN NULL
                         ELSE EXTRACT(EPOCH FROM (now() - min(created_at)))::double precision
                    END
             FROM outbox
             WHERE delivered_at IS NULL",
        )
        .fetch_one(pool)
        .await?;

        let (payouts_processing, payouts_unknown): (i64, i64) = sqlx::query_as(
            "SELECT (count(*) FILTER (WHERE status = 'processing'))::bigint,
                    (count(*) FILTER (WHERE status = 'unknown'))::bigint
             FROM payouts",
        )
        .fetch_one(pool)
        .await?;

        let negative_available_wallets: i64 =
            sqlx::query_scalar("SELECT count(*)::bigint FROM wallets WHERE available_minor < 0")
                .fetch_one(pool)
                .await?;

        let platform_raw: Vec<(String, String, i64, f64)> = sqlx::query_as(
            "SELECT c.platform,
                    e.processing_status,
                    count(*)::bigint,
                    EXTRACT(EPOCH FROM (now() - min(e.received_at)))::double precision
             FROM platform_raw_events e
             JOIN platform_connections c ON c.id = e.connection_id
             WHERE e.processing_status IN ('pending', 'rejected')
             GROUP BY c.platform, e.processing_status
             ORDER BY c.platform, e.processing_status",
        )
        .fetch_all(pool)
        .await?;

        let platform_sync: Vec<(String, i64, i64, f64)> = sqlx::query_as(
            "SELECT c.platform,
                    count(DISTINCT c.id)::bigint AS active_connections,
                    (count(DISTINCT c.id) FILTER (WHERE cp.last_success_at IS NOT NULL))::bigint
                        AS connections_with_success,
                    COALESCE(
                        EXTRACT(EPOCH FROM (now() - min(cp.last_success_at)))::double precision,
                        0
                    ) AS oldest_success_age_seconds
             FROM platform_connections c
             LEFT JOIN platform_sync_checkpoints cp ON cp.connection_id = c.id
             WHERE c.status = 'active'
             GROUP BY c.platform
             ORDER BY c.platform",
        )
        .fetch_all(pool)
        .await?;

        Ok(Self {
            outbox_pending,
            outbox_oldest_age_seconds: outbox_oldest_age_seconds.unwrap_or(0.0).max(0.0),
            payouts_processing,
            payouts_unknown,
            negative_available_wallets,
            platform_raw,
            platform_sync,
        })
    }
}

pub async fn metrics(State(state): State<AppState>) -> Result<Response> {
    let operational = OperationalSnapshot::load(&state.pool).await?;
    let body = render_metrics(&state.pool, operational);
    Ok((
        [("content-type", PROMETHEUS_CONTENT_TYPE)],
        body,
    )
        .into_response())
}

fn render_metrics(pool: &sqlx::PgPool, operational: OperationalSnapshot) -> String {
    let mut out = String::with_capacity(8192);

    help_type(&mut out, "commission_uptime_seconds", "Process uptime.", "gauge");
    sample(
        &mut out,
        "commission_uptime_seconds",
        "",
        METRICS.started_at.elapsed().as_secs_f64(),
    );

    help_type(
        &mut out,
        "commission_http_requests_in_flight",
        "HTTP requests currently executing.",
        "gauge",
    );
    sample(
        &mut out,
        "commission_http_requests_in_flight",
        "",
        METRICS.in_flight.load(Ordering::Relaxed) as f64,
    );

    help_type(
        &mut out,
        "commission_http_requests_total",
        "Completed HTTP requests by method, matched route and status.",
        "counter",
    );
    help_type(
        &mut out,
        "commission_http_request_duration_seconds",
        "HTTP request duration by method, matched route and status.",
        "histogram",
    );
    for (key, stat) in METRICS.http_snapshot() {
        let base_labels = format!(
            "method=\"{}\",route=\"{}\",status=\"{}\"",
            escape_label(key.method),
            escape_label(&key.route),
            key.status
        );
        let labels = format!("{{{base_labels}}}");
        sample(
            &mut out,
            "commission_http_requests_total",
            &labels,
            stat.count as f64,
        );
        for (index, (upper_bound, _)) in HTTP_DURATION_BUCKETS.iter().enumerate() {
            let bucket_labels = format!("{{{base_labels},le=\"{upper_bound}\"}}");
            sample(
                &mut out,
                "commission_http_request_duration_seconds_bucket",
                &bucket_labels,
                stat.duration_buckets[index] as f64,
            );
        }
        let infinity_labels = format!("{{{base_labels},le=\"+Inf\"}}");
        sample(
            &mut out,
            "commission_http_request_duration_seconds_bucket",
            &infinity_labels,
            stat.count as f64,
        );
        sample(
            &mut out,
            "commission_http_request_duration_seconds_sum",
            &labels,
            stat.duration_micros as f64 / 1_000_000.0,
        );
        sample(
            &mut out,
            "commission_http_request_duration_seconds_count",
            &labels,
            stat.count as f64,
        );
    }

    help_type(
        &mut out,
        "commission_release_worker_runs_total",
        "Automatic release worker polling ticks.",
        "counter",
    );
    sample(
        &mut out,
        "commission_release_worker_runs_total",
        "",
        METRICS.release_worker_runs.load(Ordering::Relaxed) as f64,
    );
    help_type(
        &mut out,
        "commission_release_worker_released_total",
        "Orders successfully released by the automatic worker.",
        "counter",
    );
    sample(
        &mut out,
        "commission_release_worker_released_total",
        "",
        METRICS.release_worker_released.load(Ordering::Relaxed) as f64,
    );
    help_type(
        &mut out,
        "commission_release_worker_errors_total",
        "Automatic release worker errors.",
        "counter",
    );
    sample(
        &mut out,
        "commission_release_worker_errors_total",
        "",
        METRICS.release_worker_errors.load(Ordering::Relaxed) as f64,
    );

    help_type(
        &mut out,
        "commission_db_pool_connections",
        "Current SQLx PostgreSQL pool connections.",
        "gauge",
    );
    sample(
        &mut out,
        "commission_db_pool_connections",
        "",
        pool.size() as f64,
    );
    help_type(
        &mut out,
        "commission_db_pool_idle_connections",
        "Current idle SQLx PostgreSQL pool connections.",
        "gauge",
    );
    sample(
        &mut out,
        "commission_db_pool_idle_connections",
        "",
        pool.num_idle() as f64,
    );

    help_type(
        &mut out,
        "commission_outbox_pending",
        "Undelivered outbox events.",
        "gauge",
    );
    sample(
        &mut out,
        "commission_outbox_pending",
        "",
        operational.outbox_pending as f64,
    );
    help_type(
        &mut out,
        "commission_outbox_oldest_pending_age_seconds",
        "Age of the oldest undelivered outbox event.",
        "gauge",
    );
    sample(
        &mut out,
        "commission_outbox_oldest_pending_age_seconds",
        "",
        operational.outbox_oldest_age_seconds,
    );

    help_type(
        &mut out,
        "commission_payouts_processing",
        "Payouts currently marked processing.",
        "gauge",
    );
    sample(
        &mut out,
        "commission_payouts_processing",
        "",
        operational.payouts_processing as f64,
    );
    help_type(
        &mut out,
        "commission_payouts_unknown",
        "Payouts with unknown external outcome.",
        "gauge",
    );
    sample(
        &mut out,
        "commission_payouts_unknown",
        "",
        operational.payouts_unknown as f64,
    );
    help_type(
        &mut out,
        "commission_wallets_negative_available",
        "Wallets whose available balance is negative and requires recovery attention.",
        "gauge",
    );
    sample(
        &mut out,
        "commission_wallets_negative_available",
        "",
        operational.negative_available_wallets as f64,
    );

    help_type(
        &mut out,
        "commission_platform_raw_events",
        "Platform raw events waiting for normalization or rejected by normalization.",
        "gauge",
    );
    help_type(
        &mut out,
        "commission_platform_raw_event_oldest_age_seconds",
        "Age of the oldest platform raw event for each non-terminal processing state.",
        "gauge",
    );
    for (platform, status, count, oldest_age) in operational.platform_raw {
        let labels = format!(
            "{{platform=\"{}\",status=\"{}\"}}",
            escape_label(&platform),
            escape_label(&status)
        );
        sample(
            &mut out,
            "commission_platform_raw_events",
            &labels,
            count as f64,
        );
        sample(
            &mut out,
            "commission_platform_raw_event_oldest_age_seconds",
            &labels,
            oldest_age.max(0.0),
        );
    }

    help_type(
        &mut out,
        "commission_platform_active_connections",
        "Active external platform connections.",
        "gauge",
    );
    help_type(
        &mut out,
        "commission_platform_connections_with_success",
        "Active external platform connections having at least one successful checkpoint.",
        "gauge",
    );
    help_type(
        &mut out,
        "commission_platform_oldest_sync_success_age_seconds",
        "Age of the oldest successful checkpoint among active platform connections.",
        "gauge",
    );
    for (platform, active, successful, oldest_age) in operational.platform_sync {
        let labels = format!("{{platform=\"{}\"}}", escape_label(&platform));
        sample(
            &mut out,
            "commission_platform_active_connections",
            &labels,
            active as f64,
        );
        sample(
            &mut out,
            "commission_platform_connections_with_success",
            &labels,
            successful as f64,
        );
        sample(
            &mut out,
            "commission_platform_oldest_sync_success_age_seconds",
            &labels,
            oldest_age.max(0.0),
        );
    }

    out
}

fn help_type(out: &mut String, name: &str, help: &str, metric_type: &str) {
    out.push_str("# HELP ");
    out.push_str(name);
    out.push(' ');
    out.push_str(help);
    out.push('\n');
    out.push_str("# TYPE ");
    out.push_str(name);
    out.push(' ');
    out.push_str(metric_type);
    out.push('\n');
}

fn sample(out: &mut String, name: &str, labels: &str, value: f64) {
    out.push_str(name);
    out.push_str(labels);
    out.push(' ');
    out.push_str(&format_float(value));
    out.push('\n');
}

fn format_float(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.6}")
    } else {
        "0".to_owned()
    }
}

fn escape_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_paths_and_methods_are_bounded() {
        let request = Request::builder()
            .method("BREW")
            .uri("/api/v1/orders/attacker-controlled-id")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(method_label(request.method()), "OTHER");
        assert_eq!(route_label(&request), "__unmatched_api__");
    }

    #[test]
    fn prometheus_label_escaping_is_stable() {
        assert_eq!(escape_label("a\\b\n\"c"), "a\\\\b\\n\\\"c");
    }
}

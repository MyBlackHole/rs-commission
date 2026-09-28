# 可观测性

后端 `commissiond` 使用结构化 JSON tracing，并提供 Prometheus text exposition 的 `GET /metrics`。目标是让运行问题能通过低基数信号被发现，而不是把业务标识、请求体或令牌塞进指标标签。

## 请求关联

每个 HTTP 响应都返回 `X-Request-Id`。

- 客户端传入合法 UUID 时沿用，便于端到端关联；
- 缺失或不是 UUID 时由服务端生成；
- JSON 日志记录 `request_id / method / route / status / latency_ms`；
- `route` 使用 Axum 匹配模板，例如 `/api/v1/orders/{id}`，不会记录真实订单 UUID；
- 不记录 Authorization、Idempotency-Key、请求/响应 body 或 query string。

因此可以用一个 request id 关联网关日志、应用日志和客户端错误，同时避免秘密或高基数业务 ID 进入日志字段。

## Metrics

`/metrics` 不属于公开业务 API。Compose 默认只把后端暴露在本机 `127.0.0.1:8081`，生产部署应由 Prometheus/采集器从内部网络抓取，不要把该路径通过公网网关转发。

建议抓取周期 15–60 秒。

### HTTP

- `commission_http_requests_total{method,route,status}`
- `commission_http_requests_in_flight`
- `commission_http_request_duration_seconds_bucket{method,route,status,le}`
- `commission_http_request_duration_seconds_sum`
- `commission_http_request_duration_seconds_count`

延迟桶固定为 5ms、10ms、25ms、50ms、100ms、250ms、500ms、1s、2.5s、5s 和 +Inf。method 只保留常见 HTTP method，其他统一为 `OTHER`；未匹配 API 统一为 `__unmatched_api__`，防止路径扫描制造指标基数。

PromQL 示例：

```promql
sum(rate(commission_http_requests_total{status=~"5.."}[5m]))
/
sum(rate(commission_http_requests_total[5m]))
```

```promql
histogram_quantile(
  0.95,
  sum by (le, route) (
    rate(commission_http_request_duration_seconds_bucket[5m])
  )
)
```

### PostgreSQL 与业务异常

- `commission_db_pool_connections`
- `commission_db_pool_idle_connections`
- `commission_outbox_pending`
- `commission_outbox_oldest_pending_age_seconds`
- `commission_payouts_processing`
- `commission_payouts_unknown`
- `commission_wallets_negative_available`

这些指标优先暴露“需要人工处理”的状态。特别是 `payouts_unknown > 0` 不应自动解释为失败或重新出款；必须核查外部流水。

### 外部平台同步

- `commission_platform_raw_events{platform,status}`，status 仅为 `pending/rejected`
- `commission_platform_raw_event_oldest_age_seconds{platform,status}`
- `commission_platform_active_connections{platform}`
- `commission_platform_connections_with_success{platform}`
- `commission_platform_oldest_sync_success_age_seconds{platform}`

可据此发现：

- RawEvent 长时间 pending：normalizer/同步 worker 未继续；
- rejected 增长：平台字段变化、签名/解析或状态映射可能需要更新；
- active connections 大于 connections_with_success：存在从未成功同步的连接；
- oldest sync success age 持续增大：某个平台或 stream 停止推进。

指标只使用数据库受约束的 platform/status，不使用 connection id、订单号或 cursor 作为 label。

### 自动解冻 worker

- `commission_release_worker_runs_total`
- `commission_release_worker_released_total`
- `commission_release_worker_errors_total`

错误计数增长时结合同时间段 JSON 日志中的 `automatic release failed` 与 request/DB 日志排查。

## 初始告警建议

这些阈值只是上线前的起点，应按实际吞吐/SLO 调整：

- 5 分钟 HTTP 5xx 比例持续超过 1%；
- p95 持续超过业务可接受延迟；
- `commission_release_worker_errors_total` 在 10 分钟窗口有增长；
- `commission_outbox_oldest_pending_age_seconds` 超过消费者允许延迟；
- `commission_payouts_unknown > 0`；
- `commission_platform_raw_events{status="rejected"} > 0`；
- 平台同步成功年龄超过该平台正常同步周期的数倍；
- DB pool idle 长时间为 0 且 HTTP 延迟同时上升。

告警用于提示调查，不应自动执行资金补偿、重付、改账或删除 rejected RawEvent。

## 边界

本轮没有引入分布式 trace collector、OpenTelemetry exporter、日志后端、Grafana dashboard 或自动告警系统。`/metrics` 是进程与 PostgreSQL 当前状态的观测入口；生产仍需外部 Prometheus/日志平台负责采集、持久化、展示和告警。

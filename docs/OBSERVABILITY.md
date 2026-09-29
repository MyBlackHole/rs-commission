# 可观测性

后端 `commissiond` 使用结构化 JSON tracing，并在独立内部监听器提供 Prometheus text exposition 的 `GET /metrics`。目标是让运行问题能通过低基数信号被发现，而不是把业务标识、请求体或令牌塞进指标标签。

## 请求关联

每个 HTTP 响应都返回 `X-Request-Id`。

- 客户端传入合法 UUID 时沿用，便于端到端关联；
- 缺失或不是 UUID 时由服务端生成；
- JSON 日志记录 `request_id / method / route / status / latency_ms`；
- `route` 使用 Axum 匹配模板，例如 `/api/v1/orders/{id}`，不会记录真实订单 UUID；
- 不记录 Authorization、Idempotency-Key、请求/响应 body 或 query string。

因此可以用一个 request id 关联网关日志、应用日志和客户端错误，同时避免秘密或高基数业务 ID 进入日志字段。

## Metrics

`/metrics` 不属于公开业务 API，也不注册在 `BIND_ADDR` 的业务 Router 上。`commissiond` 使用 `METRICS_BIND_ADDR` 单独监听，非 Docker 默认 `127.0.0.1:9091`；Compose 内设置为 `0.0.0.0:9091`，但不发布这个端口到宿主机，只允许 Compose 内部网络访问。即使直接访问业务端口的 `/metrics` 也只会得到 404。

每次抓取数据库业务状态最多等待 3 秒；超时返回失败 scrape，Prometheus 的 `up{job="commission"}` 会变为 0，避免监控查询在数据库异常时长期占住请求。建议抓取周期 15–60 秒。

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

- `commission_release_worker_enabled`
- `commission_release_worker_last_tick_age_seconds`
- `commission_release_worker_runs_total`
- `commission_release_worker_released_total`
- `commission_release_worker_errors_total`

错误计数增长时结合同时间段 JSON 日志中的 `automatic release failed` 与 request/DB 日志排查；worker 已启用但 last tick age 持续增长则说明任务本身没有推进。

### 平台 Runtime worker

PR #27 的真实平台 Runtime 也纳入进程指标：

- `commission_platform_sync_worker_enabled`
- `commission_platform_sync_worker_interval_seconds`
- `commission_platform_sync_worker_cycles_total`
- `commission_platform_sync_worker_errors_total`
- `commission_platform_sync_worker_last_cycle_age_seconds`
- `commission_platform_sync_attempts_total{platform,outcome}`
- `commission_platform_sync_duration_seconds_bucket{platform,outcome,le}`

`platform` 只允许 taobao/douyin/meituan/other，`outcome` 只允许 success/error，因此不会因 connection id 或外部订单号制造高基数。周期 worker 的 liveness 与单个平台的请求失败可以分别告警。平台 pull 与抖音 webhook 的常规日志也只记录 observation/duplicate 等计数摘要，不打印完整 outcome、cursor 或 raw-event 标识。

## Prometheus 与告警

仓库提供 `deploy/prometheus.yml` 与 `deploy/prometheus-alerts.yml`。本地可直接启动：

```bash
docker compose --profile observability up --build
```

Prometheus UI 仅绑定 `127.0.0.1:9090`，从 Compose 内部网络抓取独立的 `app:9091/metrics`。Grafana OSS 13.2.2 同属 `observability` profile，仅绑定 `127.0.0.1:3000`，启动时自动 provision Prometheus datasource 和 **Commission Operations Overview** dashboard。业务 API 端口 `app:8080` 不注册 metrics 路由，console Nginx 也对 `/metrics` 显式返回 404。

规则文件覆盖 metrics target down、HTTP 5xx/p95、release worker 停滞和错误、平台 worker 停滞/错误/同步失败、Outbox 积压、unknown payout、RawEvent pending/rejected、从未成功同步的连接和 DB pool 持续耗尽。CI 使用 Prometheus 3.15.0 的 `promtool` 同时校验 scrape 配置与 rules。

本地 Grafana 默认账号来自 `GRAFANA_ADMIN_USER/GRAFANA_ADMIN_PASSWORD`；Compose 仅提供 localhost 开发基线，部署到共享主机前必须更换密码并通过受控网络访问。每条 Prometheus 告警都带 `runbook_url`，对应 [告警 Runbook](OBSERVABILITY_RUNBOOK.md)。

CI 不只检查 dashboard JSON：还会提取所有 Grafana PromQL 中的 `commission_*` metric 并与 `src/observability.rs` 契约校验，同时确保每条告警存在 Runbook 章节和 URL。生产仍应由独立 Prometheus/Grafana 抓取展示，并接入 Alertmanager/现有告警平台；仓库不预置邮件、IM webhook 等通知秘密。

## 初始告警建议

这些阈值只是上线前的起点，应按实际吞吐/SLO 调整：

- 5 分钟 HTTP 5xx 比例持续超过 1%；
- p95 持续超过业务可接受延迟；
- `commission_release_worker_errors_total` 在 10 分钟窗口有增长；
- `commission_outbox_oldest_pending_age_seconds` 超过消费者允许延迟；
- `commission_payouts_unknown > 0`；
- `commission_platform_raw_events{status="rejected"} > 0`；
- 平台 worker last cycle age 超过配置周期 3 倍再加 30 秒；
- 任一平台 10 分钟出现同步失败；
- 平台同步成功年龄超过该平台正常同步周期的数倍；
- DB pool idle 长时间为 0 且存在进行中的 HTTP 请求。

告警用于提示调查，不应自动执行资金补偿、重付、改账或删除 rejected RawEvent。

## 边界

本轮仍没有引入分布式 trace collector、OpenTelemetry exporter、日志后端或 Alertmanager 通知配置。仓库已经提供可 provision 的 Grafana dashboard 与告警 Runbook。`/metrics` 是进程与 PostgreSQL 当前状态的观测入口；仓库提供可验证的 Prometheus scrape/rules 基线，但生产仍需外部监控平台负责持久化、展示、通知路由和告警值班。

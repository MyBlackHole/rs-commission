# Commission 可观测告警 Runbook

本 Runbook 对应 `deploy/prometheus-alerts.yml`。目标是帮助值班人员快速判断影响面、保存证据并采取可逆操作。

## 通用原则

1. 先确认告警持续时间、影响范围和最近发布/配置变更，不以单个瞬时样本直接执行资金操作。
2. 使用 `X-Request-Id` 关联应用 JSON 日志；外部平台问题同时查看 connection/checkpoint 的 `last_attempt_at / last_success_at / last_error`。
3. 资金相关异常优先暂停进一步动作并核对原交易，不通过 UPDATE wallet、换幂等键重试出款、删除 RawEvent 等方式“消警”。
4. Prometheus/Grafana 是观测面，不是业务事实源；最终判断仍以 PostgreSQL 账本、平台原始事实和外部渠道流水为准。
5. 处置结束后记录：告警开始/恢复时间、根因、受影响对象、执行动作、验证结果和后续改进。

Dashboard：Grafana → **Commission / Commission Operations Overview**。

## CommissionMetricsTargetDown

**含义**：Prometheus 连续无法抓取 commissiond 独立 metrics listener。

**先查**：
- commissiond 业务 API 是否仍正常；
- `METRICS_BIND_ADDR` 是否监听，Compose 内应为 `0.0.0.0:9091`；
- Prometheus 到 `app:9091` 的内部网络连通性；
- commissiond 日志是否出现 `metrics database snapshot timed out`；
- PostgreSQL 是否阻塞，导致 3 秒 metrics snapshot 超时。

**安全处置**：
- 若仅 metrics listener 异常，保持业务流量不变并恢复内部观测链路；
- 若 DB 同时异常，按数据库事故处理并关注业务 5xx/DB pool 指标；
- 恢复后确认 `up{job="commission"} == 1` 持续至少两个 scrape 周期。

## CommissionApiHigh5xxRatio

**含义**：5 分钟窗口 5xx 比例持续超过 1%。

**先查**：
- Dashboard 的 HTTP Request Rate / p95 面板定位 route；
- 按同时间段 `X-Request-Id` 查看 error 日志；
- DB pool idle、PostgreSQL 锁等待和外部平台请求；
- 最近发布、迁移和配置变化。

**安全处置**：
- 对明确的依赖故障优先隔离或暂停相关非关键 worker；
- 对 `503 retryable` 保留原幂等键重试；
- 不将未知写结果当成失败后换 key 重发。

## CommissionApiP95LatencyHigh

**含义**：至少一个 route 的 p95 延迟持续超过 1 秒。

**先查**：
- Grafana “HTTP p95 by Route” 找到慢 route；
- 同期 DB pool idle / in-flight；
- 是否存在 PostgreSQL 慢查询、锁等待；
- 若 route 涉及平台同步，查看 platform sync p95 与上游延迟。

**安全处置**：
- 先定位瓶颈，再调整超时/并发；
- 不仅通过扩大数据库连接池掩盖慢 SQL 或锁竞争。

## CommissionReleaseWorkerStalled

**含义**：自动解冻 worker 已启用，但超过预期时间没有 tick。

**先查**：
- `commission_release_worker_enabled` 是否应为 1；
- last tick age 是否持续增长；
- commissiond 进程/Tokio runtime 是否健康；
- DB 是否出现锁等待或连接耗尽。

**安全处置**：
- 修复 worker/进程后观察 tick 恢复；
- 到期订单仍可通过受控业务接口处理；
- 禁止直接 UPDATE wallet 或 ledger 来补解冻。

## CommissionReleaseWorkerErrors

**含义**：自动解冻 worker 在最近 10 分钟出现错误。

**先查**：
- JSON 日志中的 `automatic release failed`；
- 错误对应数据库约束、锁冲突还是业务数据；
- 是否集中发生在某批订单。

**安全处置**：
- 可重试错误保持原业务事实和事务语义；
- 若持续失败，暂停自动 worker 后人工排查；
- 不修改历史分录强行对平。

## CommissionPlatformSyncWorkerStalled

**含义**：平台同步 worker 已启用，最近 cycle age 超过配置周期 3 倍加 30 秒。

**先查**：
- worker interval 配置；
- 外部请求是否长期卡住；
- commissiond runtime/数据库是否阻塞；
- 单个平台 sync duration 是否异常升高。

**安全处置**：
- 先确认是否有正在执行的长请求，再决定重启；
- 保留 checkpoint，恢复后继续从原 cursor/window 推进；
- 不手工跳过 checkpoint 来“追平”。

## CommissionPlatformSyncWorkerErrors

**含义**：平台 worker 顶层无法列出 active connections。

**先查**：
- PostgreSQL 连通性；
- runtime 角色是否仍有 platform tables 查询权限；
- schema/migration 是否与二进制一致。

**安全处置**：
- 恢复 DB/权限后观察下个 cycle；
- 不通过扩大权限到超级用户长期规避权限错误。

## CommissionPlatformSyncAttemptsFailing

**含义**：淘宝/美团等具体平台同步在最近 10 分钟出现失败。

**先查**：
- 告警的 `platform` 标签；
- connection checkpoint `last_error`；
- credential_ref 对应环境变量是否存在；
- 签名、token/session、上游限流、超时和接口变更；
- 平台 sync p95 是否同时升高。

**安全处置**：
- 凭据失效时轮换 secret 引用并受控验证；
- 上游故障时保留 checkpoint 等待恢复；
- 不删除 RawEvent 或重置 cursor 掩盖漏单风险。

## CommissionOutboxBacklogOld

**含义**：最老未投递 Outbox 事件超过 5 分钟，并持续 10 分钟。

**先查**：
- Outbox pending 数量和 oldest age；
- 消费者是否存活；
- lease 是否长期未释放；
- ACK 是否失败或消费者下游不可用。

**安全处置**：
- 恢复消费者并允许幂等重投；
- 若事件已消费但 ACK 丢失，消费者必须接受重复；
- 禁止删除 Outbox 历史来消除告警。

## CommissionPayoutUnknown

**含义**：存在外部结果为 unknown 的提现。

**先查**：
- payout external_id、provider_reference、evidence；
- 外部渠道按原业务号查询真实交易；
- 是否为超时后响应丢失，而不是实际失败。

**安全处置**：
- 在确认外部结果前暂停该提现的后续重试；
- 使用原业务号核查渠道；
- **禁止换业务号或 Idempotency-Key 重新出款**。

## CommissionPlatformRawRejected

**含义**：平台 RawEvent 持续处于 rejected。

**先查**：
- platform/status；
- `error_code/error_detail`；
- 最近平台字段、状态枚举或签名规则变化；
- normalizer 版本与对应原始 payload。

**安全处置**：
- 保留原始事件，修复 normalizer 后按受控流程重处理；
- 不修改原始 payload，不删除 rejected 事件。

## CommissionPlatformRawPendingOld

**含义**：平台 RawEvent pending 超过 15 分钟并持续存在。

**先查**：
- 对应平台 worker 是否停滞；
- normalizer 是否执行；
- checkpoint 是否仍在推进；
- DB 锁与连接池是否阻塞后续 projection。

**安全处置**：
- 恢复 worker/normalizer 后确认 pending age 回落；
- 不直接将 processing_status 改成 normalized。

## CommissionPlatformConnectionNeverSynced

**含义**：active connection 数量大于至少成功一次的 connection 数量。

**先查**：
- 是否刚创建连接，仍处于合理首次同步窗口；
- credential_ref 和 secret；
- connection status、checkpoint、last_error；
- 平台 API 权限与账号绑定是否正确。

**安全处置**：
- 修复连接配置后触发受控首次 sync；
- 若连接不应继续使用，按业务流程 suspend/revoke，而不是删除历史身份。

## CommissionDbPoolSaturated

**含义**：DB pool idle 持续为 0，同时存在进行中的 HTTP 请求。

**先查**：
- DB pool size/idle 与 HTTP in-flight；
- PostgreSQL active queries、锁等待、慢 SQL；
- metrics snapshot 是否也开始超时；
- 是否有突发流量或后台 worker 与 API 争抢连接。

**安全处置**：
- 优先修复慢查询、锁和无界并发；
- 必要时降低后台同步压力；
- 调大 pool 前先确认 PostgreSQL 最大连接数、数据库容量和根因。

# 分润台 · rs-commission

Rust 抽佣、推广返佣与结算系统。**0.2 将原生 JavaScript 管理台替换为 Dioxus Rust 前端，并使用共享 Rust 协议和 HTTP SDK。**

> 开发中的资金业务系统，不是已审计的支付产品。实际覆盖和证据见 [验证记录](docs/VERIFICATION.md)。移动端配置、桌面编译检查不等于已签名安装包或真机验收。

## 架构

```text
commission-console (Dioxus Rust)
    ├─ Web / WASM
    ├─ Windows / macOS / Linux WebView
    └─ Android / iOS WebView
             │
commission-client (Rust HTTP SDK, HTTPS, stable idempotency)
             │ JSON /api/v1
commissiond (Axum + Tokio)
             │ SQLx / transaction
PostgreSQL (ledger / wallets / audit / outbox)

commission-types ← both client and server
    Money / Terms / domain arithmetic / request and response models
```

Web、桌面、移动端复用 UI 源码；不是每个平台各写一套页面。客户端不直接连接数据库，不负责最终记账。共享包默认不包含数据库驱动；仅后端启用 `postgres` 映射。当前“多平台”指操作系统与浏览器，不表示多租户。

## 目录

| 路径 | 职责 |
|---|---|
| `src/` | Rust 后端、鉴权、事务、账本、业务服务 |
| `crates/commission-types/` | 共用金额、规则、请求/响应、确定性计算 |
| `crates/commission-client/` | Rust HTTP、错误处理、类型化写请求、幂等重试 |
| `apps/console/` | 中文 Dioxus 前端和响应式 CSS |
| `migrations/` | PostgreSQL 约束、触发器、表结构 |
| `tests/` | 后端 PostgreSQL 集成测试 |
| `deploy/` | 同源代理、生产数据库权限示例 |

后端继续是模块化单体；没有引入微服务、额外消息队列或客户端离线记账。

## 本地启动

```bash
cp .env.example .env
# 将 POSTGRES_PASSWORD 改为随机十六进制密码，避免连接串转义问题。
docker compose up --build -d
docker compose exec app commissiond bootstrap
```

后端现在监听 `http://127.0.0.1:8081`，根路径返回 API 信息，**不再提供旧 JS 页面**。保存初始化命令一次性输出的管理员令牌。

安装 Rust stable、WASM target、Dioxus CLI（与框架一致）：

```bash
rustup target add wasm32-unknown-unknown
cargo install dioxus-cli --version 0.7.10 --locked
cd apps/console
dx serve --platform web
```

浏览器打开 `dx` 输出的地址，输入管理员令牌。开发代理把 `/api/` 转发到 `127.0.0.1:8081`。Web 客户端固定同源，不能在登录页随意改成第三方令牌接收地址。

完整 Web 容器部署配置：

```bash
docker compose --profile web up --build -d
# 浏览器 http://127.0.0.1:8080
```

此方式会构建 Dioxus CLI 与 WASM，配置是否实测见验证记录。生产必须在受信网关终止 HTTPS；不要直接把开发端口暴露公网。

## 桌面与移动

在 `apps/console/` 下，选择一个 renderer，不要使用 `--all-features`：

```bash
# Windows / macOS / Linux，需各平台 WebView 系统依赖
dx serve --platform desktop --no-default-features --features desktop

# Android SDK + NDK + 模拟器/真机
dx serve --platform android --no-default-features --features mobile

# macOS + Xcode + iOS Simulator
dx serve --platform ios --no-default-features --features mobile
```

原生端默认连接 `http://127.0.0.1:8081`，可在登录页配置 HTTPS 服务，或构建时设置 `COMMISSION_API_ORIGIN`。手机的 localhost 是手机自己，必须改成手机可访问的 HTTPS 服务。详见 [跨平台说明](docs/CROSS_PLATFORM.md)。

## 已有业务与本次前端范围

后台保留账户/推广关系、规则优先级与快照、订单计佣、累计部分退款、冻结/解冻、提现状态机、平衡分录、审计、内部对账、outbox 等首版逻辑。

Rust 前端包含登录、13 类数据视图、分页、金额显示、服务器试算，以及 14 种写操作的**共享类型校验 + JSON 编辑 + 明确确认**流程。它是可迭代的工程操作台，**不是原 JS 界面每个专用表单的等价重制**。后续应逐步把高级 JSON 编辑替换成专用业务表单、筛选和详情页。

金额使用整数分，JSON 使用字符串。支持负余额表达提现后退款形成的欠款；显示转换不用浮点。返佣从平台佣金池内部划分，不向商家重复扣费。

写入先冻结请求内容和幂等键，再由用户确认。超时/5xx/响应不完整时保留原请求，不自动重试，不允许直接编辑为另一笔资金请求。令牌不写浏览器存储；待确认请求也仅存在当前会话，尚无跨重启恢复能力。

**结算仍是外部人工转账后核验登记，不会自动代付。** 尚未接真实渠道、回调验签、外部账单对账、实人身份、税务与生产风控。

## 验证

```bash
cargo test --locked -p commission-types -p commission-client
# PostgreSQL 集成测试必须配置 DATABASE_URL
cargo test --locked -p commission-rs --all-targets
cargo clippy --locked -p commission-rs -p commission-types -p commission-client --all-targets -- -D warnings
cargo check --locked -p commission-console --target wasm32-unknown-unknown --no-default-features --features web
cargo check --locked -p commission-console --no-default-features --features desktop
cargo fmt --all -- --check
```

CI 将后端、纯客户端、Web/WASM、三大桌面平台分开验证，避免 server 的 SQLx/Tokio 特性意外混入浏览器依赖。

文档：[API](docs/API.md) · [架构](docs/ARCHITECTURE.md) · [跨平台](docs/CROSS_PLATFORM.md) · [运维](docs/OPERATIONS.md) · [验证](docs/VERIFICATION.md)

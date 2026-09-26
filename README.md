# 分润台 · rs-commission

Rust 抽佣、推广返佣与结算系统。0.3 使用 **Leptos CSR + Tauri 2**：一套 Rust 页面，浏览器独立发布，桌面和移动应用通过 Tauri 承载。后端仍为 Axum / Tokio / SQLx / PostgreSQL。

> 开发中的资金业务系统，不是已审计的支付产品。编译、设备交互和安装包是不同验收层次。以 [验证记录](docs/VERIFICATION.md) 与 PR 对应提交的 CI 为准，不得直接用于真实资金。

## 架构

```text
                          同一份 Leptos Rust UI
                          /                 \
                    浏览器 WASM         Tauri WebView WASM
                         |                  | 七个限定 IPC 命令
                 Rust SDK / fetch       Tauri Rust 宿主 + Rust SDK
                          \                 /
                           Axum Rust API
                                |
                         PostgreSQL 业务账本
```

Web 与 Tauri 共用 `apps/console/src/app.rs`。`platform.rs` 只隔离网络传输；没有为每个平台复制页面。共享金额和协议在 `commission-types`，鉴权、规则选择、退款与最终记账只由服务器决定。多端指操作系统和浏览器，不表示 SaaS 多租户。

Tauri 内的 Leptos 同样编译为 WASM，不是本机控件。WebView 负责 DOM/CSS；原生 Rust 宿主负责网络和命令状态。`bootstrap.js` 只加载 WASM，业务与交互逻辑不用手写 JavaScript，也不依赖 Node 运行时。

## 目录

| 路径 | 职责 |
|---|---|
| `src/` | Axum 后端、鉴权、事务、业务服务、账本 |
| `crates/commission-types/` | 共用金额、规则、请求/响应、确定性计算 |
| `crates/commission-client/` | HTTP SDK、幂等请求、可独立测试的原生会话桥 |
| `apps/console/src/` | Leptos 中文页面与平台传输隔离 |
| `apps/console/src-tauri/` | Tauri 宿主、命令权限与平台配置 |
| `migrations/` | PostgreSQL 表、约束与触发器 |
| `tests/` | 后端数据库集成及浏览器回归 |

## 后端启动

```bash
cp .env.example .env
# 把 POSTGRES_PASSWORD 改为随机十六进制密码。
docker compose up --build -d
docker compose exec app commissiond bootstrap
```

API 为 `http://127.0.0.1:8081`。保存初始化时一次性输出的管理员令牌，不要写入仓库、截图或日志。不使用 Docker 时显式设置 DATABASE_URL，先执行 `commissiond migrate`，再以 `BIND_ADDR=127.0.0.1:8081` 启动服务。程序不自动读取 .env。

## 浏览器开发

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk --version 0.21.14 --locked
cd apps/console
trunk serve
```

访问 `http://localhost:1420`。Trunk 把同源 `/api/` 代理到后端。正式静态资源：`trunk build --release --locked`，输出 `apps/console/dist`。Web 端禁止将令牌发送到任意输入的第三方服务地址。

完整 Web 容器配置：

```bash
docker compose --profile web up --build -d
# 前端 http://127.0.0.1:8080
```

Docker 双镜像构建启动是否实跑见验证记录。生产必须提供可信 HTTPS、独立数据库权限及备份，不要直接暴露开发端口。

## 桌面与移动开发

安装原生工具链及对应系统 WebView 依赖后：

```bash
cargo install tauri-cli --version 2.11.4 --locked
cd apps/console
cargo tauri dev
# 构建可执行程序（当前关闭安装包 bundler）
cargo tauri build --no-bundle
```

Tauri 自动构建同一套 Leptos 页面，启用 `tauri` transport feature，输出 `dist-tauri`。这不是把远程网页加载到窗口中。原生端服务地址默认 loopback:8081，可在登录时填写受信 HTTPS 源。

移动平台仍需各自环境与首次生成工程，以下为配置使用入口，不代表已经编译或真机验收：

```bash
# Android SDK / NDK / 设备
cargo tauri android init
cargo tauri android dev
# macOS / Xcode / iOS Simulator
cargo tauri ios init
cargo tauri ios dev
```

手机的 localhost 是手机自身。移动开发还需配置设备可访问的前端开发地址和 HTTPS API；不要全局放宽证书校验。详见 [跨平台说明](docs/CROSS_PLATFORM.md)。

## 业务范围与安全边界

单平台、多商家、多推广员、CNY。保留计佣规则与快照、累计部分退款、冻结/解冻、提现状态机、平衡分录、审计、内部对账和 Outbox。返佣从平台佣金池内划分，不向商家重复扣费。

中文 UI 包含 13 类数据视图、分页、服务器试算，以及 14 种共享类型校验的业务写操作。高级操作仍是 JSON 编辑加明确确认，不是完整专用业务表单。金额采用整数分和 JSON 字符串，不使用浮点。

原生令牌与不可变写请求保存在 Rust 进程，UI 收到会话和请求句柄；令牌录入仍经过 WebView，不能据此宣称防御已被入侵的前端或本机。权限仅开放本地 main 窗口的七个业务命令，没有通用 HTTP 代理、Shell、文件系统权限。

未知结果不能丢弃或切换账号，只能原 key/body 重试。原生已完成操作缓存结果，避免 IPC 响应丢失后再次发 HTTP 请求。当前仍无跨进程重启恢复，勿在未知结果时强制关闭或刷新；这不是持久资金队列。

**不会自动代付。** 结算仍是人工转账后的核验登记，尚未接真实渠道、验签、外部账单、实人认证、税务或风控。

## 验证命令

```bash
cargo test --locked -p commission-types -p commission-client
# 必须有可用 DATABASE_URL 才能执行数据库集成测试
cargo test --locked -p commission-rs --all-targets
cargo clippy --locked -p commission-rs -p commission-types -p commission-client --all-targets -- -D warnings
cargo check --locked -p commission-console --target wasm32-unknown-unknown
cargo check --locked -p commission-console --target wasm32-unknown-unknown --features tauri
# 先构建 apps/console/dist-tauri
cargo build --locked -p commission-shell --features custom-protocol
cargo fmt --all -- --check
```

不要将整个 workspace 按 wasm target 或 all-features 混编，原生宿主和后端不是浏览器依赖。CI 分目标验证。

[API](docs/API.md) · [账务架构](docs/ARCHITECTURE.md) · [多端](docs/CROSS_PLATFORM.md) · [运维](docs/OPERATIONS.md) · [验证](docs/VERIFICATION.md) · [迁移说明](docs/LEPTOS_TAURI_MIGRATION.md)

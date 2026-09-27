# 验证范围与证据

## 0.3 Leptos + Tauri

本轮从 0.2 的 `61396be` 迁移。替换页面、宿主与构建配置；后端账务源码和数据库迁移保留。Cargo.lock 已更新，Dioxus 依赖已移除；一次性格式化/锁定依赖的写权限 workflow 已删除，正式 CI 为 contents:read。

**每个提交的通过/失败结果以 PR #1 对应 Actions 与产物为准。本文件列出验收范围，不能把测试代码存在等同于通过。**

## 测试分层

| 层次 | 检查内容 |
|---|---|
| Rust 测试 | 47 个：10 个共享金额/领域测试，4 个 SDK/状态单元测试，4 个传输测试，3 个原生桥测试，22 个既有 PostgreSQL 集成测试，4 个外部平台接入 PostgreSQL 约束测试 |
| PostgreSQL | 使用真实 PostgreSQL 18 容器；并发幂等、退款/解冻、欠款、提现、账本约束，以及外部平台 RawEvent 去重/不可变、connection ownership、observation append-only、projection 与 checkpoint 约束 |
| Web | Leptos WASM check/Clippy，Trunk release，依赖隔离与锁文件不变 |
| Tauri 页面 | 同一 UI 按 tauri feature 构建；仍是 WASM，不是原生控件 |
| 浏览器夹具 | Chromium 加载 release WASM，在部署 CSP 下运行 HTTP API 夹具回归 |
| 真实 Web E2E | Chromium → Nginx 同源入口 → Axum → PostgreSQL；真实试算、订单入账/详情/退款、提现申请/详情/审核，不 mock API |
| IPC 传输 | Chromium 加载 tauri-feature release WASM，使用十个限定本地命令夹具验证 JSON 编码、句柄、恢复查询、503 解码和重试；不是 Tauri 真实运行时 |
| Tauri 生命周期 | Linux 上用真实 commission-shell + tauri-driver/WebKitWebDriver；503 后 SIGKILL 宿主，启动新进程、重新登录并恢复原 path/body/idempotency key |
| 桌面 | Windows/macOS/Linux 上执行 SDK 测试、cargo build 链接 Tauri 并嵌入 Leptos 资源、后端 cargo check |

原生桥测试覆盖会话隔离、令牌不返回 UI、未知结果不能覆盖/丢弃/切换账号、同键同体重试、已知结果在 IPC 丢失后缓存重放、资源白名单和角色限制；新增用临时恢复文件重建 NativeBridge，验证 503 后进程重建仍以完全相同请求重试。服务器仍执行最终授权。

浏览器夹具回归覆盖登录退出、不持久化令牌、13 类视图、服务器试算、试算后输入冻结、专用 CaptureOrder 与 503 同键同体重试、订单详情/退款、专用提现申请与 member 账户锁定、提现审核、转义文本、页面 reload 后重新登录并恢复原 key/body、恢复记录不含测试令牌、分页 offset 前进/返回、无 Rust 模板片段泄露、390px 页面宽度。

真实 Web E2E 不注册 Playwright route mock：测试通过 Nginx 同源入口访问 release WASM 和真实 Axum API，数据库为 PostgreSQL 18。管理员先在专用流程调用真实 /quotes，冻结同一组商家/客户/金额输入后通过页面提交真实 CaptureOrder；API 再核验订单 merchant、paid、commission base 和 fee pool，随后继续订单详情/退款。提现同样由管理员通过专用页面申请，再由独立 finance 凭据打开同一笔记录并审核，最终通过 API/PostgreSQL 状态核验。E2E 密钥只保存在 CI 环境和内存中，不写产物。

外部平台接入第一阶段由 `migrations/0002_platform_integration.sql` 提供，只建立 ingestion/normalization 数据域，不修改 accounts.kind、wallets、journals/ledger_entries 或现有订单计佣。Raw platform payload 先持久化到 append-first `platform_raw_events`；有外部 event id 时按 event id 去重，无 event id 时按 connection + stream + event_type + payload hash 去重。原始 payload/identity 不可修改，normalized observations append-only，current projections 与 checkpoints 可更新；组合外键强制 observation/raw-event 以及 projection/latest-observation 属于同一 connection 与业务键。平台未提供的费用保留 NULL，与已知为 0 严格区分。第一阶段不会因为淘宝/抖音/美图订单 GMV 产生任何 LedgerEntry。

## 本轮发现并修复

首轮 `081cf44` 的 40 个 Rust 测试和 Web 构建/既有回归通过，但截图人工复核发现分页按钮把未加花括号的 >= 表达式解析成文本；因此首轮的成功不作为 UI 完成证明。已将该属性表达式显式包裹，并补充下一页/上一页实际请求断言。普通资源页采用稳定 Show 分支，避免切换列表时重建整个 ReadPanel。

tests/tauri_transport_browser.py 覆盖 tauri-feature WASM 与 IPC 编码夹具；tests/tauri_lifecycle_e2e.py 则通过 tauri-driver 2.0.6 + WebKitWebDriver 驱动真实 Linux Tauri 窗口，显式 SIGKILL commission-shell 后启动全新进程，验证 app-data 恢复、同 key/body 重试、bearer 不落恢复文件以及解决后删除恢复文件。Python/夹具 JavaScript 只作测试工具。

## 证据获取

核心迁移基线见 PR #1；真实 Tauri 生命周期恢复见 PR #8 / Actions run 36280126043；外部平台接入数据域见 PR #15。

CI 产物包括 rs-commission-source、commission-console-web、commission-console-tauri-assets、browser-qa、real-stack-e2e 和 tauri-lifecycle-e2e。tauri-lifecycle-e2e 保存两阶段 driver 日志与去敏 result.json；GITHUB_SHA 在 PR CI 中可能是合并测试提交，不代表已合并 main。

本次本地 Chromium 访问回环 HTTP 被环境管理员策略阻止，未将本地尝试列为成功；浏览器实际验证使用 GitHub Actions runner，不修改或绕过本地浏览器策略。

## 尚未覆盖 / 不可据此上线

Android/iOS 编译、模拟器/真机、移动生成工程、软键盘与后台恢复；Windows/macOS 的真实 WebDriver 生命周期、桌面 IME、安装包、签名与更新；Docker 双镜像启动；容量/安全审计与真实支付渠道。Linux 已完成真实 Tauri IPC/窗口强杀重启链路，但不等于所有桌面平台生命周期均已验收。真实 Web E2E 已覆盖一条退款和一条提现审核路径，但不等于所有异常状态、并发条件或支付渠道已经完成端到端验收。

已覆盖 Web reload、NativeBridge 重建以及 Linux 真实 Tauri 窗口 SIGKILL/重启后的单笔请求恢复；尚未覆盖 OS 存储损坏/权限异常矩阵、Windows/macOS 同类生命周期和移动后台回收。恢复材料包含业务请求体，本地静态保密依赖浏览器同源/操作系统账户边界，并非硬件密钥库。产品仍为外部人工转账后的核验登记，禁止直接用于真实资金。

## 历史基线

Dioxus 0.2：`61396be` / run `36146240005` 曾通过 7 个 job、37 个 Rust 测试及旧 UI 回归。只供追溯，不可替代 Leptos/Tauri 验证。

# 验证范围与证据

## 0.3 Leptos + Tauri

本轮从 0.2 的 `61396be` 迁移。替换页面、宿主与构建配置；后端账务源码和数据库迁移保留。Cargo.lock 已更新，Dioxus 依赖已移除；一次性格式化/锁定依赖的写权限 workflow 已删除，正式 CI 为 contents:read。

**每个提交的通过/失败结果以 PR #1 对应 Actions 与产物为准。本文件列出验收范围，不能把测试代码存在等同于通过。**

## 测试分层

| 层次 | 检查内容 |
|---|---|
| Rust 测试 | 40 个：10 个共享金额/领域测试，4 个 SDK/状态单元测试，3 个传输测试，2 个原生桥测试，21 个 PostgreSQL 集成测试 |
| PostgreSQL | 使用真实 PostgreSQL 18 容器；并发幂等、退款/解冻、欠款、提现、账本约束等 |
| Web | Leptos WASM check/Clippy，Trunk release，依赖隔离与锁文件不变 |
| Tauri 页面 | 同一 UI 按 tauri feature 构建；仍是 WASM，不是原生控件 |
| 浏览器夹具 | Chromium 加载 release WASM，在部署 CSP 下运行 HTTP API 夹具回归 |
| 真实 Web E2E | Chromium → Nginx 同源入口 → Axum → PostgreSQL；真实登录、订单详情/退款、提现详情/审核，不 mock API |
| IPC 传输 | Chromium 加载 tauri-feature release WASM，使用七个本地命令夹具验证 JSON 编码、句柄、503 解码和重试；不是 Tauri 真实运行时 |
| 桌面 | Windows/macOS/Linux 上执行 SDK 测试、cargo build 链接 Tauri 并嵌入 Leptos 资源、后端 cargo check |

原生桥测试覆盖会话隔离、令牌不返回 UI、未知结果不能覆盖/丢弃/切换账号、同键同体重试、已知结果在 IPC 丢失后缓存重放，以及资源白名单和角色限制。服务器仍执行最终授权。

浏览器夹具回归覆盖登录退出、不持久化令牌、13 类视图、服务器试算、转义文本、请求准备/确认、503 保留原请求、同键重试、分页 offset 前进/返回、无 Rust 模板片段泄露、390px 页面宽度。

真实 Web E2E 不注册 Playwright route mock：测试通过 Nginx 同源入口访问 release WASM 和真实 Axum API，数据库为 PostgreSQL 18。它创建真实临时管理员/财务业务数据，使用专用订单页面登记退款并验证持久化，再用财务凭据通过专用提现页面审核提现并验证最终状态。E2E 密钥只保存在 CI 环境和内存中，不写产物。

## 本轮发现并修复

首轮 `081cf44` 的 40 个 Rust 测试和 Web 构建/既有回归通过，但截图人工复核发现分页按钮把未加花括号的 >= 表达式解析成文本；因此首轮的成功不作为 UI 完成证明。已将该属性表达式显式包裹，并补充下一页/上一页实际请求断言。普通资源页采用稳定 Show 分支，避免切换列表时重建整个 ReadPanel。

新增 tests/tauri_transport_browser.py 专门覆盖实际 IPC 传输版 WASM。它使用测试命令，不代表原生窗口的权限、系统 WebView 和生命周期已经验收。Python/夹具 JavaScript 只作测试工具。

## 证据获取

PR：https://github.com/MyBlackHole/rs-commission/pull/1

每次 CI 产物包括 rs-commission-source、commission-console-web、commission-console-tauri-assets 和 browser-qa。browser-qa 中 result.json 记录 Web 用例，tauri-transport-result.json 记录 IPC 夹具用例，截图用于人工复核。GITHUB_SHA 在 PR CI 中可能是合并测试提交，不代表已合并 main。

本次本地 Chromium 访问回环 HTTP 被环境管理员策略阻止，未将本地尝试列为成功；浏览器实际验证使用 GitHub Actions runner，不修改或绕过本地浏览器策略。

## 尚未覆盖 / 不可据此上线

Android/iOS 编译、模拟器/真机、移动生成工程、软键盘与后台恢复；桌面窗口 IPC 真联调、IME、安装包、签名与更新；Docker 双镜像启动；容量/安全审计与真实支付渠道。真实 Web E2E 已覆盖一条退款和一条提现审核路径，但不等于所有异常状态、并发条件或支付渠道已经完成端到端验收。

待确认请求仍只存内存，强制关闭/系统回收后无跨重启恢复。未知时禁用退出按钮不是持久化保障。产品仍为外部人工转账后的核验登记，禁止直接用于真实资金。

## 历史基线

Dioxus 0.2：`61396be` / run `36146240005` 曾通过 7 个 job、37 个 Rust 测试及旧 UI 回归。只供追溯，不可替代 Leptos/Tauri 验证。

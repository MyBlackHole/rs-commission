# 0.3 Leptos + Tauri 迁移决策

根据用户明确选型，使用 Leptos CSR + Tauri，替代 Dioxus 前端和宿主。保留 Axum/SQLx 后端、数据库迁移、共享金额类型与 HTTP SDK，不重写账务规则。

Leptos 0.8.20 和 Trunk 0.21.14 负责静态 Web UI；Tauri 2.11.6 负责原生宿主。页面文件只有一份 app.rs，platform.rs 只隔离 fetch 与 IPC。无需为每个平台创建前端，也不额外引入 SSR 或 Node 运行时。

原生端七个业务命令调用既有 Rust SDK，不开放任意 URL 代理。令牌与不可变写请求留在 Rust 进程，UI 仅持有句柄。输入秘密仍经过 WebView，不宣称防御恶意 UI 或本机进程。

未知结果拒绝丢弃/切换账号；原 key/body 重试；已完成但 IPC 响应丢失时重放缓存结果。授权只关联本地 main 窗口，没有远程源、Shell、文件系统、通用 HTTP 插件权限。

一份 UI 不等于一个二进制：Web 与 Tauri transport 分别构建。两者均在 WebView/浏览器执行 Leptos WASM；Tauri 原生 Rust 负责网络与系统宿主。

迁移格式化与锁文件已提交，一次性写权限 workflow 已删除。新版本必须使用自己的 CI，旧 Dioxus 测试记录只是历史。当前实际边界见 VERIFICATION.md 和 PR #1。

先验证真实业务表单、IPC、原生窗口和完整 E2E，再完成 Android/iOS 构建、真机和签名分发。请求跨重启恢复、支付接入不因 UI 迁移自动实现。

资料：
- https://v2.tauri.app/start/frontend/leptos/
- https://v2.tauri.app/start/frontend/
- https://v2.tauri.app/security/capabilities/
- https://docs.rs/leptos/0.8.20/leptos/

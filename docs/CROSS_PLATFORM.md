# Rust 跨平台架构（0.2）

## 平台边界

| 目标 | 渲染与网络 | 本轮验证目标 |
|---|---|---|
| Web | Dioxus WASM + 浏览器 fetch；同源 `/api/v1` | WASM 编译、Clippy、依赖隔离检查 |
| Windows | Dioxus 桌面 WebView2 + Rust HTTP | Windows runner 上的 SDK 测试和桌面编译 |
| macOS | Dioxus 桌面 WKWebView + Rust HTTP | macOS runner 上的 SDK 测试和桌面编译 |
| Linux | Dioxus 桌面 WebKitGTK + Rust HTTP | Linux runner 上的 SDK 测试和桌面编译 |
| Android | Dioxus mobile + 平台 WebView + Rust HTTP | 提供入口和配置；SDK/NDK、APK、真机尚待验证 |
| iOS | Dioxus mobile + WKWebView + Rust HTTP | 提供入口和配置；Xcode、签名、真机尚待验证 |

表中是构建设计，不是测试结果；实际结果以 `VERIFICATION.md` 和 PR 对应提交的 CI 为准。编译通过也不证明窗口、键盘、网络、证书、安装包签名与应用商店分发已经完成。

前端业务代码是 Rust；CSS 负责样式；Web 发布物仍包含工具链生成的 JS/WASM 启动文件。桌面/移动使用系统 WebView，不宣称采用原生控件或零 JavaScript 运行时。

## 依赖方向

`console → client → types`，`server → types`。只有 server 启用 types 的 `postgres` feature，添加 SQLx 映射实现。共享 types 中的角色辅助判断用于 UI 提示；不能替代服务端鉴权。

不要执行 `cargo build --workspace --all-features`：不同 renderer 是互斥部署目标；SQLx 原生映射也不应被合并到 WASM。CI 对目标分别调用 cargo。

UI 不复制计佣业务规则。试算调用后端，订单实际入账仍使用后端选出的规则/受益人快照。共享纯计算仅作为同一规则实现及测试基础，不能让客户端结果成为记账凭据。

## 平台开发

统一依赖：Rust stable、Dioxus CLI 0.7.10。Web 额外需要 `wasm32-unknown-unknown`。Linux 桌面需要 WebKitGTK 4.1、libxdo、OpenSSL 开发包等；Windows 需要 WebView2 Runtime；macOS/iOS 使用系统 WebKit 与 Xcode 工具链。Android 需要 SDK、NDK 和可用设备。

从 `apps/console` 运行：

```bash
dx serve --platform web
dx serve --platform desktop --no-default-features --features desktop
dx serve --platform android --no-default-features --features mobile
dx serve --platform ios --no-default-features --features mobile
```

生成发行包使用相同平台选择配合 `dx bundle`，但本轮不提供“已签名/已上架”的承诺。Android/iOS 应使用受信 HTTPS API，不要启用任意明文传输或全局跳过证书验证。

## 网络与凭据

Web 在当前页面源下访问 `/api/v1`，开发通过 Dioxus proxy，部署通过 Nginx。默认不启用跨域 CORS，不使用 `Access-Control-Allow-Origin: *` 配合资金接口。

桌面/移动直接连接 Rust API。SDK 拒绝非回环 HTTP、带用户名/密码/路径/查询串的服务源。原生 HTTP 禁止自动重定向，禁止库级透明重试；浏览器遵循同源部署与 CSP。

令牌在内存中，HeaderValue 标记 sensitive；无 localStorage、无明文配置文件。令牌仍可能被拥有设备进程权限的人读取；本轮没有声称达到硬件密钥保护。

写请求生成一次 key 和规范化 JSON，保存为不可编辑 PreparedWrite，并绑定 SDK 会话。超时、5xx、无法解析成功响应均不等同于失败。前端保留原请求供显式重试，服务端进行最终幂等判断。

已知边界：待确认请求不跨程序重启保存，切换账号后也不允许直接复用前一账号的 PreparedWrite。上线前需加入受控的请求日志、跨重启核验入口和按业务编号/幂等键定位记录的专用页面。

## 后续迭代顺序

先完成真实 Web/桌面交互验收和专用业务表单；再完成账户范围筛选、订单/提现详情与请求恢复；随后做移动端模拟器、键盘/安全区/后台切换和安装包验证；最后接真实支付渠道与外部对账。不要先堆新模块而跳过账务与客户端状态验证。

参考：Dioxus 官方 0.7 文档 https://dioxuslabs.com/learn/0.7/getting_started/ ，0.7.10 features https://docs.rs/crate/dioxus/0.7.10/features ，Reqwest WASM 说明 https://docs.rs/reqwest/latest/reqwest/ 。

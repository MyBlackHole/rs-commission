# Leptos + Tauri 多端架构（0.3）

## 一份 UI，两类宿主

所有业务页面位于 `apps/console/src/app.rs`，CSS 共用。Leptos CSR 在浏览器和 Tauri WebView 中都运行 WASM，不在手机附带本地数据库或完整服务器。当前不需要 SSR 服务、Node 运行时或每端独立前端。

| 平台 | 页面 | 调用路径 |
|---|---|---|
| Web | Leptos WASM / DOM | platform.rs → Rust SDK → 同源 fetch → API |
| Windows | 同一 Leptos UI / WebView2 | platform.rs → 限定 IPC → Rust 宿主 SDK → HTTPS |
| macOS | 同一 Leptos UI / WKWebView | 同上 |
| Linux | 同一 Leptos UI / WebKitGTK | 同上 |
| Android | 同一 Leptos UI / Android WebView | Tauri 移动入口，仍待编译/设备验证 |
| iOS | 同一 Leptos UI / WKWebView | Tauri 移动入口，仍待编译/设备验证 |

表格描述代码与目标，不表示已经生成各平台安装包。真实结果见 VERIFICATION.md 与对应 CI。浏览器测试不能替代 WebView、中文输入法、软键盘、安全区和生命周期验证。

## 依赖与构建

`console → client → types`；`shell → client → types`；`server → types`。只有 server 启用 types/postgres。`console` 的 tauri feature 只切换 IPC 传输，不把 tauri crate 或系统依赖编进 WASM。

固定 Leptos 0.8.20、Tauri 2.11.6、tauri-build 2.6.3、Trunk 0.21.14；开发 CLI 使用 tauri-cli 2.11.4。框架 crate 与 CLI 版本号不必相同，均提交或明确固定。锁文件不能在 CI 每次重新随机生成。

```bash
# apps/console 中
trunk serve
trunk build --release --locked
cargo tauri dev
cargo tauri build --no-bundle
```

独立 Web 产物为 dist；Tauri 产物为 dist-tauri，通过 Trunk `--features tauri` 生成。不能把 Web transport 的 dist 放进原生宿主，也不能把 dist-tauri 当独立网站发布：它依赖 Tauri IPC。

默认 bundle.active=false，CI 验证桌面程序编译链接，不声称验证 MSI/DMG/APK/IPA 或上架。后续分发阶段再补正式图标、签名、安装器和更新机制。

## 移动开发

Android 需 SDK、NDK、对应 Rust targets、JDK 和设备，先 `cargo tauri android init`。iOS 需 macOS、Xcode、开发者签名配置，先 `cargo tauri ios init`。生成目录在 src-tauri/gen，未作为已验证工程提交。

设备需要可访问的前端开发服务器；默认 localhost:1420 只适合本机桌面开发。使用 Tauri 移动开发指引配置 host/devUrl、网络访问和热更新端口。API 必须为手机可达、证书可信的 HTTPS 服务。不要使用任意明文传输或全局跳过证书校验。

## 原生最小能力接口

只开放 session_login、session_logout、read_resource、quote_commission、prepare_write、execute_write、discard_write 七个命令。

自定义 AppManifest 显式登记命令，capabilities 仅关联本地 main 窗口；未授权远程源、通用 HTTP、Shell 或文件系统。宿主不暴露 SQL、不负责最终资金鉴权，不接受任意 URL 的代理请求。读取资源由 SDK 白名单校验。

登录后 native::NativeBridge 持有 ApiClient、令牌、Actor 和最多一笔待确认请求。界面只收到随机会话 ID、随机请求 ID、幂等键和业务路径。PreparedWrite 和 bearer 不可序列化到 UI。输入令牌时仍经过 WebView，进程内存也不是硬件保护区。

未知写请求禁止覆盖、退出登录或丢弃。重复 execute 使用相同 key/body；终态结果缓存处理 IPC 丢失。不同会话无法调用旧句柄，过时登录/查询响应会被丢弃。Mutex 仅保护状态转换，不跨网络 await。

## 已知边界

没有本地持久请求日志，强杀、系统回收或页面刷新会丢失 UI 状态。禁止退出按钮不是完整生命周期安全措施。正式接资金前必须加入可审计的跨重启核验机制，而不是把内存当可靠队列。

浏览器端令牌暂存 WASM 所属页面内存；原生端登录后的长期令牌在 Rust 宿主内存；两者都不写 localStorage/sessionStorage。CSP 不允许任意业务网络源；生产应通过 HTTPS 网关与后端授权保证边界。

## 资料

- https://v2.tauri.app/start/frontend/leptos/
- https://v2.tauri.app/start/frontend/
- https://v2.tauri.app/security/capabilities/
- https://v2.tauri.app/develop/calling-rust/
- https://docs.rs/leptos/0.8.20/leptos/
- https://trunk-rs.github.io/trunk/guide/configuration/index.html

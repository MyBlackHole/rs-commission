# 验证记录

## 0.3 Leptos + Tauri

迁移从 `61396be` 开始，提交新的 Leptos 页面、Tauri 宿主、原生桥接状态和测试，保留后端账务实现与数据库迁移。

依赖锁定和格式化已由一次性迁移任务完成；该临时写权限 workflow 已删除。正式 CI contents:read，不自动改写源码。

**本文件提交时正在进行迁移后 CI。不要把下面的测试目标视为已经通过；本次对应结果见 PR #1 与该提交的 Actions。**

| 验证项 | 本版目标与边界 |
|---|---|
| 后端与共享 SDK | 保留 37 个基线测试，并新增 1 个状态单元测试、2 个原生桥 HTTP 测试 |
| PostgreSQL | 21 个真实 PostgreSQL 集成测试，非前端模拟数据 |
| Web | WASM check/Clippy、Trunk release、无后端依赖泄漏 |
| Tauri UI | 同一 Leptos 源码按 tauri transport 生成另一份 WASM |
| 浏览器 | Chromium 真实 release WASM；使用模拟 HTTP API，不是全链路 E2E |
| 桌面 | Windows/macOS/Linux cargo build 链接 Tauri 宿主并嵌入 Leptos 资源；不等同窗口交互或安装器 |
| 手机 | 仅代码入口和配置；Android/iOS 未编译或真机验收 |

新增原生测试涵盖会话隔离、令牌不返回界面、结果未知不可丢弃/换账号、同键同体重试、IPC 结果丢失重放缓存、资源白名单与角色限制。UI 不替代服务器鉴权。

浏览器回归测试验证同源 API 请求、13 类视图、金额显示、服务器试算、文本转义、准备/确认门禁、503 原请求重试和 390px 布局。Python/Playwright 仅作测试工具，不参与产品运行。

尚未验证：真实浏览器—服务器—数据库完整联调、原生 IPC 窗口交互、中文 IME、手机生命周期、签名和安装包、Docker 双镜像启动、性能/安全审计、支付接入。跨进程重启的请求恢复尚未实现。

## 历史基线：0.2 Dioxus（不是 0.3 的通过证据）

`61396be` / Actions run `36146240005` 曾通过 7 个 CI job、37 个 Rust 测试及 Dioxus Web 浏览器回归。此后已更换 UI 和宿主，必须重新执行上面的检查。历史源码仍可通过 Git 提交查看。

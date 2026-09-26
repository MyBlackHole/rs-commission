# 0.3 Leptos + Tauri 迁移

按用户选型调整。保留 Axum/SQLx 后端、数据库迁移、共享金额类型及业务 HTTP SDK；仅替换 UI 和客户端宿主。

- Leptos 0.8.20 CSR，Trunk 0.21.14；不引入 SSR 服务或 Node 运行时。
- Tauri 2.11.6 负责桌面/移动宿主，共用 `apps/console/src/app.rs`；`platform.rs` 只选择传输。
- 浏览器同源 HTTP；本机通过七个限定 Tauri 命令调用已有 Rust SDK，不为 WebView 提供任意 URL 代理。
- 本机令牌与不可变写请求留在 Rust 进程；界面只持有会话/请求句柄。录入令牌仍经过 WebView，不能宣称能防御已被入侵的界面或本机进程。
- 未知写入结果禁止丢弃或切换账号；重试复用 key/body；已完成但 IPC 响应丢失时重放缓存结果，不再次发送业务请求。
- Tauri capabilities 限于本地 main 窗口，未授予远程页面、Shell、文件系统或通用 HTTP 插件权限。
- Web 与 Tauri 构建不同 transport feature，复用同一份页面；不是同一个二进制。

代码正在进行迁移后验证。之前 Dioxus 的 CI 成功记录不能替代本版本测试。Android/iOS、签名安装包、真机、完整前后端 E2E、支付接入均不因此自动完成。

临时 prepare-leptos workflow 只用于一次性导出格式化源码和锁定依赖，限定写入既有迁移分支；本轮验收后删除。正式 CI 保持只读。

参考：
- https://v2.tauri.app/start/frontend/leptos/
- https://v2.tauri.app/start/frontend/
- https://v2.tauri.app/security/capabilities/
- https://docs.rs/leptos/0.8.20/leptos/

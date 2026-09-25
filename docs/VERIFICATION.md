# 验证记录

## 2026-09-25：Rust 跨平台迁移

### 已确认的迁移前基线修复

原始提交 `c96ebb8` 的 Rust 构建和格式检查失败。修复提交 `bd38546` 将事务配置从多语句 `raw_sql` 改为独立 prepared query，并明确请求数据的 Sync 约束。

GitHub Actions run `36143181027` 的 `test` job 已通过 `cargo test --locked --all-targets` 和 `cargo clippy --locked --all-targets -- -D warnings`。同一 run 的格式检查仍失败，随后迁移工作区统一格式化。

### 新工作区

共享协议、Rust SDK、Dioxus 前端、同源代理及构建矩阵已提交；当前正在对迁移后的提交运行验证。**不要把上述基线通过等同于新前端/新工作区已通过。** 最终结果见 PR 和后续本文件更新。

测试分层：共享金额/计算与序列化；真实 loopback HTTP SDK 测试；PostgreSQL 集成测试；Web/WASM 编译与 Clippy；Windows/macOS/Linux 原生 SDK 测试和桌面编译；格式检查。

旧 JS 前端及 Node 测试已经退役；旧浏览器截图、离线 UI QA 不再证明 Rust 前端可运行。

尚未声称完成：Rust 前端浏览器端到端、三端桌面交互验收、Android/iOS 编译/模拟器/真机、Docker 双镜像构建、签名安装包、支付渠道、外部账单对账、生产安全审计。

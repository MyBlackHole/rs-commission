# 验证记录

## 2026-09-25：Rust 前后端跨平台基础架构

### 已完成的完整工作区验证

提交：`c20586be44fa445510403f1afe7d20c7ec59b9fc`。

GitHub Actions：`https://github.com/MyBlackHole/rs-commission/actions/runs/36144745907`。7 个 job 全部成功：server、web、三大桌面平台、format、source。

| 检查 | 结果与边界 |
|---|---|
| Rust 测试 | 通过；包含共享计算/金额、客户端真实 loopback HTTP、PostgreSQL 集成测试 |
| PostgreSQL | 21 个 HTTP/事务集成测试通过，使用真实 PostgreSQL 18 服务容器 |
| Clippy | 后端、共享类型、SDK 的所有 targets 通过，`-D warnings` |
| Web/WASM | `cargo check`、Clippy、依赖隔离检查通过；浏览器正常依赖中无 SQLx/Axum/后端包 |
| Web 发布包 | Dioxus CLI 0.7.10 的 `dx build --platform web --release` 通过，生成 HTML/CSS/JS/WASM；Cargo.lock 未改变 |
| Windows | 共享包/SDK 测试、桌面前端 check、后端二进制 check 通过 |
| macOS | 同上，通过 |
| Linux | 同上，通过 |
| rustfmt | 全工作区格式检查通过 |

源码中当前有 37 个 Rust 测试：7 个纯业务计算/属性测试、3 个金额测试、3 个 SDK 单元测试、3 个真实 HTTP 传输测试、21 个 PostgreSQL 集成测试。未把同一测试在多个平台重复运行计算为新增测试。

上述提交之后的收尾改动只增加浏览器测试、修改 CI/部署配置/文档、删除临时迁移工作流；Rust 业务源码未改变。后续任意 Rust 改动仍须重新通过 CI。

### 浏览器交互检查

`tests/console_browser.py` 在 CI 的 web job 中运行，直接加载正式 release WASM 产物，使用与部署相同的 CSP。测试工具为 Python/Playwright，不参与前端或后端运行。API 被测试夹具替代，**不属于浏览器到真实 PostgreSQL 的端到端联调**。

检查范围：登录退出、令牌不持久化、13 类数据页、试算请求中的整数金额、文本注入不产生 HTML 节点、准备请求后锁定参数、明确确认、503 后保持原请求、相同幂等键/请求体重试、390px 页面宽度。

浏览器检查结果以具体提交的 `Release browser smoke with fixture APIs` step 和 `browser-qa/result.json` 为准。不能仅凭脚本存在就认定通过。工作容器本地尝试曾被浏览器环境策略阻止，因此不把本地尝试算为通过。

### 首版修复来源

原始提交 `c96ebb8` 的 Rust 构建与格式检查失败。`bd38546` 将事务配置从多语句 raw_sql 改为独立 prepared query，并明确请求数据的 Sync 约束。run `36143181027` 首次通过后端测试/Clippy。之后工作区统一 rustfmt 并生成提交 Cargo.lock。

临时的 `bootstrap-workspace.yml` 只用于迁移期规范化源文件与锁定依赖，已从交付树删除。正式 CI 仅有 contents:read，不自动向仓库提交修改。主分支 push 和 PR 验证分开，避免同一功能分支同时启动两套完整构建。

### 仍未完成的验证

Android/iOS 编译、模拟器与真机；三端桌面窗口交互；签名安装包；浏览器—真实后端—PostgreSQL 完整联调；Docker 双镜像构建及启动；真实支付/外部账单对账；性能容量、依赖安全和生产审计。

桌面编译通过不等于可分发安装包，移动 renderer 配置也不等于已具备 APK/IPA。当前操作台仍包含 JSON 高级表单，不宣称已完整重制旧 JS 专用业务界面。

旧 JS 前端、Node 测试和旧 FRONTEND_QA.json 已退役；旧截图不能作为 Rust 前端证据。所有浏览器截图仅使用夹具数据，不含真实访问令牌或资金事实。

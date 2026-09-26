# 业务模型与一致性设计

## 1. Rust 工作区与业务边界

`apps/console` 是 Leptos CSR Rust 前端；`apps/console/src-tauri` 是薄原生宿主；`commission-client` 是共用 HTTP SDK；`commission-types` 是共用金额、请求/响应和确定性计算。后端保持模块化单体：HTTP 解析和鉴权，service 编排事务，domain 执行纯计算，ledger 统一记账。只有后端启用共享类型的 PostgreSQL 映射；前端不引入 SQLx 或服务器运行时。

Web、桌面、移动入口共享界面源码。Leptos 在浏览器和 Tauri WebView 中均运行 WASM；原生 Rust 宿主通过七个限定 IPC 命令管理会话与 HTTP。CSS 负责样式，bootstrap.js 仅加载 WASM。不是原生控件，也不是零 JavaScript 引导文件。实际平台验证状态见 VERIFICATION.md。

一个平台、多商家、多推广员、CNY 单币种。这不是多 SaaS 租户系统或完整商城。内部角色能读取整个平台；member 只能访问绑定账户。客户端不直接连接数据库，不允许离线资金记账。不引入 Redis 锁、消息代理或微服务。

一个 PostgreSQL 事务覆盖业务记录、幂等响应、分录、余额投影、审计和 Outbox。

## 2. 核心实体

| 实体 | 职责 |
|---|---|
| accounts | 商家、推广员、唯一平台账户与清算对手账户；上级只能指向已有推广员 |
| referral_bindings | 客户业务 ID 到一级推广员；不可修改，已有订单不可补绑 |
| rules | 创建即发布新版本；可停用，不可覆盖原始参数 |
| orders / allocations | 实付、基数、规则和受益人快照、冻结时间、固定顺序分配与退款投影 |
| refunds | 追加式退款事实、唯一业务号、金额及分配差额 |
| payouts | 申请、审核、金额、收款引用与外部执行状态 |
| journals / ledger_entries | 平衡正负分录；禁止覆盖与删除 |
| wallets | 冻结、可用、占用的投影，不是独立事实源 |
| idempotency | 凭据、操作、客户端键、请求摘要、已提交响应 |
| audit_events / outbox | 与业务同提交的审计与待投递事件 |

## 3. 金额与佣金口径

前后端使用 Money(i64) 整数分，乘法使用 i128。JSON 金额为字符串，显示和解析不经过浮点或 JavaScript Number。单次业务上限为 100000000000000 分。余额累计受 BIGINT 范围限制，溢出必须拒绝事务。

费率用基点，10000 = 100%。佣金池为 `min(计佣基数, 可选封顶, floor(基数×费率/10000)+固定费用)`。两级返佣从佣金池分出，不向商家重复扣费；向下取整，尾差及不存在的推广员份额留在平台。

规则选择：启用且生效、商家专属优先、priority 大者优先、version 大者优先。基数为 [min,max)，生效时间为 [from,until)；version 是全局发布序号。

入账按服务器事务时间选择规则并开始冻结。试算返回 binding=false，不保证实际入账使用相同规则。退款按原始分配和快照计算，不重查当前规则或关系。当前是订单级退款，不支持 SKU/数量级退款、独立运费退款或业绩阶梯提成。

## 4. 累计退款分摊

不能逐人独立取整后把尾差给商家：多个边界同时变化时，一分钱退款可能产生负差额。采用一级、二级、平台、商家固定顺序；不存在的推广员不生成项。

```text
剩余退款 = 累计退款
剩余原额 = 原始实付
当前累计目标 = floor(剩余退款 × 当前原始分配 / 剩余原额)
剩余退款 -= 当前累计目标
剩余原额 -= 当前原始分配
最后一项接收剩余退款
本次冲正 = 当前累计目标 - 上次累计目标
```

对于 0<=share<=total，floor(r×share/total) 及补数随整数 r 单调不减；逐层保留单调性。各方不倒退、差额非负、合计准确，全额退款精确退回原分配。同一累计退款不受拆分次数影响。

固定顺序有舍入偏差，不等同于各项独立比例。零原额退回零。应保存顺序与算法版本，不得变更历史订单算法；固定费用退款口径须业务验收。

## 5. 业务分户账

正 delta 增加账户分类业务余额，清算账户承接反向分录。这不是完整会计总账，清算余额也不是已核验的银行存款。

100 元订单、10% 佣金池、池内返佣 30%+10%：

```text
清算.available   -10000
商家.frozen       +9000
平台.frozen        +600
一级.frozen        +300
二级.frozen        +100
合计                  0
```

解冻为同账户 frozen -N / available +N；提现申请为 available -N / reserved +N；出款成功为 reserved -N / 清算.available +N；失败/驳回释放占用；退款追加相反方向分录，不改历史。

数据库延迟约束在 COMMIT 检查 journal 至少两行且合计零，entry 触发器更新钱包。Rust 记账器也检查平衡。冻结与占用不可为负，可用可为负表达追偿。订单与 allocation 的累计退款只是投影，refunds 保存原始事实。

## 6. 并发、退款与追偿

退款和解冻锁同一订单。记账前按 UUID 排序锁相关钱包，降低死锁风险。每次自动解冻只处理一笔订单；提现申请在钱包锁内检查可用额并占用，防止并发超提。

退款不直接取消提现。可用余额变负后，未执行提现不能审核/开始执行；可驳回原占用抵扣欠款。执行中/未知不能直接驳回，须核验外部结果。真实成功保留欠款，明确失败释放占用，后续收入解冻抵扣负余额。

没有任意加余额或线下追偿入账接口，不得虚构订单或 UPDATE wallets 补账。

## 7. 提现状态机

```text
requested -> approved -> processing -> succeeded
    |            |             +----> failed
    +-> rejected <-+           +----> unknown -> succeeded / failed
```

unknown 保留占用，不因超时自动失败，不可直接 rejected。终态不可再次确认。成功必须提供唯一外部流水和核验证据；申请、审核、执行有凭据分离检查。

processing 只登记意图，不调用渠道。未来自动适配器须使用提现 UUID 等稳定渠道业务键，并实现原交易查询、验签和外部对账。数据库事务不能保证跨银行网络恰好一次。

## 8. 幂等、客户端与崩溃恢复

服务端作用域为凭据 ID、操作名、客户端键；路径目标 ID 参与摘要。同键同请求返回首次已提交响应，不同请求返回 409，每次重新认证。缓存响应可能不是最新状态，最新状态用 GET 查询。

占位、业务效果、响应同事务提交。提交前崩溃回滚；提交后响应丢失用原键恢复。订单/退款/提现业务号有唯一约束，换键或凭据不能重复记账，但未必返回原响应。

SDK PreparedWrite 冻结路径、规范化请求体和 key 并绑定会话。网络失败、5xx、成功响应无法解析均视为未知，只能显式重试原请求。原生 HTTP 禁止自动重定向和透明重试。

Tauri NativeBridge 持有令牌和最多一笔待确认请求；UI 只持有随机句柄。执行时锁内转换状态，锁外请求网络；已确认终态缓存可重放，防止 IPC 响应丢失导致重复网络执行。资源名受白名单约束；本地角色判断不代替后端鉴权。

未知结果和执行中禁止丢弃请求或退出登录，但强制关闭和刷新仍可能丢失上下文。尚无跨重启恢复。上游生产集成必须持久保存业务号、原请求和键；内存不是可靠资金队列。

Outbox 与业务同提交，消费者租约 claim、成功后 ACK；旧租约不能确认新领取。已处理未 ACK 可重投，消费者须按 event.id 幂等。ACK 不递归产生事件；没有自动 Webhook、死信、记录清理或归档。

## 9. 信任边界

随机令牌用 CSPRNG 生成，服务端只保存 SHA-256 摘要。该策略不适用于人类密码。新令牌使用 generate-token，不用低熵字符串。客户端不写 Cookie、localStorage 或 sessionStorage。

原生录入令牌仍经过 WebView，原生进程内存也不是硬件保护区。七个业务 IPC 命令只授权本地 main 窗口；没有通用 HTTP 代理、远程网页、Shell 或文件系统权限。

凭据分离不等于自然人分离；管理员可以创建多把凭据。MFA、SSO、证书管理、网关限流和自动风控尚未实现。integrator 是可信支付事实生产者，虚假 capture 会制造无真实资金支持的账。

触发器不对抗数据库超级用户；SQL 注入或数据库凭据泄漏也可破坏投影。不可变账本不是防勒索 WORM，仍需最小权限、备份、监控和外部对账。

## 10. 技术资料

- https://v2.tauri.app/start/frontend/leptos/
- https://v2.tauri.app/security/capabilities/
- https://docs.rs/leptos/0.8.20/leptos/
- https://docs.rs/axum/0.8.9/axum/
- https://docs.rs/sqlx/0.8.6/sqlx/
- https://www.postgresql.org/docs/18/explicit-locking.html
- https://www.postgresql.org/docs/18/sql-createtrigger.html
- https://docs.stripe.com/api/idempotent_requests

资料用于机制核对，不代表获得对应机构审核。

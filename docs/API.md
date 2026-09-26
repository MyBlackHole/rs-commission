# HTTP API

## 约定

前缀 `/api/v1`，认证为 `Authorization: Bearer cms_<64位小写十六进制随机值>`。金额为 JSON 字符串的整数分，例如 `"10000"`，不能使用数字 10000、100.00 或字符串 `"100.00"`。币种只接受 CNY。

业务 POST 必须使用 JSON 并携带 `Idempotency-Key`（8..128 可见 ASCII 字符）。无参数动作提交 `{}`。试算及 Outbox claim/ack 不要求该头：试算无资金效果，claim/ack 使用租约及事件 ID 语义。

成功统一返回 200 JSON。错误示例：

```json
{"error":{"code":"conflict","message":"可用余额不足或存在待追偿欠款"}}
```

400 输入错误；401 无有效凭据；403 角色禁止；404 无记录或成员无权访问；409 业务冲突；503 锁超时、死锁或事务暂时不可执行。超时、响应丢失及 5xx 后保持原请求和幂等键重试，不要直接换号重新提交。

列表参数 `limit=1..200`、`offset=0..1000000`。账户相关列表可用 `account_id=UUID`。响应 `{items,limit,offset,has_more}`。offset 分页不保证并发写入时的稳定快照。

## 角色

admin 可管理全部，但不能用申请凭据审核自己的提现。

| 角色 | 权限 |
|---|---|
| operator | 账户、推广绑定、规则、订单、退款、到期解冻、申请提现、业务与审计查询 |
| finance | 业务账务查询、到期解冻、提现审核 / 执行 / 核验、内部对账 |
| integrator | 推广绑定、订单 / 退款事实接入、试算、可信内部查询、Outbox 消费 |
| auditor | 只读业务、审计、Outbox 与对账 |
| member | 仅绑定账户的余额、分配、流水、提现，以及该账户提现申请 |

内部角色不是租户隔离；integrator 只能交给可信服务。member 不可查询完整订单或平台规则。签发及撤销凭据仅管理员可用。

## 路径清单

| 方法 | 路径 | 作用 |
|---|---|---|
| GET | `/me` | 当前凭据与期限 |
| GET | `/dashboard` | 平台或成员总览 |
| GET / POST | `/accounts` | 查看 / 创建账户 |
| GET | `/accounts/{id}/wallet` | 账户余额 |
| GET | `/wallets` | 分页余额与欠款 |
| GET / POST | `/referrals` | 查看 / 绑定归属 |
| GET / POST | `/rules` | 查看 / 发布规则版本 |
| POST | `/rules/{id}/disable` | 停用规则 |
| POST | `/quotes` | 无承诺试算 |
| GET / POST | `/orders` | 订单列表 / 已支付事实入账 |
| GET | `/orders/{id}` | 快照、分配、退款 |
| POST | `/orders/{id}/refunds` | 已核实退款事实入账 |
| POST | `/orders/{id}/release` | 到期解冻 |
| GET | `/commissions` | 分配与累计退回，含商家货款 |
| GET | `/ledger` | 不可变分录 |
| GET / POST | `/payouts` | 列表 / 提现申请 |
| GET | `/payouts/{id}` | 提现详情；member 仅能读取绑定账户 |
| POST | `/payouts/{id}/approve` | 独立凭据审核 |
| POST | `/payouts/{id}/processing` | 登记开始外部执行 |
| POST | `/payouts/{id}/reject` | 未执行前驳回 |
| POST | `/payouts/{id}/outcome` | 成功 / 失败 / 未知核验 |
| GET | `/reconciliation` | 内部一致性检查 |
| GET | `/audit` | 操作审计 |
| GET / POST | `/credentials` | 凭据列表 / 签发 |
| POST | `/credentials/{id}/revoke` | 撤销凭据 |
| GET | `/outbox` | 事件列表 |
| POST | `/outbox/claim` | 按租约领取 |
| POST | `/outbox/{id}/ack` | 确认事件 |

无需认证：`/`、`/app.js`、`/app.css`、`/health/live`、`/health/ready`。

## 创建账户与关系

`POST /accounts`：

```json
{"external_id":"shop-001","name":"示例商家","kind":"merchant","parent_id":null}
```

推广员用 `kind=promoter`，parent_id 可指向已存在推广员。不能创建 platform/clearing，也不能使用系统保留的 `__` 业务编号前缀。

`POST /referrals`：

```json
{"customer_external_id":"customer-001","promoter_id":"替换为推广员UUID"}
```

上级由账户关系读取，不允许订单临时指定。已有订单后不可补绑。

## 规则

`POST /rules`：

```json
{
  "name":"默认佣金规则",
  "merchant_id":null,
  "priority":0,
  "min_base_minor":"0",
  "max_base_minor":null,
  "effective_from":null,
  "effective_until":null,
  "terms":{
    "rate_bps":1000,
    "fixed_minor":"0",
    "cap_minor":null,
    "direct_bps":3000,
    "indirect_bps":1000,
    "freeze_seconds":604800
  }
}
```

merchant_id=null 为全局，effective_from=null 为创建时刻。direct/indirect 为佣金池内比例，10000=100%。联调可以设 freeze_seconds=0，但实际参数必须经过业务确认。

## 试算与订单

`POST /quotes`：

```json
{"merchant_id":"替换为商家UUID","customer_external_id":"customer-001","paid_minor":"10000","commission_base_minor":"10000"}
```

返回 rule、受益人 ID、split、binding=false。不会写入订单或账本，实际入账重新匹配规则。

```bash
curl --fail-with-body http://127.0.0.1:8080/api/v1/orders \
 -H "Authorization: Bearer $INTEGRATOR_TOKEN" \
 -H 'Content-Type: application/json' \
 -H 'Idempotency-Key: capture-shop-001-order-001' \
 -d '{"external_id":"order-001","merchant_id":"REPLACE_WITH_MERCHANT_UUID","customer_external_id":"customer-001","currency":"CNY","paid_minor":"10000","commission_base_minor":"10000"}'
```

上游必须先验签或查询确认渠道支付成功。当前不支持按客户端 paid_at 回溯规则，不接受商品明细或任意未知字段。示例 UUID 需替换，不代表本次已运行。

## 退款与解冻

`POST /orders/{id}/refunds`：

```json
{"external_id":"refund-001","amount_minor":"2500","reason":"已核实渠道退款流水 RF001"}
```

金额为本次退款，不是累计额；退款外部编号全局唯一。这不会实际调用支付渠道退款。

`POST /orders/{id}/release` 提交 `{}`。只允许到期操作，无强制提前入口。默认内置任务每 5 秒最多处理 32 笔、每笔独立事务，这不是吞吐性能保证。

## 提现结算

`POST /payouts`：

```json
{"external_id":"payout-001","account_id":"替换为受益账户UUID","amount_minor":"300","destination_ref":"verified-payee-001"}
```

destination_ref 是外部受控收款账户引用，不是银行卡明文，首版不自动核验该引用。申请后立即占用可用余额。

审核 `/payouts/{id}/approve` 请求 `{}`，审核凭据不能与申请凭据相同。

执行登记 `/payouts/{id}/processing`：

```json
{"reason":"财务完成核验，开始以提现ID为业务引用进行人工转账"}
```

返回 mode=manual_external_transfer 与 provider_idempotency_key。**不调用渠道转账**。

核验 `/payouts/{id}/outcome`：

```json
{"status":"unknown","provider_reference":null,"evidence":"渠道查询超时，不能确认最终结果"}
```

确定成功：

```json
{"status":"succeeded","provider_reference":"BANK-TX-2026-001","evidence":"核对银行最终成功记录，凭据 EV001"}
```

明确失败用 failed，才会释放占用。成功流水号全局唯一。终态不可再次出款。未知状态可继续核验，但不能直接 rejected。未执行前驳回 `/reject` 请求 `{"reason":"原因"}`。

## 凭据

先用 `commissiond generate-token` 生成随机令牌，或使用中文签发表单自动生成。管理员向 `/credentials` 提交：

```json
{"name":"财务审核","role":"finance","account_id":null,"expires_in_days":30,"secret":"cms_REPLACE_WITH_64_RANDOM_HEX_CHARACTERS"}
```

响应只有元数据，明文不进入幂等响应、审计和 Outbox。member 必须绑定启用的商家 / 推广员，其他角色 account_id 必须为空。不能使用人工编造的低熵字符串；格式正确不代表随机。

## Outbox

领取 `/outbox/claim`：`{"limit":50,"lease_seconds":60}`。消费者先完成自身可恢复、幂等处理，再以返回的 lease_token ACK：

```json
{"lease_token":"替换为领取返回的UUID"}
```

相同有效租约重复 ACK 成功，过期或被重领的旧租约返回冲突。已领取不等于已投递。租约是服务端并发控制，不是下游恰好一次处理保证。

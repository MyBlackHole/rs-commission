use crate::{auth, domain, error::{Error, Result}, model::*, transaction::{Start, Work}};
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub async fn active_account(tx: &mut Transaction<'_, Postgres>, id: Uuid, kind: &str) -> Result<Account> {
    let account = sqlx::query_as::<_, Account>("SELECT * FROM accounts WHERE id=$1")
        .bind(id).fetch_optional(&mut **tx).await?.ok_or(Error::NotFound)?;
    if !account.active || account.kind != kind { return Err(Error::invalid(format!("需要启用的 {kind} 账户"))); }
    Ok(account)
}

pub async fn create_account(pool: &PgPool, actor: &Actor, key: &str, input: CreateAccount) -> Result<Value> {
    actor.require(&["operator"])?;
    let mut work = match Work::begin(pool, actor, "accounts.create", key, &input).await? {
        Start::Replay(v) => return Ok(v), Start::New(w) => w,
    };
    text(&input.external_id, 96, "业务编号")?;
    text(&input.name, 120, "账户名称")?;
    if !["merchant", "promoter"].contains(&input.kind.as_str()) || (input.parent_id.is_some() && input.kind != "promoter") {
        return Err(Error::invalid("只允许创建商家或推广员账户，只有推广员可设置上级"));
    }
    if input.external_id.starts_with("__") { return Err(Error::invalid("__ 前缀保留给系统账户")); }
    if let Some(parent) = input.parent_id { active_account(&mut work.tx, parent, "promoter").await?; }
    let id = Uuid::new_v4();
    let account = sqlx::query_as::<_, Account>("INSERT INTO accounts(id,external_id,name,kind,parent_id) VALUES($1,$2,$3,$4,$5) RETURNING *")
        .bind(id).bind(input.external_id).bind(input.name).bind(input.kind).bind(input.parent_id)
        .fetch_one(&mut *work.tx).await?;
    sqlx::query("INSERT INTO wallets(account_id) VALUES($1)").bind(id).execute(&mut *work.tx).await?;
    work.complete(serde_json::to_value(account)?, Some(id)).await
}

pub async fn bind_referral(pool: &PgPool, actor: &Actor, key: &str, input: BindReferral) -> Result<Value> {
    actor.require(&["operator", "integrator"])?;
    let mut work = match Work::begin(pool, actor, "referrals.bind", key, &input).await? {
        Start::Replay(v) => return Ok(v), Start::New(w) => w,
    };
    text(&input.customer_external_id, 96, "客户业务编号")?;
    active_account(&mut work.tx, input.promoter_id, "promoter").await?;
    // Serialize binding against capture of the same customer to define attribution.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 9151))")
        .bind(&input.customer_external_id).execute(&mut *work.tx).await?;
    let ordered: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM orders WHERE customer_external_id=$1)")
        .bind(&input.customer_external_id).fetch_one(&mut *work.tx).await?;
    if ordered { return Err(Error::conflict("客户已有入账订单，不能补绑或改变推广关系")); }
    sqlx::query("INSERT INTO referral_bindings(customer_external_id,promoter_id) VALUES($1,$2)")
        .bind(&input.customer_external_id).bind(input.promoter_id).execute(&mut *work.tx).await?;
    work.complete(serde_json::to_value(input)?, None).await
}

pub async fn create_rule(pool: &PgPool, actor: &Actor, key: &str, input: CreateRule) -> Result<Value> {
    actor.require(&["operator"])?;
    let mut work = match Work::begin(pool, actor, "rules.create", key, &input).await? {
        Start::Replay(v) => return Ok(v), Start::New(w) => w,
    };
    text(&input.name, 120, "规则名称")?;
    input.terms.validate()?;
    if input.min_base_minor.0 < 0 || input.min_base_minor.0 > crate::money::MAX_OPERATION_MINOR
        || input.max_base_minor.is_some_and(|m| m <= input.min_base_minor || m.0 > crate::money::MAX_OPERATION_MINOR)
        || !(-10_000..=10_000).contains(&input.priority)
    { return Err(Error::invalid("规则基数范围或优先级无效")); }
    if let Some(merchant) = input.merchant_id { active_account(&mut work.tx, merchant, "merchant").await?; }
    let effective = input.effective_from.unwrap_or_else(Utc::now);
    if input.effective_until.is_some_and(|until| until <= effective) { return Err(Error::invalid("失效时间必须晚于生效时间")); }
    let id = Uuid::new_v4();
    let t = &input.terms;
    let rule = sqlx::query_as::<_, Rule>(
        "INSERT INTO rules(id,name,merchant_id,priority,min_base_minor,max_base_minor,rate_bps,fixed_minor,cap_minor,direct_bps,indirect_bps,freeze_seconds,effective_from,effective_until)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) RETURNING *")
        .bind(id).bind(input.name).bind(input.merchant_id).bind(input.priority)
        .bind(input.min_base_minor).bind(input.max_base_minor).bind(t.rate_bps).bind(t.fixed_minor)
        .bind(t.cap_minor).bind(t.direct_bps).bind(t.indirect_bps).bind(t.freeze_seconds)
        .bind(effective).bind(input.effective_until).fetch_one(&mut *work.tx).await?;
    work.complete(serde_json::to_value(rule)?, Some(id)).await
}

pub async fn disable_rule(pool: &PgPool, actor: &Actor, key: &str, id: Uuid) -> Result<Value> {
    actor.require(&["operator"])?;
    let mut work = match Work::begin(pool, actor, "rules.disable", key, &json!({"id":id})).await? {
        Start::Replay(v) => return Ok(v), Start::New(w) => w,
    };
    let rule = sqlx::query_as::<_, Rule>("UPDATE rules SET active=FALSE WHERE id=$1 RETURNING *")
        .bind(id).fetch_optional(&mut *work.tx).await?.ok_or(Error::NotFound)?;
    work.complete(serde_json::to_value(rule)?, Some(id)).await
}

pub struct Quoted {
    pub rule: Rule,
    pub direct: Option<Uuid>,
    pub indirect: Option<Uuid>,
    pub split: domain::Split,
}

pub async fn quote_in(tx: &mut Transaction<'_, Postgres>, input: &QuoteInput) -> Result<Quoted> {
    active_account(tx, input.merchant_id, "merchant").await?;
    domain::positive(input.paid_minor)?;
    if input.commission_base_minor.0 < 0 || input.commission_base_minor > input.paid_minor {
        return Err(Error::invalid("计佣基数必须在 0 与实付金额之间"));
    }
    let rule = sqlx::query_as::<_, Rule>(
        "SELECT * FROM rules WHERE active AND (merchant_id=$1 OR merchant_id IS NULL)
         AND min_base_minor <= $2 AND (max_base_minor IS NULL OR $2 < max_base_minor)
         AND effective_from <= now() AND (effective_until IS NULL OR effective_until > now())
         ORDER BY (merchant_id IS NOT NULL) DESC, priority DESC, version DESC LIMIT 1")
        .bind(input.merchant_id).bind(input.commission_base_minor).fetch_optional(&mut **tx).await?
        .ok_or_else(|| Error::conflict("没有匹配的有效抽佣规则"))?;
    let (mut direct, mut indirect) = (None, None);
    if let Some(customer) = &input.customer_external_id {
        text(customer, 96, "客户业务编号")?;
        let binding: Option<(Uuid, Option<Uuid>)> = sqlx::query_as(
            "SELECT a.id,a.parent_id FROM referral_bindings r JOIN accounts a ON a.id=r.promoter_id
             WHERE r.customer_external_id=$1 AND a.active AND a.kind='promoter'")
            .bind(customer).fetch_optional(&mut **tx).await?;
        if let Some((first, parent)) = binding {
            direct = Some(first);
            if let Some(parent) = parent {
                let enabled: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM accounts WHERE id=$1 AND active AND kind='promoter')")
                    .bind(parent).fetch_one(&mut **tx).await?;
                if enabled { indirect = Some(parent); }
            }
        }
    }
    let split = domain::split(input.paid_minor, input.commission_base_minor, &rule.terms(), direct.is_some(), indirect.is_some())?;
    Ok(Quoted { rule, direct, indirect, split })
}

pub async fn quote(pool: &PgPool, actor: &Actor, input: QuoteInput) -> Result<Value> {
    actor.require(&["operator", "integrator", "finance", "auditor"])?;
    let mut tx = pool.begin().await?;
    let quote = quote_in(&mut tx, &input).await?;
    tx.rollback().await?;
    Ok(json!({"rule":quote.rule,"direct_account_id":quote.direct,"indirect_account_id":quote.indirect,"split":quote.split,"binding":false}))
}

pub async fn create_credential(pool: &PgPool, actor: &Actor, key: &str, input: CreateCredential) -> Result<Value> {
    actor.require(&[])?;
    let mut work = match Work::begin(pool, actor, "credentials.create", key, &input).await? {
        Start::Replay(v) => return Ok(v), Start::New(w) => w,
    };
    let response = auth::insert_credential(&mut work.tx, &input).await?;
    work.complete(response, None).await
}

pub async fn revoke_credential(pool: &PgPool, actor: &Actor, key: &str, id: Uuid) -> Result<Value> {
    actor.require(&[])?;
    let mut work = match Work::begin(pool, actor, "credentials.revoke", key, &json!({"id":id})).await? {
        Start::Replay(v) => return Ok(v), Start::New(w) => w,
    };
    sqlx::query("SELECT pg_advisory_xact_lock(734190821)").execute(&mut *work.tx).await?;
    let target: Option<(String, bool)> = sqlx::query_as("SELECT role, revoked_at IS NULL AND expires_at>now() FROM credentials WHERE id=$1 FOR UPDATE")
        .bind(id).fetch_optional(&mut *work.tx).await?;
    let (role, valid) = target.ok_or(Error::NotFound)?;
    if role == "admin" && valid {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM credentials WHERE role='admin' AND revoked_at IS NULL AND expires_at>now()")
            .fetch_one(&mut *work.tx).await?;
        if count <= 1 { return Err(Error::conflict("不能撤销最后一个有效管理员令牌")); }
    }
    sqlx::query("UPDATE credentials SET revoked_at=COALESCE(revoked_at,now()) WHERE id=$1")
        .bind(id).execute(&mut *work.tx).await?;
    work.complete(json!({"id":id,"revoked":true}), Some(id)).await
}

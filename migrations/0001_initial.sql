-- One platform, one currency (CNY). The ledger is a business subledger, not a GL.
CREATE TABLE accounts (
    id UUID PRIMARY KEY,
    external_id TEXT NOT NULL UNIQUE CHECK (length(external_id) BETWEEN 1 AND 96),
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 120),
    kind TEXT NOT NULL CHECK (kind IN ('platform', 'clearing', 'merchant', 'promoter')),
    parent_id UUID REFERENCES accounts(id),
    active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (parent_id IS NULL OR (kind = 'promoter' AND parent_id <> id))
);
CREATE UNIQUE INDEX one_platform_account ON accounts(kind) WHERE kind IN ('platform', 'clearing');

CREATE TABLE wallets (
    account_id UUID PRIMARY KEY REFERENCES accounts(id),
    frozen_minor BIGINT NOT NULL DEFAULT 0 CHECK (frozen_minor >= 0),
    available_minor BIGINT NOT NULL DEFAULT 0,
    reserved_minor BIGINT NOT NULL DEFAULT 0 CHECK (reserved_minor >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO accounts(id, external_id, name, kind) VALUES
('00000000-0000-0000-0000-000000000001', '__platform__', '平台佣金', 'platform'),
('00000000-0000-0000-0000-000000000002', '__clearing__', '支付清算对手账户', 'clearing');
INSERT INTO wallets(account_id) SELECT id FROM accounts;

CREATE TABLE credentials (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 120),
    token_hash TEXT NOT NULL UNIQUE CHECK (length(token_hash) = 64),
    role TEXT NOT NULL CHECK (role IN ('admin', 'operator', 'finance', 'integrator', 'auditor', 'member')),
    account_id UUID REFERENCES accounts(id),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((role = 'member') = (account_id IS NOT NULL))
);

CREATE TABLE referral_bindings (
    customer_external_id TEXT PRIMARY KEY CHECK (length(customer_external_id) BETWEEN 1 AND 96),
    promoter_id UUID NOT NULL REFERENCES accounts(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE rules (
    id UUID PRIMARY KEY,
    version BIGSERIAL NOT NULL UNIQUE,
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 120),
    merchant_id UUID REFERENCES accounts(id),
    priority INTEGER NOT NULL DEFAULT 0 CHECK (priority BETWEEN -10000 AND 10000),
    min_base_minor BIGINT NOT NULL DEFAULT 0 CHECK (min_base_minor >= 0),
    max_base_minor BIGINT CHECK (max_base_minor > min_base_minor),
    rate_bps INTEGER NOT NULL CHECK (rate_bps BETWEEN 0 AND 10000),
    fixed_minor BIGINT NOT NULL DEFAULT 0 CHECK (fixed_minor >= 0),
    cap_minor BIGINT CHECK (cap_minor >= 0),
    direct_bps INTEGER NOT NULL DEFAULT 0 CHECK (direct_bps BETWEEN 0 AND 10000),
    indirect_bps INTEGER NOT NULL DEFAULT 0 CHECK (indirect_bps BETWEEN 0 AND 10000),
    freeze_seconds BIGINT NOT NULL CHECK (freeze_seconds BETWEEN 0 AND 31536000),
    effective_from TIMESTAMPTZ NOT NULL DEFAULT now(),
    effective_until TIMESTAMPTZ,
    active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (direct_bps + indirect_bps <= 10000),
    CHECK (effective_until IS NULL OR effective_until > effective_from)
);
CREATE INDEX rule_selection ON rules(merchant_id, priority DESC, version DESC) WHERE active;

CREATE TABLE orders (
    id UUID PRIMARY KEY,
    external_id TEXT NOT NULL UNIQUE CHECK (length(external_id) BETWEEN 1 AND 96),
    merchant_id UUID NOT NULL REFERENCES accounts(id),
    customer_external_id TEXT,
    currency TEXT NOT NULL DEFAULT 'CNY' CHECK (currency = 'CNY'),
    paid_minor BIGINT NOT NULL CHECK (paid_minor > 0 AND paid_minor <= 100000000000000),
    commission_base_minor BIGINT NOT NULL CHECK (commission_base_minor >= 0 AND commission_base_minor <= paid_minor),
    fee_pool_minor BIGINT NOT NULL CHECK (fee_pool_minor >= 0 AND fee_pool_minor <= commission_base_minor),
    refunded_minor BIGINT NOT NULL DEFAULT 0 CHECK (refunded_minor >= 0 AND refunded_minor <= paid_minor),
    rule_id UUID NOT NULL REFERENCES rules(id),
    rule_snapshot JSONB NOT NULL,
    input_hash TEXT NOT NULL,
    captured_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    unlock_at TIMESTAMPTZ NOT NULL,
    released_at TIMESTAMPTZ,
    CHECK (released_at IS NULL OR released_at >= unlock_at)
);
CREATE INDEX due_orders ON orders(unlock_at, id) WHERE released_at IS NULL;
CREATE INDEX orders_merchant ON orders(merchant_id, captured_at DESC, id DESC);

CREATE TABLE allocations (
    order_id UUID NOT NULL REFERENCES orders(id),
    ordinal SMALLINT NOT NULL CHECK (ordinal BETWEEN 0 AND 3),
    account_id UUID NOT NULL REFERENCES accounts(id),
    slot TEXT NOT NULL CHECK (slot IN ('direct', 'indirect', 'platform', 'merchant')),
    original_minor BIGINT NOT NULL CHECK (original_minor >= 0),
    refunded_minor BIGINT NOT NULL DEFAULT 0 CHECK (refunded_minor >= 0 AND refunded_minor <= original_minor),
    PRIMARY KEY (order_id, ordinal),
    UNIQUE (order_id, slot)
);
CREATE INDEX allocations_account ON allocations(account_id, order_id);

CREATE TABLE refunds (
    id UUID PRIMARY KEY,
    order_id UUID NOT NULL REFERENCES orders(id),
    external_id TEXT NOT NULL UNIQUE CHECK (length(external_id) BETWEEN 1 AND 96),
    amount_minor BIGINT NOT NULL CHECK (amount_minor > 0),
    cumulative_minor BIGINT NOT NULL CHECK (cumulative_minor >= amount_minor),
    reason TEXT NOT NULL CHECK (length(reason) BETWEEN 1 AND 400),
    allocation_deltas JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX refunds_order ON refunds(order_id, created_at);

CREATE TABLE payouts (
    id UUID PRIMARY KEY,
    external_id TEXT NOT NULL UNIQUE CHECK (length(external_id) BETWEEN 1 AND 96),
    account_id UUID NOT NULL REFERENCES accounts(id),
    amount_minor BIGINT NOT NULL CHECK (amount_minor > 0 AND amount_minor <= 100000000000000),
    destination_ref TEXT NOT NULL CHECK (length(destination_ref) BETWEEN 1 AND 160),
    status TEXT NOT NULL DEFAULT 'requested' CHECK (status IN ('requested', 'approved', 'processing', 'unknown', 'succeeded', 'failed', 'rejected')),
    requested_by UUID NOT NULL REFERENCES credentials(id),
    approved_by UUID REFERENCES credentials(id),
    provider_reference TEXT UNIQUE,
    evidence TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (approved_by IS NULL OR approved_by <> requested_by),
    CHECK (status <> 'succeeded' OR (provider_reference IS NOT NULL AND evidence IS NOT NULL))
);
CREATE INDEX payouts_account ON payouts(account_id, created_at DESC, id DESC);

CREATE TABLE journals (
    id UUID PRIMARY KEY,
    event_key TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL CHECK (kind IN ('capture', 'refund', 'release', 'payout_reserve', 'payout_reject', 'payout_paid', 'payout_failed')),
    order_id UUID REFERENCES orders(id),
    payout_id UUID REFERENCES payouts(id),
    actor_id UUID REFERENCES credentials(id),
    metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE ledger_entries (
    journal_id UUID NOT NULL REFERENCES journals(id),
    line_no SMALLINT NOT NULL,
    account_id UUID NOT NULL REFERENCES accounts(id),
    bucket TEXT NOT NULL CHECK (bucket IN ('frozen', 'available', 'reserved')),
    delta_minor BIGINT NOT NULL CHECK (delta_minor <> 0),
    PRIMARY KEY (journal_id, line_no),
    UNIQUE (journal_id, account_id, bucket)
);
CREATE INDEX ledger_account ON ledger_entries(account_id, journal_id);

CREATE TABLE idempotency (
    actor_id UUID NOT NULL REFERENCES credentials(id),
    operation TEXT NOT NULL,
    key TEXT NOT NULL CHECK (length(key) BETWEEN 8 AND 128),
    request_hash TEXT NOT NULL,
    response JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (actor_id, operation, key)
);

CREATE TABLE audit_events (
    id BIGSERIAL PRIMARY KEY,
    actor_id UUID REFERENCES credentials(id),
    action TEXT NOT NULL,
    target_id UUID,
    detail JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE outbox (
    id UUID PRIMARY KEY,
    topic TEXT NOT NULL,
    aggregate_id UUID,
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    attempts INTEGER NOT NULL DEFAULT 0,
    lease_token UUID,
    lease_until TIMESTAMPTZ,
    delivered_at TIMESTAMPTZ
);
CREATE INDEX outbox_ready ON outbox(created_at, id) WHERE delivered_at IS NULL;

-- Wallets are projections updated only as a side-effect of an appended entry.
CREATE FUNCTION apply_ledger_entry() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.bucket = 'frozen' THEN
        UPDATE wallets SET frozen_minor = frozen_minor + NEW.delta_minor, updated_at = now() WHERE account_id = NEW.account_id;
    ELSIF NEW.bucket = 'available' THEN
        UPDATE wallets SET available_minor = available_minor + NEW.delta_minor, updated_at = now() WHERE account_id = NEW.account_id;
    ELSE
        UPDATE wallets SET reserved_minor = reserved_minor + NEW.delta_minor, updated_at = now() WHERE account_id = NEW.account_id;
    END IF;
    IF NOT FOUND THEN RAISE EXCEPTION 'wallet does not exist'; END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER apply_entry AFTER INSERT ON ledger_entries FOR EACH ROW EXECUTE FUNCTION apply_ledger_entry();

-- Deferred until COMMIT: no journal can commit without a balanced set of entries.
CREATE FUNCTION check_journal_balance() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE jid UUID; total NUMERIC; lines BIGINT;
BEGIN
    IF TG_TABLE_NAME = 'journals' THEN jid := NEW.id; ELSE jid := NEW.journal_id; END IF;
    SELECT COALESCE(sum(delta_minor::NUMERIC), 0), count(*) INTO total, lines
      FROM ledger_entries WHERE journal_id = jid;
    IF total <> 0 OR lines < 2 THEN RAISE EXCEPTION 'unbalanced or empty journal %', jid USING ERRCODE = '23514'; END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER balanced_journal AFTER INSERT ON journals
DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION check_journal_balance();
CREATE CONSTRAINT TRIGGER balanced_entries AFTER INSERT ON ledger_entries
DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION check_journal_balance();

CREATE FUNCTION deny_mutation() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION '% is append-only', TG_TABLE_NAME USING ERRCODE = '23514';
END;
$$;
CREATE TRIGGER immutable_journals BEFORE UPDATE OR DELETE ON journals FOR EACH ROW EXECUTE FUNCTION deny_mutation();
CREATE TRIGGER immutable_entries BEFORE UPDATE OR DELETE ON ledger_entries FOR EACH ROW EXECUTE FUNCTION deny_mutation();
CREATE TRIGGER immutable_audit BEFORE UPDATE OR DELETE ON audit_events FOR EACH ROW EXECUTE FUNCTION deny_mutation();
CREATE TRIGGER immutable_refunds BEFORE UPDATE OR DELETE ON refunds FOR EACH ROW EXECUTE FUNCTION deny_mutation();
CREATE TRIGGER immutable_referrals BEFORE UPDATE OR DELETE ON referral_bindings FOR EACH ROW EXECUTE FUNCTION deny_mutation();

CREATE FUNCTION protect_original_fields() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME = 'orders' THEN
        IF (to_jsonb(NEW) - 'refunded_minor' - 'released_at') IS DISTINCT FROM
           (to_jsonb(OLD) - 'refunded_minor' - 'released_at') OR NEW.refunded_minor < OLD.refunded_minor
           OR (OLD.released_at IS NOT NULL AND NEW.released_at IS DISTINCT FROM OLD.released_at)
        THEN RAISE EXCEPTION 'original order fields are immutable' USING ERRCODE = '23514'; END IF;
    ELSIF TG_TABLE_NAME = 'allocations' THEN
        IF (to_jsonb(NEW) - 'refunded_minor') IS DISTINCT FROM (to_jsonb(OLD) - 'refunded_minor')
           OR NEW.refunded_minor < OLD.refunded_minor
        THEN RAISE EXCEPTION 'original allocation is immutable' USING ERRCODE = '23514'; END IF;
    ELSIF TG_TABLE_NAME = 'rules' THEN
        IF (to_jsonb(NEW) - 'active') IS DISTINCT FROM (to_jsonb(OLD) - 'active')
        THEN RAISE EXCEPTION 'create a new rule version instead' USING ERRCODE = '23514'; END IF;
    ELSIF TG_TABLE_NAME = 'accounts' THEN
        IF (to_jsonb(NEW) - 'active' - 'name') IS DISTINCT FROM (to_jsonb(OLD) - 'active' - 'name')
        THEN RAISE EXCEPTION 'account identity and ancestry are immutable' USING ERRCODE = '23514'; END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER original_order BEFORE UPDATE ON orders FOR EACH ROW EXECUTE FUNCTION protect_original_fields();
CREATE TRIGGER original_allocation BEFORE UPDATE ON allocations FOR EACH ROW EXECUTE FUNCTION protect_original_fields();
CREATE TRIGGER original_rule BEFORE UPDATE ON rules FOR EACH ROW EXECUTE FUNCTION protect_original_fields();
CREATE TRIGGER original_account BEFORE UPDATE ON accounts FOR EACH ROW EXECUTE FUNCTION protect_original_fields();
CREATE TRIGGER keep_orders BEFORE DELETE ON orders FOR EACH ROW EXECUTE FUNCTION deny_mutation();
CREATE TRIGGER keep_allocations BEFORE DELETE ON allocations FOR EACH ROW EXECUTE FUNCTION deny_mutation();
CREATE TRIGGER keep_rules BEFORE DELETE ON rules FOR EACH ROW EXECUTE FUNCTION deny_mutation();

-- DB owners can disable triggers. Use a separate non-owner runtime role in production.

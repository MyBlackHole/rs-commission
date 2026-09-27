-- External platform ingestion only.
-- This migration deliberately does NOT post financial ledger entries or change wallet/account semantics.
-- Platform payloads are captured durably first, normalized into append-only observations, and only then projected.

CREATE TABLE platform_connections (
    id UUID PRIMARY KEY,
    platform TEXT NOT NULL CHECK (platform IN ('taobao', 'douyin', 'meitu')),
    external_account_id TEXT NOT NULL CHECK (length(external_account_id) BETWEEN 1 AND 160),
    display_name TEXT NOT NULL CHECK (length(display_name) BETWEEN 1 AND 160),
    connection_type TEXT NOT NULL CHECK (connection_type IN ('oauth', 'app_credentials', 'private_api', 'file_import')),
    credential_ref TEXT CHECK (credential_ref IS NULL OR length(credential_ref) BETWEEN 1 AND 512),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended', 'revoked')),
    settlement_owner_account_id UUID REFERENCES accounts(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(platform, external_account_id)
);

-- A connection is historical identity and must never disappear while imported facts reference it.
CREATE TRIGGER keep_platform_connections
BEFORE DELETE ON platform_connections
FOR EACH ROW EXECUTE FUNCTION deny_mutation();

CREATE TABLE platform_sync_checkpoints (
    connection_id UUID NOT NULL REFERENCES platform_connections(id),
    stream TEXT NOT NULL CHECK (length(stream) BETWEEN 1 AND 96),
    cursor TEXT,
    window_start TIMESTAMPTZ,
    window_end TIMESTAMPTZ,
    last_platform_updated_at TIMESTAMPTZ,
    last_attempt_at TIMESTAMPTZ,
    last_success_at TIMESTAMPTZ,
    last_error TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(connection_id, stream),
    CHECK (window_end IS NULL OR window_start IS NOT NULL),
    CHECK (window_end IS NULL OR window_end >= window_start)
);

CREATE TABLE platform_raw_events (
    id UUID PRIMARY KEY,
    connection_id UUID NOT NULL REFERENCES platform_connections(id),
    stream TEXT NOT NULL CHECK (length(stream) BETWEEN 1 AND 96),
    external_event_id TEXT CHECK (external_event_id IS NULL OR length(external_event_id) BETWEEN 1 AND 256),
    event_type TEXT NOT NULL CHECK (length(event_type) BETWEEN 1 AND 128),
    payload JSONB NOT NULL,
    payload_hash TEXT NOT NULL CHECK (length(payload_hash) = 64),
    platform_created_at TIMESTAMPTZ,
    platform_updated_at TIMESTAMPTZ,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    schema_version INTEGER NOT NULL DEFAULT 1 CHECK (schema_version > 0),
    processing_status TEXT NOT NULL DEFAULT 'pending' CHECK (processing_status IN ('pending', 'normalized', 'rejected')),
    normalizer_version INTEGER CHECK (normalizer_version IS NULL OR normalizer_version > 0),
    error_code TEXT,
    error_detail TEXT,
    CHECK (platform_updated_at IS NULL OR platform_created_at IS NULL OR platform_updated_at >= platform_created_at),
    CHECK (processing_status <> 'normalized' OR normalizer_version IS NOT NULL),
    CHECK (processing_status <> 'rejected' OR error_code IS NOT NULL),
    UNIQUE(id, connection_id)
);
CREATE UNIQUE INDEX raw_event_external_id
    ON platform_raw_events(connection_id, stream, external_event_id)
    WHERE external_event_id IS NOT NULL;
CREATE UNIQUE INDEX raw_event_payload_fallback
    ON platform_raw_events(connection_id, stream, event_type, payload_hash)
    WHERE external_event_id IS NULL;
CREATE INDEX raw_event_processing
    ON platform_raw_events(processing_status, received_at, id);
CREATE INDEX raw_event_platform_time
    ON platform_raw_events(connection_id, stream, platform_updated_at, id);

-- Raw identity/payload are immutable. Only normalization outcome metadata may advance.
CREATE FUNCTION protect_platform_raw_event() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF (to_jsonb(NEW)
        - 'processing_status'
        - 'normalizer_version'
        - 'error_code'
        - 'error_detail')
       IS DISTINCT FROM
       (to_jsonb(OLD)
        - 'processing_status'
        - 'normalizer_version'
        - 'error_code'
        - 'error_detail')
    THEN
        RAISE EXCEPTION 'raw platform event identity and payload are immutable' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER immutable_platform_raw_payload
BEFORE UPDATE ON platform_raw_events
FOR EACH ROW EXECUTE FUNCTION protect_platform_raw_event();
CREATE TRIGGER keep_platform_raw_events
BEFORE DELETE ON platform_raw_events
FOR EACH ROW EXECUTE FUNCTION deny_mutation();

CREATE TABLE platform_promotion_positions (
    id UUID PRIMARY KEY,
    connection_id UUID NOT NULL REFERENCES platform_connections(id),
    external_position_id TEXT,
    external_pid TEXT,
    external_publisher_id TEXT,
    external_site_id TEXT,
    internal_account_id UUID REFERENCES accounts(id),
    internal_channel_ref TEXT,
    valid_from TIMESTAMPTZ NOT NULL DEFAULT now(),
    valid_until TIMESTAMPTZ,
    metadata JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (
        (external_position_id IS NOT NULL AND length(external_position_id) BETWEEN 1 AND 256)
        OR (external_pid IS NOT NULL AND length(external_pid) BETWEEN 1 AND 256)
    ),
    CHECK (valid_until IS NULL OR valid_until > valid_from)
);
CREATE UNIQUE INDEX promotion_position_external_id
    ON platform_promotion_positions(connection_id, external_position_id, valid_from)
    WHERE external_position_id IS NOT NULL;
CREATE UNIQUE INDEX promotion_position_pid
    ON platform_promotion_positions(connection_id, external_pid, valid_from)
    WHERE external_pid IS NOT NULL;
CREATE INDEX promotion_position_internal
    ON platform_promotion_positions(internal_account_id, valid_from)
    WHERE internal_account_id IS NOT NULL;
CREATE TRIGGER keep_platform_promotion_positions
BEFORE DELETE ON platform_promotion_positions
FOR EACH ROW EXECUTE FUNCTION deny_mutation();

CREATE TABLE external_order_observations (
    id UUID PRIMARY KEY,
    connection_id UUID NOT NULL REFERENCES platform_connections(id),
    raw_event_id UUID NOT NULL,

    external_parent_order_id TEXT,
    external_order_line_id TEXT NOT NULL CHECK (length(external_order_line_id) BETWEEN 1 AND 256),
    external_product_id TEXT,
    external_promoter_id TEXT,
    external_position_id TEXT,
    merchant_ref TEXT,
    customer_ref TEXT,
    currency TEXT NOT NULL CHECK (length(currency) BETWEEN 3 AND 8),
    paid_minor BIGINT CHECK (paid_minor IS NULL OR paid_minor >= 0),
    settlement_base_minor BIGINT CHECK (settlement_base_minor IS NULL OR settlement_base_minor >= 0),
    raw_status TEXT NOT NULL CHECK (length(raw_status) BETWEEN 1 AND 128),
    paid_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    source_updated_at TIMESTAMPTZ NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    normalizer_version INTEGER NOT NULL CHECK (normalizer_version > 0),
    observation_hash TEXT NOT NULL CHECK (length(observation_hash) = 64),
    normalized_payload JSONB NOT NULL DEFAULT '{}',
    CHECK (completed_at IS NULL OR paid_at IS NULL OR completed_at >= paid_at),
    UNIQUE(raw_event_id, observation_hash),
    UNIQUE(connection_id, external_order_line_id, id),
    FOREIGN KEY(raw_event_id, connection_id)
        REFERENCES platform_raw_events(id, connection_id)
);
CREATE INDEX external_order_observation_key
    ON external_order_observations(connection_id, external_order_line_id, source_updated_at DESC, id DESC);
CREATE TRIGGER immutable_external_order_observations
BEFORE UPDATE OR DELETE ON external_order_observations
FOR EACH ROW EXECUTE FUNCTION deny_mutation();

CREATE TABLE external_orders (
    connection_id UUID NOT NULL REFERENCES platform_connections(id),
    external_order_line_id TEXT NOT NULL CHECK (length(external_order_line_id) BETWEEN 1 AND 256),
    latest_observation_id UUID NOT NULL REFERENCES external_order_observations(id),
    external_parent_order_id TEXT,
    normalized_status TEXT NOT NULL CHECK (normalized_status IN ('unknown', 'paid', 'completed', 'closed', 'refunded')),
    raw_status TEXT NOT NULL,
    currency TEXT NOT NULL CHECK (length(currency) BETWEEN 3 AND 8),
    paid_minor BIGINT CHECK (paid_minor IS NULL OR paid_minor >= 0),
    settlement_base_minor BIGINT CHECK (settlement_base_minor IS NULL OR settlement_base_minor >= 0),
    paid_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    last_platform_updated_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(connection_id, external_order_line_id),
    FOREIGN KEY(connection_id, external_order_line_id, latest_observation_id)
        REFERENCES external_order_observations(connection_id, external_order_line_id, id)
);
CREATE UNIQUE INDEX external_orders_latest_observation
    ON external_orders(latest_observation_id);

CREATE TABLE external_commission_observations (
    id UUID PRIMARY KEY,
    connection_id UUID NOT NULL REFERENCES platform_connections(id),
    raw_event_id UUID NOT NULL,
    external_commission_key TEXT NOT NULL CHECK (length(external_commission_key) BETWEEN 1 AND 320),
    external_order_line_id TEXT NOT NULL CHECK (length(external_order_line_id) BETWEEN 1 AND 256),
    external_beneficiary_id TEXT,
    beneficiary_role TEXT NOT NULL CHECK (length(beneficiary_role) BETWEEN 1 AND 96),
    phase TEXT NOT NULL CHECK (phase IN ('estimated', 'accrued', 'settled', 'reversed', 'invalid')),
    funding_phase TEXT NOT NULL DEFAULT 'unfunded' CHECK (funding_phase IN ('unfunded', 'receivable', 'funded', 'reversed')),
    currency TEXT NOT NULL CHECK (length(currency) BETWEEN 3 AND 8),
    gross_minor BIGINT,
    platform_service_fee_minor BIGINT,
    special_service_fee_minor BIGINT,
    institution_share_minor BIGINT,
    net_minor BIGINT,
    raw_status TEXT NOT NULL CHECK (length(raw_status) BETWEEN 1 AND 128),
    source_updated_at TIMESTAMPTZ NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    normalizer_version INTEGER NOT NULL CHECK (normalizer_version > 0),
    observation_hash TEXT NOT NULL CHECK (length(observation_hash) = 64),
    metadata JSONB NOT NULL DEFAULT '{}',
    UNIQUE(raw_event_id, observation_hash),
    UNIQUE(connection_id, external_commission_key, id),
    FOREIGN KEY(raw_event_id, connection_id)
        REFERENCES platform_raw_events(id, connection_id)
);
CREATE INDEX external_commission_observation_key
    ON external_commission_observations(connection_id, external_commission_key, source_updated_at DESC, id DESC);
CREATE INDEX external_commission_order
    ON external_commission_observations(connection_id, external_order_line_id, source_updated_at DESC);
CREATE TRIGGER immutable_external_commission_observations
BEFORE UPDATE OR DELETE ON external_commission_observations
FOR EACH ROW EXECUTE FUNCTION deny_mutation();

CREATE TABLE external_commissions (
    connection_id UUID NOT NULL REFERENCES platform_connections(id),
    external_commission_key TEXT NOT NULL CHECK (length(external_commission_key) BETWEEN 1 AND 320),
    latest_observation_id UUID NOT NULL REFERENCES external_commission_observations(id),
    external_order_line_id TEXT NOT NULL,
    beneficiary_role TEXT NOT NULL,
    beneficiary_ref TEXT,
    phase TEXT NOT NULL CHECK (phase IN ('estimated', 'accrued', 'settled', 'reversed', 'invalid')),
    funding_phase TEXT NOT NULL CHECK (funding_phase IN ('unfunded', 'receivable', 'funded', 'reversed')),
    currency TEXT NOT NULL CHECK (length(currency) BETWEEN 3 AND 8),
    gross_minor BIGINT,
    platform_service_fee_minor BIGINT,
    special_service_fee_minor BIGINT,
    institution_share_minor BIGINT,
    net_minor BIGINT,
    raw_status TEXT NOT NULL,
    last_platform_updated_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(connection_id, external_commission_key),
    FOREIGN KEY(connection_id, external_commission_key, latest_observation_id)
        REFERENCES external_commission_observations(connection_id, external_commission_key, id)
);
CREATE UNIQUE INDEX external_commissions_latest_observation
    ON external_commissions(latest_observation_id);
CREATE INDEX external_commissions_order
    ON external_commissions(connection_id, external_order_line_id);

CREATE TABLE external_refund_observations (
    id UUID PRIMARY KEY,
    connection_id UUID NOT NULL REFERENCES platform_connections(id),
    raw_event_id UUID NOT NULL,
    external_refund_id TEXT NOT NULL CHECK (length(external_refund_id) BETWEEN 1 AND 256),
    external_order_line_id TEXT NOT NULL CHECK (length(external_order_line_id) BETWEEN 1 AND 256),
    refund_status TEXT NOT NULL CHECK (length(refund_status) BETWEEN 1 AND 128),
    currency TEXT NOT NULL CHECK (length(currency) BETWEEN 3 AND 8),
    refund_minor BIGINT CHECK (refund_minor IS NULL OR refund_minor >= 0),
    commission_reversal_minor BIGINT CHECK (commission_reversal_minor IS NULL OR commission_reversal_minor >= 0),
    occurred_at TIMESTAMPTZ,
    source_updated_at TIMESTAMPTZ NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    normalizer_version INTEGER NOT NULL CHECK (normalizer_version > 0),
    observation_hash TEXT NOT NULL CHECK (length(observation_hash) = 64),
    metadata JSONB NOT NULL DEFAULT '{}',
    UNIQUE(raw_event_id, observation_hash),
    FOREIGN KEY(raw_event_id, connection_id)
        REFERENCES platform_raw_events(id, connection_id)
);
CREATE INDEX external_refund_observation_key
    ON external_refund_observations(connection_id, external_refund_id, source_updated_at DESC, id DESC);
CREATE INDEX external_refund_order
    ON external_refund_observations(connection_id, external_order_line_id, source_updated_at DESC);
CREATE TRIGGER immutable_external_refund_observations
BEFORE UPDATE OR DELETE ON external_refund_observations
FOR EACH ROW EXECUTE FUNCTION deny_mutation();

CREATE TABLE external_settlement_observations (
    id UUID PRIMARY KEY,
    connection_id UUID NOT NULL REFERENCES platform_connections(id),
    raw_event_id UUID NOT NULL,
    external_settlement_id TEXT NOT NULL CHECK (length(external_settlement_id) BETWEEN 1 AND 256),
    external_commission_key TEXT,
    external_order_line_id TEXT,
    currency TEXT NOT NULL CHECK (length(currency) BETWEEN 3 AND 8),
    gross_minor BIGINT,
    fee_minor BIGINT,
    net_minor BIGINT,
    settlement_status TEXT NOT NULL CHECK (length(settlement_status) BETWEEN 1 AND 128),
    settled_at TIMESTAMPTZ,
    funded_at TIMESTAMPTZ,
    statement_period TEXT,
    provider_reference TEXT,
    source_updated_at TIMESTAMPTZ NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    normalizer_version INTEGER NOT NULL CHECK (normalizer_version > 0),
    observation_hash TEXT NOT NULL CHECK (length(observation_hash) = 64),
    metadata JSONB NOT NULL DEFAULT '{}',
    CHECK (funded_at IS NULL OR settled_at IS NULL OR funded_at >= settled_at),
    CHECK (external_commission_key IS NOT NULL OR external_order_line_id IS NOT NULL),
    UNIQUE(raw_event_id, observation_hash),
    FOREIGN KEY(raw_event_id, connection_id)
        REFERENCES platform_raw_events(id, connection_id)
);
CREATE INDEX external_settlement_observation_key
    ON external_settlement_observations(connection_id, external_settlement_id, source_updated_at DESC, id DESC);
CREATE INDEX external_settlement_commission
    ON external_settlement_observations(connection_id, external_commission_key, source_updated_at DESC)
    WHERE external_commission_key IS NOT NULL;
CREATE TRIGGER immutable_external_settlement_observations
BEFORE UPDATE OR DELETE ON external_settlement_observations
FOR EACH ROW EXECUTE FUNCTION deny_mutation();

-- Observations are append-only history; current projections are intentionally mutable/rebuildable.

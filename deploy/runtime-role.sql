-- Execute as the migration owner AFTER migrations. Example role creation:
-- CREATE ROLE commission_runtime LOGIN PASSWORD 'supply-via-your-secret-manager';
-- GRANT CONNECT ON DATABASE commission TO commission_runtime;
GRANT USAGE ON SCHEMA public TO commission_runtime;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO commission_runtime;
GRANT INSERT, UPDATE ON accounts, credentials, rules, orders, allocations, payouts, idempotency, outbox TO commission_runtime;
GRANT INSERT ON referral_bindings, refunds, journals, ledger_entries, audit_events TO commission_runtime;
GRANT INSERT, UPDATE ON wallets TO commission_runtime;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO commission_runtime;
-- Wallet UPDATE is needed by the invoker-rights ledger projection trigger.
-- No DELETE, TRUNCATE, CREATE, table ownership, or superuser capability is granted.
-- This does NOT defend against a compromised runtime able to issue arbitrary SQL;
-- audit/reconciliation and a separately hardened DB are still necessary.

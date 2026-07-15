-- accountingd migration 0006: open-item management + anonymization support
--
-- Two additions:
--
-- 1. `accounts.anonymized_at` — GDPR Art. 17 pseudonymization timestamp.
--    When set, all PII (IBAN, name, mandate refs) in this account has been replaced
--    with anonymized placeholders while financial records are preserved (§238 HGB).
--
-- 2. `anonymization_log` — immutable audit trail for GDPR erasure events.
--    Required by GDPR Art. 5(2) accountability principle: the controller must be
--    able to demonstrate compliance with Art. 17. Each anonymization is logged
--    with a timestamp, operator identity, and legal basis.
--
-- Regulatory:
--   GDPR Art. 17 — Right to erasure (Recht auf Löschung)
--   GDPR Art. 5(2) — Accountability
--   §238 HGB — Buchführungspflicht (financial records: 10 years)
--   §147 AO — Aufbewahrungsfristen (tax records: 6–10 years)
--
-- Design rationale:
--   Ledger entries (amounts, dates, entry_type) are retained — they contain no PII.
--   PII fields (IBAN, kontoinhaber, mandatsref) are anonymized in-place.
--   malo_id is a location pseudonym, not personal data per BDEW definition.

-- 1. Mark accounts as anonymized (preserves financial records, removes PII)
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS anonymized_at TIMESTAMPTZ;

COMMENT ON COLUMN accounts.anonymized_at IS
    'Set when GDPR Art. 17 erasure was applied. PII (IBAN, mandatsref, '
    'zahlungsinformation) has been replaced with anonymized placeholders. '
    'Financial records (ledger_entries) are preserved per §238 HGB.';

-- 2. Immutable anonymization audit log
CREATE TABLE IF NOT EXISTS anonymization_log (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id      UUID        NOT NULL,
    tenant          TEXT        NOT NULL,
    malo_id         TEXT        NOT NULL,
    -- Who requested the anonymization (operator/system identity)
    requested_by    TEXT        NOT NULL,
    -- Legal basis for erasure (e.g. "GDPR Art. 17 - customer request")
    legal_basis     TEXT        NOT NULL,
    -- Fields actually anonymized (JSON array of column names)
    anonymized_fields JSONB     NOT NULL DEFAULT '[]',
    anonymized_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS anon_log_account ON anonymization_log (account_id);
CREATE INDEX IF NOT EXISTS anon_log_tenant  ON anonymization_log (tenant);

COMMENT ON TABLE anonymization_log IS
    'Immutable audit trail for GDPR Art. 17 pseudonymization events. '
    'Required by GDPR Art. 5(2) accountability principle.';

-- 3. Auto-dunning run log for idempotency and audit
-- Tracks when automatic dunning was last run per tenant so background workers
-- can detect if they've already processed a given calendar day.
CREATE TABLE IF NOT EXISTS auto_dunning_runs (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant          TEXT        NOT NULL,
    run_date        DATE        NOT NULL,
    accounts_checked   INTEGER NOT NULL DEFAULT 0,
    dunning_created    INTEGER NOT NULL DEFAULT 0,
    dunning_escalated  INTEGER NOT NULL DEFAULT 0,
    run_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant, run_date)
);

COMMENT ON TABLE auto_dunning_runs IS
    'Idempotency guard and audit trail for automatic Mahnwesen escalation. '
    'One row per (tenant, calendar day) — prevents double-dunning from background worker restarts.';

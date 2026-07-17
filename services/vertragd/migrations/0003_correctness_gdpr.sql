-- vertragd migration 0003: correctness + GDPR Art. 17 anonymization
--
-- This migration fixes two issues and adds mandatory GDPR Art. 17 support:
--
-- 1. **C1 — upsert_kunde ON CONFLICT fix**
--    `pg.rs::upsert_kunde` uses `ON CONFLICT (tenant, erp_kunde_id)` but the
--    `kunden` table only has a non-unique partial index on those columns.
--    PostgreSQL requires a UNIQUE constraint or unique index for ON CONFLICT.
--    This migration adds the missing unique partial index so the idempotency
--    guarantee for `erp_kunde_id`-keyed upserts works at runtime.
--
-- 2. **H1 — GDPR Art. 17 anonymization tracking**
--    The `anonymization_log` table records every GDPR erasure request with
--    an immutable audit trail (INSERT-only, no UPDATE/DELETE).  Required to
--    prove erasure per GDPR Art. 5(2) accountability principle.
--
-- 3. **M1 — Preisgarantie override audit trail**
--    Adds `preisgarantie_override_log` to record every bypass of the price-
--    lock guard (`override_preisgarantie = true`) with operator identity and
--    timestamp.  Required for contractual/regulatory audit purposes.

-- ── C1: unique partial index for upsert_kunde ON CONFLICT ────────────────────

CREATE UNIQUE INDEX IF NOT EXISTS kunden_erp_unique
    ON kunden (tenant, erp_kunde_id)
    WHERE erp_kunde_id IS NOT NULL;

COMMENT ON INDEX kunden_erp_unique IS
    'Unique partial index enabling ON CONFLICT (tenant, erp_kunde_id) DO UPDATE '
    'in upsert_kunde. NULL erp_kunde_id rows are excluded (NULL != NULL in SQL '
    'UNIQUE semantics); duplicate kunden without erp_kunde_id are allowed.';

-- ── H1: GDPR Art. 17 — anonymization request log (immutable) ─────────────────

CREATE TABLE IF NOT EXISTS anonymization_log (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant          TEXT        NOT NULL,
    kunden_id       UUID        NOT NULL,     -- does NOT reference kunden (FK would prevent deletion)
    -- What was anonymized
    anonymized_fields   TEXT[]  NOT NULL,     -- e.g. ['geschaeftspartner', 'person', 'zahlungsinformation', 'oidc_sub']
    -- Who did it and why
    requested_by    TEXT        NOT NULL,     -- OIDC sub or operator ID of the requestor
    request_reason  TEXT,                    -- GDPR Art. 17 erasure reason (optional, for audit)
    retention_basis TEXT,                    -- legal basis for retaining non-PII (e.g. '§147 AO 10 years')
    -- When
    anonymized_at   TIMESTAMPTZ NOT NULL DEFAULT now()
    -- No updated_at — this table is INSERT-only (immutable audit log)
);

-- Index for auditors: "show all anonymizations for this customer"
CREATE INDEX IF NOT EXISTS anon_log_kunde
    ON anonymization_log (kunden_id);

-- Index for compliance reports: "show all anonymizations in time range"
CREATE INDEX IF NOT EXISTS anon_log_tenant_time
    ON anonymization_log (tenant, anonymized_at DESC);

COMMENT ON TABLE anonymization_log IS
    'Immutable audit log for GDPR Art. 17 (right to erasure) anonymization requests. '
    'INSERT-only: rows MUST NOT be updated or deleted. '
    'Proves compliance per GDPR Art. 5(2) accountability principle.';

-- ── M1: Preisgarantie override audit trail ────────────────────────────────────

CREATE TABLE IF NOT EXISTS preisgarantie_override_log (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant          TEXT        NOT NULL,
    vertrag_id      UUID        NOT NULL,
    komp_id         UUID        NOT NULL,
    -- What was bypassed
    preisgarantie_bis   DATE    NOT NULL,    -- the guarantee date that was overridden
    wirksamkeit         DATE    NOT NULL,    -- the Tarifwechsel effective date
    old_product_code    TEXT    NOT NULL,
    new_product_code    TEXT    NOT NULL,
    -- Who did it
    operator_identity   TEXT    NOT NULL,   -- OIDC sub or API-key name of the authorizing operator
    override_reason     TEXT,               -- optional reason for the override
    -- When
    overridden_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS pg_override_vertrag
    ON preisgarantie_override_log (vertrag_id);

CREATE INDEX IF NOT EXISTS pg_override_tenant_time
    ON preisgarantie_override_log (tenant, overridden_at DESC);

COMMENT ON TABLE preisgarantie_override_log IS
    'Audit trail for Preisgarantie bypass (override_preisgarantie = true). '
    'Required for contractual compliance: every override must be justifiable. '
    'INSERT-only — rows must not be modified after creation.';

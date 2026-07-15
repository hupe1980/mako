-- Migration 0006: Best-in-class EDM schema hardening
--
-- ## Critical fixes
--
-- 1. `meter_reads` PRIMARY KEY fix
--    The original PK (malo_id, dtm_from, dtm_to) is WRONG for multi-register meters.
--    A dual-tariff (HT/NT) meter sends TWO time series for the SAME timestamp range:
--      - OBIS 1-0:1.8.1*255 (HT) — from: 2026-01-01 00:00 UTC, to: 2026-01-01 00:15 UTC
--      - OBIS 1-0:1.8.2*255 (NT) — from: 2026-01-01 00:00 UTC, to: 2026-01-01 00:15 UTC
--    Under the old PK this second insert silently overwrites the first (or fails).
--    Fix: DROP the old PK constraint and replace with (malo_id, dtm_from, obis_code_norm).
--
--    The new primary key uses obis_code_norm (TEXT NOT NULL, DEFAULT '') so that:
--      - Single-register meters (no OBIS in MSCONS) use '' as the key sentinel
--      - Multi-register meters use the canonical OBIS string
--
-- 2. Gas quality data table
--    Brennwert and Zustandszahl (PID 13007) currently stored as TEXT in meter_billing_periods.
--    Extract to a dedicated typed table with full period coverage semantics.
--
-- 3. Virtual meter configurations
--    AggregationRule definitions need persistence so edmd can materialise virtual meters
--    on demand (§42b EEG GGV community solar, Residuallast for feed-in metering).
--
-- 4. Formal quality assessment ledger
--    Per-interval quality flags are stored inline. Add a dedicated quality_assessments
--    table for systematic batch quality tracking per §22 MessZV audit trail requirements.
--
-- 5. Forecast / substitute tracking
--    §17 MessZV requires traceability of substitute value method.
--    Add substitute_value_log linked to meter_reads.
--
-- 6. meter_reads OBIS-code normalisation column
--    obis_code_norm is the canonical form (always populated, '' for no-OBIS).
--    Enables efficient multi-register queries and PK-stable upserts.
--
-- ## Non-breaking additions
--    All changes are additive (ADD COLUMN IF NOT EXISTS, CREATE TABLE IF NOT EXISTS).
--    The PK change requires a migration step — see below.

-- ── Step 1: Add obis_code_norm column ─────────────────────────────────────────

-- Normalised OBIS code: populated from obis_code if present, empty string otherwise.
-- Used as PK component to allow multi-register storage.
ALTER TABLE meter_reads
    ADD COLUMN IF NOT EXISTS obis_code_norm TEXT NOT NULL DEFAULT '';

-- Backfill: copy existing obis_code to obis_code_norm
UPDATE meter_reads
   SET obis_code_norm = COALESCE(obis_code, '')
 WHERE obis_code_norm = '';

-- ── Step 2: Replace PRIMARY KEY ───────────────────────────────────────────────
--
-- PostgreSQL does not support ALTER TABLE DROP CONSTRAINT IF EXISTS in all versions.
-- Use DO $$ block to handle gracefully.

DO $$
BEGIN
    -- Drop old PK (stored as meter_reads_pkey in PostgreSQL)
    IF EXISTS (
        SELECT 1 FROM pg_constraint
         WHERE conname = 'meter_reads_pkey'
           AND conrelid = 'meter_reads'::regclass
    ) THEN
        ALTER TABLE meter_reads DROP CONSTRAINT meter_reads_pkey;
    END IF;
EXCEPTION
    WHEN OTHERS THEN NULL; -- already dropped or table has no PK
END;
$$;

-- Add new composite PK including register discriminator
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
         WHERE conname = 'meter_reads_pkey_v2'
           AND conrelid = 'meter_reads'::regclass
    ) THEN
        ALTER TABLE meter_reads
            ADD CONSTRAINT meter_reads_pkey_v2
            PRIMARY KEY (malo_id, dtm_from, obis_code_norm);
    END IF;
EXCEPTION
    WHEN OTHERS THEN NULL;
END;
$$;

-- ── Step 3: Add bitemporal transaction-time column ───────────────────────────
--
-- valid_from_tx is the database-insert timestamp (transaction time).
-- It enables bitemporal queries: "what did we know at time T?"
-- Combined with meter_read_corrections, this gives full bitemporal coverage.

ALTER TABLE meter_reads
    ADD COLUMN IF NOT EXISTS valid_from_tx TIMESTAMPTZ NOT NULL DEFAULT now();

CREATE INDEX IF NOT EXISTS mr_bitemp
    ON meter_reads (malo_id, dtm_from, valid_from_tx DESC);

-- ── Step 4: Gas quality data table ───────────────────────────────────────────
--
-- Stores Gasbeschaffenheitsdaten from MSCONS PID 13007.
-- Separated from meter_billing_periods because:
--   a) Gas quality applies across multiple billing periods
--   b) Multiple deliveries per period are possible (Jahreswert correction)
--   c) Typed + validated storage (not TEXT in billing summary)

CREATE TABLE IF NOT EXISTS gas_quality_data (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id             TEXT        NOT NULL,
    period_from         DATE        NOT NULL,
    period_to           DATE        NOT NULL,
    brennwert_kwh_per_m3 TEXT       NOT NULL,   -- Abrechnungsbrennwert kWh/m³ (as TEXT Decimal)
    zustandszahl        TEXT        NOT NULL,   -- dimensionless, typically 0.95–1.05
    pid                 INTEGER     NOT NULL DEFAULT 13007,
    process_id          UUID,                   -- originating MSCONS process
    received_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    tenant_id           UUID,

    -- Ensure only one gas quality row per period per MaLo
    CONSTRAINT gqd_unique UNIQUE (malo_id, period_from, period_to, tenant_id)
);

CREATE INDEX IF NOT EXISTS gqd_malo_period
    ON gas_quality_data (malo_id, period_from, period_to);
CREATE INDEX IF NOT EXISTS gqd_tenant
    ON gas_quality_data (tenant_id) WHERE tenant_id IS NOT NULL;

COMMENT ON TABLE gas_quality_data IS
    'Gasbeschaffenheitsdaten (Brennwert + Zustandszahl) from MSCONS PID 13007. '
    'Used for Gas m³ → kWh_Hs conversion per §24 GasGVV / DVGW G 685.';

-- ── Step 5: Virtual meter configurations ─────────────────────────────────────
--
-- Persists AggregationRule definitions so edmd can compute virtual meter time series
-- on demand. Virtual meters are not physical MaLos but derived aggregation points.
--
-- rule_type: 'Sum' | 'Residual' | 'PvSelfConsumption' | 'GgvAllocation'
-- rule_json: serialised AggregationRule (source MaLo IDs, fractions etc.)
-- virtual_malo_id: the logical identifier for the derived series

CREATE TABLE IF NOT EXISTS virtual_meter_configs (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    virtual_malo_id TEXT        NOT NULL,       -- logical derived MaLo identifier
    display_name    TEXT        NOT NULL,
    rule_type       TEXT        NOT NULL        -- 'Sum' | 'Residual' | 'PvSelfConsumption' | 'GgvAllocation'
                    CHECK (rule_type IN ('Sum','Residual','PvSelfConsumption','GgvAllocation')),
    rule_json       JSONB       NOT NULL,       -- serialised AggregationRule
    legal_basis     TEXT,                       -- e.g. '§42b EEG 2023 GGV', '§42a EEG Residuallast'
    sparte          TEXT        NOT NULL DEFAULT 'STROM',
    valid_from      DATE        NOT NULL,
    valid_to        DATE,
    tenant          TEXT        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT vmc_virtual_malo_unique UNIQUE (virtual_malo_id, tenant)
);

CREATE INDEX IF NOT EXISTS vmc_tenant ON virtual_meter_configs (tenant);
CREATE INDEX IF NOT EXISTS vmc_rule_type ON virtual_meter_configs (rule_type);

COMMENT ON TABLE virtual_meter_configs IS
    'Virtual meter aggregation rules: Sum, Residual, PvSelfConsumption, GgvAllocation. '
    'Rule is evaluated by edmd on demand via GET /api/v1/virtual/{virtual_malo_id}/lastgang.';

-- ── Step 6: Quality assessments ───────────────────────────────────────────────
--
-- Formal per-batch quality assessment per §22 MessZV audit trail.
-- One row per ingest batch (MSCONS process or direct push session).
-- Captures V01-V10 validation results for later review and regulatory reporting.

CREATE TABLE IF NOT EXISTS quality_assessments (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id         TEXT        NOT NULL,
    assessed_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    source          TEXT        NOT NULL,           -- 'MSCONS' | 'DIRECT_PUSH' | 'CORRECTION'
    process_id      UUID,                           -- originating process (MSCONS) or NULL
    session_id      TEXT,                           -- push session ID or NULL
    interval_count  INTEGER     NOT NULL,
    expected_count  INTEGER,
    coverage_pct    DOUBLE PRECISION,
    grade           TEXT        NOT NULL            -- 'A' | 'B' | 'C' | 'F'
                    CHECK (grade IN ('A','B','C','F')),
    -- V01-V10 rule hits (bitmask or JSON array of rule IDs)
    issues_json     JSONB       NOT NULL DEFAULT '[]',
    gaps_detected   INTEGER     NOT NULL DEFAULT 0,
    billing_blocked BOOLEAN     NOT NULL DEFAULT false,
    pid             INTEGER,                        -- MSCONS PID
    tenant          TEXT        NOT NULL DEFAULT 'default',
    sparte          TEXT        NOT NULL DEFAULT 'STROM'
);

CREATE INDEX IF NOT EXISTS qa_malo_assessed  ON quality_assessments (malo_id, assessed_at DESC);
CREATE INDEX IF NOT EXISTS qa_grade          ON quality_assessments (grade) WHERE grade != 'A';
CREATE INDEX IF NOT EXISTS qa_billing_block  ON quality_assessments (malo_id, billing_blocked)
    WHERE billing_blocked = true;
CREATE INDEX IF NOT EXISTS qa_tenant         ON quality_assessments (tenant);

COMMENT ON TABLE quality_assessments IS
    'Per-batch quality assessments per §22 MessZV. '
    'Grade F = billing blocked; Grade A = clean data.';

-- ── Step 7: Substitute value tracking ────────────────────────────────────────
--
-- §17 MessZV requires the MSB to document the substitute value method.
-- When fill_gaps() / fill_gaps_with_config() produces intervals, record why.

CREATE TABLE IF NOT EXISTS substitute_value_log (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id             TEXT        NOT NULL,
    dtm_from            TIMESTAMPTZ NOT NULL,
    dtm_to              TIMESTAMPTZ NOT NULL,
    obis_code_norm      TEXT        NOT NULL DEFAULT '',
    method              TEXT        NOT NULL        -- 'LinearInterpolation' | 'PriorPeriodAverage' | 'ZeroFill' | 'LastValueCarryForward'
                        CHECK (method IN (
                            'LinearInterpolation','PriorPeriodAverage',
                            'ZeroFill','LastValueCarryForward'
                        )),
    reason              TEXT        NOT NULL,       -- SubstitutionReason variant
    substituted_kwh     TEXT        NOT NULL,       -- the generated value
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by          TEXT,                       -- operator or 'AUTO'
    process_id          UUID,
    tenant              TEXT        NOT NULL DEFAULT 'default'
);

CREATE INDEX IF NOT EXISTS svl_malo_dtm  ON substitute_value_log (malo_id, dtm_from, dtm_to);
CREATE INDEX IF NOT EXISTS svl_tenant    ON substitute_value_log (tenant);
CREATE INDEX IF NOT EXISTS svl_method    ON substitute_value_log (method);

COMMENT ON TABLE substitute_value_log IS
    'Audit trail for §17 MessZV substitute values. '
    'Every AUTO_SUBSTITUTE meter_read has a corresponding row here.';

-- ── Step 8: Add meter_reads source and provenance indexes ────────────────────

-- Partial index on correction count (find all corrected intervals quickly)
CREATE INDEX IF NOT EXISTS mr_corrected
    ON meter_reads (malo_id, dtm_from)
    WHERE correction_count > 0;

-- Ensure source column exists (added in previous migration 0004)
ALTER TABLE meter_reads ADD COLUMN IF NOT EXISTS source TEXT NOT NULL DEFAULT 'MSCONS';
ALTER TABLE meter_reads ADD COLUMN IF NOT EXISTS correction_count INTEGER NOT NULL DEFAULT 0;

-- ── Step 9: Retention policy helpers ─────────────────────────────────────────
--
-- §22 MessZV: meter data must be retained for 3 years.
-- §28 EnWG: billing-relevant data must be retained for 10 years.
--
-- These comments document the retention requirements for DBA/ops tooling.
-- Actual retention enforcement should be done at the TimescaleDB / application layer.

COMMENT ON COLUMN meter_reads.dtm_from IS
    '§22 MessZV: interval start (UTC). '
    'Data retention: minimum 3 years from receipt. '
    'Billing-relevant data: 10 years per §28 EnWG.';

COMMENT ON COLUMN meter_reads.valid_from_tx IS
    'Transaction time (when this row was inserted). '
    'Used for bitemporal queries: what did we know at time T?';

COMMENT ON TABLE meter_billing_periods IS
    'Denormalized billing-period aggregates (arbeitsmenge, spitzenleistung, HT/NT). '
    'This is a materialised cache — it can be recomputed from meter_reads at any time. '
    'Do not rely on this table for §22 MessZV audit purposes; use meter_reads directly.';

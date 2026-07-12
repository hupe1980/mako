-- ── billingd schema ──────────────────────────────────────────────────────────
--
-- `billing_records`:
--   Immutable audit log of every generated invoice.  The full BO4E `Rechnung`
--   is stored as JSONB (`rechnung_json`) for §22 MessZV compliance (3-year
--   retention).  `outcome` tracks the invoice lifecycle through dispatch,
--   payment, and dispute resolution.
--
-- `billing_run_log`:
--   Tracks automated monthly batch runs (audit + idempotency).

CREATE TABLE IF NOT EXISTS billing_records (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id        TEXT        NOT NULL,
    lf_mp_id       TEXT        NOT NULL,
    product_code   TEXT        NOT NULL,
    category       TEXT        NOT NULL,           -- ENERGY | SERVICE | TELCO | BUNDLE
    period_from    DATE        NOT NULL,
    period_to      DATE        NOT NULL,
    rechnung_json  JSONB       NOT NULL,            -- rubo4e::current::Rechnung
    bo4e_version   TEXT        NOT NULL DEFAULT 'v202607.0.0',
    -- Monetary summary for fast reporting (avoids parsing JSONB).
    total_netto_eur  NUMERIC(16, 5),
    total_brutto_eur NUMERIC(16, 5),
    outcome        TEXT        NOT NULL DEFAULT 'generated'
                   CHECK (outcome IN (
                       'generated',   -- created, not yet dispatched
                       'dispatched',  -- sent to accountingd / ERP
                       'paid',        -- payment confirmed by accountingd
                       'partial',     -- partial payment
                       'disputed',    -- dispute raised
                       'cancelled'    -- cancelled before dispatch
                   )),
    -- CloudEvent ID of the emitted de.billing.rechnung.erstellt
    ce_id          UUID,
    dispatched_at  TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (malo_id, lf_mp_id, period_from, period_to, product_code)
);

CREATE INDEX IF NOT EXISTS br_malo_period  ON billing_records (malo_id, lf_mp_id, period_from DESC);
CREATE INDEX IF NOT EXISTS br_outcome      ON billing_records (outcome, lf_mp_id);
CREATE INDEX IF NOT EXISTS br_ce_pending   ON billing_records (lf_mp_id, created_at DESC)
    WHERE ce_id IS NULL AND outcome = 'generated';

-- ── Monthly batch run log ─────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS billing_run_log (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    lf_mp_id       TEXT        NOT NULL,
    billing_year   SMALLINT    NOT NULL,
    billing_month  SMALLINT    NOT NULL CHECK (billing_month BETWEEN 1 AND 12),
    run_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    records_count  INT         NOT NULL DEFAULT 0,
    errors_count   INT         NOT NULL DEFAULT 0,
    status         TEXT        NOT NULL DEFAULT 'completed'
                   CHECK (status IN ('running', 'completed', 'failed')),
    UNIQUE (lf_mp_id, billing_year, billing_month)
);

-- billingd migration 0002: Korrekturrechnung + Sammelrechnung support
--
-- L8: Add `is_correction` + `original_record_id` to billing_records so that
--     correction invoices (Stornorechnung + Korrektur) are linked to the
--     original record with a full audit trail (§22 MessZV requirement).
--
-- L2: Add `sammelrechnung_id` so that per-MaLo records can be grouped
--     into one consolidated B2B Sammelrechnung.  The Sammelrechnung itself
--     is also stored as a billing_record (malo_id = rahmenvertrag_id,
--     category = 'SAMMEL').

-- Drop the uniqueness constraint that would prevent storing a correction for
-- the same (malo, lf, period, product) tuple alongside the original.
ALTER TABLE billing_records
    DROP CONSTRAINT IF EXISTS billing_records_malo_id_lf_mp_id_period_from_period_to_produ_key;

-- Correction invoice link.
ALTER TABLE billing_records
    ADD COLUMN IF NOT EXISTS is_correction     BOOLEAN  NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS original_record_id UUID
        REFERENCES billing_records(id) ON DELETE SET NULL,
    ADD COLUMN IF NOT EXISTS correction_reason TEXT;

-- Sammelrechnung group link (NULL = standalone or not yet grouped).
ALTER TABLE billing_records
    ADD COLUMN IF NOT EXISTS sammelrechnung_id UUID
        REFERENCES billing_records(id) ON DELETE SET NULL;

-- Restore uniqueness — now scoped to original records only so corrections
-- can coexist with originals for the same period.
CREATE UNIQUE INDEX IF NOT EXISTS br_unique_original
    ON billing_records (malo_id, lf_mp_id, period_from, period_to, product_code)
    WHERE is_correction = FALSE AND sammelrechnung_id IS NULL;

-- Fast lookup: all correction records for a given original.
CREATE INDEX IF NOT EXISTS br_corrections
    ON billing_records (original_record_id)
    WHERE is_correction = TRUE;

-- Fast lookup: all per-MaLo records that belong to a Sammelrechnung.
CREATE INDEX IF NOT EXISTS br_sammel_group
    ON billing_records (sammelrechnung_id)
    WHERE sammelrechnung_id IS NOT NULL;

COMMENT ON COLUMN billing_records.is_correction IS
    'TRUE = Stornorechnung / Korrekturrechnung (rubo4e istOriginal=false). '
    'Positions are negated relative to original_record_id.';

COMMENT ON COLUMN billing_records.original_record_id IS
    'FK to the record being corrected/reversed. NULL for originals.';

COMMENT ON COLUMN billing_records.sammelrechnung_id IS
    'FK to the consolidated Sammelrechnung record (category=SAMMEL) for B2B '
    'Rahmenvertrag billing. NULL for standalone Einzelrechnungen.';

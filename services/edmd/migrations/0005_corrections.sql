-- edmd migration 0005: Bitemporal meter read corrections
--
-- Enables audit-compliant retroactive corrections per §17 MessZV.
--
-- ## Why bitemporal?
--
-- §22 MessZV requires a 3-year audit trail. When a meter read is corrected,
-- the ORIGINAL value must be preserved alongside the corrected value so that
-- auditors can reconstruct the billing basis at any point in time.
--
-- The `meter_read_corrections` table records WHAT was changed, WHEN it was
-- changed, and WHY — without ever mutating the original `meter_reads` row.
--
-- ## Correction model
--
-- 1. Original interval stored in `meter_reads`
-- 2. Correction event stored in `meter_read_corrections`
--    - Links back to original via `malo_id + dtm_from + dtm_to`
--    - Stores original value, new value, reason, operator, timestamp
-- 3. Corrected value also upserted into `meter_reads` (query layer reads latest)
-- 4. Audit queries join `meter_read_corrections` to reconstruct history

-- ── Correction records ────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS meter_read_corrections (
    correction_id       UUID        PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Which interval is being corrected
    malo_id             TEXT        NOT NULL,
    dtm_from            TIMESTAMPTZ NOT NULL,
    dtm_to              TIMESTAMPTZ NOT NULL,

    -- What the value was before the correction
    original_kwh        NUMERIC(18, 5) NOT NULL,
    original_quality    TEXT           NOT NULL,

    -- What the value is after the correction
    corrected_kwh       NUMERIC(18, 5) NOT NULL,
    corrected_quality   TEXT           NOT NULL,

    -- Why and by whom
    reason              TEXT           NOT NULL,  -- mandatory per §22 MessZV audit trail
    source              TEXT           NOT NULL,  -- e.g. 'MSB_CORRECTION', 'OPERATOR', 'MSCONS'
    corrected_by        TEXT,                     -- operator identity or system name
    corrected_at        TIMESTAMPTZ    NOT NULL DEFAULT now(),

    -- Reference to the MSCONS process that triggered the correction (if any)
    process_id          UUID,
    pid                 INTEGER,

    -- Tenant isolation
    tenant_id           UUID
);

-- Fast lookup of all corrections for a MaLo
CREATE INDEX IF NOT EXISTS mrc_malo_dtm
    ON meter_read_corrections (malo_id, dtm_from, dtm_to);

-- Chronological correction history per MaLo
CREATE INDEX IF NOT EXISTS mrc_malo_corrected_at
    ON meter_read_corrections (malo_id, corrected_at DESC);

-- Tenant filter
CREATE INDEX IF NOT EXISTS mrc_tenant
    ON meter_read_corrections (tenant_id)
    WHERE tenant_id IS NOT NULL;

-- ── Correction counter on meter_reads ────────────────────────────────────────
-- Track how many times an interval has been corrected (for anomaly detection)

ALTER TABLE meter_reads
    ADD COLUMN IF NOT EXISTS correction_count  INTEGER  NOT NULL DEFAULT 0;

-- Partial index: corrected intervals for monitoring
CREATE INDEX IF NOT EXISTS mr_corrected
    ON meter_reads (malo_id, dtm_from)
    WHERE correction_count > 0;

-- ── Meter exchange events ─────────────────────────────────────────────────────
-- Stores the exchange boundary for billing continuity across meter replacements.
-- Triggers a Sonderablesung (INSRPT PID 23003) per BDEW GPKE.

CREATE TABLE IF NOT EXISTS meter_exchange_events (
    exchange_id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    melo_id                 TEXT        NOT NULL,
    old_meter_serial        TEXT        NOT NULL,
    old_final_reading_kwh   NUMERIC(18, 5) NOT NULL,
    new_meter_serial        TEXT        NOT NULL,
    new_first_reading_kwh   NUMERIC(18, 5) NOT NULL,
    exchange_date           DATE        NOT NULL,
    exchange_at             TIMESTAMPTZ NOT NULL,
    triggered_by_pid        INTEGER,
    insrpt_process_id       TEXT,
    performed_by            TEXT,
    tenant_id               UUID
);

CREATE INDEX IF NOT EXISTS mee_melo_date
    ON meter_exchange_events (melo_id, exchange_date);

CREATE INDEX IF NOT EXISTS mee_tenant
    ON meter_exchange_events (tenant_id)
    WHERE tenant_id IS NOT NULL;

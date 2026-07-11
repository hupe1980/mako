-- 0001_initial_schema.sql
-- edmd: meter data receipts and typed reads.
--
-- meter_data_receipts: one row per received MSCONS process (process-level metadata).
-- meter_reads: typed kWh interval reads (populated when domain crates emit typed payloads).
--
-- All timestamps are TIMESTAMPTZ (UTC).

-- ── Meter data receipts ───────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS meter_data_receipts (
    process_id  UUID        PRIMARY KEY,
    pid         INTEGER     NOT NULL,
    malo_id     TEXT        NOT NULL,
    sender_mp_id  TEXT        NOT NULL,
    message_ref TEXT,
    received_at TIMESTAMPTZ NOT NULL,
    tenant_id   UUID,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS mdr_malo_received ON meter_data_receipts (malo_id, received_at DESC);
CREATE INDEX IF NOT EXISTS mdr_tenant        ON meter_data_receipts (tenant_id) WHERE tenant_id IS NOT NULL;

-- ── Typed meter reads ─────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS meter_reads (
    malo_id      TEXT        NOT NULL,
    melo_id      TEXT,
    dtm_from     TIMESTAMPTZ NOT NULL,
    dtm_to       TIMESTAMPTZ NOT NULL,
    quantity_kwh TEXT        NOT NULL,   -- stored as text to preserve decimal precision
    quality      TEXT        NOT NULL DEFAULT 'UNKNOWN',
    pid          INTEGER     NOT NULL,
    sparte       TEXT        NOT NULL DEFAULT 'STROM',
    obis_code    TEXT,                   -- OBIS-Kennzahl (e.g. "1-1:1.29.0"); NULL when not provided by MSCONS
    tenant_id    UUID,
    PRIMARY KEY (malo_id, dtm_from, dtm_to)
);

CREATE INDEX IF NOT EXISTS mr_malo_dtm    ON meter_reads (malo_id, dtm_from, dtm_to);
CREATE INDEX IF NOT EXISTS mr_tenant      ON meter_reads (tenant_id) WHERE tenant_id IS NOT NULL;

-- Optional: convert meter_reads to a TimescaleDB hypertable for time-series performance.
-- Run this manually after enabling the TimescaleDB extension:
--   SELECT create_hypertable('meter_reads', 'dtm_from', if_not_exists => TRUE);


-- ── Billing period aggregation ─────────────────────────────────────────────────
-- (merged from 0002_billing_period.sql — single schema file for fresh installs)

CREATE TABLE IF NOT EXISTS meter_billing_periods (
    malo_id              TEXT        NOT NULL,
    period_from          DATE        NOT NULL,
    period_to            DATE        NOT NULL,
    messtyp              TEXT        NOT NULL DEFAULT 'SLP',   -- 'SLP' | 'RLM' | 'IMSYS'
    sparte               TEXT        NOT NULL DEFAULT 'STROM', -- 'STROM' | 'GAS'
    arbeitsmenge_kwh     TEXT        NOT NULL,                 -- Decimal as text
    arbeitsmenge_ht_kwh  TEXT,
    arbeitsmenge_nt_kwh  TEXT,
    spitzenleistung_kw   TEXT,       -- RLM Strom only: max(qty_per_15min × 4)
    brennwert_kwh_per_m3 TEXT,       -- Gas only: Abrechnungsbrennwert kWh/m³
    zustandszahl         TEXT,       -- Gas only: compressibility factor (dimensionless)
    zaehlerstand_anfang  TEXT,
    zaehlerstand_ende    TEXT,
    quality              TEXT        NOT NULL DEFAULT 'UNKNOWN',
    tenant_id            UUID,
    computed_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (malo_id, period_from, period_to, tenant_id)
);

-- Lookup by tenant for billing system queries
CREATE INDEX IF NOT EXISTS mbp_tenant_malo
    ON meter_billing_periods (tenant_id, malo_id)
    WHERE tenant_id IS NOT NULL;

-- No tenant filter (development / single-tenant deployments)
CREATE INDEX IF NOT EXISTS mbp_malo_period
    ON meter_billing_periods (malo_id, period_from, period_to);

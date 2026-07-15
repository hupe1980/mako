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

-- TimescaleDB hypertable for meter_reads (RECOMMENDED for production).
--
-- Converting meter_reads to a TimescaleDB hypertable provides:
--   - 10-100× faster time-range queries (columnar chunks, per-chunk min/max indexes)
--   - Automatic chunk-based compression (reduces storage by 60-90%)
--   - Continuous aggregates for pre-computed billing summaries
--   - Data retention policies (automatically drop old hot-tier data after archival)
--
-- To enable: install TimescaleDB extension and run once:
--   SELECT create_hypertable('meter_reads', 'dtm_from',
--       chunk_time_interval => INTERVAL '7 days',
--       if_not_exists => TRUE);
--
-- For managed PostgreSQL (AWS RDS, Azure Flexible Server): use the
-- timescaledb_toolkit extension and verify replica support before enabling.
--
-- Production recommendation: enable this before the first data load.
-- Retrofitting a large existing table requires a lock and data copy.


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

-- 0002_archive_tracking.sql
-- edmd: Iceberg/S3 archival tracking.
--
-- archive_batches: one row per completed archival run.
-- Adds `archived` flag to `meter_reads` for O(1) archival identification.
--
-- Partitioned Parquet files are written to the configured S3 prefix with
-- hive-style layout:  {prefix}/sparte={SPARTE}/year={YYYY}/month={MM}/batch-{uuid}.parquet
-- A companion Iceberg V2 `metadata.json` is written to {prefix}/metadata/
-- for registration with Nessie, AWS Glue REST, Polaris, etc.
--
-- All timestamps are TIMESTAMPTZ (UTC).

-- ── Archive batch tracking ────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS archive_batches (
    batch_id        UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- Archival window: rows with dtm_from < cutoff_before are eligible
    cutoff_before   TIMESTAMPTZ NOT NULL,
    dtm_from_min    TIMESTAMPTZ,
    dtm_from_max    TIMESTAMPTZ,

    -- Row statistics
    row_count       BIGINT      NOT NULL DEFAULT 0,
    malo_count      INTEGER     NOT NULL DEFAULT 0,

    -- S3 storage
    s3_prefix       TEXT        NOT NULL,
    file_count      INTEGER     NOT NULL DEFAULT 0,
    bytes_written   BIGINT      NOT NULL DEFAULT 0,

    -- Lifecycle
    status          TEXT        NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending','writing','committed','failed')),
    error_msg       TEXT,
    committed_at    TIMESTAMPTZ,

    tenant_id       UUID
);

CREATE INDEX IF NOT EXISTS ab_created_at ON archive_batches (created_at DESC);
CREATE INDEX IF NOT EXISTS ab_open       ON archive_batches (status)
    WHERE status IN ('pending','writing','failed');
CREATE INDEX IF NOT EXISTS ab_tenant     ON archive_batches (tenant_id)
    WHERE tenant_id IS NOT NULL;

-- ── Hot-tier marker on meter_reads ───────────────────────────────────────────

-- Mark rows that have been exported to the Iceberg/S3 cold tier.
-- The index below makes the archival scan O(archived_rows) instead of O(table).
ALTER TABLE meter_reads ADD COLUMN IF NOT EXISTS archived BOOLEAN NOT NULL DEFAULT false;

CREATE INDEX IF NOT EXISTS mr_archive_eligible
    ON meter_reads (dtm_from)
    WHERE archived = false;

-- ── Iceberg snapshot tracking (light-weight) ──────────────────────────────────

CREATE TABLE IF NOT EXISTS iceberg_snapshots (
    snapshot_id     BIGINT      PRIMARY KEY,
    table_location  TEXT        NOT NULL,
    schema_json     JSONB       NOT NULL,
    partition_spec  JSONB       NOT NULL,
    manifest_list   TEXT        NOT NULL,   -- S3 path to manifest-list Avro file
    summary         JSONB,
    parent_id       BIGINT      REFERENCES iceberg_snapshots(snapshot_id),
    committed_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS is_committed ON iceberg_snapshots (committed_at DESC);

-- ── edmd 0003: Ablesesteuerung — Reading Order Scheduling ──────────────────
--
-- Ablesesteuerung is relevant for ALL three market roles:
--
--   LF  (Lieferant):  Zwischenablesung at Lieferbeginn/-ende (billing cutoff);
--                     Jahresablesung for annual billing; triggered by auftragd.
--
--   MSB (Messstellenbetreiber): Physical reading scheduling; iMSys remote readout
--                     scheduling (15-min RLM, daily iMSys SLP); Sonderablsung on
--                     INSRPT Störungsmeldung (PID 23001).
--
--   NB  (Netzbetreiber): Jahresablese-Kampagnen; Stammdaten/Zählerstand requests
--                     via ORDERS 17132; SLP profile plausibility.
--
-- This table is the single schedule for all reading orders regardless of role.

CREATE TABLE IF NOT EXISTS ablese_auftraege (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id             TEXT        NOT NULL,
    melo_id             TEXT,
    tenant              TEXT        NOT NULL,
    anlass              TEXT        NOT NULL
                        CHECK (anlass IN (
                            'JAHRESABLESUNG','ZWISCHENABLESUNG',
                            'LIEFERBEGINN','LIEFERENDE',
                            'SPERRUNG','ENTSPERRUNG',
                            'SONDERABLESUNG','INSRPT_STOERUNG',
                            'ISMS_AUSLESUNG'          -- iMSys automated readout
                        )),
    auftraggeber_rolle  TEXT        NOT NULL
                        CHECK (auftraggeber_rolle IN ('LF','MSB','NB')),
    ausfuehrender_msb   TEXT,                         -- MSB MP-ID (BDEW-Codenummer)
    geplant_am          DATE        NOT NULL,
    ausfuehrt_bis       DATE,                         -- latest acceptable date
    status              TEXT        NOT NULL DEFAULT 'OFFEN'
                        CHECK (status IN (
                            'OFFEN','BEAUFTRAGT','AUSGEFUEHRT',
                            'STORNIERT','FEHLGESCHLAGEN'
                        )),
    -- Result
    zaehlerstand_kwh    NUMERIC(18,3),
    zaehlerstand_qm3    NUMERIC(18,3),                -- Gas: m³ reading
    brennwert           NUMERIC(8,4),                 -- Gas: Hs kWh/m³
    zustandszahl        NUMERIC(8,4),                 -- Gas: Z
    ausgefuehrt_am      TIMESTAMPTZ,
    mscons_ref          TEXT,                         -- MSCONS message ref when transmitted
    -- Origin
    auftrag_position_id UUID,                         -- auftragd.auftrag_positionen.id (if triggered by O2C)
    insrpt_process_id   TEXT,                         -- makod process ID (if triggered by INSRPT)
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ablese_malo_status   ON ablese_auftraege (malo_id, tenant, status);
CREATE INDEX IF NOT EXISTS ablese_geplant_offen ON ablese_auftraege (geplant_am, status) WHERE status = 'OFFEN';
CREATE INDEX IF NOT EXISTS ablese_anlass_rolle  ON ablese_auftraege (anlass, auftraggeber_rolle);

-- edmd migration 0004: iMSys direct push + meter data quality tracking
--
-- M4: adds `source` column to `meter_reads` to distinguish MSCONS-ingested reads
--     from SMGW/iMSys direct-push reads. Also tracks the external session ID so
--     duplicate uploads are idempotent.
--
-- M7: adds `quality_warnings` JSONB column to `meter_reads` for automated
--     quality scores (gap detection, outlier flags, zero-read runs).
--     The direct-push handler populates this at write time; the MSCONS path
--     updates it during recomputation.
--
-- Source values:
--   'MSCONS'      — received via EDIFACT MSCONS → makod → webhook
--   'DIRECT_PUSH' — submitted via POST /api/v1/meter-reads/rlm/{malo_id}
--   'DIRECT_GAS'  — submitted via POST /api/v1/meter-reads/gas/{malo_id}
--   'API_IMPORT'  — bulk import via ERP API

ALTER TABLE meter_reads
    ADD COLUMN IF NOT EXISTS source         TEXT NOT NULL DEFAULT 'MSCONS',
    ADD COLUMN IF NOT EXISTS push_session   TEXT,          -- idempotency key from caller
    ADD COLUMN IF NOT EXISTS quality_warnings JSONB;       -- { "gaps": 0, "zeros": 0, "outliers": [] }

-- Index to speed up source-filtered queries (e.g. "show me all direct-push reads")
CREATE INDEX IF NOT EXISTS mr_source ON meter_reads (malo_id, source, dtm_from DESC);

-- Partial index for reads with quality warnings (fast anomaly queries)
CREATE INDEX IF NOT EXISTS mr_quality_warn
    ON meter_reads (malo_id, dtm_from)
    WHERE quality_warnings IS NOT NULL;

COMMENT ON COLUMN meter_reads.source IS
    'Origin of the meter read: MSCONS (EDIFACT pipeline) | DIRECT_PUSH (iMSys REST) | API_IMPORT';

COMMENT ON COLUMN meter_reads.push_session IS
    'Caller-supplied idempotency key for direct push batches. '
    'Used by POST /api/v1/meter-reads/rlm to reject duplicate uploads.';

COMMENT ON COLUMN meter_reads.quality_warnings IS
    'Automated quality flags set at ingest time. '
    'JSON: { "gaps_detected": N, "zero_run_length": N, "outlier_factor": 0.0 }. '
    'NULL = no warnings detected. Triggers de.edmd.reading.quality.warning CloudEvent.';

-- ── Push session deduplication ────────────────────────────────────────────────
-- Tracks SMGW direct-push sessions so callers can retry safely.
-- A session with status='committed' means all intervals were stored successfully.

CREATE TABLE IF NOT EXISTS direct_push_sessions (
    session_id      TEXT        PRIMARY KEY,           -- caller-supplied or server-generated
    malo_id         TEXT        NOT NULL,
    source          TEXT        NOT NULL DEFAULT 'DIRECT_PUSH',
    obis_code       TEXT,
    interval_count  INTEGER     NOT NULL DEFAULT 0,
    period_from     TIMESTAMPTZ,
    period_to       TIMESTAMPTZ,
    status          TEXT        NOT NULL DEFAULT 'committed'
                    CHECK (status IN ('committed', 'partial', 'failed')),
    quality_summary JSONB,
    tenant_id       UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS dps_malo ON direct_push_sessions (malo_id, created_at DESC);
CREATE INDEX IF NOT EXISTS dps_tenant ON direct_push_sessions (tenant_id) WHERE tenant_id IS NOT NULL;

COMMENT ON TABLE direct_push_sessions IS
    'Tracks iMSys / SMGW direct-push sessions for idempotency and audit. '
    'One row per POST /api/v1/meter-reads/rlm call.';

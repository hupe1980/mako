-- edmd schema — single authoritative DDL for a clean PostgreSQL install.
--
-- All historical migrations have been consolidated into this one file.
-- Column types reflect the final state: NUMERIC(18,5) for kWh values,
-- TEXT NOT NULL for tenant isolation on every table (no nullable UUIDs),
-- and all indexes co-located with the table they serve.
--
-- § 60 Abs. 6 MsbG requires 5 decimal place kWh precision.
-- GDPR Art. 32 requires per-tenant data isolation on every table.
-- `meter_reads` is range-partitioned monthly; see `ensure_meter_reads_partitions`.

-- `btree_gist` provides GiST equality operators for TEXT so the per-partition
-- interval-overlap EXCLUDE constraint can combine (tenant, malo_id,
-- obis_code_norm) equality with tstzrange overlap. Shipped in postgres contrib.
CREATE EXTENSION IF NOT EXISTS btree_gist;

-- ── Meter data receipts ───────────────────────────────────────────────────────
-- One row per received MSCONS process. Kept separate from meter_reads
-- so receipt metadata is available even before typed interval data arrives.

CREATE TABLE meter_data_receipts (
    process_id   UUID        PRIMARY KEY,
    pid          INTEGER     NOT NULL,
    malo_id      TEXT        NOT NULL,
    sender_mp_id TEXT        NOT NULL,
    message_ref  TEXT,
    received_at  TIMESTAMPTZ NOT NULL,
    -- tenant is TEXT NOT NULL (BDEW/DVGW Codenummer or GLN) — same type and
    -- semantics as meter_reads.tenant and all other edmd tables.
    tenant       TEXT        NOT NULL DEFAULT '',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX mdr_malo_received ON meter_data_receipts (malo_id, received_at DESC);
CREATE INDEX mdr_tenant        ON meter_data_receipts (tenant, malo_id);

-- ── Typed meter reads (hot tier) ─────────────────────────────────────────────
-- One row per 15-min (or coarser) interval per MaLo per OBIS code.
-- Quantity is NUMERIC(18,5) for exact § 60 Abs. 6 MsbG 5-decimal-place precision.
-- Tenant is TEXT NOT NULL for mandatory data isolation.

CREATE TABLE meter_reads (
    malo_id            TEXT          NOT NULL,
    melo_id            TEXT,
    dtm_from           TIMESTAMPTZ   NOT NULL,
    dtm_to             TIMESTAMPTZ   NOT NULL,
    quantity_kwh       NUMERIC(18,5) NOT NULL,
    quality            TEXT          NOT NULL DEFAULT 'UNKNOWN'
                           CHECK (quality IN (
                               'MEASURED','ESTIMATED','SUBSTITUTED','CALCULATED',
                               'CORRECTED','PRELIMINARY','FAULTY','UNKNOWN'
                           )),
    pid                INTEGER       NOT NULL,
    sparte             TEXT          NOT NULL DEFAULT 'STROM'
                           CHECK (sparte IN ('STROM','GAS','WAERME','WASSER')),
    -- Canonical storage unit. Ingest accepts Wh/MWh/GWh/GJ/MJ and litres and
    -- rescales before writing, so only these two ever land here. Gas is stored
    -- as kWh — the meter registers m³ and the Brennwert conversion (§25 Nr. 4
    -- MessEV) is applied at ingest. Water is m³ and has no calorific value, so
    -- it must never pass through that conversion. Heat is kWh_th.
    unit               TEXT          NOT NULL DEFAULT 'KWH'
                           CHECK (unit IN ('KWH','M3')),
    obis_code          TEXT,
    obis_code_norm     TEXT          NOT NULL DEFAULT '',
    -- Must match `mako_edm::domain::IngestionSource` exactly; `schema_code_guard`
    -- pins the two together.
    source             TEXT          NOT NULL DEFAULT 'MSCONS'
                           CHECK (source IN (
                               'MSCONS','DIRECT_PUSH','DIRECT_GAS',
                               'MANUAL','ESTIMATED','CORRECTION','API_IMPORT',
                               'AUTO_SUBSTITUTE','IOT_PUSH'
                           )),
    push_session       TEXT,
    quality_warnings   JSONB,
    sender_mp_id       TEXT,
    allocation_version TEXT          NOT NULL DEFAULT 'INITIAL'
                           CHECK (allocation_version IN ('INITIAL','CORRECTION','FINAL')),
    valid_from_tx      TIMESTAMPTZ   NOT NULL DEFAULT now(),
    tenant             TEXT          NOT NULL,
    correction_count   INTEGER       NOT NULL DEFAULT 0,

    archived           BOOLEAN       NOT NULL DEFAULT false,

    -- `tenant` is part of the reading's identity, not a filter applied after
    -- the fact. Without it two tenants holding the same MaLo-ID collide on one
    -- row, and the ingest upsert resolves that collision by overwriting the
    -- value *and* reassigning ownership — silent cross-tenant data loss.
    CONSTRAINT mr_pk PRIMARY KEY (tenant, malo_id, dtm_from, obis_code_norm),
    CONSTRAINT mr_valid_interval CHECK (dtm_from < dtm_to)
) PARTITION BY RANGE (dtm_from);

-- Monthly range partitions on `dtm_from`, which the primary key already
-- carries — a partitioned table requires its key in every unique constraint.
--
-- Retention becomes a catalogue operation: expiring a month is `DROP TABLE` on
-- one partition, which returns the disk immediately. The alternative, a bulk
-- `DELETE` over the hot tier, leaves dead tuples for autovacuum to reclaim and
-- competes with ingest for I/O on the busiest table in the schema.

CREATE OR REPLACE FUNCTION meter_reads_partition_name(p_month DATE)
RETURNS TEXT LANGUAGE sql IMMUTABLE AS $$
    SELECT 'meter_reads_p' || to_char(date_trunc('month', p_month), 'YYYYMM');
$$;

-- Create the partition covering `p_month` if it does not exist.
CREATE OR REPLACE FUNCTION ensure_meter_reads_partition(p_month DATE)
RETURNS TEXT LANGUAGE plpgsql AS $$
DECLARE
    v_name  TEXT := meter_reads_partition_name(p_month);
    v_start DATE := date_trunc('month', p_month);
    v_end   DATE := (date_trunc('month', p_month) + INTERVAL '1 month')::date;
BEGIN
    EXECUTE format(
        'CREATE TABLE IF NOT EXISTS %I PARTITION OF meter_reads
         FOR VALUES FROM (%L) TO (%L)',
        v_name, v_start, v_end
    );
    -- Cross-batch overlap exclusion (per partition): the PK stops identical
    -- dtm_from values and V02 catches overlaps within one batch, but only
    -- this constraint stops a second delivery from storing a range that
    -- overlaps an existing one. Half-open [) ranges keep adjacent intervals
    -- legal. PostgreSQL cannot enforce EXCLUDE on the partitioned parent
    -- (the constraint does not compare the partition key with =), so it is
    -- attached to every partition; an interval crossing a month boundary is
    -- already impossible because its row lives in the partition of dtm_from
    -- and dtm_to is capped by the CHECK to the same reading.
    BEGIN
        EXECUTE format(
            'ALTER TABLE %I ADD CONSTRAINT %I EXCLUDE USING gist (
                 tenant WITH =, malo_id WITH =, obis_code_norm WITH =,
                 tstzrange(dtm_from, dtm_to, ''[)'') WITH &&
             )',
            v_name, v_name || '_no_overlap'
        );
    EXCEPTION WHEN duplicate_object OR duplicate_table THEN
        NULL; -- constraint already exists (idempotent re-run)
    END;
    RETURN v_name;
END;
$$;

-- Keep a rolling window of partitions ahead of `now()`. Called at startup and
-- by the archival worker; ingest for a month with no partition would otherwise
-- fail outright.
CREATE OR REPLACE FUNCTION ensure_meter_reads_partitions(
    p_months_back  INT DEFAULT 2,
    p_months_ahead INT DEFAULT 3
)
RETURNS INT LANGUAGE plpgsql AS $$
DECLARE
    v_offset INT;
    v_count  INT := 0;
BEGIN
    FOR v_offset IN -p_months_back .. p_months_ahead LOOP
        PERFORM ensure_meter_reads_partition(
            (date_trunc('month', now()) + (v_offset || ' month')::interval)::date
        );
        v_count := v_count + 1;
    END LOOP;
    RETURN v_count;
END;
$$;

-- Drop partitions that lie entirely before `p_cutoff` and hold no rows still
-- awaiting export to the cold tier.
--
-- The `archived = false` guard is what makes this safe: a partition is released
-- only once every row in it is durable in Iceberg, so a stalled or failed
-- archival run delays reclamation instead of destroying unexported readings.
CREATE OR REPLACE FUNCTION drop_archived_meter_reads_partitions(p_cutoff TIMESTAMPTZ)
RETURNS TABLE (partition_name TEXT, rows_released BIGINT) LANGUAGE plpgsql AS $$
DECLARE
    v_part    RECORD;
    v_bound   TEXT;
    v_upper   TIMESTAMPTZ;
    v_pending BIGINT;
    v_rows    BIGINT;
BEGIN
    FOR v_part IN
        SELECT c.relname, pg_get_expr(c.relpartbound, c.oid) AS bound
        FROM   pg_class c
        JOIN   pg_inherits i ON i.inhrelid = c.oid
        WHERE  i.inhparent = 'meter_reads'::regclass
    LOOP
        v_bound := v_part.bound;
        -- Bound reads: FOR VALUES FROM ('...') TO ('...')
        v_upper := (regexp_match(v_bound, $re$TO \('([^']+)'\)$re$))[1]::timestamptz;
        CONTINUE WHEN v_upper IS NULL OR v_upper > p_cutoff;

        EXECUTE format('SELECT count(*) FROM %I WHERE archived = false', v_part.relname)
            INTO v_pending;
        CONTINUE WHEN v_pending > 0;

        EXECUTE format('SELECT count(*) FROM %I', v_part.relname) INTO v_rows;
        EXECUTE format('DROP TABLE %I', v_part.relname);

        partition_name := v_part.relname;
        rows_released  := v_rows;
        RETURN NEXT;
    END LOOP;
END;
$$;

SELECT ensure_meter_reads_partitions(24, 3);

-- Index-only scan for billing period aggregation (covers quantity_kwh + quality)
CREATE INDEX mr_billing_covering ON meter_reads
    (tenant, malo_id, dtm_from, dtm_to)
    INCLUDE (quantity_kwh, quality)
    WHERE quality NOT IN ('FAULTY', 'UNKNOWN');

-- V03: instant negative-energy detection
CREATE INDEX mr_negative_kwh ON meter_reads (malo_id, dtm_from)
    WHERE quantity_kwh < 0;

-- Direct-push source queries
CREATE INDEX mr_source ON meter_reads (malo_id, source, dtm_from DESC);

-- Quality warnings fast lookup
CREATE INDEX mr_quality_warn ON meter_reads (malo_id, dtm_from)
    WHERE quality_warnings IS NOT NULL;

-- Allocation version queries (mabis-syncd FINAL vs INITIAL)
CREATE INDEX mr_allocation_version ON meter_reads (malo_id, allocation_version, dtm_from)
    WHERE allocation_version != 'INITIAL';

-- Sender MSB attribution (per-interval MSB after WiM switch)
CREATE INDEX mr_sender_mp_id ON meter_reads (sender_mp_id, malo_id, dtm_from)
    WHERE sender_mp_id IS NOT NULL;

-- Corrected intervals monitoring
CREATE INDEX mr_corrected ON meter_reads (malo_id, dtm_from)
    WHERE correction_count > 0;

-- Rows still owed an export to the cold tier. Partial on the false value, so
-- it shrinks to nothing as a partition finishes archiving.
CREATE INDEX mr_archive_pending ON meter_reads (dtm_from)
    WHERE archived = false;

COMMENT ON TABLE meter_reads IS
    'Hot-tier metered interval data, range-partitioned monthly on dtm_from. '
    'NUMERIC(18,5) for § 60 Abs. 6 MsbG 5dp kWh precision. Rows older than '
    'retention_months are exported to the Iceberg cold tier and marked '
    'archived=true; a partition is dropped once all of its rows are archived.';

-- ── Billing period aggregates ─────────────────────────────────────────────────
-- Pre-computed from meter_reads after each MSCONS ingest. Avoids on-the-fly
-- aggregation in billing period API calls. All numeric columns are NUMERIC(18,5).

CREATE TABLE meter_billing_periods (
    malo_id              TEXT          NOT NULL,
    period_from          DATE          NOT NULL,
    period_to            DATE          NOT NULL,
    messtyp              TEXT          NOT NULL DEFAULT 'SLP'
                             CHECK (messtyp IN ('SLP','RLM','IMSYS')),
    sparte               TEXT          NOT NULL DEFAULT 'STROM'
                             CHECK (sparte IN ('STROM','GAS','WAERME','WASSER')),
    arbeitsmenge_kwh     NUMERIC(18,5) NOT NULL,
    arbeitsmenge_ht_kwh  NUMERIC(18,5),
    arbeitsmenge_nt_kwh  NUMERIC(18,5),
    spitzenleistung_kw   NUMERIC(18,5),
    brennwert_kwh_per_m3 NUMERIC(10,4),  -- Gas: Hs kWh/m³ (same typing as gas_quality_data)
    zustandszahl         NUMERIC(8,4),   -- Gas: compressibility factor
    zaehlerstand_anfang  NUMERIC(18,5),  -- §40 Abs. 2 Nr. 6 EnWG register reading
    zaehlerstand_ende    NUMERIC(18,5),
    quality              TEXT          NOT NULL DEFAULT 'UNKNOWN',
    tenant               TEXT          NOT NULL,
    computed_at          TIMESTAMPTZ   NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX mbp_tenant_period_unique
    ON meter_billing_periods (malo_id, period_from, period_to, tenant);

CREATE INDEX mbp_tenant_malo_v2
    ON meter_billing_periods (tenant, malo_id, period_from, period_to)
    WHERE tenant <> '';

-- ── Bitemporal corrections (§ 60 Abs. 6 MsbG audit trail) ──────────────────────────

CREATE TABLE meter_read_corrections (
    correction_id    UUID          PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id          TEXT          NOT NULL,
    dtm_from         TIMESTAMPTZ   NOT NULL,
    dtm_to           TIMESTAMPTZ   NOT NULL,
    -- Register the correction applies to, normalised the same way
    -- `meter_reads.obis_code_norm` is. Without it a point-in-time
    -- reconstruction restores one register's prior value onto every register
    -- the MaLo carries at that instant.
    obis_code_norm   TEXT          NOT NULL DEFAULT '',
    original_kwh     NUMERIC(18,5) NOT NULL,
    original_quality TEXT          NOT NULL,
    corrected_kwh    NUMERIC(18,5) NOT NULL,
    corrected_quality TEXT         NOT NULL,
    reason           TEXT          NOT NULL,
    source           TEXT          NOT NULL
                         CHECK (source IN (
                             'MSCONS_UPDATE','OPERATOR','AUTO_SUBSTITUTE',
                             'IMSYS_DIRECT_PUSH','OTHER'
                         )),
    corrected_by     TEXT,
    corrected_at     TIMESTAMPTZ   NOT NULL DEFAULT now(),
    process_id       UUID,
    pid              INTEGER,
    tenant           TEXT          NOT NULL
    -- NOTE: legacy tenant_id UUID column removed; all tenant isolation uses
    -- tenant TEXT NOT NULL, consistent with meter_data_receipts and meter_reads.
);

CREATE INDEX mrc_malo_dtm         ON meter_read_corrections (malo_id, dtm_from, dtm_to);
CREATE INDEX mrc_malo_corrected_at ON meter_read_corrections (malo_id, corrected_at DESC);
CREATE INDEX mrc_tenant_malo       ON meter_read_corrections (tenant, malo_id, dtm_from DESC);

-- ── Iceberg/S3 archival tracking ──────────────────────────────────────────────

CREATE TABLE archive_batches (
    batch_id       UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    cutoff_before  TIMESTAMPTZ NOT NULL,
    dtm_from_min   TIMESTAMPTZ,
    dtm_from_max   TIMESTAMPTZ,
    row_count      BIGINT      NOT NULL DEFAULT 0,
    malo_count     INTEGER     NOT NULL DEFAULT 0,
    s3_prefix      TEXT        NOT NULL,
    file_count     INTEGER     NOT NULL DEFAULT 0,
    bytes_written  BIGINT      NOT NULL DEFAULT 0,
    status         TEXT        NOT NULL DEFAULT 'pending'
                       CHECK (status IN ('pending','writing','committed','failed')),
    error_msg      TEXT,
    committed_at   TIMESTAMPTZ,
    tenant         TEXT        NOT NULL
);

CREATE INDEX ab_created_at ON archive_batches (created_at DESC);
CREATE INDEX ab_open       ON archive_batches (status)
    WHERE status IN ('pending','writing','failed');
CREATE INDEX ab_tenant     ON archive_batches (tenant);

-- ── Iceberg REST catalog registry ────────────────────────────────────────────
-- Tracked by GET /api/v1/iceberg/v1/... handlers for DuckDB/Snowflake interop.

CREATE TABLE iceberg_catalog_entries (
    id                    UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    namespace             TEXT        NOT NULL,
    table_name            TEXT        NOT NULL,
    location_uri          TEXT        NOT NULL,
    schema_json           JSONB       NOT NULL,
    partition_spec        JSONB,
    sort_order            JSONB,
    properties            JSONB,
    current_snapshot_id   BIGINT,
    registered_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_refreshed_at     TIMESTAMPTZ,
    tenant                TEXT        NOT NULL,

    CONSTRAINT ice_unique_ns_table UNIQUE (namespace, table_name, tenant)
);

CREATE INDEX ice_tenant ON iceberg_catalog_entries (tenant);

-- ── Ablesesteuerung (reading order scheduling) ───────────────────────────────

CREATE TABLE ablese_auftraege (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id            TEXT        NOT NULL,
    melo_id            TEXT,
    tenant             TEXT        NOT NULL,
    anlass             TEXT        NOT NULL
                           CHECK (anlass IN (
                               'JAHRESABLESUNG','ZWISCHENABLESUNG',
                               'LIEFERBEGINN','LIEFERENDE',
                               'SPERRUNG','ENTSPERRUNG',
                               'SONDERABLESUNG','INSRPT_STOERUNG','ISMS_AUSLESUNG'
                           )),
    auftraggeber_rolle TEXT        NOT NULL
                           CHECK (auftraggeber_rolle IN ('LF','MSB','NB')),
    ausfuehrender_msb  TEXT,
    geplant_am         DATE        NOT NULL,
    ausfuehrt_bis      DATE,
    status             TEXT        NOT NULL DEFAULT 'OFFEN'
                           CHECK (status IN (
                               'OFFEN','BEAUFTRAGT','AUSGEFUEHRT',
                               'STORNIERT','FEHLGESCHLAGEN'
                           )),
    zaehlerstand_kwh   NUMERIC(18,3),
    zaehlerstand_qm3   NUMERIC(18,3),
    brennwert          NUMERIC(8,4),
    zustandszahl       NUMERIC(8,4),
    ausgefuehrt_am     TIMESTAMPTZ,
    -- Why a reading could not be taken (Ablesehindernis). Required whenever
    -- status is FEHLGESCHLAGEN: a failed Jahresablesung leaves the §40 Abs. 2
    -- EnWG obligation unmet, and the reason decides whether the NB may
    -- estimate (§40a EnWG) or must re-dispatch.
    fehlschlag_grund   TEXT
                           CHECK (fehlschlag_grund IN (
                               'KEIN_ZUTRITT','ZAEHLER_UNZUGAENGLICH','ZAEHLER_DEFEKT',
                               'ZAEHLER_NICHT_AUFFINDBAR','KUNDE_VERWEIGERT',
                               'ABLESUNG_UNPLAUSIBEL','SONSTIGES'
                           )),
    fehlschlag_notiz   TEXT,
    fehlgeschlagen_am  TIMESTAMPTZ,
    mscons_ref         TEXT,
    auftrag_position_id UUID,
    insrpt_process_id  TEXT,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- A failure must name its cause, so FEHLGESCHLAGEN cannot be used to
    -- silently retire an order that is still owed a reading.
    CONSTRAINT ablese_fehlschlag_begruendet CHECK (
        status <> 'FEHLGESCHLAGEN' OR fehlschlag_grund IS NOT NULL
    )
);

-- Idempotency for INSRPT-triggered orders. `ON CONFLICT DO NOTHING` needs a
-- unique index to fire on; with only the surrogate `id` PK every redelivered
-- CloudEvent minted a fresh UUID and created a duplicate order.
CREATE UNIQUE INDEX ablese_insrpt_unique ON ablese_auftraege (tenant, insrpt_process_id)
    WHERE insrpt_process_id IS NOT NULL;

-- Idempotency for scheduled/campaign orders, which carry no process id.
CREATE UNIQUE INDEX ablese_scheduled_unique ON ablese_auftraege
    (tenant, malo_id, anlass, geplant_am)
    WHERE insrpt_process_id IS NULL;

CREATE INDEX ablese_malo_status    ON ablese_auftraege (malo_id, tenant, status);
CREATE INDEX ablese_geplant_offen  ON ablese_auftraege (geplant_am, status) WHERE status = 'OFFEN';
CREATE INDEX ablese_anlass_rolle   ON ablese_auftraege (anlass, auftraggeber_rolle);

-- ── iMSys/SMGW direct push session deduplication ────────────────────────────

CREATE TABLE direct_push_sessions (
    session_id      TEXT        PRIMARY KEY,
    malo_id         TEXT        NOT NULL,
    source          TEXT        NOT NULL DEFAULT 'DIRECT_PUSH',
    obis_code       TEXT,
    interval_count  INTEGER     NOT NULL DEFAULT 0,
    period_from     TIMESTAMPTZ,
    period_to       TIMESTAMPTZ,
    status          TEXT        NOT NULL DEFAULT 'committed'
                        CHECK (status IN ('committed','partial','failed')),
    quality_summary JSONB,
    tenant          TEXT        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX dps_malo   ON direct_push_sessions (malo_id, created_at DESC);
CREATE INDEX dps_tenant ON direct_push_sessions (tenant);

-- ── Gas quality data ─────────────────────────────────────────────────────────
-- Brennwert + Zustandszahl per MaLo per period (PID 13007).

CREATE TABLE gas_quality_data (
    id                   UUID          PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id              TEXT          NOT NULL,
    period_from          DATE          NOT NULL,
    period_to            DATE          NOT NULL,
    brennwert_kwh_per_m3 NUMERIC(10,4) NOT NULL,
    zustandszahl         NUMERIC(8,4)  NOT NULL,
    source_pid           INTEGER,
    received_at          TIMESTAMPTZ   NOT NULL DEFAULT now(),
    tenant               TEXT          NOT NULL
);

CREATE UNIQUE INDEX gqd_malo_period ON gas_quality_data (malo_id, period_from, period_to, tenant);
CREATE INDEX        gqd_tenant      ON gas_quality_data (tenant);

-- ── Virtual meter configurations ──────────────────────────────────────────────
--
-- Defines derived meters: Sum, Residual, PV self-consumption, and the
-- Gemeinschaftliche Gebäudeversorgung allocation rules (§42b EnWG).
--
-- `virtual_malo_id` — a virtual meter *is* a Marktlokation, addressed by its own
-- MaLo-ID, which is why the column is not a bare `virtual_id`.
--
-- `rule_type` must match the variants of `metering::aggregation_rule::AggregationRule`
-- exactly. `edmd` deserialises `rule_json` into that enum, so a value here that
-- the enum does not know is an unreadable row. The `virtual_meter_rule_types`
-- guard test in `crates/metering` pins the two lists together.
--
-- §42c Energy Sharing reuses `GgvProportionalAllocation`: the allocation
-- arithmetic is identical, and the two regimes are distinguished by
-- `legal_basis` (§42b = in-building, no grid transit; §42c = via the public
-- grid). Should BNetzA's §42c Festlegung — due end-2026 — mandate different
-- arithmetic, that will need its own variant rather than an overloaded one.

CREATE TABLE virtual_meter_configs (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    virtual_malo_id TEXT        NOT NULL,
    display_name    TEXT,
    rule_type       TEXT        NOT NULL
                        CHECK (rule_type IN (
                            'Sum',
                            'Residual',
                            'PvSelfConsumption',
                            'GgvConstantAllocation',
                            'GgvProportionalAllocation'
                        )),
    -- Serialised `AggregationRule`, including its source MaLo-IDs.
    rule_json       JSONB,
    -- Statutory citation, e.g. '§42b EnWG' or '§42c EnWG'. Free text: it records
    -- which regime a community operates under, which `rule_type` cannot express.
    legal_basis     TEXT,
    sparte          TEXT        CHECK (sparte IS NULL OR sparte IN ('STROM', 'GAS', 'WAERME', 'WASSER')),
    valid_from      DATE        NOT NULL DEFAULT CURRENT_DATE,
    valid_to        DATE,
    tenant          TEXT        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT vmc_validity CHECK (valid_to IS NULL OR valid_to >= valid_from)
);

CREATE INDEX vmc_tenant    ON virtual_meter_configs (tenant);
CREATE INDEX vmc_rule_type ON virtual_meter_configs (rule_type);
-- The upsert in `create_virtual_meter` targets this conflict key.
CREATE UNIQUE INDEX vmc_virtual_malo_id ON virtual_meter_configs (virtual_malo_id, tenant);

-- ── Quality assessments ───────────────────────────────────────────────────────

CREATE TABLE quality_assessments (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id        TEXT        NOT NULL,
    period_from    TIMESTAMPTZ NOT NULL,
    period_to      TIMESTAMPTZ NOT NULL,
    grade          TEXT        NOT NULL CHECK (grade IN ('A','B','C','F')),
    -- Intervals actually seen, and how many the period should hold. Coverage
    -- alone cannot answer "how much is missing" without the denominator.
    interval_count INTEGER     NOT NULL DEFAULT 0,
    expected_count INTEGER,
    gaps_detected  INTEGER     NOT NULL DEFAULT 0,
    zero_run       INTEGER     NOT NULL DEFAULT 0,
    outlier_count  INTEGER     NOT NULL DEFAULT 0,
    coverage_pct   NUMERIC(5,2),
    billing_blocked BOOLEAN    NOT NULL DEFAULT false,
    -- Ingest family the assessment was made for. Must cover every family that
    -- scores quality, or the insert fails the constraint and the history is
    -- silently missing for exactly the paths that produced it.
    source         TEXT        NOT NULL DEFAULT 'MSCONS'
                       CHECK (source IN (
                           'MSCONS','DIRECT_PUSH','IOT_PUSH','API_IMPORT',
                           'CORRECTION','BATCH_RESCORE'
                       )),
    -- Rule findings behind the grade (V01–V10), so a disputed invoice can be
    -- traced to the specific check that failed rather than to a letter.
    issues_json    JSONB,
    -- MSCONS Prüfidentifikator, when the assessment came from a MaKo process.
    pid            INTEGER,
    assessed_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    tenant         TEXT        NOT NULL
);

-- One assessment per (MaLo, period, source). Re-scoring the same window
-- supersedes the previous verdict rather than appending a second one, so the
-- history reads as a sequence of decisions and not of duplicates.
CREATE UNIQUE INDEX qa_malo_period_source ON quality_assessments
    (tenant, malo_id, period_from, period_to, source);

CREATE INDEX qa_malo_assessed  ON quality_assessments (malo_id, assessed_at DESC);
CREATE INDEX qa_grade          ON quality_assessments (grade) WHERE grade != 'A';
CREATE INDEX qa_billing_block  ON quality_assessments (malo_id, billing_blocked)
    WHERE billing_blocked = true;
CREATE INDEX qa_tenant         ON quality_assessments (tenant);

-- ── Substitute value log (§ 60 Abs. 2 MsbG audit trail) ────────────────────────────

CREATE TABLE substitute_value_log (
    id              UUID          PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id         TEXT          NOT NULL,
    dtm_from        TIMESTAMPTZ   NOT NULL,
    dtm_to          TIMESTAMPTZ   NOT NULL,
    original_kwh    NUMERIC(18,5),
    substitute_kwh  NUMERIC(18,5) NOT NULL,
    method          TEXT          NOT NULL
                        CHECK (method IN (
                            'LinearInterpolation','PriorPeriodAverage',
                            'ZeroFill','LastValueCarryForward','ManualEntry'
                        )),
    reason          TEXT,
    -- Operator who authorised the Ersatzwert (§ 60 Abs. 6 MsbG attributability).
    created_by      TEXT,
    created_at      TIMESTAMPTZ   NOT NULL DEFAULT now(),
    tenant          TEXT          NOT NULL
);

CREATE INDEX svl_malo_dtm ON substitute_value_log (malo_id, dtm_from, dtm_to);
CREATE INDEX svl_tenant   ON substitute_value_log (tenant);
CREATE INDEX svl_method   ON substitute_value_log (method);

-- ── Meter exchange events ────────────────────────────────────────────────────

CREATE TABLE meter_exchange_events (
    exchange_id           UUID          PRIMARY KEY DEFAULT gen_random_uuid(),
    melo_id               TEXT          NOT NULL,
    old_meter_serial      TEXT          NOT NULL,
    old_final_reading_kwh NUMERIC(18,5) NOT NULL,
    new_meter_serial      TEXT          NOT NULL,
    new_first_reading_kwh NUMERIC(18,5) NOT NULL,
    exchange_date         DATE          NOT NULL,
    exchange_at           TIMESTAMPTZ   NOT NULL,
    triggered_by_pid      INTEGER,
    insrpt_process_id     TEXT,
    performed_by          TEXT,
    tenant                TEXT          NOT NULL
);

CREATE INDEX mee_melo_date ON meter_exchange_events (melo_id, exchange_date);
CREATE INDEX mee_tenant    ON meter_exchange_events (tenant);

-- ── GDPR Art. 17 erasure tracking ────────────────────────────────────────────

CREATE TABLE gdpr_deletions (
    id                             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id                        TEXT        NOT NULL,
    tenant                         TEXT        NOT NULL,
    reason                         TEXT        NOT NULL,
    authorized_by                  TEXT        NOT NULL,
    requested_at                   TIMESTAMPTZ NOT NULL DEFAULT now(),
    hot_deletion_completed_at      TIMESTAMPTZ,
    archive_deletion_pending       BOOLEAN     NOT NULL DEFAULT true,
    archive_deletion_completed_at  TIMESTAMPTZ,

    CONSTRAINT gdpr_unique_malo_tenant UNIQUE (malo_id, tenant)
);

-- Iceberg data files holding rows for an erased MaLo.
--
-- iceberg-rust 0.9.1 exposes only `fast_append` on a transaction — there is no
-- public API to remove or rewrite data files — so the physical deletion is
-- performed by an external engine (Spark, Trino) against this work list. Naming
-- the files is what makes the Art. 17 obligation dischargeable instead of
-- permanently pending.
CREATE TABLE gdpr_archive_files (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    deletion_id    UUID        NOT NULL REFERENCES gdpr_deletions(id) ON DELETE CASCADE,
    file_path      TEXT        NOT NULL,
    record_count   BIGINT,
    file_size_bytes BIGINT,
    planned_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Set when the operator's rewrite job has removed this file's affected rows.
    rewritten_at   TIMESTAMPTZ,
    tenant         TEXT        NOT NULL,

    CONSTRAINT gaf_unique_file UNIQUE (deletion_id, file_path)
);

CREATE INDEX gaf_deletion ON gdpr_archive_files (deletion_id);
-- Files still owed a rewrite.
CREATE INDEX gaf_outstanding ON gdpr_archive_files (tenant, planned_at)
    WHERE rewritten_at IS NULL;

CREATE INDEX gd_archive_pending ON gdpr_deletions (archive_deletion_pending)
    WHERE archive_deletion_pending = true;

-- ── BSI TR-03109 SMGW session registry (MsbG §21c / §14a EnWG) ──────────────
--
-- One row per SMGW device (by malo_id + tenant).  The full `SmgwSession` is
-- stored as JSONB so the `metering::SmgwSession` struct can be round-tripped
-- without splitting across many relational tables.
--
-- The GIN index enables fast certificate-expiry queries without a full table scan:
--   WHERE session -> 'certificates' @> '[{"cert_type":"TLS","is_revoked":false}]'
--
-- Column extraction: `status` and `device_id` are promoted to dedicated columns
-- so the compliance worker can do initial filtering without JSONB extraction.

CREATE TABLE smgw_sessions (
    malo_id         TEXT        NOT NULL,
    tenant          TEXT        NOT NULL,
    device_id       TEXT        NOT NULL,   -- SmgwSession.device_id (SMGW serial)
    msb_mp_id       TEXT        NOT NULL,   -- responsible MSB BDEW-Codenummer
    gateway_status  TEXT        NOT NULL DEFAULT 'OPERATIONAL'
                        CHECK (gateway_status IN (
                            'PROVISIONED','COMMISSIONED','OPERATIONAL',
                            'REVOKED','REPLACED','COMMUNICATION_FAULT'
                        )),
    session         JSONB       NOT NULL,   -- serialized SmgwSession (all fields)
    last_contact_at TIMESTAMPTZ,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (malo_id, tenant)
);

CREATE INDEX smgw_tenant_status  ON smgw_sessions (tenant, gateway_status);
CREATE INDEX smgw_last_contact   ON smgw_sessions (tenant, last_contact_at DESC)
    WHERE last_contact_at IS NOT NULL;
-- GIN index enables fast queries on certificates array and CLS channels:
--   SELECT ... WHERE session @> '{"status":"OPERATIONAL"}'
CREATE INDEX smgw_session_gin    ON smgw_sessions USING GIN (session);

-- ── §14a Fernsteuerbarkeit compliance audit log ───────────────────────────────
--
-- Append-only log of every compliance issue detected by the background worker
-- or the on-demand compliance scan (`POST /api/v1/smgw/compliance/scan`).
-- Each row corresponds to one emitted `de.edmd.cls.compliance_issue` CloudEvent.
--
-- `issue_type` maps to the MSB's legal exposure:
--   CERT_EXPIRED        — BNetzA can impose fines; §14a eligibility lost
--   CERT_EXPIRING       — 30-day advance warning; MSB must renew
--   TLS_CERT_MISSING    — SMGW unreachable via SMGW Admin Protocol
--   CLS_NOT_COMPLIANT   — §14a Konfigurationsprodukt not assigned; DSO control impossible
--   COMMUNICATION_FAULT — No contact > 2h; § 60 Abs. 2 MsbG substitute values required
--   GATEWAY_REVOKED     — Security incident; immediate replacement required (MsbG §29)

CREATE TABLE cls_compliance_log (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id         TEXT        NOT NULL,
    device_id       TEXT        NOT NULL,
    issue_type      TEXT        NOT NULL CHECK (issue_type IN (
                        'CERT_EXPIRED','CERT_EXPIRING','TLS_CERT_MISSING',
                        'CLS_NOT_COMPLIANT','COMMUNICATION_FAULT','GATEWAY_REVOKED'
                    )),
    severity        TEXT        NOT NULL CHECK (severity IN ('CRITICAL','WARNING')),
    cert_serial     TEXT,           -- for CERT_* issues
    cert_type       TEXT,           -- 'TLS', 'SIG', 'ENC', 'KEY_AGREEMENT'
    days_to_expiry  INTEGER,        -- negative = already expired
    channel_id      TEXT,           -- for CLS_NOT_COMPLIANT issues
    details         JSONB,          -- full issue context
    detected_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    cloud_event_id  TEXT,           -- CloudEvent `id` of emitted event
    tenant          TEXT        NOT NULL
);

-- Fast lookups for compliance dashboard and agentd smgw-diagnostics-agent:
CREATE INDEX ccl_malo_detected  ON cls_compliance_log (malo_id, detected_at DESC);
CREATE INDEX ccl_tenant_recent  ON cls_compliance_log (tenant, detected_at DESC);
CREATE INDEX ccl_open_critical  ON cls_compliance_log (tenant, issue_type, detected_at DESC)
    WHERE severity = 'CRITICAL';
CREATE INDEX ccl_issue_type     ON cls_compliance_log (issue_type, detected_at DESC);

-- `meter_reads` is range-partitioned monthly by core PostgreSQL (see the
-- `ensure_meter_reads_partitions` function above). TimescaleDB's
-- `create_hypertable` is not applicable to an already-partitioned table, and
-- native partitioning keeps the schema installable on any PostgreSQL 15+
-- instance without an extension.

-- ── § 60 Abs. 2 MsbG — Schätz-/Ersatzwert-Bestätigung ────────────────────────
-- Every stored ESTIMATED/SUBSTITUTED interval opens a confirmation entry: the
-- MSB owes a plausibilised real value. The entry resolves automatically when
-- a MEASURED/CORRECTED value for the same slot arrives (ingest or §-audit
-- correction path); a config-gated worker marks entries UEBERFAELLIG after
-- the operator-configured deadline (default 8 weeks — aligned with the
-- MaBiS Bilanzkreisabrechnung correction window; no statute fixes a number).

CREATE TABLE estimated_read_confirmations (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant         TEXT        NOT NULL,
    malo_id        TEXT        NOT NULL,
    dtm_from       TIMESTAMPTZ NOT NULL,
    dtm_to         TIMESTAMPTZ NOT NULL,
    obis_code_norm TEXT        NOT NULL DEFAULT '',
    -- Quality at creation: ESTIMATED or SUBSTITUTED.
    quality        TEXT        NOT NULL CHECK (quality IN ('ESTIMATED','SUBSTITUTED')),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    status         TEXT        NOT NULL DEFAULT 'OFFEN'
                       CHECK (status IN ('OFFEN','BESTAETIGT','UEBERFAELLIG')),
    resolved_at    TIMESTAMPTZ,
    -- Source of the resolving real value (e.g. MSCONS, DIRECT_PUSH, OPERATOR).
    resolved_by    TEXT,
    UNIQUE (tenant, malo_id, dtm_from, obis_code_norm)
);

CREATE INDEX erc_open ON estimated_read_confirmations (tenant, created_at)
    WHERE status IN ('OFFEN','UEBERFAELLIG');

COMMENT ON TABLE estimated_read_confirmations IS
    '§ 60 Abs. 2 MsbG: open obligations to replace estimated/substituted '
    'intervals with plausibilised real values. Auto-resolved by ingest of a '
    'MEASURED/CORRECTED value for the same (malo, dtm_from, register).';

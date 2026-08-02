-- edmd schema — single authoritative DDL for a clean PostgreSQL install.
--
-- All historical migrations have been consolidated into this one file.
-- Column types reflect the final state: NUMERIC(18,5) for kWh values,
-- TEXT NOT NULL for tenant isolation on every table (no nullable UUIDs),
-- and all indexes co-located with the table they serve.
--
-- § 60 Abs. 6 MsbG requires 5 decimal place kWh precision.
-- GDPR Art. 32 requires per-tenant data isolation on every table.
-- The authoritative `meter_reads` store (hot Postgres + cold Iceberg) and the
-- ESA `esa_typ2_reads` store are owned by the `meterstore` crate, not this file.

-- `btree_gist` provides GiST equality operators for TEXT so an interval-overlap
-- EXCLUDE constraint can combine equality columns with tstzrange overlap.
-- Shipped in postgres contrib; kept because meterstore's hot tier relies on it.
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

-- ── Authoritative meter reads + ESA Typ-2 → owned by meterstore ─────────────
--
-- The hot (recent PostgreSQL window) and cold (Apache Iceberg history) tiers for
-- `meter_reads` — and the second `esa_typ2_reads` table for the non-authoritative
-- ESA "Werte nach Typ 2" stream — are created and owned by the `meterstore` crate
-- (`store.create_tables()`), together with its Iceberg SqlCatalog tables. edmd no
-- longer declares them here, nor the monthly partition helpers, the Parquet export
-- bookkeeping (`archive_batches`) or the REST-catalog registry
-- (`iceberg_catalog_entries`). Everything below is an edmd *business* table that
-- stays in edmd's own PgPool.
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
    -- The full 8-value QualityFlag vocabulary, pinned at the DB layer (not just by
    -- the application `quality_to_str`): a drifting literal is refused by Postgres,
    -- not read back as UNKNOWN. schema_code_guard keeps this list == QualityFlag::CODES.
    quality              TEXT          NOT NULL DEFAULT 'UNKNOWN'
                             CHECK (quality IN (
                                 'MEASURED','ESTIMATED','SUBSTITUTED','CALCULATED',
                                 'CORRECTED','PRELIMINARY','FAULTY','UNKNOWN'
                             )),
    tenant               TEXT          NOT NULL,
    computed_at          TIMESTAMPTZ   NOT NULL DEFAULT now(),
    CONSTRAINT mbp_period_forward CHECK (period_to >= period_from)
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
    original_quality TEXT          NOT NULL
                         CHECK (original_quality IN (
                             'MEASURED','ESTIMATED','SUBSTITUTED','CALCULATED',
                             'CORRECTED','PRELIMINARY','FAULTY','UNKNOWN'
                         )),
    corrected_kwh    NUMERIC(18,5) NOT NULL,
    corrected_quality TEXT         NOT NULL
                         CHECK (corrected_quality IN (
                             'MEASURED','ESTIMATED','SUBSTITUTED','CALCULATED',
                             'CORRECTED','PRELIMINARY','FAULTY','UNKNOWN'
                         )),
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
    tenant           TEXT          NOT NULL,
    -- An audit row must cover a real interval, not a zero-width or reversed one.
    CONSTRAINT mrc_interval_forward CHECK (dtm_to > dtm_from)
    -- NOTE: legacy tenant_id UUID column removed; all tenant isolation uses
    -- tenant TEXT NOT NULL, consistent with meter_data_receipts and meter_reads.
);

CREATE INDEX mrc_malo_dtm         ON meter_read_corrections (malo_id, dtm_from, dtm_to);
CREATE INDEX mrc_malo_corrected_at ON meter_read_corrections (malo_id, corrected_at DESC);
CREATE INDEX mrc_tenant_malo       ON meter_read_corrections (tenant, malo_id, dtm_from DESC);

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
    -- ESA is a valid Auftraggeber: an ESA may order value delivery from the MSB
    -- (WiM Strom Teil 2 Kap. 4 / §60 Abs. 1 MsbG), so a reading order can be
    -- raised on its behalf.
    auftraggeber_rolle TEXT        NOT NULL
                           CHECK (auftraggeber_rolle IN ('LF','MSB','NB','ESA')),
    ausfuehrender_msb  TEXT,
    geplant_am         DATE        NOT NULL,
    ausfuehrt_bis      DATE,
    status             TEXT        NOT NULL DEFAULT 'OFFEN'
                           CHECK (status IN (
                               'OFFEN','BEAUFTRAGT','AUSGEFUEHRT',
                               'STORNIERT','FEHLGESCHLAGEN'
                           )),
    -- Register readings at NUMERIC(18,5) and Brennwert at NUMERIC(10,4), matching
    -- meter_billing_periods and gas_quality_data — one precision for a quantity,
    -- so a value copied between tables never loses decimals.
    zaehlerstand_kwh   NUMERIC(18,5),
    zaehlerstand_qm3   NUMERIC(18,5),
    brennwert          NUMERIC(10,4),
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
    tenant               TEXT          NOT NULL,
    CONSTRAINT gqd_period_forward CHECK (period_to >= period_from)
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
                           'MSCONS','DIRECT_PUSH','DIRECT_GAS','IOT_PUSH',
                           'API_IMPORT','CORRECTION','BATCH_RESCORE'
                       )),
    -- Rule findings behind the grade (V01–V10), so a disputed invoice can be
    -- traced to the specific check that failed rather than to a letter.
    issues_json    JSONB,
    -- MSCONS Prüfidentifikator, when the assessment came from a MaKo process.
    pid            INTEGER,
    assessed_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    tenant         TEXT        NOT NULL,
    CONSTRAINT qa_period_forward CHECK (period_to >= period_from)
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
    tenant          TEXT          NOT NULL,
    CONSTRAINT svl_interval_forward CHECK (dtm_to > dtm_from)
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
--
-- edmd's record of erasure *requests* — who asked, why, when. The erasure itself
-- is the destruction of the MaLo's subject mapping in meterstore's registry
-- (`meterstore_subject_map` / `meterstore_erasures`, in this same database): once
-- the mapping is gone the readings in both tiers are unattributable, so Art. 17 is
-- discharged without rewriting append-only Parquet — there is no external
-- file-rewrite step, and hence nothing to track here beyond the request.
CREATE TABLE gdpr_deletions (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id        TEXT        NOT NULL,
    tenant         TEXT        NOT NULL,
    reason         TEXT        NOT NULL,
    authorized_by  TEXT        NOT NULL,
    requested_at   TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT gdpr_unique_malo_tenant UNIQUE (malo_id, tenant)
);

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
-- Each row corresponds to one emitted `de.messwert.cls.compliance-issue` CloudEvent.
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

-- ── SMGW certificate expiry alert dedup ──────────────────────────────────────
-- One row per (certificate, threshold tier) so each 90/30/7-day tier emits
-- exactly once as the cert ages (BSI TR-03109-4 §6.3). `valid_to` is part of the
-- key so a renewed certificate (new expiry date) gets a fresh set of alerts.
-- `emitted = false` records a tier that was passed silently because a more urgent
-- tier fired in the same sweep (e.g. a cert first seen already inside 7 days).
CREATE TABLE smgw_cert_expiry_alerts (
    tenant          TEXT        NOT NULL,
    device_id       TEXT        NOT NULL,
    cert_serial     TEXT        NOT NULL,
    cert_type       TEXT        NOT NULL,   -- 'TLS', 'SIG', 'ENC', 'KEY_AGREEMENT'
    valid_to        DATE        NOT NULL,   -- SMGW_CERT_ABLAUFDATUM
    threshold_days  SMALLINT    NOT NULL CHECK (threshold_days IN (90, 30, 7)),
    days_to_expiry  INTEGER     NOT NULL,   -- days remaining when this tier was reached
    severity        TEXT        NOT NULL CHECK (severity IN ('CRITICAL','WARNING','INFO')),
    emitted         BOOLEAN     NOT NULL DEFAULT true,
    malo_id         TEXT        NOT NULL,
    cloud_event_id  TEXT,
    alerted_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant, device_id, cert_serial, valid_to, threshold_days)
);
CREATE INDEX scea_tenant_recent ON smgw_cert_expiry_alerts (tenant, alerted_at DESC);
CREATE INDEX scea_device        ON smgw_cert_expiry_alerts (tenant, device_id, valid_to);

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
    UNIQUE (tenant, malo_id, dtm_from, obis_code_norm),
    CONSTRAINT erc_interval_forward CHECK (dtm_to > dtm_from)
);

CREATE INDEX erc_open ON estimated_read_confirmations (tenant, created_at)
    WHERE status IN ('OFFEN','UEBERFAELLIG');

COMMENT ON TABLE estimated_read_confirmations IS
    '§ 60 Abs. 2 MsbG: open obligations to replace estimated/substituted '
    'intervals with plausibilised real values. Auto-resolved by ingest of a '
    'MEASURED/CORRECTED value for the same (malo, dtm_from, register).';

-- edmd schema — single authoritative DDL for a clean PostgreSQL install.
--
-- All historical migrations have been consolidated into this one file.
-- Column types reflect the final state: NUMERIC(18,5) for kWh values,
-- TEXT NOT NULL for tenant isolation on every table (no nullable UUIDs),
-- and all indexes co-located with the table they serve.
--
-- § 147 Abs. 1 AO / GoBD require a billed quantity to be recorded exactly; NUMERIC(18,5) matches the MSCONS MIG's decimal capability.
-- GDPR Art. 32 requires per-tenant data isolation on every table.
-- The authoritative `meter_reads` store (hot Postgres + cold Iceberg) and the
-- ESA `esa_typ2_reads` store are owned by the `meterstore` crate, not this file.

-- `btree_gist` provides GiST equality operators for TEXT so an interval-overlap
-- EXCLUDE constraint can combine equality columns with tstzrange overlap.
-- Shipped in postgres contrib; kept because meterstore's hot tier relies on it.
-- ── heute() — the business date ───────────────────────────────────────────────
--
-- Every date this schema compares against is a German calendar date — the day a
-- Frist runs out, a validity window opens, an obligation falls due.
-- PostgreSQL's own `current_date` answers the *session* time zone's date, which
-- on a UTC server is still yesterday between 23:00 and midnight Berlin time
-- (22:00 in summer). `heute()` states the conversion once, so it holds however
-- the connection was opened. The Rust side reads the same date through
-- `mako_fristen::heute`.
CREATE OR REPLACE FUNCTION heute() RETURNS date
    LANGUAGE sql STABLE
    AS $$ SELECT (now() AT TIME ZONE 'Europe/Berlin')::date $$;

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


-- ── ZSG conversion audit (§ 146 Abs. 4 AO) ──────────────────────────────────
--
-- Where a difference could not be taken honestly, `metering::reading` emits no
-- interval and records why. The hole then surfaces as a V01 gap and is filled,
-- with its own audit trail, by the § 60 Abs. 2 substitute path — so the two logs
-- together say "this quarter-hour is an Ersatzwert *because* the register went
-- backwards here", which neither says alone.
--
-- A reconstructed register wrap is recorded too, and is **not** an anomaly: the
-- interval it produced is real. It is logged because a wrap is the one place the
-- conversion adds 10^digits to a difference on the strength of a configured
-- device property, and an auditor is entitled to see where that happened.

CREATE TABLE zsg_conversion_log (
    id              UUID          PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant          TEXT          NOT NULL,
    malo_id         TEXT          NOT NULL,
    obis_code_norm  TEXT          NOT NULL DEFAULT '',
    span_from       TIMESTAMPTZ   NOT NULL,
    span_to         TIMESTAMPTZ   NOT NULL,
    -- 'ROLLOVER' for a reconstructed wrap (the interval exists), otherwise the
    -- `AnomalyKind` that refused the difference (the interval is absent).
    outcome         TEXT          NOT NULL
                        CHECK (outcome IN (
                            'ROLLOVER',
                            'BACKWARDS_WITHOUT_REGISTER_WIDTH',
                            'IMPLAUSIBLE_ROLLOVER',
                            'IMPLAUSIBLE_DELTA',
                            'ZERO_LENGTH_SPAN',
                            'NON_BILLABLE_ENDPOINT'
                        )),
    previous_value  NUMERIC(18,5) NOT NULL,
    current_value   NUMERIC(18,5) NOT NULL,
    -- Set only for a ROLLOVER: the reconstructed consumption and the register
    -- capacity that explains it.
    delta           NUMERIC(18,5),
    register_capacity NUMERIC(28,5),
    session_id      TEXT,
    created_at      TIMESTAMPTZ   NOT NULL DEFAULT now()
);

CREATE INDEX zsg_log_malo ON zsg_conversion_log (tenant, malo_id, span_from DESC);
CREATE INDEX zsg_log_outcome ON zsg_conversion_log (outcome) WHERE outcome <> 'ROLLOVER';

-- ── Bitemporal corrections (§ 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) audit trail) ──────────────────────────

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
    -- Which commodity is being read. A completed Ablesung files its Zählerstand
    -- into the Zählerstandsgang store, and that delivery has to name a Sparte:
    -- it decides the register's unit (kWh or m3) and the balancing day.
    -- Inferring it from which of the two Zählerstand columns was filled cannot
    -- distinguish Strom from Wärme, or Gas from Wasser.
    sparte             TEXT        NOT NULL DEFAULT 'STROM'
                           CHECK (sparte IN ('STROM','GAS','WAERME','WASSER')),
    -- OBIS register the reading was taken from, when the caller knows it. NULL
    -- is the unlabelled register — the right answer for a single-register SLP
    -- meter, and the wrong one for a point that also delivers a labelled
    -- Zählerstandsgang, where the two would become two registers.
    obis_code          TEXT,
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

-- The key is `(tenant, session_id)`, not `session_id` alone. Every read here is
-- already tenant-scoped — two tenants may legitimately mint the same session id
-- (an SMGW serial plus a timestamp is not globally unique) — but with a
-- tenant-blind primary key the ingest upsert's `ON CONFLICT` landed on the other
-- tenant's row and overwrote its status and quality summary.
CREATE TABLE direct_push_sessions (
    session_id      TEXT        NOT NULL,
    malo_id         TEXT        NOT NULL,
    source          TEXT        NOT NULL DEFAULT 'DIRECT_PUSH',
    obis_code       TEXT,
    interval_count  INTEGER     NOT NULL DEFAULT 0,
    period_from     TIMESTAMPTZ,
    period_to       TIMESTAMPTZ,
    status          TEXT        NOT NULL DEFAULT 'committed'
                        CHECK (status IN ('committed','partial','failed')),
    quality_summary JSONB,
    -- The undecoded uplink frame, for the IoT door (LoRaWAN / wM-Bus). Network
    -- server codecs are mutable and carry no version, so the stored value can
    -- only be re-derived from the original frame.
    raw_payload     TEXT,
    -- IoT provenance: the transport the uplink arrived over (LORAWAN / MBUS /
    -- WMBUS / REST) and the device that produced it (devEUI, M-Bus secondary
    -- address). Both used to be echoed back in the response and stored nowhere.
    --
    -- `device_id` lives here and **not** in the reading's `sender_mp_id`: that
    -- column is a BDEW Codenummer and keys the meterstore version scope, so a
    -- devEUI in it gave every device a scope of its own and no replacement
    -- device's readings could supersede the ones they corrected.
    transport       TEXT,
    device_id       TEXT,
    tenant          TEXT        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant, session_id)
);

CREATE INDEX dps_malo   ON direct_push_sessions (tenant, malo_id, created_at DESC);

-- ── Gas quality data ─────────────────────────────────────────────────────────
-- Brennwert + Zustandszahl per MaLo per period (PID 13007), written by
-- `record_gas_quality` on every Gasbeschaffenheit delivery.
--
-- Both factors are nullable: a delivery may carry only `QTY+Z08` (Brennwert) or
-- only `QTY+Z10` (Zustandszahl), and a NOT NULL pair forced the handler to
-- discard a half delivery rather than record what arrived. At least one must be
-- present, or the row records nothing.

CREATE TABLE gas_quality_data (
    id                   UUID          PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id              TEXT          NOT NULL,
    period_from          DATE          NOT NULL,
    period_to            DATE          NOT NULL,
    brennwert_kwh_per_m3 NUMERIC(10,4),
    zustandszahl         NUMERIC(8,4),
    source_pid           INTEGER,
    received_at          TIMESTAMPTZ   NOT NULL DEFAULT now(),
    tenant               TEXT          NOT NULL,
    CONSTRAINT gqd_period_forward CHECK (period_to >= period_from),
    CONSTRAINT gqd_has_a_value CHECK (
        brennwert_kwh_per_m3 IS NOT NULL OR zustandszahl IS NOT NULL
    )
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
-- `rule_type` must match `metering::VirtualMeterKind::as_str` exactly. It is the
-- same string the internally-tagged `rule_json` carries in its `kind` field, so
-- the column and the document cannot disagree; `edmd` deserialises `rule_json`
-- into `AggregationRule`, and a value here the enum does not know is an
-- unreadable row. `rule_type_check_matches_the_aggregation_rule_enum` pins the
-- list.
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
                            'SUM',
                            'RESIDUAL',
                            'PV_SELF_CONSUMPTION',
                            'GGV_CONSTANT_ALLOCATION',
                            'GGV_PROPORTIONAL_ALLOCATION'
                        )),
    -- Serialised `AggregationRule`, including its source MaLo-IDs.
    rule_json       JSONB,
    -- Statutory citation, e.g. '§42b EnWG' or '§42c EnWG'. Free text: it records
    -- which regime a community operates under, which `rule_type` cannot express.
    legal_basis     TEXT,
    sparte          TEXT        CHECK (sparte IS NULL OR sparte IN ('STROM', 'GAS', 'WAERME', 'WASSER')),
    valid_from      DATE        NOT NULL DEFAULT heute(),
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
    -- Ingest family the assessment was made for: the batch's own
    -- `IngestionSource`, plus `BATCH_RESCORE` for a retroactive re-scoring,
    -- which is not an ingest at all. The list must cover every variant of that
    -- enum — an insert naming one it omits fails the constraint, and the failure
    -- is only a warning, so the history goes silently missing for exactly the
    -- door that thought it was recording one. `schema_code_guard` pins the two
    -- together.
    source         TEXT        NOT NULL DEFAULT 'MSCONS'
                       CHECK (source IN (
                           'MSCONS','DIRECT_PUSH','DIRECT_GAS','API_IMPORT',
                           'AUTO_SUBSTITUTE','CORRECTION','MANUAL','ESTIMATED',
                           'IOT_PUSH','BATCH_RESCORE'
                       )),
    -- Rule findings behind the grade (V01–V09/V11/V12), so a disputed invoice
    -- can be traced to the specific check that failed rather than to a letter.
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
    -- Operator who authorised the Ersatzwert (§ 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) attributability).
    created_by      TEXT,
    created_at      TIMESTAMPTZ   NOT NULL DEFAULT now(),
    tenant          TEXT          NOT NULL,
    CONSTRAINT svl_interval_forward CHECK (dtm_to > dtm_from)
);

CREATE INDEX svl_malo_dtm ON substitute_value_log (malo_id, dtm_from, dtm_to);
CREATE INDEX svl_tenant   ON substitute_value_log (tenant);
CREATE INDEX svl_method   ON substitute_value_log (method);

-- ── Gerätewechsel: not an edmd table ─────────────────────────────────────────
--
-- A WiM Gerätewechsel is device master data, which `marktd` owns: marktd owns
-- MaLo/MeLo/Zähler/Gerät/SMGW identity, edmd owns interval data. An empty table
-- here would look like the system of record for meter exchanges.

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

-- ── BSI TR-03109 SMGW session registry (§ 25 MsbG / §14a EnWG) ──────────────
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

-- ── §14a Fernsteuerbarkeit compliance — open-issue register ──────────────────
--
-- **A register of what is wrong now, not a log of every time we looked.**
--
-- This was an append-only log, and the daily sweep wrote one row per open issue
-- per sweep and emitted one CloudEvent to match. A gateway sitting on an expired
-- certificate therefore produced a `de.messwert.cls.compliance-issue` every day
-- for as long as nobody fixed it — for a fleet, an unbounded event stream that
-- says the same thing forever, and a table that grows without limit. Worse, the
-- fleet list counted rows in the last 24 h and called that "issues", so the
-- number measured the sweep cadence rather than the fleet.
--
-- The identity of an issue is therefore `(tenant, device_id, issue_type,
-- cert_serial, channel_id)` — deliberately **not** `days_to_expiry`, which
-- changes every day and is the reason a naive key would still re-fire. An issue
-- is opened once, re-sighted silently, and resolved when a sweep no longer finds
-- it. Events fire on the transitions, which is what an operator can act on.
--
-- `cert_serial` and `channel_id` are NOT NULL DEFAULT '' so they can sit in the
-- key: a NULL would not compare equal to itself and every sweep would insert a
-- new row, reintroducing exactly the defect this shape removes.
--
-- `issue_type` maps to the MSB's legal exposure:
--   CERT_EXPIRED        — §14a eligibility lost; the SM-PKI chain no longer validates
--   CERT_REVOKED        — the SM-PKI withdrew it; a security incident, not an
--                         expiry, and the remedy is a new certificate now rather
--                         than a renewal before a deadline (§ 25 MsbG, § 28 MsbG)
--   CERT_NOT_YET_VALID  — `valid_from` is in the future: a provisioning fault, not
--                         a lifecycle one. It was reported as CERT_EXPIRED with a
--                         negative "days ago", which reads as a renewal problem
--                         and sends an operator to the wrong remedy
--   CERT_EXPIRING       — inside the operator's renewal warning window
--   TLS_CERT_MISSING    — SMGW unreachable over the Admin interface (BSI TR-03109-4)
--   CLS_NOT_COMPLIANT   — §14a Konfigurationsprodukt not assigned (BK6-22-300)
--   COMMUNICATION_FAULT — gateway silent past the threshold; § 60 Abs. 2 MsbG
--   GATEWAY_REVOKED     — security incident; § 25 MsbG reporting duty

CREATE TABLE cls_compliance_issues (
    tenant            TEXT        NOT NULL,
    device_id         TEXT        NOT NULL,
    issue_type        TEXT        NOT NULL CHECK (issue_type IN (
                          'CERT_EXPIRED','CERT_REVOKED','CERT_NOT_YET_VALID',
                          'CERT_EXPIRING','TLS_CERT_MISSING',
                          'CLS_NOT_COMPLIANT','COMMUNICATION_FAULT','GATEWAY_REVOKED'
                      )),
    -- Part of the identity: one expiring certificate is not another.
    cert_serial       TEXT        NOT NULL DEFAULT '',
    -- Part of the identity: one non-compliant CLS channel is not another.
    channel_id        TEXT        NOT NULL DEFAULT '',

    malo_id           TEXT        NOT NULL,
    severity          TEXT        NOT NULL CHECK (severity IN ('CRITICAL','WARNING')),
    cert_type         TEXT,           -- 'TLS', 'SIG', 'ENC', 'KEY_AGREEMENT'
    days_to_expiry    INTEGER,        -- negative = already expired; refreshed each sweep
    details           JSONB,          -- full issue context as last seen

    first_detected_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Set when a sweep no longer finds the issue. A later recurrence reopens the
    -- row and restarts `first_detected_at`, so "how long has this been broken"
    -- answers about the current episode rather than the first one ever.
    resolved_at       TIMESTAMPTZ,
    cloud_event_id    TEXT,

    PRIMARY KEY (tenant, device_id, issue_type, cert_serial, channel_id)
);

-- The fleet dashboard's only question: what is open right now?
CREATE INDEX cci_open ON cls_compliance_issues (tenant, severity, first_detected_at DESC)
    WHERE resolved_at IS NULL;
CREATE INDEX cci_malo ON cls_compliance_issues (tenant, malo_id, resolved_at);

-- ── Delivery surveillance — the points that stopped ──────────────────────────
--
-- Every other quality mechanism here judges data that **arrived**: the V-rules
-- run on an ingest batch, the Hampel scorer grades one, the § 60 Abs. 2
-- confirmation loop chases estimates already written. Silence triggers none of
-- them, so a measuring point that stops delivering was invisible until a
-- settlement run came up short — by which time the window for re-reading or
-- substituting the values had usually closed.
--
-- Like `cls_compliance_issues`, this is a register of what is wrong now rather
-- than a log of every time we looked: one row per (tenant, MaLo), opened once,
-- re-sighted silently, resolved when a sweep finds the point delivering again.
-- Events fire on the transitions.

CREATE TABLE delivery_surveillance (
    tenant            TEXT        NOT NULL,
    malo_id           TEXT        NOT NULL,
    -- Which value stream this row watches. The two are never mixed: a Typ-2
    -- value has no bearing on Netznutzungs-, Bilanzkreis- or
    -- Mehr-/Mindermengenabrechnung (Codeliste der Konfigurationen 1.4 Kap. 4.6),
    -- so a silent ESA subscription and a silent billing meter are different
    -- findings with different audiences.
    stream            TEXT        NOT NULL DEFAULT 'TYP1'
                                  CHECK (stream IN ('TYP1','TYP2')),
    -- The delivered register. Empty for TYP1, whose rows are per measuring
    -- point; a Typ-2 subscription delivers named OBIS registers and one can go
    -- dark while the others keep arriving.
    obis_code         TEXT        NOT NULL DEFAULT '',
    -- `SG1 RFF+AGI` on the delivering MSCONS 13027 — the Belegnummer of the
    -- ORDERS 17007 that ordered the values (MSCONS AHB 3.2 §11.2 hint [574]).
    -- Empty for TYP1, which has no subscription, and for a Typ-2 delivery whose
    -- sender omitted the Muss.
    --
    -- Part of the key because an ESA subscription is the (Meldepunkt,
    -- Messprodukt) pair and one Meldepunkt may carry several — the catalogue
    -- offers 9991 00000 305 6 and 9991 00000 314 7 for the same Marktlokation.
    -- Two subscriptions delivering the same OBIS register would otherwise share
    -- one row, and one going silent would be masked by the other.
    subscription_ref  TEXT        NOT NULL DEFAULT '',
    -- SILENT: nothing arrived for longer than the threshold.
    -- UNDER_COVERED: still delivering, but too little of the window to settle on.
    state             TEXT        NOT NULL CHECK (state IN ('SILENT','UNDER_COVERED')),
    -- End of the newest interval seen, and the gap from it to the sweep.
    last_interval_end TIMESTAMPTZ,
    hours_silent      BIGINT      NOT NULL DEFAULT 0,
    -- Share of the window spanned by intervals. Deliberately a *duration* ratio,
    -- not an interval count: a point that legitimately moved from quarter-hours
    -- to hours has a quarter of the intervals and the same coverage.
    coverage_pct      NUMERIC(5,2),
    interval_count    BIGINT      NOT NULL DEFAULT 0,

    first_detected_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at       TIMESTAMPTZ,

    PRIMARY KEY (tenant, stream, malo_id, obis_code, subscription_ref)
);

CREATE INDEX ds_open ON delivery_surveillance (tenant, state, first_detected_at)
    WHERE resolved_at IS NULL;

-- ── SMGW certificate expiry alert dedup ──────────────────────────────────────
-- One row per (certificate, threshold tier) so each 90/30/7-day tier emits
-- exactly once as the cert ages. The ladder is operational, not statutory: BSI
-- TR-03109-4 binds certificate runtimes and the Root-CP fixes the renewal lead
-- time. `valid_to` is part of the
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

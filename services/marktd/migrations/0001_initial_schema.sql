-- 0001_initial_schema.sql — marktd complete schema
--
-- Single authoritative schema. Drop and recreate the database to reset;
-- all application data is reproducible from the EDIFACT event streams in makod.
--
-- Design decisions:
--   • All timestamps: TIMESTAMPTZ (UTC).
--   • Date columns (valid_from/valid_to): plain DATE — the business meaning is a
--     calendar date in German local time, not a wall-clock instant.
--   • bo4e_version on JSONB tables: enables zero-downtime schema migrations when
--     BO4E v202601 ships. Write path always records current version.
--   • preisblaetter.source / pricat_versions.source: discriminates operator API
--     uploads ('api') from makod-sourced PRICAT 27003 ingest ('mako').

-- ── Marktlokation ─────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS malo (
    malo_id      TEXT        PRIMARY KEY,           -- 11-digit BDEW alternating-weight ID
    sparte       TEXT        NOT NULL CHECK (sparte IN ('STROM', 'GAS')),
    version      BIGINT      NOT NULL DEFAULT 1,
    data         JSONB       NOT NULL,              -- full BO4E MARKTLOKATION
    bo4e_version TEXT        NOT NULL DEFAULT 'v202501.0.0',
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ── Lokationszuordnung (role assignments, temporal) ───────────────────────────

CREATE TABLE IF NOT EXISTS lokationszuordnung (
    malo_id          TEXT  NOT NULL REFERENCES malo (malo_id) ON DELETE CASCADE,
    zuordnungstyp    TEXT  NOT NULL,                -- NB | GNB | MSB | GMSB | LF | LFG | …
    rollencodenummer TEXT  NOT NULL,                -- 13-digit BDEW/DVGW GLN
    valid_from       DATE  NOT NULL,
    valid_to         DATE,                          -- NULL = currently valid
    PRIMARY KEY (malo_id, zuordnungstyp, valid_from)
);

CREATE INDEX IF NOT EXISTS lokationszuordnung_malo_id
    ON lokationszuordnung (malo_id);
CREATE INDEX IF NOT EXISTS lokationszuordnung_rollencodenummer
    ON lokationszuordnung (rollencodenummer);

-- ── Messlokation ──────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS melo (
    melo_id      TEXT        PRIMARY KEY,           -- DE + 31 alphanumeric chars
    malo_id      TEXT        REFERENCES malo (malo_id) ON DELETE SET NULL,
    version      BIGINT      NOT NULL DEFAULT 1,
    data         JSONB       NOT NULL,              -- full BO4E MESSLOKATION
    bo4e_version TEXT        NOT NULL DEFAULT 'v202501.0.0',
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS melo_malo_id ON melo (malo_id);

-- ── Contracts (LF supply contracts) ──────────────────────────────────────────

CREATE TABLE IF NOT EXISTS contracts (
    contract_id  TEXT        PRIMARY KEY,           -- ERP contract number or UUID
    malo_id      TEXT        REFERENCES malo (malo_id) ON DELETE SET NULL,
    sparte       TEXT        NOT NULL CHECK (sparte IN ('STROM', 'GAS')),
    vertragsart  TEXT        NOT NULL,
    valid_from   DATE,                              -- NULL = no known start (open)
    valid_to     DATE,                              -- NULL = open-ended / currently active
    version      BIGINT      NOT NULL DEFAULT 1,
    data         JSONB       NOT NULL,              -- full BO4E VERTRAG + _mdm_billing
    bo4e_version TEXT        NOT NULL DEFAULT 'v202501.0.0',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS contracts_malo_id
    ON contracts (malo_id);
CREATE INDEX IF NOT EXISTS contracts_data_gin
    ON contracts USING GIN (data jsonb_path_ops);
CREATE INDEX IF NOT EXISTS contracts_malo_valid_from
    ON contracts (malo_id, valid_from DESC NULLS LAST);
CREATE INDEX IF NOT EXISTS contracts_malo_open_ended
    ON contracts (malo_id, valid_from DESC NULLS LAST)
    WHERE valid_to IS NULL;

-- ── NB network contracts ──────────────────────────────────────────────────────
--
-- Typed NB network contracts (netzebene, bilanzierungsmethode, billing_schedule).
-- Stored as typed columns — not opaque JSONB — to enable SQL-level queries and
-- prevent field-name drift between services.

CREATE TABLE IF NOT EXISTS nb_contracts (
    contract_id           TEXT        PRIMARY KEY,
    malo_id               TEXT        NOT NULL REFERENCES malo (malo_id) ON DELETE CASCADE,
    nb_mp_id                TEXT        NOT NULL,
    sparte                TEXT        NOT NULL CHECK (sparte IN ('STROM', 'GAS')),
    netzebene             TEXT        NOT NULL
                              CHECK (netzebene IN ('NS', 'MS', 'MSP', 'HSP', 'HS', 'HöS', 'HöS/HS')),
    bilanzierungsmethode  TEXT        NOT NULL CHECK (bilanzierungsmethode IN ('RLM', 'SLP')),
    billing_schedule      TEXT        NOT NULL
                              CHECK (billing_schedule IN ('MONTHLY', 'QUARTERLY', 'ANNUALLY')),
    valid_from            DATE        NOT NULL,
    valid_to              DATE,
    version               BIGINT      NOT NULL DEFAULT 1,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    tenant                TEXT        NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS nb_contracts_malo_nb_from
    ON nb_contracts (malo_id, nb_mp_id, valid_from, tenant);
CREATE INDEX IF NOT EXISTS nb_contracts_nb_gln
    ON nb_contracts (nb_mp_id, tenant);
CREATE INDEX IF NOT EXISTS nb_contracts_malo_id
    ON nb_contracts (malo_id);

-- ── VersorgungsStatus per MaLo ────────────────────────────────────────────────
--
-- One row per (malo_id, tenant). Derived from de.mako.process.completed events
-- by the event_ingest handler. Used by processd (M17) to drive automated LFA
-- E_0624 responses without ERP involvement.
--
-- Optimistic concurrency via version: UPDATE ... WHERE malo_id=$1 AND tenant=$2
-- AND version=$3. Zero rows → conflict → caller retries after re-read.

CREATE TABLE IF NOT EXISTS versorgungsstatus (
    malo_id           TEXT        NOT NULL,
    tenant            TEXT        NOT NULL,
    lieferstatus      TEXT        NOT NULL CHECK (lieferstatus IN (
                          'Beliefert',
                          'Unbeliefert',
                          'Grundversorgung',
                          'Ersatzversorgung',
                          'Ruhend',
                          'Stillgelegt'
                      )),
    lf_mp_id            TEXT,
    lf_gln_next       TEXT,
    lieferbeginn      DATE,
    lieferende        DATE,
    msb_mp_id           TEXT,
    nb_mp_id            TEXT        NOT NULL,
    last_process_id   UUID,
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    version           BIGINT      NOT NULL DEFAULT 1,

    PRIMARY KEY (malo_id, tenant)
);

CREATE INDEX IF NOT EXISTS versorgungsstatus_tenant_status
    ON versorgungsstatus (tenant, lieferstatus);
CREATE INDEX IF NOT EXISTS versorgungsstatus_tenant_lf
    ON versorgungsstatus (tenant, lf_mp_id)
    WHERE lf_mp_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS versorgungsstatus_tenant_nb
    ON versorgungsstatus (tenant, nb_mp_id);

-- ── NB price sheets (PreisblattNetznutzung) ───────────────────────────────────
--
-- Stores BO4E PreisblattNetznutzung objects published by Netzbetreiber.
-- invoicd queries this table via GET /api/v1/preisblaetter/{nb_mp_id}?date=…
--
-- source='api'  — operator REST upload (override protection: 'mako' won't
--                 overwrite unless forced).
-- source='mako' — ingested automatically from a PRICAT 27003 message.

CREATE TABLE IF NOT EXISTS preisblaetter (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    nb_mp_id       TEXT        NOT NULL,
    valid_from   DATE,                              -- gueltigkeit.startdatum; NULL = open-started
    valid_to     DATE,                              -- gueltigkeit.enddatum;   NULL = open-ended
    data         JSONB       NOT NULL,
    bo4e_version TEXT        NOT NULL DEFAULT 'v202501.0.0',
    source       TEXT        NOT NULL DEFAULT 'api'
                             CHECK (source IN ('api', 'mako')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (nb_mp_id, valid_from)
);

CREATE INDEX IF NOT EXISTS preisblaetter_nb_gln_valid_from
    ON preisblaetter (nb_mp_id, valid_from DESC NULLS LAST);
CREATE INDEX IF NOT EXISTS preisblaetter_data_gin
    ON preisblaetter USING GIN (data jsonb_path_ops);
CREATE INDEX IF NOT EXISTS preisblaetter_api_source
    ON preisblaetter (nb_mp_id)
    WHERE source = 'api';

-- ── PRICAT version history + dispatch log ─────────────────────────────────────
--
-- pricat_versions: versioned history of PreisblattNetznutzung per NB.
--   Populated by PUT /api/v1/preisblaetter/{nb_mp_id}.
--   Replaces single-row preisblaetter as the primary versioned source;
--   preisblaetter remains for point-in-time reads (invoicd, MCP server).
--
-- pricat_dispatch_log: one row per NB × LF pair per version — audit trail of
--   every PRICAT 27003 outbound dispatch.
--
-- Dispatch pipeline:
--   1. PUT /api/v1/preisblaetter/{nb_mp_id} → writes preisblaetter + pricat_versions
--   2. Background task dispatches PRICAT 27003 per active LF GLN via MakodClient
--   3. On de.markt.partner.activated { role: "LF" }, latest pricat_version for
--      the NB is dispatched to the new partner only.

CREATE TABLE IF NOT EXISTS pricat_versions (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    nb_mp_id              TEXT        NOT NULL,
    tenant              TEXT        NOT NULL,
    valid_from          DATE        NOT NULL,
    valid_to            DATE,
    data                JSONB       NOT NULL,
    bo4e_version        TEXT        NOT NULL DEFAULT 'v202501.0.0',
    source              TEXT        NOT NULL DEFAULT 'api'
                        CHECK (source IN ('api', 'mako')),
    dispatch_queued_at  TIMESTAMPTZ,
    dispatch_done_at    TIMESTAMPTZ,
    dispatch_error      TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS pricat_versions_nb_tenant_from
    ON pricat_versions (nb_mp_id, tenant, valid_from);
CREATE INDEX IF NOT EXISTS pricat_versions_undispatched
    ON pricat_versions (tenant, nb_mp_id, valid_from DESC)
    WHERE dispatch_done_at IS NULL;

CREATE TABLE IF NOT EXISTS pricat_dispatch_log (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    pricat_version_id   UUID        NOT NULL REFERENCES pricat_versions (id) ON DELETE CASCADE,
    nb_mp_id              TEXT        NOT NULL,
    lf_mp_id              TEXT        NOT NULL,
    tenant              TEXT        NOT NULL,
    process_id          UUID,
    dispatched_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    outcome             TEXT        NOT NULL DEFAULT 'ok'
                        CHECK (outcome IN ('ok', 'error')),
    error_detail        TEXT
);

CREATE INDEX IF NOT EXISTS pricat_dispatch_log_version
    ON pricat_dispatch_log (pricat_version_id);
CREATE INDEX IF NOT EXISTS pricat_dispatch_log_lf
    ON pricat_dispatch_log (tenant, lf_mp_id, dispatched_at DESC);

-- ── Process correlation index ──────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS process_correlation (
    process_id       UUID        PRIMARY KEY,       -- makod WorkflowId
    workflow_name    TEXT,                          -- e.g. "gpke-supplier-change"
    pid              INTEGER,                       -- BDEW Prüfidentifikator
    malo_id          TEXT,
    melo_id          TEXT,
    contract_id      TEXT,
    erp_contract_id  TEXT,
    erp_order_id     TEXT,
    edifact_conv_id  UUID,                          -- from makoconvid CE extension
    marktrolle       TEXT,                          -- canonical role (NB, LF, MSB, UNB, …)
    format_version   TEXT,                          -- e.g. "FV2026-10-01"
    status           TEXT        NOT NULL CHECK (status IN ('RUNNING', 'COMPLETED', 'FAILED')),
    initiated_at     TIMESTAMPTZ NOT NULL,
    completed_at     TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS process_correlation_erp_order_id
    ON process_correlation (erp_order_id);
CREATE INDEX IF NOT EXISTS process_correlation_malo_id_status
    ON process_correlation (malo_id, status);
CREATE INDEX IF NOT EXISTS process_correlation_edifact_conv_id
    ON process_correlation (edifact_conv_id);
CREATE INDEX IF NOT EXISTS process_correlation_running
    ON process_correlation (malo_id, initiated_at)
    WHERE status = 'RUNNING';

-- ── Webhook subscriptions ─────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS subscriptions (
    subscriber_id  TEXT        PRIMARY KEY,
    webhook_url    TEXT        NOT NULL,
    webhook_secret TEXT,                            -- AES-256-GCM encrypted (base64); NULL = no HMAC
    roles          TEXT[]      NOT NULL DEFAULT '{}',
    event_types    TEXT[]      NOT NULL DEFAULT '{}',
    sparten        TEXT[]      NOT NULL DEFAULT '{}',
    active         BOOLEAN     NOT NULL DEFAULT true,
    version        BIGINT      NOT NULL DEFAULT 1,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ── Trading partners ──────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS partners (
    mp_id        TEXT        PRIMARY KEY,           -- 13-digit BDEW/DVGW/GS1 MP-ID
    display_name TEXT,
    marktrolle   TEXT,
    sparte       TEXT        CHECK (sparte IN ('STROM', 'GAS')),
    channels     JSONB       NOT NULL DEFAULT '[]',
    version      BIGINT      NOT NULL DEFAULT 1,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS malo_data_gin         ON malo     USING GIN (data jsonb_path_ops);
CREATE INDEX IF NOT EXISTS melo_data_gin         ON melo     USING GIN (data jsonb_path_ops);
CREATE INDEX IF NOT EXISTS partners_channels_gin ON partners USING GIN (channels jsonb_path_ops);

-- ── Idempotency dedup for inbound makod events ────────────────────────────────
-- Purge entries older than 7 days via a scheduled DELETE in background worker.

CREATE TABLE IF NOT EXISTS processed_events (
    event_id     TEXT        PRIMARY KEY,           -- CloudEvents "id" (UUID v4)
    processed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS processed_events_processed_at
    ON processed_events (processed_at);


-- ── Phase 3: versorgungsstatus history + nelo ──────────────────────────────────
-- (merged from 0002_phase3_history.sql)

-- ── VersorgungsStatus history ─────────────────────────────────────────────────
--
-- One row per state transition.  valid_from = UTC instant when this state
-- became active (set by the application; not a trigger, so it equals the
-- timestamp committed in the same transaction as the versorgungsstatus upsert).
--
-- Point-in-time query:
--   SELECT * FROM versorgungsstatus_history
--   WHERE malo_id = $1 AND tenant = $2
--     AND (valid_from AT TIME ZONE 'Europe/Berlin')::date <= $at_date
--   ORDER BY valid_from DESC LIMIT 1

CREATE TABLE IF NOT EXISTS versorgungsstatus_history (
    id               BIGSERIAL   PRIMARY KEY,
    malo_id          TEXT        NOT NULL,
    tenant           TEXT        NOT NULL,
    lieferstatus     TEXT        NOT NULL,
    lf_mp_id           TEXT,
    lf_gln_next      TEXT,
    lieferbeginn     DATE,
    lieferende       DATE,
    msb_mp_id          TEXT,
    nb_mp_id           TEXT        NOT NULL,
    last_process_id  UUID,
    version          BIGINT      NOT NULL,                    -- version of this state
    valid_from       TIMESTAMPTZ NOT NULL DEFAULT now()       -- when this state became active
);

-- Primary query pattern: most-recent state for a MaLo up to an instant.
CREATE INDEX IF NOT EXISTS versorgungsstatus_history_at
    ON versorgungsstatus_history (malo_id, tenant, valid_from DESC);

-- Lookup by version for audit / correlation.
CREATE INDEX IF NOT EXISTS versorgungsstatus_history_version
    ON versorgungsstatus_history (malo_id, tenant, version);

-- ── Netz-Element-Lokation (NeLo) — Redispatch 2.0 ────────────────────────────
--
-- Stores network element locations used in BDEW Redispatch 2.0 processes.
-- NeLo-ID: 16-char EIC code (ENTSO-E agency, DE3055 = ZEW) or 13-digit BDEW
-- Codenummer.  One row per (nelo_id, tenant).
--
-- Source: BDEW Redispatch 2.0 Implementierungsleitfaden v2.x.

CREATE TABLE IF NOT EXISTS nelo (
    nelo_id      TEXT        NOT NULL,                        -- EIC or BDEW Codenummer
    tenant       TEXT        NOT NULL,
    name         TEXT,                                        -- human-readable Bezeichnung
    sparte       TEXT        NOT NULL CHECK (sparte IN ('STROM', 'GAS')),
    netzebene    TEXT        CHECK (
                     netzebene IN ('NS', 'MS', 'MSP', 'HSP', 'HS', 'HöS', 'HöS/HS')
                 ),
    nb_mp_id       TEXT        NOT NULL,                        -- owning Netzbetreiber GLN
    data         JSONB       NOT NULL DEFAULT '{}',           -- additional Redispatch 2.0 attributes
    version      BIGINT      NOT NULL DEFAULT 1,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (nelo_id, tenant)
);

CREATE INDEX IF NOT EXISTS nelo_nb_gln  ON nelo (tenant, nb_mp_id);
CREATE INDEX IF NOT EXISTS nelo_tenant  ON nelo (tenant);


-- ── MaLo grid topology (N7) ─────────────────────────────────────────────────────
-- (merged from 0003_malo_grid.sql)

CREATE TABLE IF NOT EXISTS malo_grid (
    malo_id              TEXT        NOT NULL,
    tenant               TEXT        NOT NULL,
    nb_mp_id               TEXT        NOT NULL,
    bilanzierungsgebiet  TEXT,                   -- Bilanzierungsgebiet-EIC (LOC+237)
    netzgebiet           TEXT,                   -- NB-internal grid area code
    sparte               TEXT        NOT NULL,   -- 'STROM' | 'GAS'
    source               TEXT        NOT NULL DEFAULT 'manual',  -- 'mastr' | 'nis' | 'manual'
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (malo_id, tenant)
);

-- Index by NB MP-ID for bulk export (nis-syncd uses this)
CREATE INDEX IF NOT EXISTS malo_grid_nb_gln
    ON malo_grid (nb_mp_id, tenant);

-- Index by Bilanzierungsgebiet for NB area queries
CREATE INDEX IF NOT EXISTS malo_grid_big
    ON malo_grid (bilanzierungsgebiet, tenant)
    WHERE bilanzierungsgebiet IS NOT NULL;

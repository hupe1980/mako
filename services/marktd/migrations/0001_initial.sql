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

CREATE TABLE malo (
    malo_id      TEXT        PRIMARY KEY,           -- 11-digit BDEW alternating-weight ID
    sparte       TEXT        NOT NULL CHECK (sparte IN ('STROM', 'GAS')),
    -- Typed columns extracted from BO4E Marktlokation JSONB at write time.
    -- NULL when the incoming data does not carry the field.
    netzebene            TEXT,
    bilanzierungsgebiet  TEXT,  -- Bilanzierungsgebiet-EIC; drives processd NB check 4
    gasqualitaet         TEXT,  -- 'HGas' | 'LGas'; Gas routing
    energierichtung      TEXT,  -- 'Aussp' | 'Einsp'; generation vs consumption
    bilanzierungsmethode TEXT,  -- 'RLM' | 'SLP' | 'IMS' | 'TLP_*'; drives netzbilanzd Leistungspreis routing
    regelzone            TEXT,  -- Regelzone EIC code; maps MaLo → ÜNB for MABIS IFTSTA + Redispatch 2.0
    fallgruppe           TEXT,  -- Gas GaBi RLM Fallgruppe: 'GABI_RLM_MIT_TAGESBAND' | 'GABI_RLM_OHNE_TAGESBAND'
                                --   | 'GABI_RLM_IM_NOMINIERUNGSERSATZVERFAHREN'
                                -- Determines GaBi billing category for Gas RLM MaLos.
    version      BIGINT      NOT NULL DEFAULT 1,
    data         JSONB       NOT NULL,              -- full BO4E MARKTLOKATION
    bo4e_version TEXT        NOT NULL DEFAULT 'v202607.0.0',
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX malo_netzebene ON malo (netzebene) WHERE netzebene IS NOT NULL;
CREATE INDEX malo_big ON malo (bilanzierungsgebiet) WHERE bilanzierungsgebiet IS NOT NULL;
CREATE INDEX malo_bilanzierungsmethode ON malo (bilanzierungsmethode) WHERE bilanzierungsmethode IS NOT NULL;
CREATE INDEX malo_regelzone ON malo (regelzone) WHERE regelzone IS NOT NULL;
CREATE INDEX malo_fallgruppe ON malo (fallgruppe) WHERE fallgruppe IS NOT NULL;

-- ── Lokationszuordnung (role assignments, temporal) ───────────────────────────

CREATE TABLE lokationszuordnung (
    malo_id          TEXT  NOT NULL REFERENCES malo (malo_id) ON DELETE CASCADE,
    zuordnungstyp    TEXT  NOT NULL,                -- NB | GNB | MSB | GMSB | LF | LFG | …
    rollencodenummer TEXT  NOT NULL,                -- 13-digit BDEW/DVGW GLN
    valid_from       DATE  NOT NULL,
    valid_to         DATE,                          -- NULL = currently valid
    PRIMARY KEY (malo_id, zuordnungstyp, valid_from)
);

CREATE INDEX lokationszuordnung_malo_id
    ON lokationszuordnung (malo_id);
CREATE INDEX lokationszuordnung_rollencodenummer
    ON lokationszuordnung (rollencodenummer);

-- ── Messlokation ──────────────────────────────────────────────────────────────

CREATE TABLE melo (
    melo_id      TEXT        PRIMARY KEY,           -- DE + 31 alphanumeric chars
    malo_id      TEXT        REFERENCES malo (malo_id) ON DELETE SET NULL,
    -- Typed columns extracted from BO4E Messlokation JSONB at write time.
    netzebene_messung      TEXT,   -- voltage / pressure level at the metering point
    regelzone              TEXT,   -- Regelzone EIC (Standorteigenschaften.eigenschaftenStrom[0].regelzone)
                                   -- maps MeLo → ÜNB for Redispatch 2.0 Stammdaten + MABIS IFTSTA 21000
    -- Full BO4E Standorteigenschaften JSONB — stored for Redispatch 2.0 NetworkConstraintDocument
    -- and Gas billing zone lookup (StandorteigenschaftenGas.druckstufe, bilanzierungsgebietEic).
    -- NULL when the incoming PUT does not carry standorteigenschaften.
    standorteigenschaften  JSONB,
    version      BIGINT      NOT NULL DEFAULT 1,
    data         JSONB       NOT NULL,              -- full BO4E MESSLOKATION
    bo4e_version TEXT        NOT NULL DEFAULT 'v202607.0.0',
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX melo_malo_id ON melo (malo_id);
CREATE INDEX melo_regelzone ON melo (regelzone) WHERE regelzone IS NOT NULL;
CREATE INDEX melo_standorteigenschaften_gin
    ON melo USING GIN (standorteigenschaften jsonb_path_ops)
    WHERE standorteigenschaften IS NOT NULL;

-- ── Contracts (LF supply contracts) ──────────────────────────────────────────

CREATE TABLE contracts (
    contract_id  TEXT        PRIMARY KEY,           -- ERP contract number or UUID
    malo_id      TEXT        REFERENCES malo (malo_id) ON DELETE SET NULL,
    sparte       TEXT        NOT NULL CHECK (sparte IN ('STROM', 'GAS')),
    vertragsart  TEXT        NOT NULL,
    valid_from   DATE,                              -- NULL = no known start (open)
    valid_to     DATE,                              -- NULL = open-ended / currently active
    version      BIGINT      NOT NULL DEFAULT 1,
    data         JSONB       NOT NULL,              -- full BO4E VERTRAG + _mdm_billing
    bo4e_version TEXT        NOT NULL DEFAULT 'v202607.0.0',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX contracts_malo_id
    ON contracts (malo_id);
CREATE INDEX contracts_data_gin
    ON contracts USING GIN (data jsonb_path_ops);
CREATE INDEX contracts_malo_valid_from
    ON contracts (malo_id, valid_from DESC NULLS LAST);
CREATE INDEX contracts_malo_open_ended
    ON contracts (malo_id, valid_from DESC NULLS LAST)
    WHERE valid_to IS NULL;

-- ── NB network contracts ──────────────────────────────────────────────────────
--
-- Typed NB network contracts (netzebene, bilanzierungsmethode, billing_schedule).
-- Stored as typed columns + full BO4E Vertrag JSONB for ERP digital LRV exchange (L1).
-- Typed columns remain for fast SQL-level queries by invoicd and processd.

CREATE TABLE nb_contracts (
    contract_id           TEXT        PRIMARY KEY,
    malo_id               TEXT        NOT NULL REFERENCES malo (malo_id) ON DELETE CASCADE,
    nb_mp_id              TEXT        NOT NULL,
    sparte                TEXT        NOT NULL CHECK (sparte IN ('STROM', 'GAS')),
    -- netzebene: Strom (NS/MS/MSP/HSP/HS/HöS/HöS/HS) + Gas (GND/GMT/GHD) values allowed.
    -- Free-text to support all energy types; validated at the application layer.
    netzebene             TEXT        NOT NULL,
    -- bilanzierungsmethode: RLM | SLP | IMS | TLP_GEMEINSAM | TLP_GETRENNT | PAUSCHAL
    bilanzierungsmethode  TEXT        NOT NULL,
    billing_schedule      TEXT        NOT NULL
                              CHECK (billing_schedule IN ('MONTHLY', 'QUARTERLY', 'ANNUALLY')),
    valid_from            DATE        NOT NULL,
    valid_to              DATE,
    -- Full BO4E Vertrag payload — stored for ERP digital LRV exchange.
    -- _typ auto-injected to "VERTRAG" on write. Empty object for records
    -- created before L1 was deployed (re-PUT to populate).
    data                  JSONB       NOT NULL DEFAULT '{}'::jsonb,
    -- vertragsart: extracted from data["vertragsart"] — fast filter for LRV vs Netznutzung.
    vertragsart           TEXT        DEFAULT 'NETZNUTZUNGSVERTRAG',
    -- vertragsstatus: extracted from data["vertragsstatus"] — lifecycle (AKTIV / BEENDET / …).
    vertragsstatus        TEXT        DEFAULT 'AKTIV',
    version               BIGINT      NOT NULL DEFAULT 1,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    tenant                TEXT        NOT NULL
);

CREATE UNIQUE INDEX nb_contracts_malo_nb_from
    ON nb_contracts (malo_id, nb_mp_id, valid_from, tenant);
CREATE INDEX nb_contracts_nb_gln
    ON nb_contracts (nb_mp_id, tenant);
CREATE INDEX nb_contracts_malo_id
    ON nb_contracts (malo_id);
CREATE INDEX nb_contracts_vertragsart
    ON nb_contracts (vertragsart, tenant) WHERE vertragsart IS NOT NULL;

-- ── VersorgungsStatus per MaLo ────────────────────────────────────────────────
--
-- One row per (malo_id, tenant). Derived from de.mako.process.completed events
-- by the event_ingest handler. Used by processd (M17) to drive automated LFA
-- E_0624 responses without ERP involvement.
--
-- Optimistic concurrency via version: UPDATE ... WHERE malo_id=$1 AND tenant=$2
-- AND version=$3. Zero rows → conflict → caller retries after re-read.

CREATE TABLE versorgungsstatus (
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
    lf_mp_id            TEXT,                -- MP-ID of the active Lieferant (set when lieferstatus = 'Beliefert')
    lf_mp_id_next       TEXT,                -- MP-ID of the announced future LF (WHO; set on 55001/44001 receipt; cleared on 55003/44003 or 55004/44004)
    lf_next_lieferbeginn DATE,               -- Announced Lieferbeginn of the future LF (WHEN; paired with lf_mp_id_next)
    lieferbeginn      DATE,
    lieferende        DATE,
    msb_mp_id           TEXT,
    nb_mp_id            TEXT        NOT NULL,
    last_process_id   UUID,
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    version           BIGINT      NOT NULL DEFAULT 1,

    PRIMARY KEY (malo_id, tenant)
);

CREATE INDEX versorgungsstatus_tenant_status
    ON versorgungsstatus (tenant, lieferstatus);
CREATE INDEX versorgungsstatus_tenant_lf
    ON versorgungsstatus (tenant, lf_mp_id)
    WHERE lf_mp_id IS NOT NULL;
CREATE INDEX versorgungsstatus_tenant_nb
    ON versorgungsstatus (tenant, nb_mp_id);

-- ── NB price sheets (PreisblattNetznutzung) ───────────────────────────────────
--
-- Stores BO4E PreisblattNetznutzung objects published by Netzbetreiber.
-- invoicd queries this table via GET /api/v1/preisblaetter/{nb_mp_id}?date=…
--
-- source='api'  — operator REST upload (override protection: 'mako' won't
--                 overwrite unless forced).
-- source='mako' — ingested automatically from a PRICAT 27003 message.

CREATE TABLE preisblaetter (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    nb_mp_id       TEXT        NOT NULL,
    valid_from   DATE,                              -- gueltigkeit.startdatum; NULL = open-started
    valid_to     DATE,                              -- gueltigkeit.enddatum;   NULL = open-ended
    data         JSONB       NOT NULL,
    bo4e_version TEXT        NOT NULL DEFAULT 'v202607.0.0',
    source       TEXT        NOT NULL DEFAULT 'api'
                             CHECK (source IN ('api', 'mako')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (nb_mp_id, valid_from)
);

CREATE INDEX preisblaetter_nb_gln_valid_from
    ON preisblaetter (nb_mp_id, valid_from DESC NULLS LAST);
CREATE INDEX preisblaetter_data_gin
    ON preisblaetter USING GIN (data jsonb_path_ops);
CREATE INDEX preisblaetter_api_source
    ON preisblaetter (nb_mp_id)
    WHERE source = 'api';

-- ── PreisblattMessung — MSB metering price sheets (B5) ───────────────────────
--
-- Stores BO4E PreisblattMessung objects published by Messstellenbetreiber (MSB).
-- invoicd queries this table for PID 31009 (MSB-Rechnung) tariff plausibility
-- checks (positions 4+5: Grundpreis + Arbeitspreis Messung).
--
-- source='api'  — operator REST upload.
-- source='mako' — ingested automatically from a PRICAT message (future).

CREATE TABLE preisblaetter_messung (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    msb_mp_id    TEXT        NOT NULL,
    valid_from   DATE,                              -- gueltigkeit.startdatum; NULL = open-started
    valid_to     DATE,                              -- gueltigkeit.enddatum;   NULL = open-ended
    data         JSONB       NOT NULL,
    bo4e_version TEXT        NOT NULL DEFAULT 'v202607.0.0',
    source       TEXT        NOT NULL DEFAULT 'api'
                             CHECK (source IN ('api', 'mako')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (msb_mp_id, valid_from)
);

CREATE INDEX preisblaetter_messung_msb_valid_from
    ON preisblaetter_messung (msb_mp_id, valid_from DESC NULLS LAST);
CREATE INDEX preisblaetter_messung_data_gin
    ON preisblaetter_messung USING GIN (data jsonb_path_ops);
CREATE INDEX preisblaetter_messung_api_source
    ON preisblaetter_messung (msb_mp_id)
    WHERE source = 'api';

-- ── PreisblattKonzessionsabgabe — KA price sheets (B3) ───────────────────────
--
-- Stores BO4E PreisblattKonzessionsabgabe objects published by Netzbetreiber.
-- netzbilanzd queries this table for KA tariff positions in INVOIC 31001/31002.
-- §17 StromNZV requires Konzessionsabgabe as a separate position in every NNE
-- invoice; `kundengruppe_ka` differentiates Tarifkunden and Sondervertragskunden.
--
-- source='api'  — operator REST upload.
-- source='mako' — ingested automatically (future).

CREATE TABLE preisblaetter_konzessionsabgabe (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    nb_mp_id        TEXT        NOT NULL,
    sparte          TEXT        NOT NULL DEFAULT 'STROM' CHECK (sparte IN ('STROM', 'GAS')),
    kundengruppe_ka TEXT,                              -- 'Tarifkunden' | 'Sondervertragskunden' | NULL = both
    valid_from      DATE,
    valid_to        DATE,
    data            JSONB       NOT NULL,
    bo4e_version    TEXT        NOT NULL DEFAULT 'v202607.0.0',
    source          TEXT        NOT NULL DEFAULT 'api'
                                CHECK (source IN ('api', 'mako')),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (nb_mp_id, sparte, kundengruppe_ka, valid_from)
);

CREATE INDEX preisblaetter_ka_nb_valid_from
    ON preisblaetter_konzessionsabgabe (nb_mp_id, sparte, valid_from DESC NULLS LAST);
CREATE INDEX preisblaetter_ka_data_gin
    ON preisblaetter_konzessionsabgabe USING GIN (data jsonb_path_ops);
CREATE INDEX preisblaetter_ka_api_source
    ON preisblaetter_konzessionsabgabe (nb_mp_id)
    WHERE source = 'api';

-- ── PreisblattDienstleistung — MSB service price sheets (M2/MSB) ─────────────
--
-- Stores BO4E PreisblattDienstleistung objects published by Messstellenbetreiber.
-- invoic-checker uses this for INVOIC 31009 service position plausibility.
-- REQOTE/QUOTES (PIDs 35001–35005) use this as the basis for Messentgelte offers.

CREATE TABLE preisblaetter_dienstleistung (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    msb_mp_id    TEXT        NOT NULL,
    valid_from   DATE,
    valid_to     DATE,
    data         JSONB       NOT NULL,
    bo4e_version TEXT        NOT NULL DEFAULT 'v202607.0.0',
    source       TEXT        NOT NULL DEFAULT 'api' CHECK (source IN ('api', 'mako')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (msb_mp_id, valid_from)
);

CREATE INDEX preisblaetter_dl_msb ON preisblaetter_dienstleistung (msb_mp_id, valid_from DESC NULLS LAST);
CREATE INDEX preisblaetter_dl_gin ON preisblaetter_dienstleistung USING GIN (data jsonb_path_ops);

-- ── PreisblattHardware — MSB hardware rental price sheets (M3/MSB) ───────────
--
-- Stores BO4E PreisblattHardware objects published by Messstellenbetreiber.
-- Required for NB → MSB settlement INVOIC 31009 hardware positions.
-- invoic-checker check 5 cannot validate hardware without a typed tariff.

CREATE TABLE preisblaetter_hardware (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    msb_mp_id    TEXT        NOT NULL,
    valid_from   DATE,
    valid_to     DATE,
    data         JSONB       NOT NULL,
    bo4e_version TEXT        NOT NULL DEFAULT 'v202607.0.0',
    source       TEXT        NOT NULL DEFAULT 'api' CHECK (source IN ('api', 'mako')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (msb_mp_id, valid_from)
);

CREATE INDEX preisblaetter_hw_msb ON preisblaetter_hardware (msb_mp_id, valid_from DESC NULLS LAST);
CREATE INDEX preisblaetter_hw_gin ON preisblaetter_hardware USING GIN (data jsonb_path_ops);

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

CREATE TABLE pricat_versions (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    nb_mp_id              TEXT        NOT NULL,
    tenant              TEXT        NOT NULL,
    valid_from          DATE        NOT NULL,
    valid_to            DATE,
    data                JSONB       NOT NULL,
    bo4e_version        TEXT        NOT NULL DEFAULT 'v202607.0.0',
    source              TEXT        NOT NULL DEFAULT 'api'
                        CHECK (source IN ('api', 'mako')),
    dispatch_queued_at  TIMESTAMPTZ,
    dispatch_done_at    TIMESTAMPTZ,
    dispatch_error      TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX pricat_versions_nb_tenant_from
    ON pricat_versions (nb_mp_id, tenant, valid_from);
CREATE INDEX pricat_versions_undispatched
    ON pricat_versions (tenant, nb_mp_id, valid_from DESC)
    WHERE dispatch_done_at IS NULL;

CREATE TABLE pricat_dispatch_log (
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

CREATE INDEX pricat_dispatch_log_version
    ON pricat_dispatch_log (pricat_version_id);
CREATE INDEX pricat_dispatch_log_lf
    ON pricat_dispatch_log (tenant, lf_mp_id, dispatched_at DESC);

-- ── Process correlation index ──────────────────────────────────────────────────

CREATE TABLE process_correlation (
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

CREATE INDEX process_correlation_erp_order_id
    ON process_correlation (erp_order_id);
CREATE INDEX process_correlation_malo_id_status
    ON process_correlation (malo_id, status);
CREATE INDEX process_correlation_edifact_conv_id
    ON process_correlation (edifact_conv_id);
CREATE INDEX process_correlation_running
    ON process_correlation (malo_id, initiated_at)
    WHERE status = 'RUNNING';

-- ── Webhook subscriptions ─────────────────────────────────────────────────────

CREATE TABLE subscriptions (
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

CREATE TABLE partners (
    mp_id          TEXT        PRIMARY KEY,           -- 13-digit BDEW/DVGW/GS1 MP-ID
    display_name   TEXT,
    marktrolle     TEXT,
    sparte         TEXT        CHECK (sparte IN ('STROM', 'GAS')),
    -- B2 typed fields extracted from BO4E Marktteilnehmer.
    rollencodetyp  TEXT,                              -- 'BDEW' | 'DVGW' | 'GS1'; coding authority
    makoadresse    TEXT[],                            -- AS4 endpoint URL list (makoadresse: Vec<String>)
    channels       JSONB       NOT NULL DEFAULT '[]',
    version        BIGINT      NOT NULL DEFAULT 1,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX partners_rollencodetyp ON partners (rollencodetyp) WHERE rollencodetyp IS NOT NULL;
CREATE INDEX partners_makoadresse   ON partners USING GIN (makoadresse) WHERE makoadresse IS NOT NULL;

CREATE INDEX malo_data_gin         ON malo     USING GIN (data jsonb_path_ops);
CREATE INDEX melo_data_gin         ON melo     USING GIN (data jsonb_path_ops);
CREATE INDEX partners_channels_gin ON partners USING GIN (channels jsonb_path_ops);

-- ── Idempotency dedup for inbound makod events ────────────────────────────────
-- Purge entries older than 7 days via a scheduled DELETE in background worker.

CREATE TABLE processed_events (
    event_id     TEXT        PRIMARY KEY,           -- CloudEvents "id" (UUID v4)
    processed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX processed_events_processed_at
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

CREATE TABLE versorgungsstatus_history (
    id               BIGSERIAL   PRIMARY KEY,
    malo_id          TEXT        NOT NULL,
    tenant           TEXT        NOT NULL,
    lieferstatus     TEXT        NOT NULL,
    lf_mp_id           TEXT,
    lf_mp_id_next      TEXT,                -- announced future LF (WHO; paired with lf_next_lieferbeginn = WHEN)
    lf_next_lieferbeginn DATE,
    lieferbeginn     DATE,
    lieferende       DATE,
    msb_mp_id          TEXT,
    nb_mp_id           TEXT        NOT NULL,
    last_process_id  UUID,
    version          BIGINT      NOT NULL,                    -- version of this state
    valid_from       TIMESTAMPTZ NOT NULL DEFAULT now()       -- when this state became active
);

-- Primary query pattern: most-recent state for a MaLo up to an instant.
CREATE INDEX versorgungsstatus_history_at
    ON versorgungsstatus_history (malo_id, tenant, valid_from DESC);

-- Lookup by version for audit / correlation.
CREATE INDEX versorgungsstatus_history_version
    ON versorgungsstatus_history (malo_id, tenant, version);

-- ── Netz-Element-Lokation (NeLo) — Redispatch 2.0 ────────────────────────────
--
-- Stores network element locations used in BDEW Redispatch 2.0 processes.
-- NeLo-ID: 16-char EIC code (ENTSO-E agency, DE3055 = ZEW) or 13-digit BDEW
-- Codenummer.  One row per (nelo_id, tenant).
--
-- Source: BDEW Redispatch 2.0 Implementierungsleitfaden v2.x.

CREATE TABLE nelo (
    nelo_id      TEXT        NOT NULL,                        -- EIC or BDEW Codenummer
    tenant       TEXT        NOT NULL,
    name         TEXT,                                        -- human-readable Bezeichnung
    sparte       TEXT        NOT NULL CHECK (sparte IN ('STROM', 'GAS')),
    netzebene    TEXT        CHECK (
                     netzebene IN ('NS', 'MS', 'MSP', 'HSP', 'HS', 'HöS', 'HöS/HS')
                 ),
    nb_mp_id       TEXT        NOT NULL,                        -- owning Netzbetreiber GLN
    -- ── Typed columns extracted from the BO4E Netzlokation payload (B6) ──────
    steuerkanal              BOOLEAN,     -- Redispatch 2.0: can be remote-controlled
    eigenschaft_msb_lokation TEXT,        -- gMSB Marktrolle ('NB' | 'MSB' | …)
    grundzustaendiger_msb_codenr TEXT,    -- gMSB MP-ID (13-digit BDEW/DVGW Codenummer)
    -- ─────────────────────────────────────────────────────────────────────────
    data         JSONB       NOT NULL DEFAULT '{}',           -- additional Redispatch 2.0 attributes
    version      BIGINT      NOT NULL DEFAULT 1,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (nelo_id, tenant)
);

CREATE INDEX nelo_nb_gln      ON nelo (tenant, nb_mp_id);
CREATE INDEX nelo_tenant     ON nelo (tenant);
CREATE INDEX nelo_steuerkanal ON nelo (tenant) WHERE steuerkanal = true;

-- ── Lokationszuordnung graph (B5) ─────────────────────────────────────────────
--
-- Stores directed edges of the MaKo location graph:
--   MaLo ↔ MeLo ↔ NeLo ↔ SteuerbareRessource ↔ TechnischeRessource
--
-- Each edge has an optional temporal validity window (valid_from / valid_to).
-- NULL valid_from means "from the beginning of time".
-- NULL valid_to means "open-ended (still active)".
--
-- The recursive CTE in `find_graph` traverses the full reachable subgraph from
-- any root node in a single query, enabling O(1)-latency topology lookups for
-- Redispatch 2.0 DELORD/DELRES, iMS E-mobility Steuerungsauftrag routing, and
-- MSB Stammdaten hierarchy queries.
--
-- Source: BO4E Lokationszuordnung; BK6-24-174 §6 (iMS); Redispatch 2.0 BDEW.

CREATE TABLE lokationszuordnungen (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant       TEXT        NOT NULL,
    von_id       TEXT        NOT NULL,   -- source node ID (MaLo/MeLo/NeLo/SR/TR)
    von_typ      TEXT        NOT NULL CHECK (von_typ  IN ('malo', 'melo', 'nelo', 'sr', 'tr')),
    nach_id      TEXT        NOT NULL,   -- target node ID
    nach_typ     TEXT        NOT NULL CHECK (nach_typ IN ('malo', 'melo', 'nelo', 'sr', 'tr')),
    valid_from   DATE,                   -- NULL = from epoch
    valid_to     DATE,                   -- NULL = open-ended
    data         JSONB       NOT NULL DEFAULT '{}',  -- full BO4E Lokationszuordnung
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Unique: one open-ended edge per (tenant, von_id, nach_id) where valid_from IS NULL.
-- Unique: one dated edge per (tenant, von_id, nach_id, valid_from) where valid_from IS NOT NULL.
-- Together these allow temporal succession while preventing duplicates.
CREATE UNIQUE INDEX lz_unique_open   ON lokationszuordnungen (tenant, von_id, nach_id)
    WHERE valid_from IS NULL;
CREATE UNIQUE INDEX lz_unique_dated  ON lokationszuordnungen (tenant, von_id, nach_id, valid_from)
    WHERE valid_from IS NOT NULL;

-- Traversal indexes
CREATE INDEX lz_von  ON lokationszuordnungen (tenant, von_id);
CREATE INDEX lz_nach ON lokationszuordnungen (tenant, nach_id);
-- Partial index for currently-active open-ended edges (most frequent query pattern)
CREATE INDEX lz_active ON lokationszuordnungen (tenant, von_id) WHERE valid_to IS NULL;


-- ── MaLo grid topology (N7) ─────────────────────────────────────────────────────
-- (merged from 0003_malo_grid.sql)

CREATE TABLE malo_grid (
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
CREATE INDEX malo_grid_nb_gln
    ON malo_grid (nb_mp_id, tenant);

-- Index by Bilanzierungsgebiet for NB area queries
CREATE INDEX malo_grid_big
    ON malo_grid (bilanzierungsgebiet, tenant)
    WHERE bilanzierungsgebiet IS NOT NULL;

-- ── Fanout dead-letter queue ─────────────────────────────────────────────────
-- When the fan-out worker exhausts all retry attempts for a subscriber,
-- the failed event is persisted here instead of being silently dropped.
-- Operators inspect via GET /admin/fanout/dlq; retry via POST /admin/fanout/dlq/{id}/retry;
-- discard via DELETE /admin/fanout/dlq/{id}.
--
-- §22 MessZV: silent drop of an `de.mako.process.initiated` event to invoicd
-- would cause the INVOIC plausibility check never to run, violating the
-- 3-year receipt retention obligation. This table provides the recovery path.
CREATE TABLE fanout_dlq (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    subscriber_id  TEXT        NOT NULL,
    webhook_url    TEXT        NOT NULL,
    event_type     TEXT        NOT NULL,    -- CloudEvents `type` for quick filtering
    event_body     JSONB       NOT NULL,
    attempts       INT         NOT NULL DEFAULT 0,
    last_error     TEXT,
    failed_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at    TIMESTAMPTZ             -- set when retried successfully or discarded
);

CREATE INDEX fanout_dlq_subscriber ON fanout_dlq (subscriber_id, failed_at DESC);
CREATE INDEX fanout_dlq_unresolved  ON fanout_dlq (failed_at DESC) WHERE resolved_at IS NULL;

-- ── SteuerbareRessource (B4b) ─────────────────────────────────────────────────
--
-- Stores BO4E SteuerbareRessource objects used in WiM iMS Steuerungsauftrag
-- processes (PID 55168 / WiM Strom Teil 3).
--
-- sr_id: 11-char BDEW Steuerbarer-Ressource-ID (format: C[A-Z0-9]{9}[0-9]).
-- Source: WiM AHB BK6-24-174; BDEW Identifikatoren AWH V1.2.

CREATE TABLE steuerbare_ressourcen (
    sr_id        TEXT        NOT NULL,
    tenant       TEXT        NOT NULL,
    malo_id      TEXT,                   -- associated MaLo (optional at registration)
    melo_id      TEXT,                   -- associated MeLo (optional)
    data         JSONB       NOT NULL DEFAULT '{}',  -- full BO4E SteuerbareRessource
    -- Contracted iMS control products (Vec<Konfigurationsprodukt>).
    -- NULL = not yet populated from WiM Stammdaten.
    -- Required for pre-dispatch eligibility checks in wim.steuerungsauftrag.bestaetigen.
    konfigurationsprodukte JSONB,
    bo4e_version TEXT        NOT NULL DEFAULT 'v202607.0.0',
    version      BIGINT      NOT NULL DEFAULT 1,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (sr_id, tenant)
);

CREATE INDEX sr_tenant        ON steuerbare_ressourcen (tenant);
CREATE INDEX sr_malo          ON steuerbare_ressourcen (tenant, malo_id) WHERE malo_id IS NOT NULL;
CREATE INDEX sr_konfigurationsprodukte_gin
    ON steuerbare_ressourcen USING GIN (konfigurationsprodukte jsonb_path_ops)
    WHERE konfigurationsprodukte IS NOT NULL;

-- ── Device registry: Zaehler + Geraete (B3) ──────────────────────────────────
--
-- zaehler: one row per Zähler (meter) linked to a MeLo.
-- geraete: one row per Gerät (device/component) linked to a Zähler.
--
-- Both store full BO4E objects in JSONB (Zaehler / Geraet).
-- Source: WiM AHB BK6-24-174; BO4E Zaehler / Geraet schemas.

CREATE TABLE zaehler (
    zaehler_id   TEXT        NOT NULL,   -- manufacturer serial or UUID
    tenant       TEXT        NOT NULL,
    melo_id      TEXT        NOT NULL,   -- owning MeLo
    zaehler_typ  TEXT,                   -- e.g. 'DREHSTROMZAEHLER', 'GASZAEHLER'
    eichung_bis  DATE,                   -- calibration valid until (Eichgültigkeitsdatum)
    data         JSONB       NOT NULL DEFAULT '{}',  -- full BO4E Zaehler object
    bo4e_version TEXT        NOT NULL DEFAULT 'v202607.0.0',
    version      BIGINT      NOT NULL DEFAULT 1,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (zaehler_id, tenant)
);

CREATE INDEX zaehler_melo ON zaehler (tenant, melo_id);

CREATE TABLE geraete (
    geraet_id              TEXT        NOT NULL,   -- manufacturer serial or UUID
    tenant                 TEXT        NOT NULL,
    zaehler_id             TEXT        NOT NULL,   -- owning Zähler
    geraet_typ             TEXT,                   -- Geraetetyp (e.g. 'WANDLER', 'INTELLIGENTESMESSYSTEM')
    data                   JSONB       NOT NULL DEFAULT '{}',  -- full BO4E Geraet object
    -- Typed device-configuration entries per MsbG §23 + BSI TR-03109 + §14a EnWG.
    -- Stored separately from `data` to support atomic partial updates and GIN queries
    -- (e.g. "all devices with SMGW_CERT_ABLAUFDATUM <= 30 days from now").
    -- Schema: [{parameter: "FIRMWARE_VERSION", wert: "2.4.1", updated_at: "...", notiz: null}, ...]
    geraet_konfigurationen JSONB       NOT NULL DEFAULT '[]',
    bo4e_version           TEXT        NOT NULL DEFAULT 'v202607.0.0',
    version                BIGINT      NOT NULL DEFAULT 1,
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (geraet_id, tenant)
);

CREATE INDEX geraete_zaehler ON geraete (tenant, zaehler_id);
-- GIN index allows fast JSONB containment queries on configuration entries:
--   SELECT * FROM geraete WHERE geraet_konfigurationen @> '[{"parameter":"SMGW_CERT_ABLAUFDATUM"}]'
CREATE INDEX geraete_konfigurationen_gin ON geraete USING GIN (geraet_konfigurationen);

-- ── TechnischeRessource (B9) ──────────────────────────────────────────────────
--
-- Stores BO4E TechnischeRessource objects for E-mobility (Wallbox/EV charging),
-- generation (PV/Wind), and storage (battery).  Linked to MaLo/MeLo via
-- Lokationszuordnung.  Used by WiM iMS Steuerungsauftrag (EMobilitaetsart) and
-- Redispatch 2.0 flexibility registration.
--
-- tr_id: TrId format (Technische-Ressource-ID per rubo4e::identifiers::TrId).
-- Source: BK6-24-174 §6 (iMS); Redispatch 2.0 BDEW Implementierungsleitfaden.

CREATE TABLE technische_ressourcen (
    tr_id             TEXT        NOT NULL,
    tenant            TEXT        NOT NULL,
    malo_id           TEXT,                   -- linked MaLo (zugeordnete_marktlokation_id)
    melo_id           TEXT,                   -- linked MeLo (vorgelagerte_messlokation_id)
    tr_typ            TEXT,                   -- 'EMobilitaet' | 'Erzeugung' | 'Speicher' | NULL
    ist_fernschaltbar BOOLEAN,               -- can be remote-controlled (Redispatch 2.0)
    data              JSONB       NOT NULL DEFAULT '{}',  -- full BO4E TechnischeRessource
    bo4e_version      TEXT        NOT NULL DEFAULT 'v202607.0.0',
    version           BIGINT      NOT NULL DEFAULT 1,
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (tr_id, tenant)
);

CREATE INDEX tr_tenant  ON technische_ressourcen (tenant);
CREATE INDEX tr_malo    ON technische_ressourcen (tenant, malo_id)  WHERE malo_id IS NOT NULL;
CREATE INDEX tr_melo    ON technische_ressourcen (tenant, melo_id)  WHERE melo_id IS NOT NULL;
CREATE INDEX tr_typ     ON technische_ressourcen (tenant, tr_typ)   WHERE tr_typ IS NOT NULL;

-- ── CloudEvent replay log (B11) ───────────────────────────────────────────────
--
-- Durable append-only log of every inbound CloudEvent received by marktd.
-- Populated by POST /api/v1/events before fan-out.
-- Enables full replay: after a subscriber bug-fix or new service onboarding,
-- replay events from a point in time without data loss.
--
-- Read via GET /admin/events?from=&to=&type=&limit=
-- Retention: operator-managed; can be partitioned or archived by received_at.

CREATE TABLE event_log (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    event_id    TEXT        NOT NULL UNIQUE,    -- CloudEvents "id" field
    ce_type     TEXT        NOT NULL,
    ce_source   TEXT,
    subject     TEXT,
    data        JSONB,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX event_log_type_time ON event_log (ce_type, received_at DESC);
CREATE INDEX event_log_time      ON event_log (received_at DESC);

-- Migration 0002: MMMA / MMM settlement price store
--
-- Both `netzbilanzd` (NB — generates INVOIC 31002/31005/31007/31008) and
-- `invoicd` (LF — validates inbound MMM invoices) need monthly settlement prices:
--
--   • Gas:   Trading Hub Europe (THE) publishes `mmma_preise_gas` monthly.
--   • Strom: Each ÜNB publishes `mmm_preise_strom` per §22 StromNZV monthly.
--
-- Both services query `marktd` instead of requiring the ERP to supply prices
-- manually on every billing run (eliminates the current single point of failure).

-- ── Gas MMM Abrechnungspreise (THE / MGV) ────────────────────────────────────

CREATE TABLE mmma_preise_gas (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    -- First day of the billing month (German local time).
    price_month     DATE        NOT NULL,
    -- Marktgebiet — 'THE' (Trading Hub Europe, the only German gas market area since 2021).
    marktgebiet     TEXT        NOT NULL DEFAULT 'THE',
    -- Ausgleichsenergiepreis Überschuss: price for Mehrmengen (LF over-consumed) ct/kWh.
    mehr_ct_kwh     NUMERIC     NOT NULL CHECK (mehr_ct_kwh >= 0),
    -- Ausgleichsenergiepreis Defizit: price for Mindermengen (LF under-consumed) ct/kWh.
    minder_ct_kwh   NUMERIC     NOT NULL CHECK (minder_ct_kwh >= 0),
    -- How this record entered the system.
    source          TEXT        NOT NULL DEFAULT 'manual'
                                CHECK (source IN ('manual', 'the-api', 'csv-import')),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (price_month, marktgebiet)
);

CREATE INDEX mmma_gas_month
    ON mmma_preise_gas (price_month DESC, marktgebiet);

-- ── Strom MMM Ausgleichsenergie prices (ÜNB per §22 StromNZV) ────────────────

CREATE TABLE mmm_preise_strom (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    -- First day of the billing month (German local time).
    price_month     DATE        NOT NULL,
    -- ÜNB MP-ID (BDEW-Codenummer 99…): 50Hertz, TenneT, Amprion, TransnetBW.
    unb_mp_id       TEXT        NOT NULL,
    -- Surplus energy price (Mehrmengen, LF over-consumed) ct/kWh.
    mehr_ct_kwh     NUMERIC     NOT NULL CHECK (mehr_ct_kwh >= 0),
    -- Deficit energy price (Mindermengen, LF under-consumed) ct/kWh.
    minder_ct_kwh   NUMERIC     NOT NULL CHECK (minder_ct_kwh >= 0),
    source          TEXT        NOT NULL DEFAULT 'manual'
                                CHECK (source IN ('manual', 'uenb-api', 'csv-import')),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (price_month, unb_mp_id)
);

CREATE INDEX mmm_strom_month
    ON mmm_preise_strom (price_month DESC, unb_mp_id);

-- ── marktd migration 0003 — ZaehlzeitRegister + ZaehlzeitSaison ─────────────
--
-- Provides the PostgreSQL persistence for iMSys Time-of-Use (TOU) register
-- definitions.  Required for §14a Modul 2 accurate HT/NT window classification
-- from smart meter data.
--
-- Sources:
--   - MsbG §19; BO4E Zaehlwerk schema (v202607)
--   - BDEW WiM AHB BK6-24-174: Stammdaten ZAK+ZD segment
--   - §14a EnWG Modul 2: time-banded grid fee windows

-- ── ZaehlzeitRegister: one metering register per Zähler ─────────────────────

CREATE TABLE zaehler_register (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    zaehler_id       TEXT        NOT NULL,    -- owning Zähler serial number
    tenant           TEXT        NOT NULL,    -- operator GLN
    bezeichnung      TEXT        NOT NULL,    -- "HT", "NT", "Gesamt", etc.
    -- BO4E Zaehlerauspraegung: HT | NT | EINZEL
    zaehlerauspraegung TEXT      NOT NULL
                     CHECK (zaehlerauspraegung IN ('HT', 'NT', 'EINZEL')),
    -- IEC 62056-61 OBIS kennzahl identifying this register in MSCONS
    -- e.g. "1-1:1.29.0" for HT import, "1-1:2.8.0" for NT export
    obis_kennzahl    TEXT,
    -- Unit: default KWH; KVAR for reactive energy, KW for demand
    einheit          TEXT        NOT NULL DEFAULT 'KWH',
    valid_from       DATE        NOT NULL,
    valid_to         DATE,                    -- NULL = currently active
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (zaehler_id, tenant, bezeichnung, valid_from)
);

CREATE INDEX zr_zaehler_tenant ON zaehler_register (zaehler_id, tenant);
CREATE INDEX zr_obis           ON zaehler_register (obis_kennzahl, tenant)
    WHERE obis_kennzahl IS NOT NULL;
CREATE INDEX zr_active         ON zaehler_register (zaehler_id, tenant)
    WHERE valid_to IS NULL;

-- ── ZaehlzeitSaison: time-of-use windows per register ───────────────────────

CREATE TABLE zaehler_saisons (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    register_id      UUID        NOT NULL REFERENCES zaehler_register (id) ON DELETE CASCADE,
    -- Season key: SOMMER | WINTER | GESAMT (year-round)
    saison           TEXT        NOT NULL
                     CHECK (saison IN ('SOMMER', 'WINTER', 'GESAMT')),
    -- Days-of-week bitmask stored as a JSONB integer array.
    -- ISO weekday: 1=Mon … 7=Sun.  Example: [1,2,3,4,5] = Mon–Fri.
    wochentage       JSONB       NOT NULL,
    -- Window start/end in local German time (HH:MM, 24-h clock).
    -- Start is inclusive; end is exclusive (standard half-open interval).
    zeit_von         TEXT        NOT NULL,    -- e.g. "07:00"
    zeit_bis         TEXT        NOT NULL,    -- e.g. "22:00"
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX zs_register ON zaehler_saisons (register_id);

-- marktd migration 0004: NB Energiemix authority table
--
-- N8: NB publishes annual grid-area Energiemix per §42 EnWG.
--
-- The NB is the authoritative source for the renewable energy mix in their grid
-- area, derived from local EEG plants feeding into the grid.  LFs and portald
-- query this for §42 Abs. 5 EnWG Reststrommix disclosure and Ökostrom labelling.
--
-- One row per (tenant, nb_mp_id, gueltig_fuer) with the most recent being the
-- active disclosure.

CREATE TABLE nb_energiemix (
    nb_mp_id        TEXT        NOT NULL,
    tenant          TEXT        NOT NULL,
    -- Calendar year this Energiemix is valid for (§42 EnWG annual disclosure).
    gueltig_fuer    SMALLINT    NOT NULL DEFAULT extract(year FROM now()),
    -- rubo4e::current::Energiemix COM JSON (camelCase, validated on PUT).
    energiemix      JSONB       NOT NULL,
    -- Snapshot of total EEG feed-in kWh this year (optional, informational).
    eeg_einspeisung_kwh NUMERIC(18, 0),
    -- Snapshot of total grid withdrawal kWh this year (for percentage calc).
    gesamtentnahme_kwh  NUMERIC(18, 0),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant, nb_mp_id, gueltig_fuer)
);

CREATE INDEX nb_energiemix_nb    ON nb_energiemix (nb_mp_id, gueltig_fuer DESC);
CREATE INDEX nb_energiemix_year  ON nb_energiemix (tenant, gueltig_fuer DESC);

COMMENT ON TABLE nb_energiemix IS
    'Annual grid-area Energiemix published by the NB per §42 EnWG. '
    'One row per (nb_mp_id, year). LFs use this for §42 Abs. 5 Reststrommix disclosure.';

-- 0001_initial_schema.sql — marktd complete schema
--
-- Single authoritative schema. Drop and recreate the database to reset;
-- all application data is reproducible from the EDIFACT event streams in makod.
--
-- Required extensions:
--   btree_gist — lets a GiST exclusion constraint mix equality columns (a MaLo
--     ID, a role) with a range column, which is how every "no two of these may
--     be valid at the same time" rule below is expressed. PostgreSQL 18's
--     native `WITHOUT OVERLAPS` would remove the need for it; the platform
--     targets 15+.
--   (pgcrypto is NOT required: gen_random_uuid() is built in since PG 13.)
CREATE EXTENSION IF NOT EXISTS btree_gist;

-- Design decisions:
--   • All timestamps: TIMESTAMPTZ (UTC).
--   • Date columns (valid_from/valid_to): plain DATE — the business meaning is a
--     calendar date in German local time, not a wall-clock instant. Validity is
--     half-open [valid_from, valid_to): the day a successor starts is the day
--     the predecessor stops, with no ambiguous shared day.
--   • Overlap is a constraint, not a convention. Every dated assignment carries
--     an EXCLUDE … USING gist over daterange(valid_from, valid_to, \'[)\'), so
--     "who was the NB on this date" can never have two answers. A NULL
--     valid_from reads as -infinity and a NULL valid_to as +infinity, so an
--     open-ended row still collides with an overlapping one.
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
    -- Every enum column below holds a BO4E wire value and nothing else. The
    -- CHECK lists are the schema's own VARIANTS, pinned by the
    -- `bo4e_check_constraints_match_the_schema` test in `mako-markt`. Two
    -- writers reach these columns — the REST payload and the UTILMD
    -- Stammdatenänderung patch — so one vocabulary is a constraint, not a
    -- convention.
    netzebene            TEXT CHECK (netzebene IN (
                                'NSP', 'MSP', 'HSP', 'HSS',
                                'MSP_NSP_UMSP', 'HSP_MSP_UMSP', 'HSS_HSP_UMSP',
                                'HD', 'MD', 'ND')),
    bilanzierungsgebiet  TEXT,  -- Bilanzierungsgebiet-EIC; drives processd NB check 4
    gasqualitaet         TEXT CHECK (gasqualitaet IN ('H_GAS', 'L_GAS')),
    -- Named from the grid's point of view: EINSP (Einspeisung) feeds the grid
    -- and is the *generating* location; AUSSP (Ausspeisung) draws from it.
    energierichtung      TEXT CHECK (energierichtung IN ('AUSSP', 'EINSP')),
    bilanzierungsmethode TEXT CHECK (bilanzierungsmethode IN (
                                'RLM', 'SLP', 'TLP_GEMEINSAM', 'TLP_GETRENNT',
                                'PAUSCHAL', 'IMS')),
    regelzone            TEXT,  -- Regelzone EIC code; maps MaLo → ÜNB for MABIS IFTSTA + Redispatch 2.0
    -- Gas GaBi RLM Fallgruppe. A Bilanzierung field, not a Marktlokation one:
    -- written by the Bilanzierung resource and the UTILMD TM+Z10 patch; a MaLo
    -- PUT leaves it alone.
    fallgruppe           TEXT CHECK (fallgruppe IN (
                                'GABI_RLM_MIT_TAGESBAND', 'GABI_RLM_OHNE_TAGESBAND',
                                'GABI_RLM_IM_NOMINIERUNGSERSATZVERFAHREN')),
    lokationsbuendel_objektcode TEXT,  -- Marktlokation.lokationsbuendelObjektcode (UTILMD Lokationsbündelstruktur)
    -- §14a EnWG Status der Fernsteuerbarkeit (UTILMD CCI+Z24++Z97 = true /
    -- Z96 = false). No BO4E field exists for it; a MaLo PUT leaves it alone.
    fernsteuerbar        BOOLEAN,
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

-- ── Rollenzuordnung (MaLo role assignments, temporal) ───────────────────────────

CREATE TABLE rollenzuordnungen (
    malo_id          TEXT  NOT NULL REFERENCES malo (malo_id) ON DELETE CASCADE,
    zuordnungstyp    TEXT  NOT NULL,                -- NB | GNB | MSB | GMSB | LF | LFG | …
    rollencodenummer TEXT  NOT NULL,                -- 13-digit BDEW/DVGW GLN
    valid_from       DATE  NOT NULL,
    valid_to         DATE,                          -- NULL = currently valid
    PRIMARY KEY (malo_id, zuordnungstyp, valid_from),

    -- `GET /api/v1/malos/{id}` answers with every assignment whose window
    -- contains the query date. Without this, two overlapping NB rows are both
    -- "currently valid" and the API returns two Netzbetreiber for one MaLo —
    -- a contradiction the caller has no way to resolve and which the primary
    -- key above does not prevent (the start dates differ).
    CONSTRAINT rollenzuordnungen_no_overlap EXCLUDE USING gist (
        malo_id       WITH =,
        zuordnungstyp WITH =,
        daterange(valid_from, valid_to, '[)') WITH &&
    )
);

CREATE INDEX rollenzuordnungen_malo_id
    ON rollenzuordnungen (malo_id);
CREATE INDEX rollenzuordnungen_rollencodenummer
    ON rollenzuordnungen (rollencodenummer);

-- ── Messlokation ──────────────────────────────────────────────────────────────

CREATE TABLE melo (
    melo_id      TEXT        PRIMARY KEY,           -- DE + 31 alphanumeric chars
    malo_id      TEXT        REFERENCES malo (malo_id) ON DELETE SET NULL,
    -- Typed columns extracted from BO4E Messlokation JSONB at write time.
    -- BO4E `Netzebene` wire value, same vocabulary and same CHECK-drift guard
    -- as malo.netzebene.
    netzebene_messung      TEXT CHECK (netzebene_messung IN (
                                'NSP', 'MSP', 'HSP', 'HSS',
                                'MSP_NSP_UMSP', 'HSP_MSP_UMSP', 'HSS_HSP_UMSP',
                                'HD', 'MD', 'ND')),
    regelzone              TEXT,   -- Regelzone EIC (Standorteigenschaften.eigenschaftenStrom[0].regelzone)
                                   -- maps MeLo → ÜNB for Redispatch 2.0 Stammdaten + MABIS IFTSTA 21000
    -- Full BO4E Standorteigenschaften — stored for Redispatch 2.0
    -- NetworkConstraintDocument and Gas billing zone lookup
    -- (StandorteigenschaftenGas.druckstufe, bilanzierungsgebietEic).
    --
    -- Standorteigenschaften is a standalone BO (#25), not a Messlokation
    -- field, so it arrives in the extension map where typed deserialization
    -- does not reach. The write path parses it as the BO it names and derives
    -- regelzone from the typed value. NULL when the PUT does not carry it.
    standorteigenschaften  JSONB,
    lokationsbuendel_objektcode TEXT,  -- Messlokation.lokationsbuendelObjektcode (UTILMD Lokationsbündelstruktur)
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

-- ── MSB-Zuordnung je Messlokation (dated timeline) ────────────────────────────
-- WiM Teil 2 UC 4.1.1: a historical Werteanfrage must resolve which MSB served a
-- specific MeLo at a past date. MaLo-level MSB (rollenzuordnungen /
-- versorgungsstatus.msb_mp_id) is insufficient — a MaLo can bundle several MeLos
-- whose MSB history differs (e.g. a partial MSB-Wechsel). This per-MeLo dated
-- table is the authoritative source for point-in-time MSB resolution.
CREATE TABLE melo_msb_zuordnungen (
    tenant       TEXT        NOT NULL,
    melo_id      TEXT        NOT NULL REFERENCES melo (melo_id) ON DELETE CASCADE,
    msb_mp_id    TEXT        NOT NULL,              -- 13-digit MSB GLN
    valid_from   DATE        NOT NULL,
    valid_to     DATE,                              -- NULL = currently valid
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant, melo_id, valid_from),

    -- `find_msb_at` must resolve to exactly one MSB for any past date; a
    -- backdated correction that failed to close the later row would otherwise
    -- make the answer depend on row order.
    CONSTRAINT melo_msb_no_overlap EXCLUDE USING gist (
        tenant  WITH =,
        melo_id WITH =,
        daterange(valid_from, valid_to, '[)') WITH &&
    )
);

-- Point-in-time lookup: newest assignment at or before a given date.
CREATE INDEX melo_msb_at ON melo_msb_zuordnungen (tenant, melo_id, valid_from DESC);

-- ── Bilanzierung (BO4E) — first-class, temporal balancing resource ────────────
-- BO4E `Bilanzierung` (BO #3): the balancing-relevant data of a Marktlokation
-- with its own identity and validity — Bilanzkreis (EIC), Bilanzierungsgebiet,
-- Aggregationsverantwortung, Prognosegrundlage, Fallgruppenzuordnung, Lastprofil,
-- Jahresverbrauchsprognose, Kundenwert. Previously smeared across `malo` columns
-- (denormalised current-value), the dead `mako-edm::BilanzzuordnungRecord`, and
-- `metering::LoadProfile`. This table is the authoritative temporal home; the
-- `data` JSONB is the full BO4E `Bilanzierung`, typed columns are extracted for
-- indexing. Keyed per (MaLo, validity-start) for point-in-time resolution.
CREATE TABLE bilanzierungen (
    tenant                    TEXT        NOT NULL,
    malo_id                   TEXT        NOT NULL,   -- BO4E marktlokationsId
    -- Temporal validity (BO4E bilanzierungsbeginn/ende).
    bilanzierungsbeginn       TIMESTAMPTZ NOT NULL,   -- validity start (inclusive)
    bilanzierungsende         TIMESTAMPTZ,            -- validity end (exclusive); NULL = open
    -- Typed columns extracted from the BO4E Bilanzierung JSONB.
    bilanzkreis               TEXT,                   -- Bilanzkreis EIC
    aggregationsverantwortung TEXT,                   -- NB | ÜNB
    prognosegrundlage         TEXT,                   -- SLP | Prognose | …
    fallgruppenzuordnung      TEXT,                   -- GaBi Fallgruppe
    -- Full BO4E Bilanzierung (round-trip-preserving).
    data                      JSONB       NOT NULL,
    bo4e_version              TEXT        NOT NULL DEFAULT 'v202607.0.0',
    updated_at                TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (tenant, malo_id, bilanzierungsbeginn)
);

COMMENT ON TABLE bilanzierungen IS
    'BO4E Bilanzierung (BO #3): first-class temporal balancing resource per MaLo. '
    'Authoritative home for Bilanzkreis/Aggregationsverantwortung/Prognosegrundlage/'
    'Fallgruppe with bilanzierungsbeginn/ende validity.';

-- Point-in-time lookup: newest Bilanzierung effective at a given instant.
CREATE INDEX biz_malo_at ON bilanzierungen (tenant, malo_id, bilanzierungsbeginn DESC);
CREATE INDEX biz_bilanzkreis ON bilanzierungen (bilanzkreis) WHERE bilanzkreis IS NOT NULL;
CREATE INDEX biz_data_gin ON bilanzierungen USING GIN (data jsonb_path_ops);

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
    -- The Netznutzer this contract is with. `LETZTVERBRAUCHER` marks a
    -- Selbstzahler, who takes the LF role in GPKE except for the LF's
    -- Lieferantenwechsel-Meldungen (Teil 1, Vorbemerkung).
    netznutzer_mp_id      TEXT        NOT NULL,
    netznutzer_typ        TEXT        NOT NULL DEFAULT 'LIEFERANT'
                              CHECK (netznutzer_typ IN ('LIEFERANT', 'LETZTVERBRAUCHER')),
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
-- One network contract per MaLo per NB at any instant: two overlapping ones
-- would let a settlement pick either Netzebene or Bilanzierungsmethode.
ALTER TABLE nb_contracts ADD CONSTRAINT nb_contracts_no_overlap EXCLUDE USING gist (
    tenant   WITH =,
    malo_id  WITH =,
    nb_mp_id WITH =,
    daterange(valid_from, valid_to, '[)') WITH &&
);
CREATE INDEX nb_contracts_nb_gln
    ON nb_contracts (nb_mp_id, tenant);
CREATE INDEX nb_contracts_malo_id
    ON nb_contracts (malo_id);
CREATE INDEX nb_contracts_vertragsart
    ON nb_contracts (vertragsart, tenant) WHERE vertragsart IS NOT NULL;
-- Selbstzahler lookup: „which of my MaLos has the Letztverbraucher as Netznutzer".
CREATE INDEX nb_contracts_selbstzahler
    ON nb_contracts (tenant, netznutzer_mp_id)
    WHERE netznutzer_typ = 'LETZTVERBRAUCHER';

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
    -- MP-ID of the announced future LF (WHO). Set on receipt of an Anmeldung
    -- (55001 / 55077 / 44001) and cleared by its Ablehnung (55003 / 55080 /
    -- 44003) or by the Bestätigung that promotes it. The *first* announcement
    -- wins: a competing supplier's Anmeldung does not overwrite it, because
    -- mako-pruefung decides E_0622 Prüfschritt 70 „Andere Anmeldung in Bearbeitung"
    -- by comparing this column against the requesting supplier.
    lf_mp_id_next       TEXT,
    lf_next_lieferbeginn DATE,               -- Announced Lieferbeginn of the future LF (WHEN; paired with lf_mp_id_next)
    lieferbeginn      DATE,
    lieferende        DATE,
    msb_mp_id           TEXT,
    nb_mp_id            TEXT        NOT NULL,
    eog_seit          DATE,               -- Start of the running Ersatz-/Grundversorgung (§38/§36 EnWG);
                                          -- anchors the §38 Abs. 4 maximum: the Ersatzversorgung
                                          -- relationship ends at the latest three months after it began.
                                          -- (Abs. 1 establishes the relationship; the cap is in Abs. 4.)
                                          -- Set by begin_eog_supply, cleared on confirm_supply / end_supply.
    last_process_id   UUID,
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    version           BIGINT      NOT NULL DEFAULT 1,

    PRIMARY KEY (malo_id, tenant),
    -- eog_seit exists exactly while the statutory fallback supply runs.
    CONSTRAINT versorgungsstatus_eog_seit_scope CHECK (
        (lieferstatus IN ('Ersatzversorgung', 'Grundversorgung')) = (eog_seit IS NOT NULL)
    )
);

CREATE INDEX versorgungsstatus_tenant_status
    ON versorgungsstatus (tenant, lieferstatus);
CREATE INDEX versorgungsstatus_tenant_lf
    ON versorgungsstatus (tenant, lf_mp_id)
    WHERE lf_mp_id IS NOT NULL;
CREATE INDEX versorgungsstatus_tenant_nb
    ON versorgungsstatus (tenant, nb_mp_id);
-- §38 Abs. 4 timer scans: all running Ersatzversorgungen ordered by start date.
CREATE INDEX versorgungsstatus_eog
    ON versorgungsstatus (tenant, eog_seit)
    WHERE lieferstatus = 'Ersatzversorgung';

-- ── Grundversorger (§36 Abs. 2 EnWG) ──────────────────────────────────────────
--
-- The supplier with the most Haushaltskunden in the Netzgebiet, festgestellt
-- by the NB every three years (zum 1. Juli, published by 30. September).
-- Master data maintained by the operator; read by the processd gap-closure
-- automation to address the UTILMD 55013/44013 EoG Zuordnung.

CREATE TABLE grundversorger (
    tenant          TEXT        NOT NULL,
    nb_mp_id        TEXT        NOT NULL,
    sparte          TEXT        NOT NULL CHECK (sparte IN ('STROM', 'GAS')),
    gv_mp_id        TEXT        NOT NULL,
    festgestellt_am DATE,               -- date of the §36 Abs. 2 Feststellung
    default_bilanzkreis TEXT,           -- GPKE Teil 4 deposited default BK (EoG ohne Antwort)
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (tenant, nb_mp_id, sparte)
);

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
    -- NULLS NOT DISTINCT: `valid_from IS NULL` means "open-started", of which
    -- there can only be one per party. Under the default NULLS DISTINCT,
    -- PostgreSQL treats every NULL as unique, so repeated PUTs of an
    -- open-started sheet silently accumulate rows and the point-in-time read
    -- (`ORDER BY valid_from DESC NULLS LAST LIMIT 1`) picks an arbitrary one.
    UNIQUE NULLS NOT DISTINCT (nb_mp_id, valid_from),

    -- Two price sheets valid on the same day for the same party would make the
    -- tariff a lottery — invoic-checker validates INVOIC plausibility against
    -- whichever one the read happened to return.
    CONSTRAINT preisblaetter_no_overlap EXCLUDE USING gist (
        nb_mp_id WITH =,
        daterange(valid_from, valid_to, '[)') WITH &&
    )
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
    -- NULLS NOT DISTINCT: `valid_from IS NULL` means "open-started", of which
    -- there can only be one per party. Under the default NULLS DISTINCT,
    -- PostgreSQL treats every NULL as unique, so repeated PUTs of an
    -- open-started sheet silently accumulate rows and the point-in-time read
    -- (`ORDER BY valid_from DESC NULLS LAST LIMIT 1`) picks an arbitrary one.
    UNIQUE NULLS NOT DISTINCT (msb_mp_id, valid_from),

    -- Two price sheets valid on the same day for the same party would make the
    -- tariff a lottery — invoic-checker validates INVOIC plausibility against
    -- whichever one the read happened to return.
    CONSTRAINT preisblaetter_messung_no_overlap EXCLUDE USING gist (
        msb_mp_id WITH =,
        daterange(valid_from, valid_to, '[)') WITH &&
    )
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
-- KAV §2 requires Konzessionsabgabe as a separate position in every NNE
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
    -- `kundengruppe_ka IS NULL` means "both customer groups" and `valid_from
    -- IS NULL` means "open-started" — each a single distinguished row, not an
    -- unlimited family of them.
    UNIQUE NULLS NOT DISTINCT (nb_mp_id, sparte, kundengruppe_ka, valid_from),

    CONSTRAINT preisblaetter_ka_no_overlap EXCLUDE USING gist (
        nb_mp_id        WITH =,
        sparte          WITH =,
        kundengruppe_ka WITH =,
        daterange(valid_from, valid_to, '[)') WITH &&
    )
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
    -- NULLS NOT DISTINCT: `valid_from IS NULL` means "open-started", of which
    -- there can only be one per party. Under the default NULLS DISTINCT,
    -- PostgreSQL treats every NULL as unique, so repeated PUTs of an
    -- open-started sheet silently accumulate rows and the point-in-time read
    -- (`ORDER BY valid_from DESC NULLS LAST LIMIT 1`) picks an arbitrary one.
    UNIQUE NULLS NOT DISTINCT (msb_mp_id, valid_from),

    -- Two price sheets valid on the same day for the same party would make the
    -- tariff a lottery — invoic-checker validates INVOIC plausibility against
    -- whichever one the read happened to return.
    CONSTRAINT preisblaetter_dienstleistung_no_overlap EXCLUDE USING gist (
        msb_mp_id WITH =,
        daterange(valid_from, valid_to, '[)') WITH &&
    )
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
    -- NULLS NOT DISTINCT: `valid_from IS NULL` means "open-started", of which
    -- there can only be one per party. Under the default NULLS DISTINCT,
    -- PostgreSQL treats every NULL as unique, so repeated PUTs of an
    -- open-started sheet silently accumulate rows and the point-in-time read
    -- (`ORDER BY valid_from DESC NULLS LAST LIMIT 1`) picks an arbitrary one.
    UNIQUE NULLS NOT DISTINCT (msb_mp_id, valid_from),

    -- Two price sheets valid on the same day for the same party would make the
    -- tariff a lottery — invoic-checker validates INVOIC plausibility against
    -- whichever one the read happened to return.
    CONSTRAINT preisblaetter_hardware_no_overlap EXCLUDE USING gist (
        msb_mp_id WITH =,
        daterange(valid_from, valid_to, '[)') WITH &&
    )
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

-- ── PRICAT 27003 dispatch ledger ──────────────────────────────────────────────
--
-- `pricat_versions` is not a second copy of `preisblaetter`. The two answer
-- different questions and carry different constraints:
--
--   preisblaetter    — "which Preisblatt Netznutzung is valid on date X?"
--                      State. One answer per party per day, enforced by an
--                      EXCLUDE … USING gist no-overlap constraint. A correction
--                      replaces the row.
--
--   pricat_versions  — "which document did we transmit to the LFs, and when?"
--                      An audit trail. Deliberately has NO no-overlap
--                      constraint: a superseded version stays queryable,
--                      because a PRICAT sent last quarter remains a fact after
--                      the price sheet behind it is corrected. `data` is a
--                      snapshot taken at dispatch time for exactly that reason —
--                      pointing at the live sheet instead would let a later
--                      correction retroactively rewrite what was sent.
--
-- Both are written in the same transaction as the `de.markt.pricat.published`
-- outbox row (handlers/preisblatt.rs), so the sheet, the snapshot and the
-- dispatch trigger cannot diverge.
--
-- Only the NNE sheet feeds this ledger. PRICAT 27003 is the NB→LF Preisblatt
-- Netznutzung transmission; `preisblaetter_messung` (MSB) and
-- `preisblaetter_konzessionsabgabe` have no PRICAT of their own.
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
    webhook_secret TEXT,                            -- HMAC-SHA256 signing key, plaintext at rest; NULL = no signature. Protect via Postgres least-privilege / storage encryption. See marktd README "Webhook secret at rest".
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
    -- BO4E Marktrolle and Rollencodetyp wire values. Both are served verbatim
    -- from GET /partners/{id}/marktteilnehmer, so the column is constrained and
    -- the read path parses with from_wire: a value outside the vocabulary is
    -- reported, not silently dropped.
    marktrolle     TEXT CHECK (marktrolle IN (
                       'BIKO', 'BKV', 'BTR', 'DP', 'EIV', 'ESA', 'KN',
                       'LF', 'MGV', 'MSB', 'NB', 'RB', 'UENB')),
    sparte         TEXT        CHECK (sparte IN ('STROM', 'GAS')),
    -- B2 typed fields extracted from BO4E Marktteilnehmer.
    -- The coding authority. Note the third value is BO4E's `GLN`, not `GS1`:
    -- GS1 is the issuing organisation, GLN the code it issues, and only the
    -- latter is a `Rollencodetyp`.
    rollencodetyp  TEXT CHECK (rollencodetyp IN ('BDEW', 'DVGW', 'GLN')),
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
    eog_seit         DATE,                                    -- snapshotted §38/§36 fallback start
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
    -- BO4E Netzebene wire value. Netzlokation carries no netzebene field in
    -- BO4E, so the column is mako's — but the UTILMD Stammdatenänderung patch
    -- is one shared map routed by object type and writes into malo, melo and
    -- nelo alike, so the vocabulary is the schema's.
    netzebene    TEXT        CHECK (netzebene IN (
                     'NSP', 'MSP', 'HSP', 'HSS',
                     'MSP_NSP_UMSP', 'HSP_MSP_UMSP', 'HSS_HSP_UMSP',
                     'HD', 'MD', 'ND')),
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

-- ── Tranche (GPKE Teil 4 „Daten der Tranche") ─────────────────────────────────
--
-- A Tranche is a share of a Marktlokation's energy assigned to a distinct
-- balancing responsibility (BO4E `Tranche`). GPKE Teil 4 „Änderung Daten der
-- Tranche" (PIDs 55619/55642/55652/55662/55686) applies to the typed columns
-- below via the object-generic Stammdatenänderung apply path (LOC+Z21).

CREATE TABLE tranche (
    tranche_id           TEXT        NOT NULL,           -- e.g. <MaLo>-T01
    tenant               TEXT        NOT NULL,
    malo_id              TEXT,                            -- parent Marktlokation
    -- ── Typed columns from the BO4E Tranche / Stammdatenänderung (LOC+Z21) ──────
    bilanzierungsgebiet  TEXT,                            -- Bilanzierungsgebiet-EIC (LOC+237)
    netzebene            TEXT,                            -- voltage / pressure level
    energierichtung      TEXT,                            -- EINSPEISUNG | ENTNAHME
    -- ─────────────────────────────────────────────────────────────────────────
    data                 JSONB       NOT NULL DEFAULT '{}',  -- full BO4E Tranche
    version              BIGINT      NOT NULL DEFAULT 1,
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (tranche_id, tenant)
);

CREATE INDEX tranche_malo   ON tranche (tenant, malo_id) WHERE malo_id IS NOT NULL;
CREATE INDEX tranche_tenant ON tranche (tenant);

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
    von_typ      TEXT        NOT NULL CHECK (von_typ  IN ('MALO', 'MELO', 'NELO', 'SR', 'TR')),  -- BO4E Lokationstyp
    nach_id      TEXT        NOT NULL,   -- target node ID
    nach_typ     TEXT        NOT NULL CHECK (nach_typ IN ('MALO', 'MELO', 'NELO', 'SR', 'TR')),  -- BO4E Lokationstyp
    valid_from   DATE,                   -- NULL = from epoch
    valid_to     DATE,                   -- NULL = open-ended
    lokationsbuendelcode TEXT,           -- extracted from data.lokationsbuendelcode on upsert
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
    source               TEXT        NOT NULL DEFAULT 'manual'
                         CHECK (source IN ('mastr', 'manual')),  -- NB-role PUT or MaStR import
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (malo_id, tenant)
);

-- Index by NB MP-ID for bulk export
CREATE INDEX malo_grid_nb_gln
    ON malo_grid (nb_mp_id, tenant);

-- Index by Bilanzierungsgebiet for NB area queries
CREATE INDEX malo_grid_big
    ON malo_grid (bilanzierungsgebiet, tenant)
    WHERE bilanzierungsgebiet IS NOT NULL;

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
    -- BO4E `Zaehlertyp` wire value. Constrained because §42c Energy-Sharing
    -- eligibility reads this column: an unrecognised value there silently
    -- degrades a delivery point to UNKNOWN. `mako-markt`'s
    -- `bo4e_check_constraints_match_the_schema` test pins this list to
    -- `rubo4e::current::Zaehlertyp::VARIANTS`.
    --
    -- 'UNKNOWN' is absent: it is BO4E's forward-compatibility catch-all, not a
    -- schema variant. The write path runs Bo4eStrict::ensure_known_enums before
    -- deriving the column, so an unrecognised Zählertyp is a 422 naming the
    -- field.
    -- Note: `Zaehlertyp` spells it INTELLIGENTES_MESSSYSTEM (three S);
    -- `Geraetetyp` uses INTELLIGENTES_MESSYSTEM (two). That is a BO4E quirk,
    -- not a typo here.
    zaehler_typ  TEXT        CHECK (zaehler_typ IS NULL OR zaehler_typ IN (
                     'BALGENGASZAEHLER',
                     'DREHKOLBENZAEHLER',
                     'DREHSTROMZAEHLER',
                     'ELEKTRONISCHER_ZAEHLER',
                     'INTELLIGENTES_MESSSYSTEM',
                     'LEISTUNGSZAEHLER',
                     'MAXIMUMZAEHLER',
                     'MODERNE_MESSEINRICHTUNG',
                     'TURBINENRADGASZAEHLER',
                     'ULTRASCHALLGASZAEHLER',
                     'WASSERZAEHLER',
                     'WECHSELSTROMZAEHLER',
                     'WIRBELGASZAEHLER'
                 )),
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
    -- BO4E `Geraetetyp` wire value, e.g. 'STROMWANDLER', 'MODEM_GSM',
    -- 'INTELLIGENTES_MESSYSTEM' (two S — see the note on zaehler.zaehler_typ).
    -- Not CHECK-constrained: the enum has 48 variants and turns over between
    -- BO4E versions, so an inline list would be the next thing to drift.
    geraet_typ             TEXT,
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
    -- BO4E TechnischeRessourceNutzung: STROMVERBRAUCHSART | STROMERZEUGUNGSART | SPEICHER
    nutzung           TEXT,
    -- BO4E TechnischeRessourceVerbrauchsart (only for STROMVERBRAUCHSART):
    -- KRAFT_LICHT | WAERME | E_MOBILITAET | STRASSENBELEUCHTUNG
    verbrauchsart     TEXT,
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
CREATE INDEX tr_nutzung ON technische_ressourcen (tenant, nutzung) WHERE nutzung IS NOT NULL;

-- ── Durable event outbox / fan-out log (B11) ──────────────────────────────────
--
-- The full CloudEvent envelope, written BEFORE fan-out (persist-before-dispatch).
-- This is the crash-safe source of truth for the marktd fan-out: a producer's
-- enqueue INSERT here is fatal, so no event is fanned out unless it is durable.
--
-- The fan-out worker (Phase 1) claims rows WHERE fanned_out_at IS NULL, snapshots
-- the matching subscriber set into event_delivery, then stamps fanned_out_at.
--
-- Read via GET /admin/events?from=&to=&type=&limit= (full-envelope replay).
-- Retention: operator-managed; can be partitioned or archived by received_at.

CREATE TABLE event_log (
    event_id      TEXT        PRIMARY KEY,
    -- Monotonic publication order. `received_at` cannot serve as one: it defaults
    -- to now(), the *transaction start* time, so every event from one ingest
    -- shares a timestamp and their relative order is undefined.
    seq           BIGSERIAL   NOT NULL UNIQUE,
    ce_type       TEXT        NOT NULL,
    marktrole     TEXT,
    -- 'STROM' / 'GAS' from the `marktsparte` CloudEvents extension. NULL means
    -- the event is not Sparte-scoped and matches every subscriber filter.
    sparte        TEXT,
    -- Aggregate this event is about (the MaLo-ID, else the MeLo-ID). Deliveries
    -- to one subscriber are ordered within an ordering_key; NULL is unordered.
    ordering_key  TEXT,
    envelope      JSONB       NOT NULL,        -- the ENTIRE serialized MarktEvent
    fanned_out_at TIMESTAMPTZ,
    received_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX event_log_pending   ON event_log (seq) WHERE fanned_out_at IS NULL;
CREATE INDEX event_log_type_time ON event_log (ce_type, received_at DESC);

-- ── Per-subscriber delivery ledger ────────────────────────────────────────────
--
-- One row per (event, subscriber) snapshotted at fan-out time. At-least-once
-- delivery with claim-with-lease (FOR UPDATE SKIP LOCKED) and a status-column
-- DLQ: dead_lettered_at IS NOT NULL is the dead-letter queue. Inspect / requeue
-- via /admin/fanout/dlq.
--
-- GoBD (Vollständigkeit) / § 147 AO: a de.mako.process.initiated event to
-- invoicd announces a message that becomes a Buchungsbeleg (8-year retention);
-- silently dropping it would break the audit trail's completeness.
-- Dead-lettering (never dropping) provides the recovery path.
--
-- Ordering: per **aggregate**, not per endpoint. `seq` and `ordering_key` are
-- denormalised from event_log so the claim can hold a delivery back while an
-- earlier event for the same Marktlokation is still outstanding to the same
-- subscriber. Events about different MaLos never wait for each other, and a
-- dead-lettered row stops blocking its key.
CREATE TABLE event_delivery (
    event_id         TEXT        NOT NULL REFERENCES event_log(event_id) ON DELETE CASCADE,
    subscriber_id    TEXT        NOT NULL,
    webhook_url      TEXT        NOT NULL,
    seq              BIGINT      NOT NULL,
    ordering_key     TEXT,
    attempts         SMALLINT    NOT NULL DEFAULT 0,
    next_attempt_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at     TIMESTAMPTZ,
    dead_lettered_at TIMESTAMPTZ,
    last_error       TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (event_id, subscriber_id)
);
CREATE INDEX event_delivery_due  ON event_delivery (seq)
    WHERE delivered_at IS NULL AND dead_lettered_at IS NULL;
-- Backs the "is an earlier event for this aggregate still outstanding?" probe.
CREATE INDEX event_delivery_order ON event_delivery (subscriber_id, ordering_key, seq)
    WHERE delivered_at IS NULL AND dead_lettered_at IS NULL AND ordering_key IS NOT NULL;
CREATE INDEX event_delivery_dead ON event_delivery (dead_lettered_at) WHERE dead_lettered_at IS NOT NULL;

-- Migration 0002: MMMA / MMM settlement price store
--
-- Both `netzbilanzd` (NB — generates INVOIC 31002/31005/31007/31008) and
-- `invoicd` (LF — validates inbound MMM invoices) need monthly settlement prices:
--
--   • Gas:   Trading Hub Europe (THE) publishes `mmma_preise_gas` monthly.
--   • Strom: Each VNB publishes `mmm_preise_strom` per GPKE (BK6-24-174) Teil 1 Kap. 8.4 monthly.
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

-- ── Strom Mehr-/Mindermengenpreise (BDEW, bundesweit einheitlich) ────────────
--
-- § 13 Abs. 3 StromNZV requires *einheitliche* Mehr-/Mindermengenpreise
-- calculated from monthly market prices. Since 2016 the BDEW determines and
-- publishes them centrally, as one nationwide series with a Mehr and a Minder
-- value per application month. Every Netzbetreiber settles against that same
-- series.
--
-- The month is therefore the whole key. An earlier `vnb_mp_id` column modelled
-- a per-ÜNB (or per-VNB — the comments disagreed with each other) series that
-- does not exist: it let several rows claim the same month with different
-- prices and no rule for choosing between them, and it made netzbilanzd refuse
-- every Strom MMM settlement until an operator configured an ÜNB whose price
-- series was never published.
--
-- Gas is genuinely different and keeps its `marktgebiet` key: there the
-- Marktgebietsverantwortliche (THE) publishes per market area.

CREATE TABLE mmm_preise_strom (
    -- First day of the application month (German local time).
    price_month     DATE        PRIMARY KEY,
    -- Surplus price (Mehrmengen, LF over-delivered) ct/kWh.
    -- Published to four decimals in ct/kWh; NUMERIC keeps that exactly.
    mehr_ct_kwh     NUMERIC     NOT NULL CHECK (mehr_ct_kwh >= 0),
    -- Deficit price (Mindermengen, LF under-delivered) ct/kWh.
    minder_ct_kwh   NUMERIC     NOT NULL CHECK (minder_ct_kwh >= 0),
    source          TEXT        NOT NULL DEFAULT 'manual'
                                CHECK (source IN ('manual', 'bdew-csv', 'csv-import')),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE mmm_preise_strom IS
    'Bundesweit einheitliche Mehr-/Mindermengenpreise Strom (§ 13 Abs. 3 StromNZV), '
    'monatlich vom BDEW ermittelt und veroeffentlicht. Keyed by month alone — there '
    'is no per-Netzbetreiber series.';

CREATE INDEX mmm_strom_month ON mmm_preise_strom (price_month DESC);

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
    -- ISO weekdays the window applies to: 1=Mon … 7=Sun. `SMALLINT[]` rather
    -- than JSONB so the values are typed and constrained — a JSONB array
    -- accepted `["monday"]` and `[0]` just as happily as `[1,2,3,4,5]`.
    wochentage       SMALLINT[]  NOT NULL
                     CHECK (array_length(wochentage, 1) BETWEEN 1 AND 7
                            AND wochentage <@ ARRAY[1,2,3,4,5,6,7]::SMALLINT[]),
    -- Window in German local time, half-open [zeit_von, zeit_bis).
    -- `TIME` rather than `TEXT`: as text, '7:00' and '07:00' were different
    -- values that compared and sorted differently, and nothing rejected '25:00'.
    zeit_von         TIME        NOT NULL,
    zeit_bis         TIME        NOT NULL,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- A zero-length or inverted window classifies no reading at all, which
    -- shows up as silently missing HT/NT energy rather than as an error.
    CONSTRAINT zaehler_saisons_window_ordered CHECK (zeit_von < zeit_bis),
    -- One definition per (register, season, window start).
    UNIQUE (register_id, saison, zeit_von)
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

-- ── ESA consent registry (§49 Abs. 2 Nr. 9 MsbG) ─────────────────────────────
--
-- The Energieserviceanbieter des Anschlussnutzers (ESA) is the only §49-berechtigte
-- Stelle whose authority is purely consent-derived: it may receive metering values
-- for any location for which it holds a GDPR-Art.-7-compliant Einwilligung of the
-- Anschlussnutzer. The consent document itself never travels in a market message
-- (the MSB holds only the ESA's self-assertion), so this registry records the
-- consent's existence and lifecycle — NOT its form.
--
-- Evidence-agnostic by regulatory requirement: BNetzA forbids rejecting a consent
-- for deviating from the BDEW Muster-Einwilligungserklärung. `evidence_uri` /
-- `evidence_hash` are stored verbatim and NEVER validated for shape.

CREATE TABLE esa_einwilligungen (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant              TEXT        NOT NULL,
    -- Opaque reference to the Anschlussnutzer who granted consent (no PII stored
    -- here — the ESA/operator maps this to the natural person).
    anschlussnutzer_ref TEXT        NOT NULL,
    -- MP-ID of the ESA the consent authorises.
    esa_mp_id           TEXT        NOT NULL,
    -- Locations (MaLo/MeLo/NeLo/ZPB) the consent covers.
    location_ids        TEXT[]      NOT NULL,
    -- Free-form scope of the consent (e.g. "lastgang", "zaehlerstaende").
    scope               TEXT        NOT NULL DEFAULT 'werte',
    granted_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Effective window; valid_to NULL = open-ended until revoked.
    valid_from          DATE        NOT NULL DEFAULT CURRENT_DATE,
    valid_to            DATE,
    -- GDPR Art. 7(3): set on Widerruf. Non-NULL ⇒ consent no longer a lawful basis.
    revoked_at          TIMESTAMPTZ,
    -- Opaque evidence pointer + hash of the Einwilligungserklärung. Stored
    -- verbatim, never validated for form (BNetzA: any legally sufficient
    -- consent must be accepted).
    evidence_uri        TEXT,
    evidence_hash       TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One active (non-revoked) consent per (tenant, esa, Anschlussnutzer) — a new
-- grant supersedes by revoking the old one in the handler.
CREATE UNIQUE INDEX esa_einw_active
    ON esa_einwilligungen (tenant, esa_mp_id, anschlussnutzer_ref)
    WHERE revoked_at IS NULL;
CREATE INDEX esa_einw_esa    ON esa_einwilligungen (tenant, esa_mp_id);
-- GIN index so "which consents cover location X" is fast (revocation fan-out).
CREATE INDEX esa_einw_locs   ON esa_einwilligungen USING GIN (location_ids);

COMMENT ON TABLE esa_einwilligungen IS
    'ESA consent registry (§49 Abs. 2 Nr. 9 MsbG). Evidence-agnostic: '
    'evidence_uri/hash are stored verbatim and never validated for form '
    '(BNetzA forbids rejecting consent for deviating from the BDEW template). '
    'Revocation (Art. 7(3) GDPR) fires the 17008 Abbestellung.';

-- ── ESA framework agreements (EDI-Vereinbarung MSB ↔ ESA) ────────────────────
--
-- The bilateral EDI@Energy framework agreement and certificate state between the
-- MSB and an ESA. Required for the AS4 leg to carry ESA value delivery.

CREATE TABLE esa_framework_agreements (
    tenant          TEXT        NOT NULL,
    -- MSB the ESA has an agreement with.
    msb_mp_id       TEXT        NOT NULL,
    esa_mp_id       TEXT        NOT NULL,
    signed_at       TIMESTAMPTZ,
    -- Whether the EDI@Energy framework agreement is in place.
    edi_agreement   BOOLEAN     NOT NULL DEFAULT false,
    -- AS4 certificate exchange state (e.g. "pending", "active", "expired").
    cert_state      TEXT        NOT NULL DEFAULT 'pending',
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant, msb_mp_id, esa_mp_id)
);

COMMENT ON TABLE esa_framework_agreements IS
    'Bilateral EDI@Energy framework agreement + AS4 cert state between MSB and ESA.';

-- ── §20b EnWG Netzzugangsplattform — request registry ─────────────────────────
--
-- Projection of §20b requests submitted through the makod netzzugang adapter:
-- Zählpunktanordnungen (Abs. 2 Nr. 1), Verrechnungskonzepte (Abs. 2 Nr. 2) and
-- the Registrierung von §42c-Vereinbarungen (Abs. 2 Nr. 3). The national
-- platform has no published interface yet (no Festlegung under §20b Abs. 3 as
-- of 2026-07); `payload` is the canonical JSON the adapter delivers and
-- `platform_ref` the platform's reference once one exists.
CREATE TABLE netzzugang_antraege (
    id                 UUID        PRIMARY KEY,
    tenant             TEXT        NOT NULL,
    antrag_typ         TEXT        NOT NULL
        CHECK (antrag_typ IN ('zaehlpunktanordnung',
                              'verrechnungskonzept',
                              'energysharing_vereinbarung')),
    aktion             TEXT        NOT NULL
        CHECK (aktion IN ('bestellung', 'aenderung', 'abbestellung',
                          'registrierung')),
    netzanschluss_id   TEXT        NOT NULL,
    nb_mp_id           TEXT        NOT NULL,
    -- Opaque requester reference (Anschlussnehmer/-nutzer) — no PII.
    antragsteller_ref  TEXT        NOT NULL,
    status             TEXT        NOT NULL DEFAULT 'erfasst'
        CHECK (status IN ('erfasst', 'uebermittelt', 'bestaetigt',
                          'abgelehnt', 'fehlgeschlagen')),
    payload            JSONB       NOT NULL DEFAULT '{}'::jsonb,
    platform_ref       TEXT,
    -- Optimistic-locking version; incremented on every successful write.
    version            BIGINT      NOT NULL DEFAULT 1,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    submitted_at       TIMESTAMPTZ,
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX nz_antraege_tenant_status
    ON netzzugang_antraege (tenant, status);
CREATE INDEX nz_antraege_anschluss
    ON netzzugang_antraege (tenant, netzanschluss_id);

COMMENT ON TABLE netzzugang_antraege IS
    '§20b EnWG Netzzugangsplattform requests (Zählpunktanordnung / Verrechnungskonzept / §42c-Registrierung) and their lifecycle state.';

-- ── Messstellenbetreiberrahmenvertrag Gas (GeLi Gas 3.0) ──────────────────────
--
-- GeLi Gas 3.0 (BK7-24-01-009, Tenor Ziff. 13–16): the old BNetzA-imposed Gas
-- MSB-Rahmenvertrag (BK7-17-026) is revoked effective 01.10.2026; GNB and MSB
-- must conclude the market-developed replacement (KoV XV Anlage 8, versioned
-- with the KoV) in its jeweils gültige Fassung. GNB duties: publish the
-- contract on their website, enable conclusion for any MSB
-- non-discriminatorily, and migrate existing BK7-17-026 contracts by the
-- deadline. One row per (GNB, MSB) conclusion; legal basis §9 Abs. 1 Nr. 3
-- i.V.m. Abs. 4 MsbG.
CREATE TABLE msb_rahmenvertraege_gas (
    id            UUID        PRIMARY KEY,
    tenant        TEXT        NOT NULL,
    gnb_mp_id     TEXT        NOT NULL,
    msb_mp_id     TEXT        NOT NULL,
    -- Contract text edition, e.g. 'KoV XV Anlage 8' (pre-01.10.2026 legacy:
    -- 'BK7-17-026').
    fassung       TEXT        NOT NULL DEFAULT 'KoV XV Anlage 8',
    status        TEXT        NOT NULL DEFAULT 'angeboten'
        CHECK (status IN ('angeboten', 'abgeschlossen',
                          'anpassung_erforderlich', 'beendet')),
    signed_at     TIMESTAMPTZ,
    valid_from    DATE        NOT NULL,
    valid_to      DATE,
    -- Full BO4E Vertrag payload (vertragsart RAHMENVERTRAG).
    vertrag       JSONB       NOT NULL DEFAULT '{}'::jsonb,
    version       BIGINT      NOT NULL DEFAULT 1,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX msb_rv_gas_parties
    ON msb_rahmenvertraege_gas (tenant, gnb_mp_id, msb_mp_id, valid_from);
CREATE INDEX msb_rv_gas_status
    ON msb_rahmenvertraege_gas (tenant, status);

COMMENT ON TABLE msb_rahmenvertraege_gas IS
    'Gas MSB framework contracts (GeLi Gas 3.0 Tenor 13–16, KoV XV Anlage 8): per-(GNB,MSB) conclusion state incl. the BK7-17-026 migration duty by 01.10.2026.';

-- ── MaBiS-Zählpunkt assignments ──────────────────────────────────────────────
--
-- Which MaBiS-Zählpunkt a Bilanzierungsgebiet's Summenzeitreihen are filed
-- under. MSCONS Summenzeitreihen (13003/13023) carry the Meldepunkt as SG6
-- LOC+172 and the Bilanzierungsgebiet as LOC+107 — different identifiers with
-- different meanings, both free text at the MIG level. A wrong Meldepunkt
-- therefore produces a message that parses and validates and is, to the BIKO,
-- indistinguishable from a correct one.
--
-- Master data rather than service configuration so that a territory with no
-- assignment fails its submission loudly instead of silently substituting the
-- Bilanzierungsgebiet EIC. Read by `mabis-syncd` before every submission.
--
-- BNetzA BK6-24-174 Anlage 3 (MaBiS); MSCONS AHB 3.2 SG6.

CREATE TABLE mabis_zaehlpunkte (
    bilanzierungsgebiet  TEXT        NOT NULL,   -- EIC, 16 chars (LOC+107)
    tenant               TEXT        NOT NULL,
    mabis_zp_id          TEXT        NOT NULL    -- Meldepunkt (LOC+172)
                         CHECK (length(trim(mabis_zp_id)) > 0),
    -- No `sparte`: MaBiS is the Marktregeln für die Durchführung der
    -- Bilanzkreisabrechnung **Strom**. Gas balancing runs under GaBi Gas, which
    -- has no MaBiS-Zählpunkt at all, so a row with sparte = 'GAS' described
    -- something that does not exist and invited an operator to record one.
    source               TEXT        NOT NULL DEFAULT 'manual',
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (bilanzierungsgebiet, tenant),

    -- The Meldepunkt must never be the territory code it belongs to. That
    -- substitution is the exact defect this table exists to prevent, and it is
    -- invisible once the message is on the wire.
    CONSTRAINT mabis_zp_not_the_gebiet CHECK (mabis_zp_id <> bilanzierungsgebiet),

    -- …and it must not be *any* territory code. A Bilanzierungsgebiet EIC is 16
    -- characters, a Zählpunktbezeichnung 33, so the length alone separates them.
    -- Without this, territory A's EIC stored as territory B's Meldepunkt passes
    -- the inequality above and reads as valid master data until a submission run
    -- refuses it — long after the assignment was made.
    CONSTRAINT mabis_zp_ist_zaehlpunktbezeichnung
        CHECK (length(trim(mabis_zp_id)) = 33)
);

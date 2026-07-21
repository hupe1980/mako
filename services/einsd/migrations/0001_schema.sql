-- ── einsd schema — Einspeiser Registry + EEG/KWKG Settlement ─────────────────
--
-- Regulatory frameworks:
--   EEG 2000–2023 (§§20–22, §24, §21 Abs. 3, §42b, §44b, §48–54)
--   KWKG 2023 (§7 KWK-Zuschlag)
--   § 147 AO / GoBD (3-year settlement audit trail)
--
-- Tables:
--   eeg_anlagen               — central plant register (composite PK: tr_id + tenant)
--   settlement_receipts       — monthly settlement audit log (§ 147 AO / GoBD)
--   settlement_state_transitions — state machine audit trail
--   settlement_receipt_history — § 147 AO / GoBD immutable correction snapshots
--   eeg_verguetungssaetze     — EEG/KWKG tariff reference data
--   epex_monthly_prices       — monthly EPEX Spot reference (Marktprämie)
--   jahresmarktwert_preise    — technology-specific Jahresmarktwert (§20 Abs. 2 EEG)
--   sect53b_reductions        — §53b regional Grünstromkennzeichnung reductions
--   sect54_reductions         — §54 BNetzA Ausschreibungsreduzierung per plant

-- ── EEG/KWKG plant register ───────────────────────────────────────────────────

CREATE TABLE eeg_anlagen (
    tr_id              TEXT        NOT NULL,
    tenant             TEXT        NOT NULL,
    malo_id            TEXT        NOT NULL,
    melo_id            TEXT,

    -- EEG law year (2000|2004|2009|2012|2017|2021|2023) or 0 for KWKG
    eeg_gesetz         SMALLINT    NOT NULL
                       CHECK (eeg_gesetz IN (0, 2000, 2004, 2009, 2012, 2017, 2021, 2023)),

    inbetriebnahme     DATE        NOT NULL,
    leistung_kwp       NUMERIC(12, 3) NOT NULL,

    erzeugungsart      TEXT        NOT NULL CHECK (erzeugungsart IN (
        -- Solar PV
        'SOLAR',            -- generic / backward compat
        'SOLAR_AUFDACH',    -- roof-mounted (§48 Abs. 1 EEG 2023)
        'SOLAR_FREFLAECHE', -- ground-mounted (tendering >1 MWp)
        'SOLAR_AGRIPV',     -- §51a Agri-PV (+0.5 ct/kWh premium)
        'SOLAR_MIETERSTROM',-- §21 Abs. 3 building community solar
        'SOLAR_STECKER',    -- Balkonkraftwerk <800 W (simplified)
        -- Wind
        'WIND_ONSHORE',     -- §21 EEG (tendering >750 kW)
        'WIND_OFFSHORE',    -- §§70ff EEG
        -- Biomass / Gas
        'BIOMASSE',         -- generic solid biomass
        'BIOMASSE_HOLZ',    -- wood biomass (§42a EEG restricted)
        'BIOGAS',           -- fermentation biogas
        'BIOMETHANE',       -- upgraded biogas (grid injection)
        'KLAEGAS',          -- sewage gas (§41 EEG)
        'GRUBENGAS',        -- mine gas (§41 EEG)
        'DEPONIEGAS',       -- landfill gas (§41 EEG)
        -- Hydro / Geo
        'WASSERKRAFT',      -- run-of-river hydro (§40 EEG)
        'GEOTHERMIE',       -- geothermal (§45 EEG)
        'GEZEITEN',         -- tidal / wave
        -- CHP
        'KWKG',             -- combined heat & power (KWKG)
        'SONSTIGE'
    )),

    -- Feed-in tariff fixed at inbetriebnahme for the full Förderungsdauer (§20 EEG)
    verguetungssatz_ct NUMERIC(8, 4) NOT NULL,
    foerderendedatum   DATE         NOT NULL,

    -- Settlement model (accepts both legacy German names and canonical enum names)
    settlement_model   TEXT        NOT NULL DEFAULT 'VERGUETUNG' CHECK (settlement_model IN (
        -- Legacy German names
        'VERGUETUNG', 'DIREKTVERMARKTUNG', 'AUSSCHREIBUNG', 'POST_EEG_SPOT',
        'EIGENVERBRAUCH', 'MIETERSTROM', 'KWKG_ZUSCHLAG', 'FLEXIBILITAET',
        'FLEXIBILITAET_ZUSCHLAG',
        -- Canonical SettlementScheme names (Rust enum)
        'FEED_IN_TARIFF', 'MARKET_PREMIUM', 'TENANT_ELECTRICITY', 'POST_EEG',
        'KWK_SURCHARGE', 'FLEXIBILITY_PREMIUM', 'FLEXIBILITY_SURCHARGE',
        'TEMPORARY_FEED_IN_TARIFF',
        -- Solarpaket I + §21a
        'GGV',                      -- §42b EEG 2023 Gemeinschaftliche Gebäudeversorgung
        'SONSTIGE_DIREKTVERMARKTUNG' -- §21a direct third-party sale
    )),

    direktvermarktung          BOOLEAN     NOT NULL DEFAULT false,
    -- §20 EEG: Anzulegender Wert in ct/kWh for Marktprämie
    direktverm_aw_ct           NUMERIC(8, 4),
    direktverm_mp_id           TEXT,
    -- §21 Abs. 3 EEG: Mieterstrom surcharge ct/kWh above base tariff
    mieter_zuschlag_ct         NUMERIC(6, 4),
    -- BNetzA Zuschlag-ID from tender result (Ausschreibungsanlagen)
    ausschreibungs_zuschlag_id TEXT,
    -- §22 EEG 2023: the awarded anzulegender Wert, distinct from a contracted
    -- Direktvermarktung value. Held separately so an award is never confused
    -- with a bilaterally agreed rate.
    zuschlagswert_ct           NUMERIC(8, 4),
    zuschlag_datum             DATE,
    -- §39n EEG 2023: Innovationsausschreibung pays a fixed market premium.
    ist_innovationsausschreibung BOOLEAN NOT NULL DEFAULT false,
    -- §22b EEG 2023: a Bürgerenergiegesellschaft (§3 Nr. 15) is exempt from
    -- needing a valid Zuschlag, so it settles at the statutory rate even in an
    -- auction-eligible size class.
    ist_buergerenergie         BOOLEAN NOT NULL DEFAULT false,

    -- §22 EEG 2023 Repowering (clock reset)
    ist_repowering             BOOLEAN     NOT NULL DEFAULT false,
    ursprungs_inbetriebnahme   DATE,       -- original commissioning before repowering
    repowering_datum           DATE,

    -- §24 EEG 2023 Zusammenlegung (merged plants)
    parent_tr_id               TEXT,

    -- KWKG: KWK-Zuschlag duration
    kwk_foerderdauer_h         INTEGER,    -- full-load hours (>2 MW: 30,000 h)
    kwk_foerderdauer_years     SMALLINT,   -- years (≤2 MW)
    kwk_strom_kwh_gesamt       NUMERIC(14, 3), -- accumulated kWh for hour-limit tracking

    -- §50 EEG: Flexibilitätsprämie (biomass demand response)
    flex_leistung_kw           NUMERIC(8, 3),
    flex_praemie_ct_kwh        NUMERIC(6, 4),

    -- Plant status
    status                     TEXT        NOT NULL DEFAULT 'aktiv' CHECK (status IN (
                                    'aktiv', 'abgemeldet', 'foerderung_beendet', 'repowered'
                                )),
    notes                      TEXT,

    -- ── MaStR registration (migration 0002) ──────────────────────────────────
    mastr_registriert          BOOLEAN     NOT NULL DEFAULT true,
    mastr_nummer               TEXT,
    mastr_datum                DATE,

    -- Bank account for EEG settlement payments
    bank_iban                  TEXT,
    bank_bic                   TEXT,
    zahlungsempfaenger         TEXT,

    -- ── Plant attributes (migration 0003) ────────────────────────────────────
    -- 'Neubau' | 'Repowering' | 'Modernisierung'
    inbetriebnahme_typ         TEXT,
    -- §36k EEG 2023: Wind Standortgütegrad for Korrekturfaktor computation
    wind_guetegrad             NUMERIC(5, 3),
    wind_korrekturfaktor       NUMERIC(6, 5),
    wind_standortklasse        TEXT,
    -- §48 EEG: Bauform for solar PV tariff lookup (Freifläche | Aufdach | BIPV)
    solar_bauform              TEXT,
    -- §9 EEG / §29 MsbG: date Fernsteuerbarkeit (remote control) was installed
    fernsteuerbarkeit_datum    DATE,
    -- §21b EEG: direct marketing obligation flag (>100 kW mandatory from 2012)
    direktvermarktung_pflicht  BOOLEAN     NOT NULL DEFAULT false,
    -- §24 Erweiterung: capacity blocks JSONB (Vec<CapacityBlock>)
    capacity_blocks            JSONB,
    -- Metering type: SLP | RLM | IMSYS
    metering_mode              TEXT        CHECK (metering_mode IN ('SLP', 'RLM', 'IMSYS')),

    -- ── Settlement lifecycle (migration 0004) ─────────────────────────────────
    -- Active | Reduced | Suspended | PostEeg | Ended
    settlement_state           TEXT,
    -- §52 Abs. 6 EEG 2023: enable Pflichtzahlung netting against Vergütung
    sect52_netting_enabled     BOOLEAN     NOT NULL DEFAULT true,

    -- ── §51b EEG 2023: biogas Ausschreibungsanlage flag ──────────────────────
    is_biogas_sect51b          BOOLEAN     NOT NULL DEFAULT false,

    -- ── Ausschreibung lifecycle (migration 0006) ──────────────────────────────
    -- §35a EEG: date the Zuschlag expires if plant not commissioned
    zuschlag_erloeschen_datum  DATE,
    award_expired              BOOLEAN     NOT NULL DEFAULT false,
    -- §52: cumulative violation start dates for Pflichtzahlung
    mastr_violation_start      DATE,
    fernsteuerbarkeit_violation_start DATE,
    -- §21b guard: date of last Veräußerungsform switch (monthly lock)
    last_veraeusserungsform_switch DATE,
    -- §51a: cumulative quarter-hours for Förderzeitraum extension
    verlaengerungsanspruch_qh_gesamt BIGINT NOT NULL DEFAULT 0,

    -- ── §53b regional reduction ───────────────────────────────────────────────
    grid_area                  TEXT,

    -- ── §44b EEG 2023: Biogas annual 45%-cap quota ───────────────────────────
    biogas_quota_kwh_ytd       NUMERIC(14, 3) NOT NULL DEFAULT 0,
    biogas_quota_ytd_year      SMALLINT,

    -- ── §51 Abs. 2 Nr. 1 EEG 2023: iMSys rollout ────────────────────────────
    imesys_rollout_datum       DATE,

    -- ── §42b EEG 2023: GGV Nutzungsplan ──────────────────────────────────────
    ggv_nutzungsplan           JSONB,

    -- ── §21c notification tracking ───────────────────────────────────────────
    veraeusserungsform_notification_sent_at TIMESTAMPTZ,

    -- ── §§42–44 EEG 2023: Biomass fuel composition ───────────────────────────
    biomasse_hauptbrennstoff   TEXT,
    biomasse_guelle_anteil     NUMERIC(5, 4),
    biomasse_energiepflanzen_anteil NUMERIC(5, 4),

    created_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (tr_id, tenant)
);

COMMENT ON TABLE eeg_anlagen IS
    'Central EEG/KWKG plant register. Composite PK (tr_id, tenant) for multi-tenant isolation. '
    'foerderendedatum = inbetriebnahme + 20 years for EEG; reset on repowering.';

CREATE INDEX ea_malo_tenant      ON eeg_anlagen (malo_id, tenant);
CREATE INDEX ea_foerderend       ON eeg_anlagen (foerderendedatum, tenant) WHERE status = 'aktiv';
CREATE INDEX ea_settlement       ON eeg_anlagen (settlement_model, tenant) WHERE status = 'aktiv';
CREATE INDEX ea_gesetz_tenant    ON eeg_anlagen (eeg_gesetz, tenant);
CREATE INDEX ea_repowering       ON eeg_anlagen (tenant) WHERE ist_repowering = true;
CREATE INDEX ea_zusammenlegung   ON eeg_anlagen (parent_tr_id, tenant) WHERE parent_tr_id IS NOT NULL;
CREATE INDEX ea_kwkg             ON eeg_anlagen (tenant) WHERE settlement_model = 'KWKG_ZUSCHLAG';
CREATE INDEX ea_grid_area        ON eeg_anlagen (tenant, grid_area) WHERE grid_area IS NOT NULL;
CREATE INDEX ea_award_expired    ON eeg_anlagen (tenant, award_expired) WHERE award_expired = true;
CREATE INDEX ea_mastr_violation  ON eeg_anlagen (tenant, mastr_violation_start) WHERE mastr_violation_start IS NOT NULL;
CREATE INDEX ea_biogas_quota     ON eeg_anlagen (tenant, biogas_quota_ytd_year)
    WHERE erzeugungsart = 'BIOGAS' AND is_biogas_sect51b = false;
CREATE INDEX ea_notification_pending ON eeg_anlagen (tenant, last_veraeusserungsform_switch)
    WHERE veraeusserungsform_notification_sent_at IS NULL AND last_veraeusserungsform_switch IS NOT NULL;

-- ── Monthly settlement receipts ───────────────────────────────────────────────
-- § 147 AO / GoBD: 3-year retention. Written before any CloudEvent dispatch.
-- Correction receipts (is_correction = true) coexist freely with originals.

CREATE TABLE settlement_receipts (
    id                          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tr_id                       TEXT        NOT NULL,
    tenant                      TEXT        NOT NULL,
    billing_year                SMALLINT    NOT NULL,
    billing_month               SMALLINT    NOT NULL CHECK (billing_month BETWEEN 1 AND 12),
    settlement_model            TEXT        NOT NULL,
    einspeisemenge_kwh          NUMERIC(14, 3),
    settlement_eur              NUMERIC(14, 5),
    -- §26 Abs. 1 EEG: Fälligkeitsdatum = 15th of following month
    faelligkeitsdatum           DATE,
    -- §51a EEG: quarter-hours of Förderzeitraum extension accrued this period
    verlaengerungsanspruch_qh   BIGINT      NOT NULL DEFAULT 0,
    -- §25/§26 EEG: fraction of billing month with entitlement (mid-month events)
    billing_days_fraction       NUMERIC(8, 6),
    -- §52 EEG: separate Pflichtzahlung (penalty) amount
    pflichtzahlung_eur          NUMERIC(14, 5),
    -- § 147 AO / GoBD: itemized position snapshot for audit trail
    positions_json              JSONB,
    -- Full rubo4e::current::Rechnung JSONB
    rechnung_json               JSONB,
    bo4e_version                TEXT        NOT NULL DEFAULT 'v202607.0.0',
    -- 'calculated' | 'price_missing' | 'no_data' | 'error' | 'foerderung_beendet'
    status                      TEXT        NOT NULL DEFAULT 'calculated',
    -- Active | Reduced | Suspended | PostEeg | Ended (at time of settlement)
    settlement_state            TEXT,
    -- CloudEvent ID (de.eeg.verguetung.berechnet or de.eeg.marktpraemie.berechnet)
    ce_id                       UUID,
    error_detail                TEXT,

    -- Correction chain support (§ 147 AO / GoBD)
    is_correction               BOOLEAN     NOT NULL DEFAULT false,
    correction_of               UUID,       -- FK to original receipt.id
    correction_reason           TEXT,

    settled_at                  TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE settlement_receipts IS
    '§ 147 AO / GoBD: 3-year immutable settlement audit log. '
    'Correction receipts (is_correction=true) coexist with originals via partial unique index.';

-- Partial unique: exactly one non-correction receipt per plant × period.
-- Upserts must repeat the predicate — ON CONFLICT (cols) WHERE is_correction = false —
-- because Postgres cannot infer a partial index from the column list alone.
CREATE UNIQUE INDEX sr_unique_initial
    ON settlement_receipts (tr_id, tenant, billing_year, billing_month)
    WHERE is_correction = false;

COMMENT ON INDEX sr_unique_initial IS
    '§ 147 AO / GoBD: exactly one initial receipt per billing period per plant. '
    'Correction receipts excluded — they accumulate freely as an immutable audit chain. '
    'Named index supports ON CONFLICT ON CONSTRAINT sr_unique_initial in upsert code.';

CREATE INDEX sr_tr_period       ON settlement_receipts (tr_id, tenant, billing_year DESC, billing_month DESC);
CREATE INDEX sr_ce_pending      ON settlement_receipts (tenant, settled_at DESC)
    WHERE ce_id IS NULL AND status = 'calculated';
CREATE INDEX sr_faelligkeitsdatum ON settlement_receipts (tenant, faelligkeitsdatum)
    WHERE faelligkeitsdatum IS NOT NULL;
CREATE INDEX sr_correction      ON settlement_receipts (correction_of)
    WHERE is_correction = true;

-- ── Settlement state audit log ────────────────────────────────────────────────

CREATE TABLE settlement_state_transitions (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tr_id           TEXT        NOT NULL,
    tenant          TEXT        NOT NULL,
    from_state      TEXT        NOT NULL,
    to_state        TEXT        NOT NULL,
    effective_from  DATE        NOT NULL,
    reason          TEXT        NOT NULL,
    notes           TEXT,
    recorded_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT fk_sst_anlage FOREIGN KEY (tr_id, tenant)
        REFERENCES eeg_anlagen (tr_id, tenant) ON DELETE CASCADE
);

COMMENT ON TABLE settlement_state_transitions IS
    'Audit log of SettlementPeriodState changes. '
    'INSERT-only. Used for regulatory audit and §52 compliance reporting.';

CREATE INDEX sst_tr_id ON settlement_state_transitions (tr_id, tenant, effective_from DESC);

-- ── § 147 AO / GoBD: correction receipt snapshots ─────────────────────────────────

CREATE TABLE settlement_receipt_history (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    original_id     UUID        NOT NULL,   -- settlement_receipts.id before correction
    snapshot_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    tr_id           TEXT        NOT NULL,
    tenant          TEXT        NOT NULL,
    billing_year    SMALLINT    NOT NULL,
    billing_month   SMALLINT    NOT NULL,
    settlement_eur  NUMERIC(14, 5),
    status          TEXT,
    -- Full copy of the pre-correction receipt row for forensic audit
    settlement_data JSONB       NOT NULL
);

COMMENT ON TABLE settlement_receipt_history IS
    '§ 147 AO / GoBD: immutable snapshot taken before each correction upsert. '
    'settlement_data = full receipt JSONB for forensic tracing.';

CREATE INDEX srh_original_id ON settlement_receipt_history (original_id);
CREATE INDEX srh_tr_id       ON settlement_receipt_history (tr_id, tenant, billing_year, billing_month);

-- ── EEG/KWKG tariff reference table ──────────────────────────────────────────
-- Lookup table for statutory feed-in tariff / premium rates.
-- verguetungsform: UEBERSCHUSS | VOLLEINSPEISUNG | KWK_ZUSCHLAG
-- Quarterly degression (§23a EEG) is applied by the eeg-billing degression module.
-- Import updated rates via PUT /api/v1/verguetungssaetze or xtask import.

CREATE TABLE eeg_verguetungssaetze (
    id                  SERIAL      PRIMARY KEY,
    erzeugungsart       TEXT        NOT NULL DEFAULT 'SOLAR',
    leistung_min_kwp    NUMERIC(10, 3) NOT NULL,
    leistung_max_kwp    NUMERIC(10, 3),         -- NULL = no upper bound
    -- 'UEBERSCHUSS' | 'VOLLEINSPEISUNG' | 'KWK_ZUSCHLAG'
    verguetungsform     TEXT        DEFAULT 'UEBERSCHUSS',
    billing_start       DATE        NOT NULL,
    billing_end         DATE,                   -- NULL = currently valid
    verguetungssatz_ct  NUMERIC(8, 4) NOT NULL,
    eeg_gesetz          SMALLINT    NOT NULL,
    notes               TEXT,
    UNIQUE (erzeugungsart, leistung_min_kwp, billing_start)
);

COMMENT ON TABLE eeg_verguetungssaetze IS
    'EEG/KWKG statutory tariff reference. '
    'verguetungssatz_ct = NET rate for EEG (§53 deduction already applied where applicable). '
    'Quarterly degression computed by eeg-billing degression module for post-billing_start quarters.';

CREATE INDEX evs_lookup ON eeg_verguetungssaetze (erzeugungsart, billing_start, billing_end);

-- Seed: EEG 2023 + Solarpaket I rates (BGBl. I Nr. 107, 16.05.2024)
-- Operators MUST verify against current BNetzA publications before production use.
INSERT INTO eeg_verguetungssaetze (erzeugungsart, leistung_min_kwp, leistung_max_kwp, verguetungsform, verguetungssatz_ct, billing_start, billing_end, eeg_gesetz, notes) VALUES
-- Solar Aufdach / SOLAR — Überschusseinspeisung (§48 Abs. 1 EEG 2023, Solarpaket I)
('SOLAR_AUFDACH',   0,   10, 'UEBERSCHUSS',   8.11, '2024-05-01', NULL, 2023, 'Solarpaket I §48 Abs. 1 Nr. 1a, ≤10 kWp'),
('SOLAR_AUFDACH',  10,   40, 'UEBERSCHUSS',   7.03, '2024-05-01', NULL, 2023, 'Solarpaket I §48 Abs. 1 Nr. 1b, >10–40 kWp'),
('SOLAR_AUFDACH',  40,  100, 'UEBERSCHUSS',   5.74, '2024-05-01', NULL, 2023, 'Solarpaket I §48 Abs. 1 Nr. 1c, >40–100 kWp'),
('SOLAR_AUFDACH', 100,  400, 'UEBERSCHUSS',   4.30, '2024-05-01', NULL, 2023, 'Solarpaket I §48 Abs. 1 Nr. 1d, >100–400 kWp'),
('SOLAR_AUFDACH', 400, 1000, 'UEBERSCHUSS',   3.14, '2024-05-01', NULL, 2023, 'Solarpaket I §48 Abs. 1 Nr. 1e, >400 kWp–1 MWp'),
-- Solar Aufdach — Volleinspeisung (§48 Abs. 2a EEG 2023, +4.8 ct/kWh bonus)
('SOLAR_AUFDACH',   0,   10, 'VOLLEINSPEISUNG', 12.91, '2024-05-01', NULL, 2023, 'Solarpaket I §48 Abs. 2a, ≤10 kWp'),
('SOLAR_AUFDACH',  10,   40, 'VOLLEINSPEISUNG', 10.83, '2024-05-01', NULL, 2023, 'Solarpaket I §48 Abs. 2a, >10–40 kWp'),
('SOLAR_AUFDACH',  40,  100, 'VOLLEINSPEISUNG',  9.54, '2024-05-01', NULL, 2023, 'Solarpaket I §48 Abs. 2a, >40–100 kWp'),
-- Generic SOLAR alias (backward compat)
('SOLAR',   0,   10, 'UEBERSCHUSS', 8.11, '2024-05-01', NULL, 2023, 'Solarpaket I generic alias'),
('SOLAR',  10,   40, 'UEBERSCHUSS', 7.03, '2024-05-01', NULL, 2023, 'Solarpaket I generic alias'),
('SOLAR',  40,  100, 'UEBERSCHUSS', 5.74, '2024-05-01', NULL, 2023, 'Solarpaket I generic alias'),
-- Pre-Solarpaket I rates (EEG 2023 initial, 2023-02–2024-04)
('SOLAR_AUFDACH',   0,  10, 'UEBERSCHUSS', 7.71, '2023-02-01', '2024-04-30', 2023, 'EEG 2023 initial §48 Abs. 1 Nr. 1a'),
('SOLAR_AUFDACH',  10,  40, 'UEBERSCHUSS', 6.69, '2023-02-01', '2024-04-30', 2023, 'EEG 2023 initial §48 Abs. 1 Nr. 1b'),
('SOLAR_AUFDACH',  40, 100, 'UEBERSCHUSS', 5.76, '2023-02-01', '2024-04-30', 2023, 'EEG 2023 initial §48 Abs. 1 Nr. 1c'),
-- EEG 2021 solar
('SOLAR',   0,  10, 'UEBERSCHUSS', 9.58, '2021-01-01', '2021-12-31', 2021, 'EEG 2021 ≤10 kWp'),
('SOLAR',  10,  40, 'UEBERSCHUSS', 9.33, '2021-01-01', '2021-12-31', 2021, 'EEG 2021 10–40 kWp'),
-- EEG 2017 solar
('SOLAR',   0,  10, 'UEBERSCHUSS', 9.87, '2017-04-01', '2020-12-31', 2017, 'EEG 2017 ≤10 kWp avg'),
-- Wind onshore statutory (≤750 kW only; >750 kW via Ausschreibung)
('WIND_ONSHORE',  0, 750, 'UEBERSCHUSS', 7.35, '2023-01-01', NULL, 2023, 'EEG 2023 §21 Onshore ≤750 kW'),
-- KWKG 2023 (§7 Abs. 1 KWKG 2023)
('KWKG',    0,   50, 'KWK_ZUSCHLAG', 8.00, '2023-01-01', NULL, 0, 'KWKG 2023 §7 Abs. 1 Nr. 1, ≤50 kW_el'),
('KWKG',   50,  100, 'KWK_ZUSCHLAG', 6.00, '2023-01-01', NULL, 0, 'KWKG 2023 §7 Abs. 1 Nr. 2, >50–100 kW_el'),
('KWKG',  100,  250, 'KWK_ZUSCHLAG', 5.00, '2023-01-01', NULL, 0, 'KWKG 2023 §7 Abs. 1 Nr. 3, >100–250 kW_el'),
('KWKG',  250, 2000, 'KWK_ZUSCHLAG', 4.00, '2023-01-01', NULL, 0, 'KWKG 2023 §7 Abs. 1 Nr. 4, >250 kW–2 MW_el'),
('KWKG', 2000, NULL, 'KWK_ZUSCHLAG', 3.00, '2023-01-01', NULL, 0, 'KWKG 2023 §7 Abs. 1 Nr. 5, >2 MW_el'),
-- Biomasse (§42–43 EEG 2023)
('BIOMASSE',   0, 150, 'UEBERSCHUSS', 16.40, '2023-01-01', NULL, 2023, 'EEG 2023 §21 Biomasse ≤150 kW'),
('BIOMASSE', 150, 500, 'UEBERSCHUSS', 14.30, '2023-01-01', NULL, 2023, 'EEG 2023 §21 Biomasse 150–500 kW'),
-- Gas-related (§41 EEG 2023)
('KLAEGAS', 0, NULL, 'UEBERSCHUSS', 12.50, '2023-01-01', NULL, 2023, 'EEG 2023 §41 Klärgas'),
('GRUBENGAS', 0, NULL, 'UEBERSCHUSS', 12.50, '2023-01-01', NULL, 2023, 'EEG 2023 §41 Grubengas'),
('DEPONIEGAS', 0, NULL, 'UEBERSCHUSS', 9.10, '2023-01-01', NULL, 2023, 'EEG 2023 §41 Deponiegas'),
-- Wasserkraft (§40 EEG 2023)
('WASSERKRAFT',    0,  500, 'UEBERSCHUSS', 12.48, '2023-01-01', NULL, 2023, 'EEG 2023 §40 ≤500 kW'),
('WASSERKRAFT',  500, 5000, 'UEBERSCHUSS',  8.59, '2023-01-01', NULL, 2023, 'EEG 2023 §40 500 kW–5 MW'),
('WASSERKRAFT', 5000, NULL, 'UEBERSCHUSS',  7.56, '2023-01-01', NULL, 2023, 'EEG 2023 §40 >5 MW')
ON CONFLICT DO NOTHING;

-- ── EPEX Spot monthly reference prices ───────────────────────────────────────
-- Required for DIREKTVERMARKTUNG (Marktprämie) and POST_EEG_SPOT settlement.

CREATE TABLE epex_monthly_prices (
    billing_year    SMALLINT    NOT NULL,
    billing_month   SMALLINT    NOT NULL CHECK (billing_month BETWEEN 1 AND 12),
    avg_ct_kwh      NUMERIC(8, 4) NOT NULL,
    source          TEXT        NOT NULL DEFAULT 'manual',
    imported_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (billing_year, billing_month)
);

COMMENT ON TABLE epex_monthly_prices IS
    'Generic monthly EPEX Spot reference. '
    'Used when no technology-specific Jahresmarktwert is available. '
    'Import via PUT /api/v1/epex-monthly/{year}/{month}.';

-- ── §20 Abs. 2 + Anlage 1 EEG 2023: technology-specific Jahresmarktwert ──────

CREATE TABLE jahresmarktwert_preise (
    billing_year    SMALLINT    NOT NULL,
    billing_month   SMALLINT    NOT NULL CHECK (billing_month BETWEEN 1 AND 12),
    -- Matches erzeugungsart values; 'DEFAULT' = generic fallback
    erzeugungsart   TEXT        NOT NULL,
    avg_ct_kwh      NUMERIC(8, 4) NOT NULL
                    CHECK (avg_ct_kwh BETWEEN -100 AND 1000),
    source          TEXT        NOT NULL DEFAULT 'manual',
    imported_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (billing_year, billing_month, erzeugungsart)
);

COMMENT ON TABLE jahresmarktwert_preise IS
    '§20 Abs. 2 + Anlage 1 EEG 2023: technology-specific monthly Marktwert. '
    'Published by ÜNB (netztransparenz.de). '
    'Lookup order: exact erzeugungsart → DEFAULT → epex_monthly_prices.';

CREATE INDEX jmw_period    ON jahresmarktwert_preise (billing_year DESC, billing_month DESC);
CREATE INDEX jmw_art_period ON jahresmarktwert_preise (erzeugungsart, billing_year DESC, billing_month DESC);

-- ── §53b EEG 2023: Regional Grünstromkennzeichnung reductions ─────────────────
-- Applied per Netzgebiet (not per plant). The settlement engine queries this table
-- to reduce tariffs for all plants in a certified grid area.

CREATE TABLE sect53b_reductions (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant          TEXT        NOT NULL,
    grid_area       TEXT        NOT NULL,
    reduction_ct_kwh NUMERIC(8, 5) NOT NULL,
    bnetza_ref      TEXT        NOT NULL,
    effective_from  DATE        NOT NULL,
    effective_until DATE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE sect53b_reductions IS
    '§53b EEG 2023 Grünstromkennzeichnung reductions per Netzgebiet. '
    'Applied per grid_area — all plants in the area are affected.';

CREATE INDEX sect53b_area ON sect53b_reductions (tenant, grid_area, effective_from DESC);

-- ── §54 EEG 2023: Ausschreibungsreduzierung per plant ────────────────────────
-- BNetzA may reduce the awarded Anzulegender Wert after commissioning.
-- effective_aw = awarded_aw - SUM(deduction_ct_kwh) (floor 0).

CREATE TABLE sect54_reductions (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tr_id           TEXT        NOT NULL,
    tenant          TEXT        NOT NULL,
    deduction_ct_kwh NUMERIC(8, 5) NOT NULL,
    bnetza_ref      TEXT        NOT NULL,
    effective_from  DATE        NOT NULL,
    effective_until DATE,
    notes           TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT fk_sect54_anlage FOREIGN KEY (tr_id, tenant)
        REFERENCES eeg_anlagen (tr_id, tenant) ON DELETE CASCADE
);

COMMENT ON TABLE sect54_reductions IS
    '§54 EEG 2023 BNetzA Ausschreibungsreduzierungen. '
    'Each row = one BNetzA reduction notification for an Ausschreibungsanlage.';

CREATE INDEX sect54_tr_id ON sect54_reductions (tr_id, tenant, effective_from DESC);

-- ── Jahresabrechnung (§25 EEG 2023 / §14 UStG annual reconciliation) ──────────
--
-- The monthly settlements are the payment obligation; this is the annual
-- statement that reconciles them. It is derived from settlement_receipts rather
-- than recomputed, so it can never disagree with what was actually paid, and it
-- records which months are missing rather than quietly summing eleven.
CREATE TABLE jahresabrechnungen (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tr_id                   TEXT        NOT NULL,
    tenant                  TEXT        NOT NULL,
    billing_year            SMALLINT    NOT NULL,
    -- Sum over the year's receipts.
    einspeisemenge_kwh      NUMERIC(18, 3) NOT NULL DEFAULT 0,
    settlement_eur          NUMERIC(14, 2) NOT NULL DEFAULT 0,
    -- §52 EEG 2023 Pflichtzahlungen, kept apart from the Vergütung: they are a
    -- separate claim and are never netted into the settlement total.
    pflichtzahlung_eur      NUMERIC(14, 2) NOT NULL DEFAULT 0,
    -- Months with a receipt, and the ones without.
    months_settled          SMALLINT    NOT NULL DEFAULT 0,
    missing_months          SMALLINT[]  NOT NULL DEFAULT '{}',
    -- §51a: quarter-hours accrued across the year toward the Vergütungszeitraum.
    verlaengerungsanspruch_qh BIGINT    NOT NULL DEFAULT 0,
    -- Number of corrections issued in the year (§ 147 AO / GoBD audit signal).
    correction_count        SMALLINT    NOT NULL DEFAULT 0,
    status                  TEXT        NOT NULL DEFAULT 'vorlaeufig'
                            CHECK (status IN ('vorlaeufig', 'endgueltig')),
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT fk_ja_anlage FOREIGN KEY (tr_id, tenant)
        REFERENCES eeg_anlagen (tr_id, tenant) ON DELETE CASCADE
);

-- One statement per plant and year; re-running replaces it.
CREATE UNIQUE INDEX ja_unique ON jahresabrechnungen (tr_id, tenant, billing_year);
CREATE INDEX ja_year ON jahresabrechnungen (tenant, billing_year);

-- ── einsd schema — Einspeiser Registry + EEG/KWKG Settlement ─────────────────
--
-- Covers:
--   EEG 2023 (§§20–22): Einspeisevergütung, Marktprämie, Post-EEG Spot,
--   Mieterstrom (§38a), Ausschreibungsanlagen (§§22a, 28 ff.)
--   EEG §24: Zusammenlegung (merged plants, parent_tr_id)
--   EEG §22: Repowering (reset Förderendedatum)
--   EEG §50: Flexibilitätsprämie (biomass demand response)
--   KWKG 2023: KWK-Zuschlag for combined heat & power

CREATE TABLE IF NOT EXISTS eeg_anlagen (
    tr_id              TEXT        NOT NULL,
    tenant             TEXT        NOT NULL,
    malo_id            TEXT        NOT NULL,
    melo_id            TEXT,
    eeg_gesetz         SMALLINT    NOT NULL,   -- EEG year (2000, 2004, …, 2023) or 0 for KWKG
    inbetriebnahme     DATE        NOT NULL,
    leistung_kwp       NUMERIC(12,3) NOT NULL,
    erzeugungsart      TEXT        NOT NULL
                       CHECK (erzeugungsart IN (
                           -- Solar PV (§21 EEG 2023)
                           'SOLAR',            -- generic (backward compat)
                           'SOLAR_AUFDACH',    -- rooftop PV — higher rates
                           'SOLAR_FREFLAECHE', -- ground-mounted — lower rates, tendering >1MW
                           'SOLAR_AGRIPV',     -- §51a Agri-PV — premium +0.5 ct/kWh
                           'SOLAR_MIETERSTROM',-- §38a building community solar
                           'SOLAR_STECKER',    -- Balkonkraftwerk <800W (simplified registration)
                           -- Wind
                           'WIND_ONSHORE',     -- §21 EEG; tendering >750kW
                           'WIND_OFFSHORE',    -- §§70ff EEG; Offshore-Zuschlag via BNetzA
                           -- Biomass / Gas (§21 EEG 2023)
                           'BIOMASSE',         -- generic solid biomass
                           'BIOMASSE_HOLZ',    -- wood biomass (§42a EEG — restricted)
                           'BIOGAS',           -- fermentation biogas
                           'BIOMETHANE',       -- upgraded biogas (injected into grid)
                           'KLAEGAS',          -- sewage gas (§41 EEG)
                           'GRUBENGAS',        -- mine gas (§41 EEG)
                           'DEPONIEGAS',       -- landfill gas (§41 EEG)
                           -- Water / Geothermal
                           'WASSERKRAFT',      -- run-of-river hydro (§22 EEG 2023)
                           'GEOTHERMIE',       -- geothermal energy
                           'GEZEITEN',         -- tidal / wave energy
                           -- CHP (not EEG, separate KWKG)
                           'KWKG',             -- combined heat & power (Kraft-Wärme-Kopplung)
                           'SONSTIGE'          -- other / not classifiable
                       )),
    -- EEG feed-in tariff rate in ct/kWh. Fixed at `inbetriebnahme` for the full
    -- Förderungsdauer.  For Ausschreibungsanlagen: the tender-awarded value.
    -- For KWKG: the KWK-Zuschlag rate in ct/kWh.
    verguetungssatz_ct NUMERIC(8,4) NOT NULL,
    -- Förderendedatum: computed as inbetriebnahme + 20 years for EEG.
    -- For KWKG: inbetriebnahme + kwk_foerderdauer_years.
    -- For Repowering: repowering_date + 20 years.
    foerderendedatum   DATE        NOT NULL,
    -- ── Settlement model ─────────────────────────────────────────────────────
    settlement_model   TEXT        NOT NULL DEFAULT 'VERGUETUNG'
                       CHECK (settlement_model IN (
                           'VERGUETUNG',         -- §21 EEG: fixed tariff NB→Anlagenbetreiber
                           'DIREKTVERMARKTUNG',  -- §20 EEG: Gleitende Marktprämie (AW−EPEX)
                           'AUSSCHREIBUNG',       -- §§22a,28 EEG: BNetzA tender value
                           'POST_EEG_SPOT',      -- post-Förderung: Marktwert Strom spot reference
                           'EIGENVERBRAUCH',     -- self-consumption; no settlement payment
                           'MIETERSTROM',        -- §38a EEG: building community solar + Zuschlag
                           'KWKG_ZUSCHLAG',      -- KWKG: KWK-Zuschlag on market price
                           'FLEXIBILITAET'       -- §50 EEG: biomass Flexibilitätsprämie
                       )),
    -- Direktvermarktung / Ausschreibung: Anzulegender Wert in ct/kWh
    direktverm_aw_ct   NUMERIC(8,4),
    -- Optional: MP-ID of the Direktvermarkter (LF acting as market participant)
    direktverm_mp_id   TEXT,
    -- Mieterstrom §38a surcharge in ct/kWh (above base VERGUETUNG rate)
    mieter_zuschlag_ct NUMERIC(6,4),
    -- Ausschreibung: BNetzA Zuschlag-ID from tender result
    ausschreibungs_zuschlag_id TEXT,
    -- ── Repowering (§22 EEG 2023) ────────────────────────────────────────────
    -- When TRUE: foerderendedatum = repowering_date + 20 yr (clock reset).
    -- verguetungssatz_ct is the rate at repowering_date, not ursprungs_inbetriebnahme.
    ist_repowering     BOOLEAN     NOT NULL DEFAULT false,
    -- Original commissioning date before repowering (preserved for audit trail).
    ursprungs_inbetriebnahme DATE,
    -- Date of repowering (Inbetriebnahme of new/replacement components).
    repowering_datum   DATE,
    -- ── Zusammenlegung (§24 EEG 2023) ────────────────────────────────────────
    -- For merged plants: reference to the parent entity (the merged TR-ID).
    parent_tr_id       TEXT,
    -- ── KWKG (Kraft-Wärme-Kopplungsgesetz) ───────────────────────────────────
    -- CHP plants get KWK-Zuschlag (ct/kWh on top of market price), not EEG Tarif.
    -- Duration depends on plant size:
    --   <50 kW:   20 years (§8 KWKG)
    --   50kW–2MW: 10 years
    --   >2MW:     30,000 full-load hours (max 15 years)
    kwk_foerderdauer_h INTEGER,       -- Full-load hours (for >2MW: e.g. 30000)
    kwk_foerderdauer_years SMALLINT,  -- Years (for ≤2MW)
    kwk_strom_kwh_gesamt NUMERIC(14,3), -- Accumulated kWh for tracking hour limit
    -- ── Flexibilitätsprämie §50 EEG ─────────────────────────────────────────
    -- Biomass plants providing flexible demand response get additional premium.
    flex_leistung_kw   NUMERIC(8,3), -- Registered flex capacity in kW
    flex_praemie_ct_kwh NUMERIC(6,4), -- Flex premium rate ct/kWh (§50 EEG)
    -- ── Status + metadata ────────────────────────────────────────────────────
    direktvermarktung  BOOLEAN     NOT NULL DEFAULT false,
    status             TEXT        NOT NULL DEFAULT 'aktiv'
                       CHECK (status IN (
                           'aktiv',
                           'abgemeldet',         -- deregistered by operator
                           'foerderung_beendet', -- Förderungsdauer expired
                           'repowered'           -- plant repowered (historical record kept)
                       )),
    notes              TEXT,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tr_id, tenant)
);

CREATE INDEX IF NOT EXISTS ea_malo_tenant     ON eeg_anlagen (malo_id, tenant);
CREATE INDEX IF NOT EXISTS ea_foerderend      ON eeg_anlagen (foerderendedatum, tenant)
    WHERE status = 'aktiv';
CREATE INDEX IF NOT EXISTS ea_settlement      ON eeg_anlagen (settlement_model, tenant)
    WHERE status = 'aktiv';
CREATE INDEX IF NOT EXISTS ea_gesetz_tenant   ON eeg_anlagen (eeg_gesetz, tenant);
CREATE INDEX IF NOT EXISTS ea_repowering      ON eeg_anlagen (tenant)
    WHERE ist_repowering = true;
CREATE INDEX IF NOT EXISTS ea_zusammenlegung  ON eeg_anlagen (parent_tr_id, tenant)
    WHERE parent_tr_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS ea_kwkg            ON eeg_anlagen (tenant)
    WHERE settlement_model = 'KWKG_ZUSCHLAG';

-- ── Monthly settlement receipts ───────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS settlement_receipts (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tr_id              TEXT        NOT NULL,
    tenant             TEXT        NOT NULL,
    billing_year       SMALLINT    NOT NULL,
    billing_month      SMALLINT    NOT NULL CHECK (billing_month BETWEEN 1 AND 12),
    settlement_model   TEXT        NOT NULL,
    einspeisemenge_kwh NUMERIC(14,3),
    settlement_eur     NUMERIC(14,5),
    -- For KWKG_ZUSCHLAG: tracks cumulative kWh toward the hour/year limit
    kwk_strom_kwh_this_period NUMERIC(14,3),
    rechnung_json      JSONB,
    bo4e_version       TEXT        NOT NULL DEFAULT 'v202607.0.0',
    -- 'calculated' | 'price_missing' | 'no_data' | 'error' | 'foerderung_beendet'
    status             TEXT        NOT NULL DEFAULT 'calculated',
    ce_id              UUID,
    error_detail       TEXT,
    settled_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tr_id, tenant, billing_year, billing_month)
);

CREATE INDEX IF NOT EXISTS sr_tr_period   ON settlement_receipts (tr_id, tenant, billing_year DESC, billing_month DESC);
CREATE INDEX IF NOT EXISTS sr_ce_pending  ON settlement_receipts (tenant, settled_at DESC)
    WHERE ce_id IS NULL AND status = 'calculated';

-- ── EPEX monthly reference prices ────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS epex_monthly_prices (
    billing_year       SMALLINT    NOT NULL,
    billing_month      SMALLINT    NOT NULL CHECK (billing_month BETWEEN 1 AND 12),
    avg_ct_kwh         NUMERIC(8,4) NOT NULL,
    source             TEXT        NOT NULL DEFAULT 'manual',
    imported_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (billing_year, billing_month)
);
--
-- `eeg_anlagen`:
--   Central plant register. `foerderendedatum` is derived automatically from
--   `inbetriebnahme + 20 years` (§20 EEG 2023) and drives the 180-day alert.
--
-- `settlement_receipts`:
--   Immutable audit log of every monthly settlement calculation. Stores the
--   full BO4E `Rechnung` JSONB for downstream ERP import and §22 MessZV
--   compliance.
--
-- `epex_monthly_prices`:
--   Optional: operator-supplied or fetched from netztransparenz.de / tarifbd.
--   Used by DIREKTVERMARKTUNG (Marktprämie) and POST_EEG_SPOT settlement models.
--   When absent, the affected settlement models emit a CloudEvent with `status =
--   "price_missing"` instead of calculating.

-- ── EEG Anlage register ───────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS eeg_anlagen (
    tr_id              TEXT        NOT NULL,
    tenant             TEXT        NOT NULL,
    malo_id            TEXT        NOT NULL,
    melo_id            TEXT,
    eeg_gesetz         SMALLINT    NOT NULL,   -- EEG year (2000, 2004, 2009, …, 2023)
    inbetriebnahme     DATE        NOT NULL,
    leistung_kwp       NUMERIC(12,3) NOT NULL,
    erzeugungsart      TEXT        NOT NULL
                       CHECK (erzeugungsart IN (
                           'SOLAR','WIND_ONSHORE','WIND_OFFSHORE','BIOMASSE',
                           'WASSER','GEOTHERMIE','GEZEITEN','GRUBENGAS','KLAEGAS','SONSTIGE'
                       )),
    -- EEG feed-in tariff fixed at Inbetriebnahme for the full 20-year period.
    -- Immutable after initial registration. See §20 EEG 2023 Vergütungssätze table.
    verguetungssatz_ct NUMERIC(8,4) NOT NULL,  -- ct/kWh
    -- foerderendedatum: computed as inbetriebnahme + 20 years.
    -- Stored as a real column (not generated) for SQLx compatibility.
    foerderendedatum   DATE        NOT NULL,
    direktvermarktung  BOOLEAN     NOT NULL DEFAULT false,
    direktverm_aw_ct   NUMERIC(8,4),            -- Anzulegender Wert ct/kWh (Modul I)
    settlement_model   TEXT        NOT NULL DEFAULT 'VERGUETUNG'
                       CHECK (settlement_model IN (
                           'VERGUETUNG',         -- §21 EEG: fixed tariff NB→Anlagenbetreiber
                           'DIREKTVERMARKTUNG',  -- §20 EEG: Marktprämie NB→ÜNB
                           'POST_EEG_SPOT',      -- post-20yr: Marktwert Strom reference
                           'EIGENVERBRAUCH'      -- self-consumption; no settlement
                       )),
    -- Optional: link to the NB or LF acting as Direktvermarkter
    direktverm_mp_id   TEXT,
    -- Mieterstrom surcharge (§38a EEG): additional ct/kWh above base tariff
    mieter_zuschlag_ct NUMERIC(6,4),
    status             TEXT        NOT NULL DEFAULT 'aktiv'
                       CHECK (status IN ('aktiv','abgemeldet','foerderung_beendet')),
    notes              TEXT,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tr_id, tenant)
);

CREATE INDEX IF NOT EXISTS ea_malo_tenant     ON eeg_anlagen (malo_id, tenant);
CREATE INDEX IF NOT EXISTS ea_foerderend      ON eeg_anlagen (foerderendedatum, tenant)
    WHERE status = 'aktiv';
CREATE INDEX IF NOT EXISTS ea_settlement      ON eeg_anlagen (settlement_model, tenant)
    WHERE status = 'aktiv';
CREATE INDEX IF NOT EXISTS ea_gesetz_tenant   ON eeg_anlagen (eeg_gesetz, tenant);

-- ── Monthly settlement receipts ───────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS settlement_receipts (
    id                 UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tr_id              TEXT        NOT NULL,
    tenant             TEXT        NOT NULL,
    billing_year       SMALLINT    NOT NULL,
    billing_month      SMALLINT    NOT NULL CHECK (billing_month BETWEEN 1 AND 12),
    settlement_model   TEXT        NOT NULL,
    -- kWh actually fed in during the billing month (from edmd or manual entry)
    einspeisemenge_kwh NUMERIC(14,3),
    -- Calculated settlement amount in EUR (positive = payment to Anlagenbetreiber)
    settlement_eur     NUMERIC(14,5),
    -- Full BO4E Rechnung JSONB (null when settlement_model = 'EIGENVERBRAUCH' or
    -- when price_missing = true)
    rechnung_json      JSONB,
    bo4e_version       TEXT        NOT NULL DEFAULT 'v202607.0.0',
    -- 'calculated' | 'price_missing' | 'no_data' | 'error'
    status             TEXT        NOT NULL DEFAULT 'calculated',
    -- CloudEvent ID of the emitted de.eeg.verguetung.berechnet or
    -- de.eeg.marktpraemie.berechnet
    ce_id              UUID,
    error_detail       TEXT,
    settled_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tr_id, tenant, billing_year, billing_month)
);

CREATE INDEX IF NOT EXISTS sr_tr_period   ON settlement_receipts (tr_id, tenant, billing_year DESC, billing_month DESC);
CREATE INDEX IF NOT EXISTS sr_ce_pending  ON settlement_receipts (tenant, settled_at DESC)
    WHERE ce_id IS NULL AND status = 'calculated';

-- ── EPEX monthly reference prices ────────────────────────────────────────────
-- Required for DIREKTVERMARKTUNG (Marktprämie) and POST_EEG_SPOT settlement.
-- Import via PUT /api/v1/epex-monthly/{year}/{month} or sync from tarifbd.

CREATE TABLE IF NOT EXISTS epex_monthly_prices (
    billing_year       SMALLINT    NOT NULL,
    billing_month      SMALLINT    NOT NULL CHECK (billing_month BETWEEN 1 AND 12),
    -- Monthly average EPEX Spot day-ahead in ct/kWh
    avg_ct_kwh         NUMERIC(8,4) NOT NULL,
    source             TEXT        NOT NULL DEFAULT 'manual',
    imported_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (billing_year, billing_month)
);

-- ── einsd migration 0002 — EEG Vergütungssätze reference data ────────────────
--
-- Reference table for EEG/KWKG feed-in tariff / premium rates.
-- Indexed by: erzeugungsart, leistung range (kWp), billing_start / billing_end.
--
-- Sources:
--   Solar PV:     §21 EEG 2023, BNetzA quarterly announcements
--   Wind onshore: §21 EEG 2023 §46–46a (Referenzstandort)
--   Wind offshore: §§70ff EEG 2023
--   Biomasse:     §42–43 EEG 2023
--   Wasserkraft:  §40 EEG 2023
--   Gas (Klär/Gruben/Deponiegas): §41 EEG 2023
--   KWKG:         §7 KWKG 2023
--
-- IMPORTANT: Rates below are indicative/representative.
-- Operators MUST verify against current BNetzA publications before production use.
-- Import updated rates via `PUT /api/v1/verguetungssatz-lookup` operator endpoint
-- or the xtask import task.

CREATE TABLE IF NOT EXISTS eeg_verguetungssaetze (
    id               SERIAL PRIMARY KEY,
    erzeugungsart    TEXT        NOT NULL DEFAULT 'SOLAR',
    leistung_min_kwp NUMERIC(10,3) NOT NULL,
    leistung_max_kwp NUMERIC(10,3),               -- NULL = no upper bound
    billing_start    DATE        NOT NULL,
    billing_end      DATE,                         -- NULL = currently valid
    verguetungssatz_ct NUMERIC(8,4) NOT NULL,      -- ct/kWh
    eeg_gesetz       SMALLINT    NOT NULL,          -- 2000|2004|2009|2012|2017|2021|2023|0=KWKG
    notes            TEXT,
    UNIQUE (erzeugungsart, leistung_min_kwp, billing_start)
);

CREATE INDEX IF NOT EXISTS evs_lookup
    ON eeg_verguetungssaetze (erzeugungsart, billing_start, billing_end);

-- ── SOLAR / SOLAR_AUFDACH (roof-mounted PV) — EEG 2023 quarterly ─────────────

INSERT INTO eeg_verguetungssaetze
    (erzeugungsart, leistung_min_kwp, leistung_max_kwp, billing_start, billing_end, verguetungssatz_ct, eeg_gesetz, notes)
VALUES
('SOLAR',   0,    10,    '2023-02-01', '2023-04-30',  8.20, 2023, 'EEG 2023 Q1 ≤10 kWp Volleinspeiser'),
('SOLAR',  10,    40,    '2023-02-01', '2023-04-30',  7.10, 2023, 'EEG 2023 Q1 10–40 kWp'),
('SOLAR',  40,   100,    '2023-02-01', '2023-04-30',  5.80, 2023, 'EEG 2023 Q1 40–100 kWp'),
('SOLAR', 100,  NULL,    '2023-02-01', '2023-04-30',  4.60, 2023, 'EEG 2023 Q1 >100 kWp Freifläche'),
('SOLAR',   0,    10,    '2023-05-01', '2023-07-31',  8.11, 2023, 'EEG 2023 Q2 ≤10 kWp'),
('SOLAR',  10,    40,    '2023-05-01', '2023-07-31',  7.03, 2023, 'EEG 2023 Q2 10–40 kWp'),
('SOLAR',  40,   100,    '2023-05-01', '2023-07-31',  5.74, 2023, 'EEG 2023 Q2 40–100 kWp'),
('SOLAR', 100,  NULL,    '2023-05-01', '2023-07-31',  4.55, 2023, 'EEG 2023 Q2 >100 kWp'),
('SOLAR',   0,    10,    '2023-08-01', '2023-10-31',  8.03, 2023, 'EEG 2023 Q3 ≤10 kWp'),
('SOLAR',  10,    40,    '2023-08-01', '2023-10-31',  6.95, 2023, 'EEG 2023 Q3 10–40 kWp'),
('SOLAR',  40,   100,    '2023-08-01', '2023-10-31',  5.68, 2023, 'EEG 2023 Q3 40–100 kWp'),
('SOLAR', 100,  NULL,    '2023-08-01', '2023-10-31',  4.50, 2023, 'EEG 2023 Q3 >100 kWp'),
('SOLAR',   0,    10,    '2023-11-01', '2024-01-31',  7.95, 2023, 'EEG 2023 Q4 ≤10 kWp'),
('SOLAR',  10,    40,    '2023-11-01', '2024-01-31',  6.88, 2023, 'EEG 2023 Q4 10–40 kWp'),
('SOLAR',  40,   100,    '2023-11-01', '2024-01-31',  5.62, 2023, 'EEG 2023 Q4 40–100 kWp'),
('SOLAR', 100,  NULL,    '2023-11-01', '2024-01-31',  4.45, 2023, 'EEG 2023 Q4 >100 kWp'),
('SOLAR',   0,    10,    '2024-02-01', '2024-04-30',  7.83, 2023, 'EEG 2023 Q1/2024 ≤10 kWp'),
('SOLAR',  10,    40,    '2024-02-01', '2024-04-30',  6.79, 2023, 'EEG 2023 Q1/2024 10–40 kWp'),
('SOLAR',  40,   100,    '2024-02-01', '2024-04-30',  5.56, 2023, 'EEG 2023 Q1/2024 40–100 kWp'),
('SOLAR', 100,  NULL,    '2024-02-01', '2024-04-30',  4.40, 2023, 'EEG 2023 Q1/2024 >100 kWp'),
-- SOLAR_AUFDACH mirrors SOLAR rates (identical rates for Aufdach)
('SOLAR_AUFDACH',   0,    10,    '2024-02-01', NULL,  7.83, 2023, 'EEG 2023 Aufdach ≤10 kWp approx'),
('SOLAR_AUFDACH',  10,    40,    '2024-02-01', NULL,  6.79, 2023, 'EEG 2023 Aufdach 10–40 kWp approx'),
('SOLAR_AUFDACH',  40,   100,    '2024-02-01', NULL,  5.56, 2023, 'EEG 2023 Aufdach 40–100 kWp approx'),
-- SOLAR_FREFLAECHE: lower rates (only plants ≤1MW; >1MW via Ausschreibung)
('SOLAR_FREFLAECHE', 0,  1000,   '2024-02-01', NULL,  5.20, 2023, 'EEG 2023 Freifläche ≤1 MWp approx'),
-- SOLAR_AGRIPV: §51a EEG 2023 — 0.5 ct/kWh premium over standard rates
('SOLAR_AGRIPV',    0,   100,    '2023-02-01', NULL,  9.20, 2023, 'EEG 2023 Agri-PV standard + §51a bonus approx'),
-- EEG 2021 historical
('SOLAR',   0,    10,    '2021-01-01', '2021-12-31',  9.58, 2021, 'EEG 2021 ≤10 kWp'),
('SOLAR',  10,    40,    '2021-01-01', '2021-12-31',  9.33, 2021, 'EEG 2021 10–40 kWp'),
('SOLAR',  40,   750,    '2021-01-01', '2021-12-31',  7.05, 2021, 'EEG 2021 40–750 kWp'),
-- EEG 2017
('SOLAR',   0,    10,    '2017-04-01', '2020-12-31',  9.87, 2017, 'EEG 2017 ≤10 kWp avg'),
('SOLAR',  10,    40,    '2017-04-01', '2020-12-31',  9.61, 2017, 'EEG 2017 10–40 kWp avg'),
-- EEG 2012
('SOLAR',   0,    10,    '2012-04-01', '2017-03-31', 17.42, 2012, 'EEG 2012 ≤10 kWp avg'),
('SOLAR',  10,    40,    '2012-04-01', '2017-03-31', 16.50, 2012, 'EEG 2012 10–40 kWp avg'),
-- EEG 2009
('SOLAR',   0,   100,    '2009-01-01', '2012-03-31', 29.37, 2009, 'EEG 2009 ≤100 kWp avg'),
-- EEG 2004
('SOLAR',   0,   100,    '2004-08-01', '2008-12-31', 46.75, 2004, 'EEG 2004 ≤100 kWp avg'),
-- EEG 2000
('SOLAR',   0,   100,    '2000-04-01', '2004-07-31', 50.62, 2000, 'EEG 2000 ≤100 kWp'),

-- ── WIND_ONSHORE (§21 EEG 2023, §46 Referenzstandort) ────────────────────────
-- Rates for plants ≤750 kW (larger plants via Ausschreibung §28 EEG).
-- EEG 2023 base rate: ~7.35 ct/kWh + optional Systemdienstleistungsbonus.
-- Historical rates for Bestandsanlagen.
('WIND_ONSHORE',  0,   750,    '2023-01-01', NULL,   7.35, 2023, 'EEG 2023 Onshore ≤750 kW §21 Einspeisevergütung'),
('WIND_ONSHORE',  0,   750,    '2021-01-01', '2022-12-31', 5.87, 2021, 'EEG 2021 Onshore ≤750 kW'),
('WIND_ONSHORE',  0,  NULL,    '2017-01-01', '2020-12-31', 8.26, 2017, 'EEG 2017 Onshore avg (Referenzstandort 100%)'),
('WIND_ONSHORE',  0,  NULL,    '2012-01-01', '2016-12-31', 8.93, 2012, 'EEG 2012 Onshore avg'),
('WIND_ONSHORE',  0,  NULL,    '2009-01-01', '2011-12-31', 9.20, 2009, 'EEG 2009 Onshore avg'),
('WIND_ONSHORE',  0,  NULL,    '2004-08-01', '2008-12-31', 8.53, 2004, 'EEG 2004 Onshore avg'),
('WIND_ONSHORE',  0,  NULL,    '2000-04-01', '2004-07-31', 9.10, 2000, 'EEG 2000 Onshore avg'),

-- ── WIND_OFFSHORE (§§70ff EEG 2023) ──────────────────────────────────────────
-- Offshore plants exclusively via Ausschreibung for new plants (§28b EEG 2023).
-- Historical fixed rates for Bestandsanlagen.
('WIND_OFFSHORE', 0,  NULL,    '2023-01-01', NULL,   6.89, 2023, 'EEG 2023 Offshore Bestandsanlagen (pre-Ausschreibung)'),
('WIND_OFFSHORE', 0,  NULL,    '2017-01-01', '2022-12-31', 3.90, 2017, 'EEG 2017 Offshore avg (Ausschreibungswert)'),
('WIND_OFFSHORE', 0,  NULL,    '2012-01-01', '2016-12-31', 15.00, 2012, 'EEG 2012 Offshore (Anfangsvergütung 15 ct/kWh)'),

-- ── BIOMASSE (§42–43 EEG 2023) ────────────────────────────────────────────────
-- Rates vary significantly by size and feedstock category.
-- ≤150 kW (small plants): higher rates; >150 kW: tendering.
('BIOMASSE',      0,   150,    '2023-01-01', NULL,  16.40, 2023, 'EEG 2023 Biomasse ≤150 kW §21'),
('BIOMASSE',    150,   500,    '2023-01-01', NULL,  14.30, 2023, 'EEG 2023 Biomasse 150–500 kW §21'),
('BIOMASSE',      0,   500,    '2021-01-01', '2022-12-31', 13.07, 2021, 'EEG 2021 Biomasse ≤500 kW avg'),
('BIOMASSE',      0,   500,    '2017-01-01', '2020-12-31', 13.05, 2017, 'EEG 2017 Biomasse ≤500 kW avg'),
('BIOMASSE',      0,   500,    '2012-01-01', '2016-12-31', 12.30, 2012, 'EEG 2012 Biomasse ≤500 kW avg'),
('BIOMASSE',      0,   500,    '2009-01-01', '2011-12-31', 11.67, 2009, 'EEG 2009 Biomasse avg'),
-- Biogas (fermentation)
('BIOGAS',        0,   150,    '2023-01-01', NULL,  18.00, 2023, 'EEG 2023 Biogas ≤150 kW approx §21'),
('BIOGAS',      150,   500,    '2023-01-01', NULL,  15.50, 2023, 'EEG 2023 Biogas 150–500 kW approx'),

-- ── KLÄRGAS / GRUBENGAS / DEPONIEGAS (§41 EEG 2023) ─────────────────────────
-- Gas from sewage treatment, mines, and landfills — flat rates independent of size.
('KLAEGAS',       0,  NULL,    '2023-01-01', NULL,  12.50, 2023, 'EEG 2023 Klärgas §41 fester Tarif'),
('KLAEGAS',       0,  NULL,    '2017-01-01', '2022-12-31', 12.37, 2017, 'EEG 2017 Klärgas avg'),
('GRUBENGAS',     0,  NULL,    '2023-01-01', NULL,  12.50, 2023, 'EEG 2023 Grubengas §41 fester Tarif'),
('GRUBENGAS',     0,  NULL,    '2017-01-01', '2022-12-31', 10.29, 2017, 'EEG 2017 Grubengas avg'),
('DEPONIEGAS',    0,  NULL,    '2023-01-01', NULL,   9.10, 2023, 'EEG 2023 Deponiegas §41 fester Tarif'),
('DEPONIEGAS',    0,  NULL,    '2017-01-01', '2022-12-31', 8.00, 2017, 'EEG 2017 Deponiegas avg'),

-- ── WASSERKRAFT (§40 EEG 2023) ────────────────────────────────────────────────
-- Run-of-river hydro; rates decrease with plant size.
('WASSERKRAFT',   0,   500,    '2023-01-01', NULL,  12.48, 2023, 'EEG 2023 Wasser ≤500 kW §21'),
('WASSERKRAFT', 500,  5000,    '2023-01-01', NULL,   8.59, 2023, 'EEG 2023 Wasser 500 kW–5 MW'),
('WASSERKRAFT', 5000, NULL,    '2023-01-01', NULL,   7.56, 2023, 'EEG 2023 Wasser >5 MW'),
('WASSERKRAFT',   0,   500,    '2017-01-01', '2022-12-31', 12.78, 2017, 'EEG 2017 Wasser ≤500 kW avg'),

-- ── KWKG — KWK-Zuschlag (§7 KWKG 2023) ───────────────────────────────────────
-- CHP plants receive the KWK-Zuschlag (ct/kWh on top of market price).
-- The rate depends on plant size and applies for the KWKG Förderdauer.
-- Source: §7 Abs.1 KWKG 2023 (01.01.2023 publication).
('KWKG',     0,    50,    '2023-01-01', NULL,   8.00, 0, 'KWKG 2023 ≤50 kW KWK-Zuschlag §7(1)1'),
('KWKG',    50,   100,    '2023-01-01', NULL,   6.00, 0, 'KWKG 2023 50–100 kW KWK-Zuschlag §7(1)2'),
('KWKG',   100,   250,    '2023-01-01', NULL,   5.00, 0, 'KWKG 2023 100–250 kW KWK-Zuschlag §7(1)3'),
('KWKG',   250,  2000,    '2023-01-01', NULL,   4.40, 0, 'KWKG 2023 250 kW–2 MW KWK-Zuschlag §7(1)4'),
('KWKG',  2000,  NULL,    '2023-01-01', NULL,   3.10, 0, 'KWKG 2023 >2 MW KWK-Zuschlag §7(1)5'),
-- Historical KWKG rates for Bestandsanlagen
('KWKG',     0,    50,    '2017-01-01', '2022-12-31',  8.00, 0, 'KWKG 2017–2022 ≤50 kW avg'),
('KWKG',    50,   250,    '2017-01-01', '2022-12-31',  6.00, 0, 'KWKG 2017–2022 50–250 kW avg'),

-- ── GEOTHERMIE (§45 EEG 2023) ────────────────────────────────────────────────
('GEOTHERMIE',    0,   100,    '2023-01-01', NULL,  25.20, 2023, 'EEG 2023 Geothermie ≤100 kW §21'),
('GEOTHERMIE',  100,  NULL,    '2023-01-01', NULL,  25.20, 2023, 'EEG 2023 Geothermie >100 kW §21'),
-- ── GEZEITEN / Tidal (§46 EEG 2023) ─────────────────────────────────────────
('GEZEITEN',      0,  NULL,    '2023-01-01', NULL,  34.10, 2023, 'EEG 2023 Gezeiten §21 (nascent technology)');

--
-- Reference table for EEG feed-in tariff rates for roof-mounted photovoltaic
-- systems (Anlagenkategorie 'SOLAR'). Rates are indexed by:
--   billing_start   — first day of validity (inclusive, quarterly adjustment)
--   billing_end     — last day of validity  (inclusive; NULL = still valid)
--   leistung_min_kwp — lower power bound (kWp), inclusive
--   leistung_max_kwp — upper power bound (kWp), exclusive (NULL = unbounded)
--
-- Source: §21 EEG 2023, BNetzA quarterly adjustment announcements.
-- Historical rates: EEG 2000, 2004, 2009, 2012, 2017, 2021.
--
-- EIGENVERBRAUCH (Teileinspeisung): the tariff for self-consumption with
-- partial feed-in (Überschusseinspeisung) is tracked in the
-- `mieter_zuschlag_ct` field of `eeg_anlagen`, not here.
--
-- This table covers Volleinspeiser (full feed-in) and standard partial feed-in
-- tariffs (§21(1) EEG 2023). Wind, Biomasse, Wasser: use manual entry or
-- future extension of this table for other Erzeugungsarten.

CREATE TABLE IF NOT EXISTS eeg_verguetungssaetze (
    id               SERIAL PRIMARY KEY,
    erzeugungsart    TEXT        NOT NULL DEFAULT 'SOLAR',
    leistung_min_kwp NUMERIC(10,3) NOT NULL,
    leistung_max_kwp NUMERIC(10,3),               -- NULL = no upper bound
    billing_start    DATE        NOT NULL,
    billing_end      DATE,                         -- NULL = currently valid
    verguetungssatz_ct NUMERIC(8,4) NOT NULL,      -- ct/kWh
    eeg_gesetz       SMALLINT    NOT NULL,          -- 2000|2004|2009|2012|2017|2021|2023
    notes            TEXT,
    UNIQUE (erzeugungsart, leistung_min_kwp, billing_start)
);

CREATE INDEX IF NOT EXISTS evs_lookup
    ON eeg_verguetungssaetze (erzeugungsart, billing_start, billing_end);

-- ── Seed data: Solar PV Aufdach — EEG 2023 quarterly rates ───────────────────
-- Source: BNetzA / BMWK quarterly announcements under §23 EEG 2023.
-- Rate segments (kWp): ≤10, 10–40, 40–100, >100 (Volleinspeiser).
-- Rates below are illustrative of the EEG 2023 structure and represent
-- the initial values from 2023-02-01 publication.
-- Operators MUST update with current BNetzA values before production use.

INSERT INTO eeg_verguetungssaetze
    (erzeugungsart, leistung_min_kwp, leistung_max_kwp, billing_start, billing_end, verguetungssatz_ct, eeg_gesetz, notes)
VALUES
-- ── EEG 2023 (§21 EEG 2023) — Solar Aufdach ─────────────────────────────────
-- 2023-02-01 initial publication
('SOLAR',   0,    10,    '2023-02-01', '2023-04-30',  8.20, 2023, 'EEG 2023 Q1 ≤10 kWp Volleinspeiser'),
('SOLAR',  10,    40,    '2023-02-01', '2023-04-30',  7.10, 2023, 'EEG 2023 Q1 10–40 kWp'),
('SOLAR',  40,   100,    '2023-02-01', '2023-04-30',  5.80, 2023, 'EEG 2023 Q1 40–100 kWp'),
('SOLAR', 100,  NULL,    '2023-02-01', '2023-04-30',  4.60, 2023, 'EEG 2023 Q1 >100 kWp Freifläche'),
-- Q2 2023
('SOLAR',   0,    10,    '2023-05-01', '2023-07-31',  8.11, 2023, 'EEG 2023 Q2 ≤10 kWp'),
('SOLAR',  10,    40,    '2023-05-01', '2023-07-31',  7.03, 2023, 'EEG 2023 Q2 10–40 kWp'),
('SOLAR',  40,   100,    '2023-05-01', '2023-07-31',  5.74, 2023, 'EEG 2023 Q2 40–100 kWp'),
('SOLAR', 100,  NULL,    '2023-05-01', '2023-07-31',  4.55, 2023, 'EEG 2023 Q2 >100 kWp'),
-- Q3 2023
('SOLAR',   0,    10,    '2023-08-01', '2023-10-31',  8.03, 2023, 'EEG 2023 Q3 ≤10 kWp'),
('SOLAR',  10,    40,    '2023-08-01', '2023-10-31',  6.95, 2023, 'EEG 2023 Q3 10–40 kWp'),
('SOLAR',  40,   100,    '2023-08-01', '2023-10-31',  5.68, 2023, 'EEG 2023 Q3 40–100 kWp'),
('SOLAR', 100,  NULL,    '2023-08-01', '2023-10-31',  4.50, 2023, 'EEG 2023 Q3 >100 kWp'),
-- Q4 2023
('SOLAR',   0,    10,    '2023-11-01', '2024-01-31',  7.95, 2023, 'EEG 2023 Q4 ≤10 kWp'),
('SOLAR',  10,    40,    '2023-11-01', '2024-01-31',  6.88, 2023, 'EEG 2023 Q4 10–40 kWp'),
('SOLAR',  40,   100,    '2023-11-01', '2024-01-31',  5.62, 2023, 'EEG 2023 Q4 40–100 kWp'),
('SOLAR', 100,  NULL,    '2023-11-01', '2024-01-31',  4.45, 2023, 'EEG 2023 Q4 >100 kWp'),
-- Q1 2024
('SOLAR',   0,    10,    '2024-02-01', '2024-04-30',  7.83, 2023, 'EEG 2023 Q1/2024 ≤10 kWp'),
('SOLAR',  10,    40,    '2024-02-01', '2024-04-30',  6.79, 2023, 'EEG 2023 Q1/2024 10–40 kWp'),
('SOLAR',  40,   100,    '2024-02-01', '2024-04-30',  5.56, 2023, 'EEG 2023 Q1/2024 40–100 kWp'),
('SOLAR', 100,  NULL,    '2024-02-01', '2024-04-30',  4.40, 2023, 'EEG 2023 Q1/2024 >100 kWp'),
-- Q2–Q4 2024 and 2025 rates: add via PUT /api/v1/verguetungssaetze or xtask import
-- See BNetzA EEG-Einspeisevergütung quarterly announcements.

-- ── EEG 2021 (§20 EEG 2021) — Solar Aufdach ─────────────────────────────────
-- Representative rates for plants commissioned in 2021 (immutable for 20 years).
('SOLAR',   0,    10,    '2021-01-01', '2021-12-31',  9.58, 2021, 'EEG 2021 ≤10 kWp'),
('SOLAR',  10,    40,    '2021-01-01', '2021-12-31',  9.33, 2021, 'EEG 2021 10–40 kWp'),
('SOLAR',  40,   750,    '2021-01-01', '2021-12-31',  7.05, 2021, 'EEG 2021 40–750 kWp'),
-- ── EEG 2017 ─────────────────────────────────────────────────────────────────
('SOLAR',   0,    10,    '2017-04-01', '2020-12-31',  9.87, 2017, 'EEG 2017 ≤10 kWp avg'),
('SOLAR',  10,    40,    '2017-04-01', '2020-12-31',  9.61, 2017, 'EEG 2017 10–40 kWp avg'),
-- ── EEG 2012 ─────────────────────────────────────────────────────────────────
('SOLAR',   0,    10,    '2012-04-01', '2017-03-31', 17.42, 2012, 'EEG 2012 ≤10 kWp avg'),
('SOLAR',  10,    40,    '2012-04-01', '2017-03-31', 16.50, 2012, 'EEG 2012 10–40 kWp avg'),
-- ── EEG 2009 ─────────────────────────────────────────────────────────────────
('SOLAR',   0,   100,    '2009-01-01', '2012-03-31', 29.37, 2009, 'EEG 2009 ≤100 kWp avg'),
-- ── EEG 2004 ─────────────────────────────────────────────────────────────────
('SOLAR',   0,   100,    '2004-08-01', '2008-12-31', 46.75, 2004, 'EEG 2004 ≤100 kWp avg'),
-- ── EEG 2000 ─────────────────────────────────────────────────────────────────
('SOLAR',   0,   100,    '2000-04-01', '2004-07-31', 50.62, 2000, 'EEG 2000 ≤100 kWp');

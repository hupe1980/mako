-- ── einsd migration 0003 — Anlagenmerkmale (plant attributes) ────────────────
--
-- Background:
--   The eeg-billing crate now has rich domain models for:
--   - InbetriebnahmeTyp (§22 EEG — repowering, extension, Zusammenlegung)
--   - Wind §36k Gütegrad and Standortklasse
--   - Solar §48 Bauform (Aufdach, Freifläche, Agri-PV, Mieterstrom, Stecker)
--   - §9 EEG 2023 Fernsteuerbarkeit installation date
--   - §20 EEG mandatory Direktvermarktung flag
--   - §24 Anlagenerweiterung capacity blocks (JSONB)
--   - Direktvermarktungsperioden history (JSONB)
--
--   These columns enable:
--   1. Correct Ausschreibungspflicht validation (§22 EEG) in processd
--   2. Automatic Fernsteuerbarkeit compliance tracking (§52 Abs. 1 Nr. 3)
--   3. Multi-block §24 settlement without caller-side DB joins
--   4. Full audit trail for Direktvermarktung switches (§21 Abs. 3)
--
-- Dependencies:
--   0001_initial.sql (eeg_anlagen base table)
--   0002_mastr_bankverbindung.sql (mastr/status/bank columns)

-- ── 1. Inbetriebnahme type ────────────────────────────────────────────────────
-- Tracks whether the commissioning was an initial start, repowering, etc.
-- Affects Förderdauer reset rules (only Repowering resets the 20-year clock).

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS inbetriebnahme_typ TEXT
        CHECK (inbetriebnahme_typ IN (
            'ERSTINBETRIEBNAHME',   -- First commissioning (default)
            'WIEDERINBETRIEBNAHME', -- Re-commissioning after shutdown (§24 EEG)
            'MODERNISIERUNG',       -- Technical upgrade without full repowering
            'REPOWERING',           -- Full repowering (§22 EEG — Förderdauer reset)
            'ZUSAMMENLEGUNG',       -- Merger of two separate plants (§24 EEG)
            'ERWEITERUNG'           -- Capacity extension (§24 EEG — new block)
        ));

COMMENT ON COLUMN eeg_anlagen.inbetriebnahme_typ IS
    'How the plant was commissioned. Affects Förderdauer reset rules: '
    'only REPOWERING resets the 20-year clock per §22 EEG 2023. '
    'NULL = legacy record, treat as ERSTINBETRIEBNAHME.';

-- ── 2. Wind §36k site quality ─────────────────────────────────────────────────

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS wind_guetegrad NUMERIC(5, 3),
    ADD COLUMN IF NOT EXISTS wind_korrekturfaktor NUMERIC(6, 5),
    ADD COLUMN IF NOT EXISTS wind_standortklasse TEXT
        CHECK (wind_standortklasse IN (
            'EXCELLENT',        -- Gütegrad ≥ 150 %
            'ABOVE_REFERENCE',  -- 110 % ≤ Gütegrad < 150 %
            'REFERENCE',        -- 90 % ≤ Gütegrad < 110 %
            'BELOW_REFERENCE',  -- 80 % ≤ Gütegrad < 90 % (Grundvergütungsperiode)
            'MARGINAL'          -- Gütegrad < 80 % (not EEG-eligible)
        ));

COMMENT ON COLUMN eeg_anlagen.wind_guetegrad IS
    'Wind plant Gütegrad: ratio of actual site yield to reference yield (§36k EEG 2023). '
    'E.g. 0.850 = 85 % of reference yield. Certified by BNetzA-accredited Windgutachter. '
    'Only applies to WIND_ONSHORE plants commissioned from 2017-01-01 (EEG 2017+). '
    'NULL for solar, biomasse, and pre-EEG2017 wind plants (Bestandsschutz §36k Abs. 4).';

COMMENT ON COLUMN eeg_anlagen.wind_korrekturfaktor IS
    'Pre-certified §36k Korrekturfaktor from BNetzA table. '
    'Effective AW = base_aw × korrekturfaktor. Typical range: 0.70 – 1.30. '
    'Derived from wind_guetegrad by the Windgutachter. '
    'When both this column and wind_guetegrad are set, this value is authoritative.';

COMMENT ON COLUMN eeg_anlagen.wind_standortklasse IS
    'Wind site quality classification (§36k EEG 2023). '
    'Derived from wind_guetegrad. Stored redundantly for query filtering.';

-- ── 3. Solar bauform ──────────────────────────────────────────────────────────

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS solar_bauform TEXT
        CHECK (solar_bauform IN (
            'GEBAEUDE',         -- Gebäudeanlage (rooftop, §48 EEG 2023)
            'LAERMSCHUTZWAND',  -- Noise barrier PV
            'FREIFLAECHE',      -- Ground-mounted (Freifläche, §49 EEG 2023)
            'AGRI_PV',          -- Agri-PV (§51a EEG 2023 Verlängerungsanspruch ×0.5)
            'FLOATING',         -- Floating PV (§49 Abs. 5 EEG 2023)
            'PARKPLATZ',        -- Carport/parking area PV
            'STECKER_PV'        -- Balkonkraftwerk (simplified registration ≤2 kWp)
        ));

COMMENT ON COLUMN eeg_anlagen.solar_bauform IS
    'Solar PV installation type (§48 EEG 2023). Affects: '
    '§51a §36k Verlängerungsanspruch factor (AGRI_PV: ×0.5 per §51a Abs. 2 EEG), '
    'Ausschreibungspflicht thresholds (FREIFLAECHE: >750 kWp per §22 EEG 2023), '
    '§12 Abs. 3 UStG eligibility (STECKER_PV always exempt). '
    'Only relevant for SOLAR_* erzeugungsart plants.';

-- ── 4. §9 EEG Fernsteuerbarkeit ───────────────────────────────────────────────

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS fernsteuerbarkeit_datum DATE;

COMMENT ON COLUMN eeg_anlagen.fernsteuerbarkeit_datum IS
    'Date when §9 EEG remote controllability (Fernsteuerbarkeit) was installed. '
    'NULL = not yet installed or not required (plants < 7 kWp exempt under §9 Abs. 1 S. 2). '
    'When NULL and leistung_kwp >= 7.0 and status = angemeldet|aktiv: '
    '§52 Abs. 1 Nr. 3 EEG 2023 Pflichtzahlung may accrue (€10/kW/month). '
    'Note: for EEG ≤2021 plants (§100 Übergangsregelung), non-compliance triggers '
    'VerguetungAufMarktwert (not Pflichtzahlung).';

CREATE INDEX IF NOT EXISTS ea_fernsteuerbarkeit_missing ON eeg_anlagen (tenant, status)
    WHERE fernsteuerbarkeit_datum IS NULL
      AND leistung_kwp >= 7.0
      AND status IN ('angemeldet', 'aktiv');

-- ── 5. Direktvermarktung metadata ─────────────────────────────────────────────

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS direktvermarktung_pflicht BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS direktvermarktung_perioden JSONB;

COMMENT ON COLUMN eeg_anlagen.direktvermarktung_pflicht IS
    'Whether this plant is subject to mandatory Direktvermarktung under §20 EEG 2023. '
    'True when leistung_kwp > 100. Computed on plant creation and on capacity updates. '
    'Plants with this flag = true that are on FeedInTariff scheme use Ausfallvergütung '
    'if their Direktvermarkter fails (§21 Abs. 1 Nr. 2 EEG 2023), not regular Vergütung.';

COMMENT ON COLUMN eeg_anlagen.direktvermarktung_perioden IS
    'History of Direktvermarktung periods as a JSONB array. '
    'Each entry: { beginn_datum, ende_datum, direktvermarkter_mp_id, ist_freiwillig, anzulegender_wert_ct }. '
    'Sorted by beginn_datum ascending. NULL = no Direktvermarktung history. '
    'Use current_period() helper in eeg-billing::direktverm to find the active period.';

-- Derived index: plants currently in Direktvermarktung (open period)
CREATE INDEX IF NOT EXISTS ea_direktverm_aktiv ON eeg_anlagen (tenant, status)
    WHERE direktvermarktung_perioden IS NOT NULL;

-- ── 6. §24 multi-block capacity data ─────────────────────────────────────────

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS capacity_blocks JSONB;

COMMENT ON COLUMN eeg_anlagen.capacity_blocks IS
    'Additional capacity blocks for §24 EEG Anlagenerweiterung / Zusammenlegung. '
    'The primary block is described by (leistung_kwp, inbetriebnahme, foerderendedatum, eeg_gesetz). '
    'Each entry: { leistung_kwp, verguetungssatz_ct, inbetriebnahme, foerderendedatum }. '
    'NULL = single-block plant (the vast majority). '
    'Non-NULL enables pro-rata settlement allocation across blocks in eeg-billing::formula.';

-- ── 7. Brutto-Netto distinction for §14a ──────────────────────────────────────

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS leistung_kw_brutto NUMERIC(12, 3);

COMMENT ON COLUMN eeg_anlagen.leistung_kw_brutto IS
    'Gross installed capacity in kW (before §14a Modul 2 HT/NT reduction). '
    'For §14a-enrolled plants, leistung_kwp stores the NET billing capacity. '
    'Brutto is required for Ausschreibungspflicht threshold check (§22 EEG). '
    'NULL when the plant is not enrolled in §14a or when brutto == netto.';

-- ── 8. Computed Direktvermarktungspflicht trigger ─────────────────────────────
-- Automatically set direktvermarktung_pflicht for existing plants > 100 kWp.

UPDATE eeg_anlagen
SET direktvermarktung_pflicht = true
WHERE leistung_kwp > 100
  AND direktvermarktung_pflicht = false;

-- ── 9. View: plants with Fernsteuerbarkeit compliance gap ─────────────────────

CREATE OR REPLACE VIEW eeg_anlagen_fernsteuerbarkeit_ausstehend AS
SELECT
    tr_id,
    tenant,
    malo_id,
    erzeugungsart,
    leistung_kwp,
    eeg_gesetz,
    inbetriebnahme,
    status,
    CURRENT_DATE - inbetriebnahme AS tage_ohne_fernsteuerbarkeit,
    -- §52 Abs. 1 Nr. 3 EEG 2023: €10/kW/month for EEG 2023 plants
    CASE WHEN eeg_gesetz = 2023 THEN leistung_kwp * 10 ELSE 0 END
        AS monatliche_pflichtzahlung_eur,
    -- For EEG ≤2021 plants: Vergütung reduced to EPEX Marktwert (no €/kW penalty)
    CASE WHEN eeg_gesetz < 2023 THEN 'VerguetungAufMarktwert' ELSE NULL END
        AS alt_sanktion
FROM eeg_anlagen
WHERE fernsteuerbarkeit_datum IS NULL
  AND leistung_kwp >= 7.0
  AND status IN ('angemeldet', 'aktiv');

COMMENT ON VIEW eeg_anlagen_fernsteuerbarkeit_ausstehend IS
    'Plants with outstanding §9 EEG Fernsteuerbarkeit installation. '
    'EEG 2023: §52 Abs. 1 Nr. 3 Pflichtzahlung €10/kW/month. '
    'EEG ≤2021: VerguetungAufMarktwert (§100 Übergangsregelung).';

-- ── 10. View: Direktvermarktungspflicht overview ──────────────────────────────

CREATE OR REPLACE VIEW eeg_anlagen_direktverm_pflicht AS
SELECT
    tr_id,
    tenant,
    malo_id,
    erzeugungsart,
    leistung_kwp,
    eeg_gesetz,
    inbetriebnahme,
    status,
    direktvermarktung_pflicht,
    direktvermarktung_perioden IS NOT NULL AS hat_direktverm_history,
    -- Check whether the open period has a Direktvermarkter
    (direktvermarktung_perioden -> -1 ->> 'ende_datum') IS NULL
        AND direktvermarktung_perioden IS NOT NULL AS direktverm_aktuell_aktiv
FROM eeg_anlagen
WHERE direktvermarktung_pflicht = true
  AND status IN ('angemeldet', 'aktiv');

COMMENT ON VIEW eeg_anlagen_direktverm_pflicht IS
    'Plants subject to mandatory Direktvermarktung (§20 EEG 2023, > 100 kW). '
    'Plants with direktverm_aktuell_aktiv = false are on Ausfallvergütung (§21 Abs. 1 Nr. 2).';

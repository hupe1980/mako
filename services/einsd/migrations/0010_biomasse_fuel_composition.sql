-- ══ Migration 0010: §§42–44 EEG 2023 Biomass Fuel Composition ════════════════
--
-- Closes the biomasse: None gap in build_settle_input.
--
-- §43 Abs. 1 Nr. 2 EEG 2023: Biomass plants must not use more than 40 % of
-- their energy input from Energiepflanzen vom Acker (arable energy crops).
-- When this cap is exceeded for a billing period, EEG support for that period
-- is suspended entirely (Sanctioned, EUR 0).
--
-- §44 EEG 2023 Güllekleinanlage: Plants ≤ 75 kW_el with ≥ 80 % liquid/solid
-- manure input qualify for a higher Güllekleinanlage Anzulegender Wert.
-- The `ist_guellebonusanlage` flag is derived at settlement time from the
-- two stored fractions and the plant capacity — it is NOT stored separately.
--
-- Design: individual typed columns (not JSONB) for:
--   1. Direct SQL queries on fuel composition (compliance monitoring)
--   2. No over-normalisation — only the inputs needed to reconstruct
--      BiomassSettlementData::new() at settlement time
--   3. NULL = non-biomass plant (no §43/§44 enforcement)
--
-- Legal bases:
--   §43 Abs. 1 Nr. 2 EEG 2023 (BGBl. I 2023 Nr. 1) — substrate cap 40 %
--   §44 EEG 2023 — Güllekleinanlage bonus (≤75 kW, ≥80 % Gülle)
--   §42a EEG 2023 — Holzbiomasse Neuanlagen-Einschränkung (ab 2026)
-- ═════════════════════════════════════════════════════════════════════════════

ALTER TABLE eeg_anlagen
    -- Primary fuel type fed into the plant.
    -- Matches BiomassBrennstoff enum: PFLANZLICHE_BIOMASSE | BIOMETHAN_AUS_BIOMASSE |
    -- GUELLE | FESTMIST | HOLZBIOMASSE | KLAEGAS | DEPONIEGAS | GRUBENGAS | BIOABFALL
    -- NULL for non-biomass plants (solar, wind, hydro, KWKG, etc.).
    ADD COLUMN IF NOT EXISTS biomasse_hauptbrennstoff      TEXT
        CHECK (biomasse_hauptbrennstoff IN (
            'PFLANZLICHE_BIOMASSE',
            'BIOMETHAN_AUS_BIOMASSE',
            'GUELLE',
            'FESTMIST',
            'HOLZBIOMASSE',
            'KLAEGAS',
            'DEPONIEGAS',
            'GRUBENGAS',
            'BIOABFALL'
        )),

    -- §44 EEG 2023: fraction of energy input from liquid/solid manure (0.00–1.00).
    -- ≥ 0.80 + leistung_kwp ≤ 75 → Güllekleinanlage bonus applies.
    -- NULL when plant is not manure-fed.
    ADD COLUMN IF NOT EXISTS biomasse_guelle_anteil        NUMERIC(5, 4)
        CHECK (biomasse_guelle_anteil BETWEEN 0 AND 1),

    -- §43 Abs. 1 Nr. 2 EEG 2023: fraction of energy input from Energiepflanzen
    -- vom Acker (dedicated energy crops from arable land, 0.00–1.00).
    -- > 0.40 → substrate cap exceeded → EEG support suspended for the period.
    -- NULL for non-biomass plants or when operator has not submitted fuel data.
    -- Treat NULL as 0.00 at settlement time (conservative: cap not applied).
    ADD COLUMN IF NOT EXISTS biomasse_energiepflanzen_anteil NUMERIC(5, 4)
        CHECK (biomasse_energiepflanzen_anteil BETWEEN 0 AND 1);

COMMENT ON COLUMN eeg_anlagen.biomasse_hauptbrennstoff IS
    '§§42–44 EEG 2023: primary fuel type (BiomassBrennstoff). '
    'NULL = not a biomass/biogas plant. '
    'Determines §43 substrate cap applicability and §44 Güllekleinanlage eligibility.';

COMMENT ON COLUMN eeg_anlagen.biomasse_guelle_anteil IS
    '§44 EEG 2023 Güllekleinanlage: fraction of energy input from Gülle/Festmist (0.0–1.0). '
    'When ≥ 0.80 AND leistung_kwp ≤ 75 kW: Güllekleinanlage bonus rate applies. '
    'NULL when biomasse_hauptbrennstoff is NULL or non-manure fuel.';

COMMENT ON COLUMN eeg_anlagen.biomasse_energiepflanzen_anteil IS
    '§43 Abs. 1 Nr. 2 EEG 2023: fraction of energy input from Energiepflanzen '
    'vom Acker (arable energy crops, 0.0–1.0). '
    '> 0.40 = substrate cap exceeded → EEG Vergütung suspended for that billing period. '
    'NULL = cap not enforced (non-biomass plant or fraction not yet submitted by operator).';

-- Fast lookup for compliance monitoring: plants near or over the substrate cap
CREATE INDEX IF NOT EXISTS ea_biomasse_substrat
    ON eeg_anlagen (tenant, biomasse_energiepflanzen_anteil)
    WHERE biomasse_energiepflanzen_anteil IS NOT NULL;

-- Fast lookup for Güllekleinanlage identification
CREATE INDEX IF NOT EXISTS ea_guelleanlage
    ON eeg_anlagen (tenant, leistung_kwp, biomasse_guelle_anteil)
    WHERE biomasse_hauptbrennstoff IN ('GUELLE', 'FESTMIST')
      AND biomasse_guelle_anteil >= 0.80;

-- Migration 0007: grid_area column + initial EEG tariff rates seed
--
-- 1. Add grid_area to eeg_anlagen for §53b regional reduction lookups.
--    §53b EEG 2023 reductions are applied per BNetzA-certified Netzgebiet.
--    Plants must be assigned to a Netzgebiet to participate in §53b lookups.
--
-- 2. Seed initial eeg_verguetungssaetze with Solarpaket I reference rates
--    (BGBl. I Nr. 107, 16.05.2024 — effective Q2 2024).
--    Without seed data, POST /api/v1/verguetungssatz-lookup returns 404
--    and plant registration fails when verguetungssatz_ct is unknown.
--
-- Legal basis:
--   §53b EEG 2023 — Verringerung bei Regionalnachweisen (grid area)
--   §48 EEG 2023 n.F. (Solarpaket I) — §48 Abs. 1 Satz 1 solar PV rates

-- ── 1. grid_area column ────────────────────────────────────────────────────

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS grid_area TEXT;

COMMENT ON COLUMN eeg_anlagen.grid_area IS
    'Netzgebiet identifier for §53b EEG 2023 regional reduction lookups. '
    'Set to the BNetzA-assigned grid area identifier (e.g. "DE-TN-001"). '
    'NULL = no §53b regional reduction applicable for this plant.';

CREATE INDEX IF NOT EXISTS ea_grid_area
    ON eeg_anlagen (tenant, grid_area)
    WHERE grid_area IS NOT NULL;

-- ── 2. Solarpaket I seed rates (§48 EEG 2023 n.F., BGBl. I Nr. 107, 16.05.2024) ──
-- Net rates after §53 deduction (gross AW − 0.4 ct/kWh for solar).
-- Volleinspeisung rates include the §48 Abs. 2a bonus.
-- These are the Q2 2024 (Solarpaket I reference) rates; quarterly degression applies
-- for commissioning quarters after 2024-Q2 via the eeg-billing degression module.

INSERT INTO eeg_verguetungssaetze
    (erzeugungsart, leistung_min_kwp, leistung_max_kwp,
     verguetungssatz_ct, verguetungsform, billing_start, notes)
VALUES
    -- Solar PV roof (SOLAR_AUFDACH / SOLAR) — Überschusseinspeisung
    -- §48 Abs. 1 Nr. 1 EEG 2023: gross 8.51 ct/kWh − 0.40 ct (§53) = 8.11 ct net
    ('SOLAR_AUFDACH',    0,   10,  8.11, 'UEBERSCHUSS',   '2024-05-01', 'Solarpaket I §48 Abs. 1 Nr. 1a, ≤10 kWp'),
    ('SOLAR_AUFDACH',   10,   40,  7.03, 'UEBERSCHUSS',   '2024-05-01', 'Solarpaket I §48 Abs. 1 Nr. 1b, >10–40 kWp'),
    ('SOLAR_AUFDACH',   40,  100,  5.74, 'UEBERSCHUSS',   '2024-05-01', 'Solarpaket I §48 Abs. 1 Nr. 1c, >40–100 kWp'),
    ('SOLAR_AUFDACH',  100,  400,  4.30, 'UEBERSCHUSS',   '2024-05-01', 'Solarpaket I §48 Abs. 1 Nr. 1d, >100–400 kWp'),
    ('SOLAR_AUFDACH',  400, 1000,  3.14, 'UEBERSCHUSS',   '2024-05-01', 'Solarpaket I §48 Abs. 1 Nr. 1e, >400 kWp–1 MWp'),
    -- Solar PV roof — Volleinspeisung (§48 Abs. 2a: +4.8 ct/kWh bonus over Überschuss)
    -- gross 13.31 ct/kWh − 0.40 ct (§53) = 12.91 ct net
    ('SOLAR_AUFDACH',    0,   10, 12.91, 'VOLLEINSPEISUNG', '2024-05-01', 'Solarpaket I §48 Abs. 2a, ≤10 kWp Volleinspeisung'),
    ('SOLAR_AUFDACH',   10,   40, 10.83, 'VOLLEINSPEISUNG', '2024-05-01', 'Solarpaket I §48 Abs. 2a, >10–40 kWp Volleinspeisung'),
    ('SOLAR_AUFDACH',   40,  100,  9.54, 'VOLLEINSPEISUNG', '2024-05-01', 'Solarpaket I §48 Abs. 2a, >40–100 kWp Volleinspeisung'),
    -- Backward compat: 'SOLAR' (generic, maps to SOLAR_AUFDACH rates)
    ('SOLAR',           0,   10,  8.11, 'UEBERSCHUSS',   '2024-05-01', 'Solarpaket I generic alias'),
    ('SOLAR',          10,   40,  7.03, 'UEBERSCHUSS',   '2024-05-01', 'Solarpaket I generic alias'),
    ('SOLAR',          40,  100,  5.74, 'UEBERSCHUSS',   '2024-05-01', 'Solarpaket I generic alias'),
    ('SOLAR',         100,  400,  4.30, 'UEBERSCHUSS',   '2024-05-01', 'Solarpaket I generic alias'),
    -- Pre-Solarpaket I rates (EEG 2023 initial, 2023-02 through 2024-04)
    -- §48 EEG 2023 original version — lower rates before Solarpaket I
    ('SOLAR_AUFDACH',    0,   10,  7.71, 'UEBERSCHUSS',   '2023-02-01', 'EEG 2023 initial §48 Abs. 1 Nr. 1a, ≤10 kWp'),
    ('SOLAR_AUFDACH',   10,   40,  6.69, 'UEBERSCHUSS',   '2023-02-01', 'EEG 2023 initial §48 Abs. 1 Nr. 1b, >10–40 kWp'),
    ('SOLAR_AUFDACH',   40,  100,  5.76, 'UEBERSCHUSS',   '2023-02-01', 'EEG 2023 initial §48 Abs. 1 Nr. 1c, >40–100 kWp'),
    ('SOLAR',           0,   10,  7.71, 'UEBERSCHUSS',   '2023-02-01', 'EEG 2023 initial generic alias'),
    ('SOLAR',          10,   40,  6.69, 'UEBERSCHUSS',   '2023-02-01', 'EEG 2023 initial generic alias'),
    -- Wind onshore statutory rates (§46 EEG 2023 — applies to plants ≤750 kW only)
    -- These are approximations; actual rates depend on §36k Korrekturfaktor + Standort.
    -- Plants >750 kW must participate in Ausschreibung and have no statutory AW.
    -- Net rate: no §53 deduction for statutory AW (§53 applies at payout, not for lookup)
    -- Note: For wind Direktvermarktung, direktverm_aw_ct is plant-specific; store separately.
    -- Omitted here as wind statutory rates are Standort-dependent (§46 EEG 2023).

    -- KWKG 2023 rates (§7 Abs. 1 KWKG 2023, effective 01.01.2023)
    -- These are the KWK-Zuschlag rates — no §53 deduction applies to KWKG
    ('KWKG',    0,   50,  8.00, 'KWK_ZUSCHLAG', '2023-01-01', 'KWKG 2023 §7 Abs. 1 Nr. 1, ≤50 kW_el'),
    ('KWKG',   50,  100,  6.00, 'KWK_ZUSCHLAG', '2023-01-01', 'KWKG 2023 §7 Abs. 1 Nr. 2, >50–100 kW_el'),
    ('KWKG',  100,  250,  5.00, 'KWK_ZUSCHLAG', '2023-01-01', 'KWKG 2023 §7 Abs. 1 Nr. 3, >100–250 kW_el'),
    ('KWKG',  250, 2000,  4.00, 'KWK_ZUSCHLAG', '2023-01-01', 'KWKG 2023 §7 Abs. 1 Nr. 4, >250 kW–2 MW_el'),
    ('KWKG', 2000, NULL,  3.00, 'KWK_ZUSCHLAG', '2023-01-01', 'KWKG 2023 §7 Abs. 1 Nr. 5, >2 MW_el')
ON CONFLICT DO NOTHING;

COMMENT ON TABLE eeg_verguetungssaetze IS
    'EEG/KWKG tariff reference table. '
    'verguetungssatz_ct = NET rate (after §53 deduction for EEG). '
    'Populated with statutory base rates; use POST /api/v1/verguetungssatz-lookup '
    'to query for a specific plant. Add custom rates via PUT /api/v1/verguetungssaetze. '
    'Quarterly degression (§23a EEG) is computed by the eeg-billing degression module '
    'for commissioning quarters after the billing_start date.';

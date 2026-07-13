-- ── einsd migration 0002 — MaStR registration + Bankverbindung ──────────────
--
-- Background:
--   §52 Abs. 1 Nr. 11 EEG 2023 requires plant operators to register in the
--   Marktstammdatenregister (MaStR, §111e EnWG). Non-registration triggers:
--     - EEG ≤2021 (old §52/§47 via §100 Übergangsregelung): Vergütung = 0
--     - EEG 2023: €10/kW/month Pflichtzahlung to NB (Vergütung remains payable)
--
--   Previously tracked via notes LIKE '%mastr_not_registered%' — replaced here
--   with proper typed columns.
--
-- Payment flow:
--   einsd emits de.eeg.verguetung.berechnet → accountingd posts Gutschrift.
--   For SEPA CT outgoing payment, the NB billing system needs the operator's IBAN.
--   Stored here on the Anlage record (not in accountingd which handles consumer SEPA DD).
--
-- Status lifecycle:
--   ANGEMELDET → AKTIV → (ABGEMELDET | FOERDERUNG_BEENDET | REPOWERED)
--
--   angemeldet:        Plant physically commissioned, registered in einsd,
--                      but MaStR registration not yet confirmed.
--                      §52 penalty or Vergütung suspension applies until aktiv.
--   aktiv:             MaStR confirmed, Vergütung flows normally.
--   abgemeldet:        Plant deregistered (operator request or decommissioning).
--   foerderung_beendet: 20-year Förderdauer expired.
--   repowered:         Historical record of a repowered plant.

-- ── 1. Add MaStR columns ──────────────────────────────────────────────────────

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS mastr_registriert BOOLEAN   NOT NULL DEFAULT true,
    ADD COLUMN IF NOT EXISTS mastr_nummer      TEXT,
    ADD COLUMN IF NOT EXISTS mastr_datum       DATE;

COMMENT ON COLUMN eeg_anlagen.mastr_registriert IS
    'Whether the plant is registered in MaStR (Marktstammdatenregister, §111e EnWG). '
    'When false under EEG 2023: Pflichtzahlung €10/kW/month (§52 Abs. 1 Nr. 11). '
    'Under EEG ≤2021 via §100 Übergangsregelung: Vergütung = 0.';

COMMENT ON COLUMN eeg_anlagen.mastr_nummer IS
    'MaStR Registrierungsnummer (format: SEE000000000000, e.g. SEE900000000001). '
    'Issued by Bundesnetzagentur after registration at marktstammdatenregister.de.';

COMMENT ON COLUMN eeg_anlagen.mastr_datum IS
    'Date of successful MaStR registration confirmation.';

-- ── 2. Migrate existing notes-based MaStR flag ───────────────────────────────

-- Plants with 'mastr_not_registered' note → mastr_registriert = false
UPDATE eeg_anlagen
SET mastr_registriert = false
WHERE notes LIKE '%mastr_not_registered%';

-- Plants without the note → keep default true (already registered or unknown → optimistic)

-- ── 3. Add Bankverbindung columns for EEG Vergütung payment (SEPA CT) ────────

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS bank_iban           TEXT,
    ADD COLUMN IF NOT EXISTS bank_bic            TEXT,
    ADD COLUMN IF NOT EXISTS zahlungsempfaenger  TEXT;

COMMENT ON COLUMN eeg_anlagen.bank_iban IS
    'IBAN of the plant operator for EEG Vergütung payment (SEPA Credit Transfer). '
    'Required for NB billing system to dispatch monthly Vergütung. '
    'Stored here (not in accountingd) because direction is NB→Betreiber (CT, not DD).';

COMMENT ON COLUMN eeg_anlagen.bank_bic IS
    'BIC/SWIFT of the operator bank. Optional — derivable from IBAN for SEPA reachable IBANs.';

COMMENT ON COLUMN eeg_anlagen.zahlungsempfaenger IS
    'Full name of the payment recipient (Zahlungsempfänger) for SEPA CT reference.';

-- ── 4. Add ANGEMELDET status ──────────────────────────────────────────────────
-- Represents: plant commissioned, registered in einsd, awaiting MaStR confirmation.

ALTER TABLE eeg_anlagen
    DROP CONSTRAINT IF EXISTS eeg_anlagen_status_check;

ALTER TABLE eeg_anlagen
    ADD CONSTRAINT eeg_anlagen_status_check
    CHECK (status IN (
        'angemeldet',        -- commissioned, MaStR pending → penalty accrues
        'aktiv',             -- MaStR confirmed, Vergütung normal
        'abgemeldet',        -- deregistered
        'foerderung_beendet',-- 20-year Förderdauer expired
        'repowered'          -- historical record (plant repowered)
    ));

-- ── 5. Index for MaStR lookup ─────────────────────────────────────────────────

CREATE INDEX IF NOT EXISTS ea_mastr_nummer ON eeg_anlagen (mastr_nummer)
    WHERE mastr_nummer IS NOT NULL;

CREATE INDEX IF NOT EXISTS ea_pending_mastr ON eeg_anlagen (tenant, status)
    WHERE mastr_registriert = false AND status = 'angemeldet';

-- ── 6. View: plants pending MaStR registration ───────────────────────────────
-- Useful for NB compliance monitoring — §52 penalty accrues daily for these.

CREATE OR REPLACE VIEW eeg_anlagen_mastr_ausstehend AS
SELECT
    tr_id,
    tenant,
    malo_id,
    erzeugungsart,
    leistung_kwp,
    eeg_gesetz,
    inbetriebnahme,
    status,
    -- Days since commissioning without MaStR registration
    CURRENT_DATE - inbetriebnahme AS tage_ohne_mastr,
    -- Monthly penalty for EEG 2023 plants (€10/kW)
    CASE WHEN eeg_gesetz = 2023
         THEN leistung_kwp * 10
         ELSE 0
    END AS monatliche_pflichtzahlung_eur,
    notes
FROM eeg_anlagen
WHERE mastr_registriert = false
  AND status IN ('angemeldet', 'aktiv');

COMMENT ON VIEW eeg_anlagen_mastr_ausstehend IS
    'Plants with outstanding MaStR registration. '
    'NB must monitor and apply §52 EEG 2023 Pflichtzahlung (€10/kW/month) '
    'or Vergütung suspension (old EEG ≤2021 via §100 Übergangsregelung).';

-- ── 7. Add CHECK constraint for eeg_gesetz values ────────────────────────────
-- Enforce canonical EEG law years. 0=KWKG; valid EEG years: 2000,2004,2009,2012,2017,2021,2023.
-- Operators storing 2014 (EEG 2014 amendment) should use 2012.
-- From-DB-year parsing in eeg-billing accepts ranges for defensive correctness,
-- but the DB should store only canonical values.
ALTER TABLE eeg_anlagen
    DROP CONSTRAINT IF EXISTS eeg_anlagen_eeg_gesetz_check;

ALTER TABLE eeg_anlagen
    ADD CONSTRAINT eeg_anlagen_eeg_gesetz_check
    CHECK (eeg_gesetz IN (0, 2000, 2004, 2009, 2012, 2014, 2017, 2021, 2023));

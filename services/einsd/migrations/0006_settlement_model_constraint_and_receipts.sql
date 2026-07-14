-- Migration 0006: Fix settlement_model CHECK constraint + extend settlement_receipts
--
-- Fixes:
--
-- 1. The original settlement_model CHECK constraint (migration 0001) only allowed old
--    German model names (VERGUETUNG, DIREKTVERMARKTUNG, ...). The API now uses both
--    the old names (for backward compat) and canonical SettlementScheme names
--    (FEED_IN_TARIFF, MARKET_PREMIUM, ...). Any plant registered with a new name
--    would fail the constraint at runtime. Drop and recreate the constraint to
--    accept all valid names from both naming conventions.
--
-- 2. The settlement_receipts table (migration 0001) is missing key computed fields:
--    faelligkeitsdatum, verlaengerungsanspruch_qh, billing_days_fraction,
--    positions_json. These are now computed by eeg-billing but never persisted,
--    making the audit trail incomplete under §22 MessZV.
--
-- 3. Add violation_start_date to support cumulative §52 Pflichtzahlung tracking
--    (monate_des_verstosses should be cumulative from onset, not always 1).
--
-- Legal basis:
--   §22 MessZV: 3-year retention of billing records
--   §52 EEG 2023: cumulative monthly penalties
--   §26 Abs. 1 EEG 2023: Fälligkeitsdatum per receipt

-- ── 1. settlement_model CHECK constraint update ───────────────────────────────

-- Drop old constraint by recreating the column with the new CHECK.
-- PostgreSQL doesn't support DROP CONSTRAINT on inline column constraints
-- added via CREATE TABLE; we use ALTER TABLE to add a named constraint
-- after dropping the inline one via a table rewrite approach.

-- First, add a named constraint that covers all valid values:
ALTER TABLE eeg_anlagen
    DROP CONSTRAINT IF EXISTS eeg_anlagen_settlement_model_check;

ALTER TABLE eeg_anlagen
    ADD CONSTRAINT eeg_anlagen_settlement_model_check
    CHECK (settlement_model IN (
        -- Old names (backward compatibility — existing DB rows)
        'VERGUETUNG',
        'DIREKTVERMARKTUNG',
        'AUSSCHREIBUNG',
        'POST_EEG_SPOT',
        'EIGENVERBRAUCH',
        'MIETERSTROM',
        'KWKG_ZUSCHLAG',
        'FLEXIBILITAET',
        -- New canonical SettlementScheme names (Rust enum SCREAMING_SNAKE_CASE)
        'FEED_IN_TARIFF',
        'MARKET_PREMIUM',
        'TENANT_ELECTRICITY',
        'POST_EEG',
        'KWK_SURCHARGE',
        'FLEXIBILITY_PREMIUM',
        'FLEXIBILITY_SURCHARGE',
        'FLEXIBILITAET_ZUSCHLAG',
        'TEMPORARY_FEED_IN_TARIFF'
    ));

-- Also fix the settlement_receipts.settlement_model column (no constraint there,
-- but ensure it's documented as accepting the same set).
COMMENT ON COLUMN eeg_anlagen.settlement_model IS
    'Settlement scheme — accepts both legacy German names (VERGUETUNG, DIREKTVERMARKTUNG, …) '
    'and canonical SettlementScheme names (FEED_IN_TARIFF, MARKET_PREMIUM, …). '
    'The run_settlement function normalises both to the eeg-billing SettlementScheme enum.';

-- ── 2. settlement_receipts: add missing computed fields ──────────────────────

-- §26 Abs. 1 EEG 2023: Fälligkeitsdatum = 15th of the following calendar month.
-- Computed by eeg-billing from billing_date; persisted here for payment dispatch.
ALTER TABLE settlement_receipts
    ADD COLUMN IF NOT EXISTS faelligkeitsdatum DATE;

COMMENT ON COLUMN settlement_receipts.faelligkeitsdatum IS
    '§26 Abs. 1 EEG 2023: 15th of the month following the billing month. '
    'NULL when billing_date was not set at calculation time.';

-- §51a EEG 2023: Verlängerungsanspruch in quarter-hours accrued in this period.
-- Cumulative total per plant must be aggregated from all receipts.
ALTER TABLE settlement_receipts
    ADD COLUMN IF NOT EXISTS verlaengerungsanspruch_qh BIGINT NOT NULL DEFAULT 0;

COMMENT ON COLUMN settlement_receipts.verlaengerungsanspruch_qh IS
    '§51a EEG 2023: quarter-hours by which the Förderzeitraum is extended in this period. '
    'Solar PV: ceil(lost_qh/2); all others: lost_qh (1:1). '
    'Cumulative total = SUM(verlaengerungsanspruch_qh) over all receipts per plant.';

-- §25 Abs. 1 Satz 3 EEG: billing_days_fraction for mid-month commissioning/expiry.
-- Stored for audit trail and correction settlement computation.
ALTER TABLE settlement_receipts
    ADD COLUMN IF NOT EXISTS billing_days_fraction NUMERIC(8,6);

COMMENT ON COLUMN settlement_receipts.billing_days_fraction IS
    '§25 / §26 EEG: fraction of billing month with entitlement (mid-month commissioning '
    'or Förderendedatum). NULL = full month (1.0). Stored for Endabrechnung correction.';

-- JSONB snapshot of billing positions for full audit trail.
-- Required by §22 MessZV for 3-year retention of itemized settlement records.
ALTER TABLE settlement_receipts
    ADD COLUMN IF NOT EXISTS positions_json JSONB;

COMMENT ON COLUMN settlement_receipts.positions_json IS
    '§22 MessZV: JSONB snapshot of billing positions at settlement time. '
    'Each element: { description, legal_basis, kwh, rate_ct_kwh, eur }. '
    'Immutable after first write; preserved for audit/correction purposes.';

-- ── 3. eeg_anlagen: violation tracking for cumulative §52 penalties ──────────

-- When a §52 violation was first observed. Used to compute cumulative
-- monate_des_verstosses for Pflichtzahlung calculation.
ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS mastr_violation_start DATE;

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS fernsteuerbarkeit_violation_start DATE;

COMMENT ON COLUMN eeg_anlagen.mastr_violation_start IS
    '§52 Abs. 1 Nr. 11 EEG 2023: date when MaStR non-registration violation began. '
    'NULL when mastr_registriert = true. Used to compute monate_des_verstosses.';

COMMENT ON COLUMN eeg_anlagen.fernsteuerbarkeit_violation_start IS
    '§52 Abs. 1 Nr. 1 EEG 2023: date when Fernsteuerbarkeit-missing violation began. '
    'NULL when fernsteuerbarkeit_datum IS NOT NULL. Used to compute monate_des_verstosses.';

-- ── 4. eeg_anlagen: missing Förderdauer extension tracking ───────────────────

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS verlaengerungsanspruch_qh_gesamt BIGINT NOT NULL DEFAULT 0;

COMMENT ON COLUMN eeg_anlagen.verlaengerungsanspruch_qh_gesamt IS
    '§51a EEG 2023: cumulative quarter-hours accrued for Förderzeitraum extension. '
    'Summed from settlement_receipts.verlaengerungsanspruch_qh. '
    'When non-zero, foerderendedatum must be extended by this many quarter-hours.';

-- ── 5. eeg_anlagen: Ausschreibung lifecycle ──────────────────────────────────

-- Date when the BNetzA Zuschlag expires (Erlöschen des Zuschlags per §35a EEG).
ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS zuschlag_erloeschen_datum DATE;

-- Whether the Zuschlag has been formally revoked/expired.
ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS award_expired BOOLEAN NOT NULL DEFAULT FALSE;

COMMENT ON COLUMN eeg_anlagen.zuschlag_erloeschen_datum IS
    '§35a EEG 2023: date when the BNetzA Zuschlag automatically expires if plant '
    'is not commissioned. NULL for non-Ausschreibungsanlagen.';

COMMENT ON COLUMN eeg_anlagen.award_expired IS
    '§35a EEG 2023: true when the Zuschlag has expired or been formally revoked. '
    'When true, settlement returns FoerderungBeendet regardless of foerderendedatum.';

-- ── 6. last_switch_date for §21b Veräußerungsform monthly switch guard ────────

ALTER TABLE eeg_anlagen
    ADD COLUMN IF NOT EXISTS last_veraeusserungsform_switch DATE;

COMMENT ON COLUMN eeg_anlagen.last_veraeusserungsform_switch IS
    '§21b EEG 2023: date of the last Veräußerungsform switch. '
    'Plants may only switch once per calendar month (§21c Abs. 1 Satz 1 EEG 2023). '
    'Used by validate_switch_to_vergütung() to enforce the monthly guard.';

-- ── Indexes ───────────────────────────────────────────────────────────────────

CREATE INDEX IF NOT EXISTS sr_faelligkeitsdatum
    ON settlement_receipts (tenant, faelligkeitsdatum)
    WHERE faelligkeitsdatum IS NOT NULL;

CREATE INDEX IF NOT EXISTS ea_award_expired
    ON eeg_anlagen (tenant, award_expired)
    WHERE award_expired = true;

CREATE INDEX IF NOT EXISTS ea_mastr_violation
    ON eeg_anlagen (tenant, mastr_violation_start)
    WHERE mastr_violation_start IS NOT NULL;

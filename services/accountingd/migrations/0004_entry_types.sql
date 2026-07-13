-- accountingd migration 0004: extended entry_type CHECK + new entry types
--
-- Problems fixed:
--   1. `KORREKTURRECHNUNG` was used in code but missing from CHECK → silent DB errors.
--      Renamed to `STORNO` (billing reversal / Stornorechnung), semantically clearer.
--   2. Added `EEG_MARKTPRAEMIE` for Direktvermarktung EEG settlements
--      (de.eeg.marktpraemie.berechnet from einsd).
--   3. Added `JAHRESABSCHLUSS` for annual settlement entries so they are
--      self-documenting in the ledger (previously reused RECHNUNG/GUTSCHRIFT).
--
-- Regulatory basis:
--   - §40 Abs. 1 EnWG: Abschlussrechnung must reflect Mehr-/Mindermenge.
--   - §21 EEG 2023: Direktvermarktung Marktprämie is a separate payment category.
--   - §238 HGB: Buchführungspflicht — every entry must have a clear Buchungstext.
--
-- The old `KORREKTUR` type is kept for manual operator corrections.
-- Billing reversals from billingd now use `STORNO`.

ALTER TABLE ledger_entries DROP CONSTRAINT IF EXISTS ledger_entries_entry_type_check;

ALTER TABLE ledger_entries
    ADD CONSTRAINT ledger_entries_entry_type_check
    CHECK (entry_type IN (
        'RECHNUNG',        -- debit:  invoice from billingd/netzbilanzd
        'STORNO',          -- debit:  invoice reversal / Stornorechnung (was KORREKTURRECHNUNG)
        'ZAHLUNG',         -- credit: incoming payment (CAMT.054, ERP confirm, direct debit)
        'GUTSCHRIFT',      -- credit: credit note from billingd (de.billing.gutschrift.erstellt)
        'EEG_GUTSCHRIFT',  -- credit: EEG Einspeisevergütung (de.eeg.verguetung.berechnet)
        'EEG_MARKTPRAEMIE',-- credit: EEG Direktvermarktung Marktprämie (de.eeg.marktpraemie.berechnet)
        'BANKRUECKLAST',   -- debit:  returned SEPA direct debit
        'MAHNGEBUEHR',     -- debit:  dunning fee
        'ABSCHLAG',        -- debit:  monthly advance payment
        'JAHRESABSCHLUSS', -- signed: annual Mehr-/Mindermengenabrechnung settlement
        'KORREKTUR'        -- signed: manual operator correction
    ));

COMMENT ON COLUMN ledger_entries.entry_type IS
    'Buchungsart. Positive amount_ct = Debit (Forderung); negative amount_ct = Credit (Gutschrift). '
    'STORNO replaces the former KORREKTURRECHNUNG (billing system reversal). '
    'EEG_MARKTPRAEMIE is for Direktvermarktung Marktprämie payments from einsd. '
    'JAHRESABSCHLUSS is the annual Mehr-/Mindermengenabrechnung settlement entry.';

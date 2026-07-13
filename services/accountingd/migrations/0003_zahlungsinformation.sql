-- accountingd migration 0003: BO4E Zahlungsinformation typed payment details
--
-- Adds a typed `zahlungsinformation JSONB` column to `accounts` for structured
-- payment information stored as `rubo4e::current::Zahlungsinformation` camelCase:
--   iban, bic, kontoinhaber, sepaReferenz, zahlungsart
--
-- The existing `iban` column is kept and kept in sync by the handler so that
-- `import_payments` (CAMT.054) matching and `run_sepa` (pain.008) continue to
-- work unchanged.
--
-- New endpoint: PUT/GET /api/v1/accounts/{malo_id}/zahlungsinformation
--
-- Regulatory: §40 Abs. 3 EnWG — payment method must be documented.
-- BO4E Zahlungsinformation enables ERP-side typed SEPA batch import.

ALTER TABLE accounts ADD COLUMN IF NOT EXISTS zahlungsinformation JSONB;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ NOT NULL DEFAULT now();

COMMENT ON COLUMN accounts.zahlungsinformation IS
    'BO4E Zahlungsinformation COM (rubo4e::current::Zahlungsinformation). '
    'Fields: iban, bic, kontoinhaber, sepaReferenz, zahlungsart. '
    'Validated on PUT /accounts/{malo_id}/zahlungsinformation. '
    'iban column is kept in sync for CAMT.054 payment matching.';

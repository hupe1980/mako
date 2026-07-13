-- vertragd migration 0002: BO4E Zahlungsinformation typed payment details
--
-- Adds a typed `zahlungsinformation JSONB` column to `kunden` for structured
-- payment information stored as `rubo4e::current::Zahlungsinformation` camelCase:
--   iban, bic, kontoinhaber, sepaReferenz, zahlungsart
--
-- New endpoints:
--   PUT/GET /api/v1/kunden/{id}/zahlungsinformation
--
-- Regulatory: §40 Abs. 3 EnWG — payment method must be documented.
-- BO4E Zahlungsinformation enables ERP-side typed SEPA batch import
-- and structured GDPR Art. 15 data export.

ALTER TABLE kunden ADD COLUMN IF NOT EXISTS zahlungsinformation JSONB;

COMMENT ON COLUMN kunden.zahlungsinformation IS
    'BO4E Zahlungsinformation COM (rubo4e::current::Zahlungsinformation). '
    'Fields: iban, bic, kontoinhaber, sepaReferenz, zahlungsart. '
    'Validated on PUT /kunden/{id}/zahlungsinformation. '
    'Enables ERP-side BO4E SEPA batch import. IBAN validated via mod-97 on PUT.';

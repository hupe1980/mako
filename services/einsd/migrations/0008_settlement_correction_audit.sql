-- Migration 0008: correction_of traceability in settlement_receipts
--
-- §22 MessZV requires a 3-year audit trail with traceability between
-- original and correction receipts. Add correction_of + is_correction
-- to allow POST /anlagen/{tr_id}/settlements/{year}/{month}/correction
-- to create a correction receipt that references the original.
--
-- Also: add a history snapshot table to preserve the original receipt
-- content before the correction upsert overwrites it.
--
-- Legal basis: §22 MessZV (Bilanzierungsverordnung) 3-year retention requirement.

-- ── settlement_receipts: correction traceability ──────────────────────────

ALTER TABLE settlement_receipts
    ADD COLUMN IF NOT EXISTS is_correction BOOLEAN NOT NULL DEFAULT false;

ALTER TABLE settlement_receipts
    ADD COLUMN IF NOT EXISTS correction_of UUID;

ALTER TABLE settlement_receipts
    ADD COLUMN IF NOT EXISTS correction_reason TEXT;

COMMENT ON COLUMN settlement_receipts.is_correction IS
    '§22 MessZV: true when this receipt supersedes a prior calculation for the same period. '
    'Original receipt is preserved in settlement_receipt_history.';

COMMENT ON COLUMN settlement_receipts.correction_of IS
    '§22 MessZV: UUID of the original settlement_receipts row this corrects. '
    'Enables auditors to trace the full correction chain.';

COMMENT ON COLUMN settlement_receipts.correction_reason IS
    'CorrectionReason enum value: MeterDataCorrected, TariffCorrected, '
    'MastrRegistrationConfirmed, CapacityCorrected, RegulatoryReprocessing, '
    'FoerderendedatumCorrected, Other.';

-- ── settlement_receipt_history: immutable snapshot before correction ──────
-- Captures the full state of a receipt before it is overwritten by a correction.
-- This preserves the original values for audit purposes without changing the
-- main table's unique constraint (one live receipt per billing period per plant).

CREATE TABLE IF NOT EXISTS settlement_receipt_history (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    original_id     UUID        NOT NULL,  -- settlement_receipts.id before correction
    snapshot_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    tr_id           TEXT        NOT NULL,
    tenant          TEXT        NOT NULL,
    billing_year    SMALLINT    NOT NULL,
    billing_month   SMALLINT    NOT NULL,
    settlement_eur  NUMERIC(14,5),
    status          TEXT,
    settlement_data JSONB       NOT NULL   -- full copy of the original receipt row
);

CREATE INDEX IF NOT EXISTS srh_original_id ON settlement_receipt_history (original_id);
CREATE INDEX IF NOT EXISTS srh_tr_id ON settlement_receipt_history (tr_id, tenant, billing_year, billing_month);

COMMENT ON TABLE settlement_receipt_history IS
    '§22 MessZV immutable audit snapshots. '
    'A row is inserted here before every correction upsert via '
    'POST /anlagen/{tr_id}/settlements/{year}/{month}/correction. '
    'settlement_data contains the full receipt as JSONB for forensic tracing.';

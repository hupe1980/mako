-- 0002_malo_id_and_dispute_resolution.sql
--
-- (a) Add `malo_id` as a first-class indexed column populated on INSERT.
--     Extracted from `rechnung->>'marktlokationsId'` so zahlungsstatus queries
--     use an index instead of a full JSONB scan.
--
-- (b) Back-fill existing rows from the JSONB field.
--
-- (c) Add `dispute_resolved_at` and `dispute_resolution_note` columns so that
--     the Disputed → Resolved state transition can be recorded via
--     `POST /api/v1/receipts/{id}/resolve-dispute`.
--
-- (d) Extend the `outcome` CHECK constraint to include 'Resolved'.

-- ── (a) Add columns ────────────────────────────────────────────────────────────

ALTER TABLE invoic_receipts
    ADD COLUMN IF NOT EXISTS malo_id TEXT;

ALTER TABLE invoic_receipts
    ADD COLUMN IF NOT EXISTS dispute_resolved_at  TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS dispute_resolution_note TEXT;

-- ── (b) Back-fill malo_id from existing JSONB ─────────────────────────────────
-- rubo4e v202607 field name is `marktlokationsId` (camelCase in JSON).
UPDATE invoic_receipts
SET malo_id = rechnung->>'marktlokationsId'
WHERE malo_id IS NULL
  AND rechnung ? 'marktlokationsId';

-- ── (c) Index for zahlungsstatus and payment-lifecycle queries ─────────────────
CREATE INDEX IF NOT EXISTS invoic_receipts_malo_id_tenant
    ON invoic_receipts (tenant, malo_id)
    WHERE malo_id IS NOT NULL;

-- ── (d) Extend the outcome CHECK constraint ────────────────────────────────────
-- PostgreSQL requires DROP + ADD to change a CHECK constraint.
ALTER TABLE invoic_receipts
    DROP CONSTRAINT IF EXISTS invoic_receipts_outcome_check;

ALTER TABLE invoic_receipts
    ADD CONSTRAINT invoic_receipts_outcome_check CHECK (outcome IN (
        'Ok',              -- accepted; REMADV 33001 dispatched
        'AcceptedPartial', -- accepted with remarks
        'Warn',            -- validation warning; auto-approved
        'Dispute',         -- blocked; REMADV 33002 dispatched
        'Resolved',        -- dispute resolved by operator (POST /resolve-dispute)
        'Dispatched',      -- outbound 31006 sent; awaiting NB REMADV
        'Paid'             -- outbound 31006 settled
    ));

-- 0002_tenant_correction_rechnungsart.sql
--
-- (a) Add `tenant` column to `invoice_drafts` for multi-tenancy enforcement.
--     Back-fill existing rows with 'default'.
--
-- (b) Add `rechnungsart` for first-class Storno/Korrektur queries without JSONB scan.
--
-- (c) Add `original_draft_id` FK for audit trail of correction chains.
--
-- (d) Add 'paid' to the `status` CHECK constraint (ERP payment confirmation).
--
-- (e) Index on tenant + malo_id for billing history lookups.

-- ── (a) tenant column ─────────────────────────────────────────────────────────

ALTER TABLE invoice_drafts
    ADD COLUMN IF NOT EXISTS tenant TEXT NOT NULL DEFAULT 'default';

-- Back-fill existing rows
UPDATE invoice_drafts SET tenant = 'default' WHERE tenant IS NULL OR tenant = '';

-- ── (b) rechnungsart column ───────────────────────────────────────────────────
-- Values: 'RECHNUNG' | 'STORNORECHNUNG' | 'KORREKTURRECHNUNG'
-- Default 'RECHNUNG' for all existing rows (they are standard invoices).

ALTER TABLE invoice_drafts
    ADD COLUMN IF NOT EXISTS rechnungsart TEXT NOT NULL DEFAULT 'RECHNUNG'
    CHECK (rechnungsart IN ('RECHNUNG', 'STORNORECHNUNG', 'KORREKTURRECHNUNG'));

UPDATE invoice_drafts SET rechnungsart = 'RECHNUNG' WHERE rechnungsart IS NULL;

-- ── (c) original_draft_id FK ──────────────────────────────────────────────────

ALTER TABLE invoice_drafts
    ADD COLUMN IF NOT EXISTS original_draft_id UUID
    REFERENCES invoice_drafts(id) ON DELETE SET NULL;

-- ── (d) Extend status CHECK constraint to include 'paid' ─────────────────────

ALTER TABLE invoice_drafts
    DROP CONSTRAINT IF EXISTS invoice_drafts_status_check;

ALTER TABLE invoice_drafts
    ADD CONSTRAINT invoice_drafts_status_check
    CHECK (status IN ('draft', 'dispatched', 'rejected', 'paid'));

-- ── (e) Indexes ───────────────────────────────────────────────────────────────

CREATE INDEX IF NOT EXISTS id_tenant_malo
    ON invoice_drafts (tenant, malo_id, created_at DESC);

CREATE INDEX IF NOT EXISTS id_tenant_status
    ON invoice_drafts (tenant, status, pid);

CREATE INDEX IF NOT EXISTS id_rechnungsart
    ON invoice_drafts (rechnungsart)
    WHERE rechnungsart IN ('STORNORECHNUNG', 'KORREKTURRECHNUNG');

CREATE INDEX IF NOT EXISTS id_original_draft
    ON invoice_drafts (original_draft_id)
    WHERE original_draft_id IS NOT NULL;

-- ── Add 'paid' status to kostenblatt_records too ─────────────────────────────
-- (BKV/ÜNB confirms Kostenblatt payment)

ALTER TABLE kostenblatt_records
    DROP CONSTRAINT IF EXISTS kostenblatt_records_status_check;

ALTER TABLE kostenblatt_records
    ADD CONSTRAINT kostenblatt_records_status_check
    CHECK (status IN ('pending', 'submitted', 'confirmed', 'disputed', 'paid'));

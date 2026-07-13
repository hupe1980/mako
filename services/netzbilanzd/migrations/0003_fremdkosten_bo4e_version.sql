-- 0003_fremdkosten_bo4e_version.sql
--
-- (a) Create `fremdkosten_records` table — typed BO4E Fremdkosten/FremdkostenBlock/
--     FremdkostenPosition pass-through linked to invoice_drafts (§22 MessZV external
--     cost records).  Was referenced in pg.rs but never created in a migration.
--
-- (b) Add `bo4e_version` to `invoice_drafts` for schema-version traceability.
--     Pattern consistent with all other services (marktd, invoicd, einsd).
--
-- (c) Add partial unique index on invoice_drafts to prevent double-billing.
--     Only RECHNUNG drafts are constrained — Stornorechnung/Korrekturrechnung are
--     explicitly allowed to reference the same period.

-- ── (a) fremdkosten_records ───────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS fremdkosten_records (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant              TEXT        NOT NULL DEFAULT 'default',

    -- FK to the INVOIC draft this Fremdkosten block augments.
    draft_id            UUID        NOT NULL
                        REFERENCES invoice_drafts(id) ON DELETE CASCADE,

    -- Full rubo4e::current::Fremdkosten JSONB (includes FremdkostenBlock list).
    -- _typ is auto-injected as "FREMDKOSTEN" by the API if absent.
    fremdkosten_json    JSONB       NOT NULL,

    -- Optional human-readable label (e.g. "ÜNB Ausgleichsenergie").
    bezeichnung         TEXT,

    -- Pre-computed total (sum of all FremdkostenPosition.betrag.wert).
    -- Used for invoic-checker validation; stored as NUMERIC for precision.
    total_eur           NUMERIC(16, 5) NOT NULL DEFAULT 0,

    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- One Fremdkosten record per draft per tenant.
    -- Multiple FremdkostenBlock entries are embedded inside the JSONB.
    UNIQUE (tenant, draft_id)
);

CREATE INDEX IF NOT EXISTS fk_draft_id  ON fremdkosten_records (draft_id);
CREATE INDEX IF NOT EXISTS fk_tenant    ON fremdkosten_records (tenant, created_at DESC);

COMMENT ON TABLE fremdkosten_records IS
    '§22 MessZV typed external-cost pass-through. '
    'One record per invoice draft; multiple cost blocks embedded in fremdkosten_json.';

-- ── (b) bo4e_version ─────────────────────────────────────────────────────────

ALTER TABLE invoice_drafts
    ADD COLUMN IF NOT EXISTS bo4e_version TEXT NOT NULL DEFAULT 'v202607.0.0';

-- ── (c) Partial unique index — prevent double-billing RECHNUNG ────────────────
--
-- A single MaLo must not be billed twice for the same period and PID in one
-- tenant.  Correction types (STORNORECHNUNG, KORREKTURRECHNUNG) are excluded
-- — they deliberately reference the same period as the original.
--
-- This index catches operator errors (submitting the same billing run twice)
-- without blocking legitimate Stornorechnung + re-billing workflows.

CREATE UNIQUE INDEX IF NOT EXISTS id_no_double_billing
    ON invoice_drafts (tenant, malo_id, period_from, period_to, pid)
    WHERE rechnungsart = 'RECHNUNG' AND status != 'rejected';

COMMENT ON INDEX id_no_double_billing IS
    'Prevents duplicate NNE/MMM billing for the same MaLo, period, and PID. '
    'Stornorechnung/Korrekturrechnung are excluded (rechnungsart != RECHNUNG). '
    'Rejected drafts are excluded to allow retry after operator rejection.';

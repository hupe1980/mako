-- 0002_improvements.sql
-- Correctness & performance improvements:
--   1. Add updated_at to contracts (was tracking create time only).
--   2. GIN indexes on JSONB data columns for future JSON-path queries.
--   3. Partial index on process_correlation to speed up active-process lookups.

-- ── contracts: add updated_at ──────────────────────────────────────────────

ALTER TABLE contracts
    ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ NOT NULL DEFAULT now();

-- Back-fill existing rows from created_at.
UPDATE contracts SET updated_at = created_at WHERE updated_at = now();

-- ── GIN indexes on JSONB data columns ─────────────────────────────────────

-- Enables fast JSON-path queries, e.g. for bilanzierungsgebiet filtering.
CREATE INDEX IF NOT EXISTS malo_data_gin      ON malo      USING GIN (data jsonb_path_ops);
CREATE INDEX IF NOT EXISTS melo_data_gin      ON melo      USING GIN (data jsonb_path_ops);
CREATE INDEX IF NOT EXISTS contracts_data_gin ON contracts  USING GIN (data jsonb_path_ops);
CREATE INDEX IF NOT EXISTS partners_channels_gin ON partners USING GIN (channels jsonb_path_ops);

-- ── process_correlation: partial index for running processes ───────────────

CREATE INDEX IF NOT EXISTS process_correlation_running
    ON process_correlation (malo_id, initiated_at)
    WHERE status = 'RUNNING';

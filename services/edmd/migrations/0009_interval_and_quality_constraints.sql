-- Migration 0009: Interval validity and quality constraints
--
-- F-11: Add CHECK (dtm_from < dtm_to) to prevent zero-duration or negative-
-- duration intervals from being stored. V06 (InconsistentIntervalLength) catches
-- this at the application layer, but a database-level constraint is the safety net
-- for direct SQL inserts or bypassed validation paths.
--
-- F-02 follow-up: Enforce quality CHECK to prevent future source values that
-- are not in the controlled vocabulary used by `AND quality NOT IN ('FAULTY','UNKNOWN')`.

-- ── Interval validity constraint ──────────────────────────────────────────────

DO $$
BEGIN
    ALTER TABLE meter_reads
        ADD CONSTRAINT mr_valid_interval CHECK (dtm_from < dtm_to);
EXCEPTION WHEN duplicate_object THEN NULL;
END;
$$;

COMMENT ON CONSTRAINT mr_valid_interval ON meter_reads IS
    'Ensures every interval has a positive duration. '
    'Zero-duration intervals (dtm_from = dtm_to) are invalid per §17 MessZV. '
    'Application layer: V06 InconsistentIntervalLength.';

-- ── Quality value constraint ──────────────────────────────────────────────────
-- Enumerate all allowed quality values so free-text pollution cannot enter the table.

DO $$
BEGIN
    ALTER TABLE meter_reads
        ADD CONSTRAINT mr_quality_check CHECK (
            quality IN (
                'MEASURED',
                'ESTIMATED',
                'SUBSTITUTED',
                'CALCULATED',
                'CORRECTED',
                'PRELIMINARY',
                'FAULTY',
                'UNKNOWN'
            )
        );
EXCEPTION WHEN duplicate_object THEN NULL;
END;
$$;

COMMENT ON CONSTRAINT mr_quality_check ON meter_reads IS
    'Restricts quality to the 8 typed variants of QualityFlag (mako-edm::domain::QualityFlag). '
    'FAULTY and UNKNOWN are excluded from billing aggregations per §22 MessZV.';

-- ── Allocation version constraint (redundant hardening) ───────────────────────
-- migration 0007 already adds this as part of the ALTER TABLE; this is a no-op guard.

DO $$
BEGIN
    ALTER TABLE meter_reads
        ADD CONSTRAINT mr_allocation_version_check CHECK (
            allocation_version IN ('INITIAL', 'CORRECTION', 'FINAL')
        );
EXCEPTION WHEN duplicate_object THEN NULL;
END;
$$;

COMMENT ON CONSTRAINT mr_allocation_version_check ON meter_reads IS
    'Restricts allocation_version to INITIAL / CORRECTION / FINAL per BK6-22-024 §6.4 '
    '(MaBiS AllocationVersion). Added as defensive hardening alongside migration 0007.';

-- ── Partial index: billable intervals (fast aggregation hot path) ─────────────

CREATE INDEX IF NOT EXISTS mr_billable
    ON meter_reads (malo_id, dtm_from, dtm_to)
    WHERE quality NOT IN ('FAULTY', 'UNKNOWN');

COMMENT ON INDEX mr_billable IS
    'Partial index covering only billable intervals (quality NOT IN (FAULTY, UNKNOWN)). '
    'Used by billing period aggregation queries to avoid full-table-scan on quality filter. '
    '§22 MessZV: billed data must be validated (Faulty/Unknown excluded from Rechnung).';

-- ── F-17: GDPR §17 DSGVO erasure tracking ────────────────────────────────────
--
-- Records right-to-erasure requests per MaLo. Hot storage (meter_reads) is
-- anonymized immediately. Cold Iceberg Parquet data requires an async rewrite
-- job triggered by `archive_deletion_pending = true`.
--
-- Idempotent on (malo_id, tenant) — re-submitting an erasure request updates
-- the reason and sets archive_deletion_pending = true again.

CREATE TABLE IF NOT EXISTS gdpr_deletions (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id                 TEXT        NOT NULL,
    tenant                  TEXT        NOT NULL,
    reason                  TEXT        NOT NULL,
    authorized_by           TEXT        NOT NULL,
    requested_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    hot_deletion_completed_at TIMESTAMPTZ,
    archive_deletion_pending  BOOLEAN   NOT NULL DEFAULT true,
    archive_deletion_completed_at TIMESTAMPTZ,

    CONSTRAINT gdpr_unique_malo_tenant UNIQUE (malo_id, tenant)
);

CREATE INDEX IF NOT EXISTS gd_archive_pending
    ON gdpr_deletions (archive_deletion_pending)
    WHERE archive_deletion_pending = true;

COMMENT ON TABLE gdpr_deletions IS
    'GDPR Art. 17 right-to-erasure audit log per MaLo. '
    'Hot storage (meter_reads) is anonymized immediately on erasure request. '
    'Cold Iceberg Parquet deletion is scheduled via archive rewrite pipeline '
    '(archive_deletion_pending = true until completed).';

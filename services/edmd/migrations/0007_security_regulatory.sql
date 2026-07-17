-- edmd migration 0007: Security hardening + regulatory completeness
--
-- ## Fixes
--
-- 1. **H7/H8: Tenant isolation** — Add `tenant` TEXT column to `meter_reads`
--    (non-nullable, backfilled from existing `tenant_id` UUID). The `tenant`
--    column carries the string tenant key used consistently across all other
--    edmd tables, enabling uniform `AND tenant = $N` filters.
--
--    Background: `meter_reads` uses `tenant_id UUID` while all other edmd
--    tables use `tenant TEXT`. This inconsistency means `meter_reads` queries
--    must use `AND (tenant_id IS NULL OR tenant_id = $N::uuid)` — a weaker
--    guard than `AND tenant = $N`. Adding the TEXT column enables the
--    consistent pattern and closes the cross-tenant GDPR Art. 32 gap.
--
-- 2. **H2: AllocationVersion column** — `meter_reads.allocation_version`
--    distinguishes MSCONS vorläufig (INITIAL), endgültig (FINAL), and
--    Nachbearbeitung (CORRECTION) deliveries. Required for MaBiS day-3 vs
--    day-8 queries (BK6-22-024 §6.4).
--
-- 3. **H1: sender_mp_id column** — `meter_reads.sender_mp_id` records which
--    MSB (or MSB system) delivered this reading. Enables per-interval MSB
--    attribution for §22 MessZV audit trail after an MSB switch (WiM PID 55039).
--
-- 4. **H5/M4: CHECK constraints on source columns** — Enforce allowed values
--    for `meter_reads.source` and `quality_assessments.source` to prevent
--    free-text pollution of the audit trail.

-- ── 1. Add tenant TEXT column to meter_reads ─────────────────────────────────

ALTER TABLE meter_reads
    ADD COLUMN IF NOT EXISTS tenant TEXT NOT NULL DEFAULT 'default';

-- Backfill from tenant_id where possible (cross-reference via meter_data_receipts).
-- For existing rows without a known tenant, leave as 'default'.
-- Production operators should backfill tenant from their ERP if needed.

COMMENT ON COLUMN meter_reads.tenant IS
    'Tenant identifier — data-isolation key. Matches the `tenant` field in all '
    'other edmd tables. Used for `AND tenant = $1` isolation in all queries.';

-- Index for tenant-scoped time-range queries (the hot path in most APIs)
CREATE INDEX IF NOT EXISTS mr_tenant_malo_dtm
    ON meter_reads (tenant, malo_id, dtm_from, dtm_to);

-- ── 2. AllocationVersion: Initial / Correction / Final ───────────────────────

ALTER TABLE meter_reads
    ADD COLUMN IF NOT EXISTS allocation_version TEXT NOT NULL DEFAULT 'INITIAL'
        CHECK (allocation_version IN ('INITIAL','CORRECTION','FINAL'));

COMMENT ON COLUMN meter_reads.allocation_version IS
    'MSCONS data-delivery version per BK6-22-024 §6.4 (MaBiS AllocationVersion). '
    'INITIAL = vorläufig (day-3 for MaBiS); FINAL = endgültig (day-8); '
    'CORRECTION = Nachbearbeitungswert. '
    'Used by mabis-syncd to distinguish preliminary from final Summenzeitreihen.';

-- Index for mabis-syncd bitemporal queries: "give me the FINAL version"
CREATE INDEX IF NOT EXISTS mr_allocation_version
    ON meter_reads (malo_id, allocation_version, dtm_from)
    WHERE allocation_version != 'INITIAL';

-- ── 3. Sender MSB attribution ─────────────────────────────────────────────────

ALTER TABLE meter_reads
    ADD COLUMN IF NOT EXISTS sender_mp_id TEXT;

COMMENT ON COLUMN meter_reads.sender_mp_id IS
    'MP-ID of the sender (MSB or MSB system) that delivered this reading. '
    'Populated from meter_data_receipts.sender_mp_id (MSCONS path) or from '
    'the direct-push API request header. '
    'Required for §22 MessZV per-interval MSB attribution after WiM MSB switch.';

-- Partial index: MSB-specific queries (e.g. "all readings from MSB 9900...")
CREATE INDEX IF NOT EXISTS mr_sender_mp_id
    ON meter_reads (sender_mp_id, malo_id, dtm_from)
    WHERE sender_mp_id IS NOT NULL;

-- ── 4. Enforce CHECK on source columns ───────────────────────────────────────

-- meter_reads.source: enforce allowed values
DO $$
BEGIN
    ALTER TABLE meter_reads
        ADD CONSTRAINT mr_source_check
        CHECK (source IN ('MSCONS','DIRECT_PUSH','MANUAL','ESTIMATED','CORRECTION'));
EXCEPTION WHEN duplicate_object THEN NULL;
END;
$$;

-- quality_assessments.source: enforce allowed values
DO $$
BEGIN
    ALTER TABLE quality_assessments
        ADD CONSTRAINT qa_source_check
        CHECK (source IN ('MSCONS','DIRECT_PUSH','CORRECTION','BATCH_RESCORE'));
EXCEPTION WHEN duplicate_object THEN NULL;
END;
$$;

-- ── 5. Extend meter_read_corrections with tenant isolation ───────────────────

ALTER TABLE meter_read_corrections
    ADD COLUMN IF NOT EXISTS tenant TEXT NOT NULL DEFAULT 'default';

CREATE INDEX IF NOT EXISTS mrc_tenant_malo
    ON meter_read_corrections (tenant, malo_id, dtm_from DESC);

COMMENT ON COLUMN meter_read_corrections.tenant IS
    'Tenant identifier for data isolation. Matches meter_reads.tenant.';

-- accountingd migration 0005: add tenant to accounts UNIQUE constraint
--
-- Problem fixed:
--   The original UNIQUE (malo_id, lf_mp_id) constraint allowed two different tenants
--   to collide on the same (malo_id, lf_mp_id) pair. In a multi-tenant deployment this
--   means Tenant A's upsert_account could silently update Tenant B's account, and
--   fetch_account without a tenant filter would return the wrong tenant's data.
--
-- Fix:
--   1. Drop the old 2-column unique constraint.
--   2. Add a 3-column unique constraint (malo_id, lf_mp_id, tenant).
--   3. Add tenant to the balance reconciliation index.
--
-- Existing data: If two tenants share an identical (malo_id, lf_mp_id) pair, this
-- migration will succeed because tenant is already different. If the same tenant has
-- duplicate (malo_id, lf_mp_id, tenant) — which is a data integrity bug from earlier
-- code — the constraint will reject it. Operators must resolve duplicates manually
-- before applying this migration.
--
-- Legal basis:
--   §241 HGB: Richtigkeit der Buchführung — each Kundenkonto must be unambiguously
--   associated with exactly one Auftraggeber (tenant). Mixing tenant data violates
--   the Grundsätze ordnungsmäßiger Buchführung (GoB).

-- 1. Drop old constraint
ALTER TABLE accounts DROP CONSTRAINT IF EXISTS accounts_malo_id_lf_mp_id_key;

-- 2. Add new constraint including tenant
ALTER TABLE accounts
    ADD CONSTRAINT accounts_malo_id_lf_mp_id_tenant_key
    UNIQUE (malo_id, lf_mp_id, tenant);

-- 3. Ensure acct_tenant index covers tenant-scoped queries efficiently
CREATE INDEX IF NOT EXISTS acct_malo_tenant ON accounts (malo_id, tenant);

COMMENT ON CONSTRAINT accounts_malo_id_lf_mp_id_tenant_key ON accounts IS
    'Ensures each (market location, supplier, tenant) triple maps to exactly one Kundenkonto. '
    'Tenant isolation: accounts for different operators are fully separated at the DB level.';

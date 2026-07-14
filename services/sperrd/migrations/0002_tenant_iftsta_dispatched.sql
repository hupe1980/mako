-- 0002_tenant_iftsta_dispatched.sql
-- Multi-tenant isolation: add tenant column for row-level isolation.
-- IFTSTA dispatch tracking: add iftsta_dispatched_at for SLA reporting.

ALTER TABLE sperr_orders
    ADD COLUMN IF NOT EXISTS tenant TEXT NOT NULL DEFAULT '';

-- Backfill note: operators must manually set tenant = '<their_BDEW_code>'
-- for existing rows, e.g.:
--   UPDATE sperr_orders SET tenant = '9900357000004' WHERE tenant = '';

CREATE INDEX IF NOT EXISTS so_tenant_status
    ON sperr_orders (tenant, status);

-- Track exact IFTSTA dispatch time for SLA compliance (BK6-22-024 execution window).
ALTER TABLE sperr_orders
    ADD COLUMN IF NOT EXISTS iftsta_dispatched_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS so_executed_no_iftsta
    ON sperr_orders (id)
    WHERE status = 'executed' AND iftsta_dispatched_at IS NULL;

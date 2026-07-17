-- ── sperrd schema ─────────────────────────────────────────────────────────────
--
-- Tracks remote disconnection / reconnection execution orders.
--
-- Lifecycle:
--   pending → executed  (field confirmation → auto-dispatches IFTSTA 21039)
--   pending → failed    (field failure → operator escalation)
--   pending → cancelled (operator cancels before execution)
--
-- Source: GPKE BK6-22-024; BDEW ORDERS AHB PIDs 17115–17117.

CREATE TABLE sperr_orders (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id             TEXT        NOT NULL,
    lf_mp_id            TEXT        NOT NULL,
    tenant              TEXT        NOT NULL,
    -- 'sperrung' | 'entsperrung'
    order_type          TEXT        NOT NULL
                        CHECK (order_type IN ('sperrung', 'entsperrung')),
    -- makod ORDERS process UUID
    process_id          TEXT,
    planned_date        DATE,
    status              TEXT        NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending', 'executed', 'failed', 'cancelled')),
    executed_at         TIMESTAMPTZ,
    -- Field service confirmation note
    execution_note      TEXT,
    fail_reason         TEXT,
    -- dispatched IFTSTA 21039 makod command UUID
    iftsta_ref          TEXT,
    -- Exact IFTSTA dispatch time for SLA reporting (BK6-22-024 execution window)
    iftsta_dispatched_at TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE sperr_orders IS
    'Sperrung / Entsperrung execution lifecycle per GPKE BK6-22-024. '
    'IFTSTA 21039 is auto-dispatched when status transitions to executed.';

COMMENT ON COLUMN sperr_orders.iftsta_dispatched_at IS
    'Exact time the IFTSTA 21039 command was dispatched to makod. '
    'NULL = not yet dispatched (execution confirmed but IFTSTA pending). '
    'Used for SLA compliance tracking (BK6-22-024 §3.4 execution window).';

CREATE INDEX so_tenant_status  ON sperr_orders (tenant, status);
CREATE INDEX so_malo_status    ON sperr_orders (malo_id, status);
CREATE INDEX so_pending        ON sperr_orders (planned_date)
    WHERE status = 'pending';
CREATE INDEX so_executed_no_iftsta ON sperr_orders (id)
    WHERE status = 'executed' AND iftsta_dispatched_at IS NULL;

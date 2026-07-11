-- ── sperrd schema ─────────────────────────────────────────────────────────────
--
-- `sperr_orders`: tracks remote disconnection/reconnection execution orders.
--
-- Lifecycle: pending → executed (field confirmation → auto-dispatch IFTSTA 21039)
--            pending → failed   (field failure → operator escalation)
--            pending → cancelled (operator cancels before execution)
--
-- Source: GPKE BK6-22-024; BDEW ORDERS AHB PIDs 17115-17117.

CREATE TABLE IF NOT EXISTS sperr_orders (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id          TEXT        NOT NULL,           -- 11-digit MaLo-ID
    lf_mp_id         TEXT        NOT NULL,           -- requesting Lieferant
    order_type       TEXT        NOT NULL            -- 'sperrung' | 'entsperrung'
                     CHECK (order_type IN ('sperrung', 'entsperrung')),
    process_id       TEXT,                           -- makod ORDERS process UUID
    planned_date     DATE,                           -- planned execution date
    status           TEXT        NOT NULL DEFAULT 'pending'
                     CHECK (status IN ('pending', 'executed', 'failed', 'cancelled')),
    executed_at      TIMESTAMPTZ,                    -- actual execution time
    execution_note   TEXT,                           -- field-service note
    fail_reason      TEXT,                           -- failure description
    iftsta_ref       TEXT,                           -- dispatched IFTSTA 21039 command ID
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS so_malo_status  ON sperr_orders (malo_id, status);
CREATE INDEX IF NOT EXISTS so_status       ON sperr_orders (status);
CREATE INDEX IF NOT EXISTS so_pending      ON sperr_orders (planned_date) WHERE status = 'pending';

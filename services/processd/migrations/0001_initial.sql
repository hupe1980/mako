-- 0001_initial.sql — processd initial schema
--
-- Two tables:
--
--   approval_queue         — LF module: events that need ERP operator decision.
--   anmeldung_decisions    — NB module: STP decision audit log (Accept/Reject/Escalate).
--
-- Both tables enforce idempotency on (process_id, tenant) — duplicate events
-- from the at-least-once EventBus are silently ignored (ON CONFLICT DO NOTHING).

-- ── LF approval queue ─────────────────────────────────────────────────────────
--
-- One row per process that `processd` could not auto-decide.  Used by the LF
-- module for E_0624 events where the MaLo is not in the tenant's master data.
--
-- status values: Pending | Approved | Rejected | Expired
--
-- Background expiry task runs every 60 s and sets status = Expired for rows
-- where expires_at < now() AND status = Pending.

CREATE TABLE IF NOT EXISTS approval_queue (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    process_id  UUID        NOT NULL,
    pid         INTEGER    NOT NULL,
    malo_id     TEXT,                                    -- resolved from event payload if available
    reason      TEXT        NOT NULL,                    -- why this was not auto-decided
    status      TEXT        NOT NULL
                CHECK (status IN ('Pending','Approved','Rejected','Expired')),
    expires_at  TIMESTAMPTZ NOT NULL,                    -- deadline minus safety margin (5 min)
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    decided_at  TIMESTAMPTZ,
    tenant      TEXT        NOT NULL,

    UNIQUE (process_id, tenant)
);

CREATE INDEX IF NOT EXISTS approval_queue_tenant_status
    ON approval_queue (tenant, status, expires_at);

CREATE INDEX IF NOT EXISTS approval_queue_process_id
    ON approval_queue (process_id);

-- ── NB anmeldung decisions ────────────────────────────────────────────────────
--
-- One row per Anmeldung (PID 55001/55016/44001) that `processd` processed.
-- Written before `MakodClient::post_command` is called (at-least-once: retry
-- is safe because ON CONFLICT DO NOTHING).
--
-- decision values: Accept | Reject | Escalate
--
-- initiator_is_affiliate = true when lf_mp_id == own_gln (§6b EnWG integrated
-- deployment).  Used by `obsd` §20 EnWG parity reports.

CREATE TABLE IF NOT EXISTS anmeldung_decisions (
    id                     UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    process_id             UUID        NOT NULL,
    pid                    INTEGER    NOT NULL,
    malo_id                TEXT        NOT NULL,
    lf_mp_id                 TEXT        NOT NULL,
    decision               TEXT        NOT NULL
                           CHECK (decision IN ('Accept','Reject','Escalate')),
    erc_code               TEXT,                         -- e.g. 'A06' when decision = Reject
    detail                 TEXT,                         -- human-readable for BNetzA audit log
    initiator_is_affiliate BOOLEAN     NOT NULL DEFAULT false,
    decided_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    tenant                 TEXT        NOT NULL,

    UNIQUE (process_id, tenant)
);

-- STP rate KPI query: count(Accept) / count(Accept + Reject)
CREATE INDEX IF NOT EXISTS anmeldung_decisions_decided_at
    ON anmeldung_decisions (tenant, decided_at DESC);

-- BNetzA §20 parity report: all affiliate-initiated decisions
CREATE INDEX IF NOT EXISTS anmeldung_decisions_affiliate
    ON anmeldung_decisions (tenant, initiator_is_affiliate, decided_at DESC)
    WHERE initiator_is_affiliate = true;

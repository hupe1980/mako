-- processd schema — Process Decision Engine
--
-- `approval_queue`: LF module — events requiring ERP operator decision (E_0624 etc.).
-- `anmeldung_decisions`: NB module — Anmeldung STP audit log (Accept/Reject/Escalate).
--
-- Both tables use (process_id, tenant) as idempotency key;
-- duplicate CloudEvents from the at-least-once EventBus are silently ignored.
--
-- Regulatory: §20 Abs. 1 S. 3 EnWG parity (initiator_is_affiliate).

-- ── LF approval queue ─────────────────────────────────────────────────────────

CREATE TABLE approval_queue (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    process_id  UUID        NOT NULL,
    pid         INTEGER     NOT NULL,
    malo_id     TEXT,
    reason      TEXT        NOT NULL,
    status      TEXT        NOT NULL CHECK (status IN ('Pending','Approved','Rejected','Expired')),
    expires_at  TIMESTAMPTZ NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    decided_at  TIMESTAMPTZ,
    tenant      TEXT        NOT NULL,
    UNIQUE (process_id, tenant)
);

COMMENT ON TABLE approval_queue IS
    'LF module: events that could not be auto-decided. '
    'One row per process awaiting ERP operator decision. '
    'Background worker expires Pending rows past expires_at.';

CREATE INDEX aq_tenant_status ON approval_queue (tenant, status, expires_at);
CREATE INDEX aq_process_id    ON approval_queue (process_id);

-- ── NB Anmeldung decisions ────────────────────────────────────────────────────

CREATE TABLE anmeldung_decisions (
    id                     UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    process_id             UUID        NOT NULL,
    pid                    INTEGER     NOT NULL,
    malo_id                TEXT        NOT NULL,
    lf_mp_id               TEXT        NOT NULL,
    decision               TEXT        NOT NULL CHECK (decision IN ('Accept','Reject','Escalate')),
    erc_code               TEXT,
    detail                 TEXT,
    -- §20 EnWG parity: TRUE when lf_mp_id = operator's own GLN
    initiator_is_affiliate BOOLEAN     NOT NULL DEFAULT false,
    decided_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    tenant                 TEXT        NOT NULL,
    UNIQUE (process_id, tenant)
);

COMMENT ON TABLE anmeldung_decisions IS
    'NB STP decision audit log. Written before POST /api/v1/commands to makod. '
    'decision=Escalate when netz-checker requires operator review (iMSys/SR/no-Zaehler). '
    'Used for BNetzA §20 EnWG parity reports and STP rate KPI.';

-- STP rate KPI
CREATE INDEX ad_tenant_decided ON anmeldung_decisions (tenant, decided_at DESC);
-- §20 parity report
CREATE INDEX ad_affiliate      ON anmeldung_decisions (tenant, initiator_is_affiliate, decided_at DESC)
    WHERE initiator_is_affiliate = true;

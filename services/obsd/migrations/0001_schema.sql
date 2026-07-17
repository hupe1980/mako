-- ── obsd schema — Business-Process Observability ─────────────────────────────
--
-- Read-model for MaKo process projections. One row per business process,
-- updated on every de.mako.* CloudEvent received from marktd.
--
-- Regulatory: §20 Abs. 1 S. 3 EnWG (non-discrimination parity evidence).
-- BNetzA audit export: GET /api/v1/audit/bnetza-report.

CREATE TABLE process_projections (
    process_id          UUID        PRIMARY KEY,
    pid                 INTEGER     NOT NULL,
    family              TEXT        NOT NULL DEFAULT '',
    workflow_name       TEXT        NOT NULL DEFAULT '',
    state               TEXT        NOT NULL DEFAULT 'initiated',
    malo_id             TEXT,
    partner_mp_id       TEXT,
    -- Market role of the counterparty (LF | NB | MSB | BKV | ÜNB)
    mdm_role            TEXT,
    -- Regulatory deadline timestamp (CET/CEST); NULL = no deadline registered
    deadline_at         TIMESTAMPTZ,
    -- 'green' | 'amber' | 'red'  (computed by obsd deadline worker)
    deadline_risk       TEXT        NOT NULL DEFAULT 'green',
    -- §20 EnWG parity: TRUE when initiating LF MP-ID = operator's own MP-ID
    -- (vertically integrated utility). Used in /audit/bnetza-report.
    initiator_is_affiliate BOOLEAN  NOT NULL DEFAULT false,
    started_at          TIMESTAMPTZ NOT NULL,
    last_event_at       TIMESTAMPTZ NOT NULL,
    -- Terminal state timestamp for cycle-time KPI reporting
    completed_at        TIMESTAMPTZ,
    erc_code            TEXT,
    tenant              TEXT        NOT NULL,
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE process_projections IS
    'Process-level read-model for KPI, deadline, and §20 EnWG parity reporting. '
    'Updated on each de.mako.* CloudEvent. One row per business process.';

COMMENT ON COLUMN process_projections.initiator_is_affiliate IS
    '§20 EnWG Diskriminierungsfreiheitspflicht: TRUE when the process was initiated '
    'by the operator''s own Lieferant MP-ID (vertically integrated utility). '
    'Enables structured parity audit evidence for BNetzA.';

COMMENT ON COLUMN process_projections.completed_at IS
    'Set when state reaches completed / rejected / cancelled. '
    'Used for cycle-time computation in KPI reports.';

CREATE INDEX pp_tenant       ON process_projections (tenant, state);
CREATE INDEX pp_pid_state    ON process_projections (pid, state);
CREATE INDEX pp_malo_id      ON process_projections (malo_id)
    WHERE malo_id IS NOT NULL;
CREATE INDEX pp_partner      ON process_projections (partner_mp_id)
    WHERE partner_mp_id IS NOT NULL;
CREATE INDEX pp_deadline     ON process_projections (deadline_at)
    WHERE deadline_at IS NOT NULL;
CREATE INDEX pp_started      ON process_projections (started_at DESC);
CREATE INDEX pp_completed_at ON process_projections (completed_at)
    WHERE completed_at IS NOT NULL;
-- §20 parity report: all affiliate-initiated Anmeldungen in a period
CREATE INDEX pp_affiliate    ON process_projections (tenant, initiator_is_affiliate, started_at DESC)
    WHERE initiator_is_affiliate = true;

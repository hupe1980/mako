-- ── obsd schema — Business-Process Observability ─────────────────────────────
--
-- Read-model for MaKo process projections. One row per business process,
-- updated on every de.mako.* CloudEvent received from marktd.
--
-- Regulatory: the affiliate/third-party columns produce evidence for the
-- Gleichbehandlungsbericht the Gleichbehandlungsbeauftragte of a vertically
-- integrated undertaking files by 31 March each year (§ 7a Abs. 5 EnWG), over
-- the preceding calendar year. Lieferantenwechsel is one of the areas those
-- reports examine. The underlying duties are § 6a EnWG (informatorische
-- Entflechtung) and § 20 Abs. 1 Satz 1 EnWG (diskriminierungsfreier
-- Netzzugang). Export: GET /api/v1/audit/gleichbehandlung.

CREATE TABLE process_projections (
    process_id          UUID        PRIMARY KEY,
    pid                 INTEGER     NOT NULL,
    family              TEXT        NOT NULL DEFAULT '',
    workflow_name       TEXT        NOT NULL DEFAULT '',
    -- 'initiated' | 'running' | 'aperak_timeout' | 'completed' | 'rejected' | 'failed'
    -- Written by ProcessState::as_str; `terminal_state_sql_matches_domain`
    -- pins the SQL literals against the enum.
    state               TEXT        NOT NULL DEFAULT 'initiated',
    malo_id             TEXT,
    partner_mp_id       TEXT,
    -- Market role of the counterparty (LF | NB | MSB | BKV | ÜNB)
    mdm_role            TEXT,
    -- The BUSINESS Antwortfrist from mako_fristen::antwort — NOT the APERAK
    -- clock, which is minutes and arrives as its own de.mako.aperak.timeout
    -- event. NULL means no Festlegung this codebase has read quantifies the
    -- window for this PID: unknown, never unbounded. Such a process is
    -- deliberately absent from every breach sweep rather than measured against
    -- an instant nobody can cite.
    deadline_at         TIMESTAMPTZ,
    -- Fundstelle for deadline_at, so an alert and a regulator's question can
    -- both name the Festlegung instead of asserting a number.
    deadline_source     TEXT,
    -- 'unknown' | 'green' | 'amber' | 'red'. 'unknown' when deadline_at IS NULL
    -- — an unread Festlegung must not read as healthy.
    deadline_risk       TEXT        NOT NULL DEFAULT 'unknown',
    -- Set by the deadline sweep worker when it has emitted
    -- de.obs.deadline.approaching for this process — makes the alert idempotent
    -- (emit once per process entering the approaching window).
    deadline_alerted_at TIMESTAMPTZ,
    -- § 7a Abs. 5 EnWG parity: TRUE when the initiating Lieferant belongs to the
    -- same vertically integrated undertaking as this operator.
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
    'Process-level read-model for KPI, Antwortfrist and § 7a Abs. 5 EnWG parity '
    'reporting. Updated on each de.mako.* CloudEvent. One row per business process.';

COMMENT ON COLUMN process_projections.deadline_at IS
    'Business Antwortfrist (mako_fristen::antwort). NULL = no published window for '
    'this PID — unknown, never unbounded. Distinct from the APERAK acknowledgement '
    'clock, which is reported as its own event and lands in state = aperak_timeout.';

COMMENT ON COLUMN process_projections.initiator_is_affiliate IS
    '§ 7a Abs. 5 EnWG Gleichbehandlung: TRUE when the process was initiated by a '
    'Lieferant inside the operator''s own vertically integrated undertaking. '
    'Grouping key of the annual Gleichbehandlungsbericht evidence.';

COMMENT ON COLUMN process_projections.completed_at IS
    'Set when state reaches completed / rejected / failed. '
    'Used for cycle-time computation in KPI reports.';

CREATE INDEX pp_tenant       ON process_projections (tenant, state);
CREATE INDEX pp_pid_state    ON process_projections (pid, state);
CREATE INDEX pp_family       ON process_projections (tenant, family, started_at DESC);
CREATE INDEX pp_malo_id      ON process_projections (malo_id)
    WHERE malo_id IS NOT NULL;
CREATE INDEX pp_partner      ON process_projections (partner_mp_id)
    WHERE partner_mp_id IS NOT NULL;
CREATE INDEX pp_deadline     ON process_projections (deadline_at)
    WHERE deadline_at IS NOT NULL;
-- Deadline sweep: open, not-yet-alerted rows ordered by deadline.
CREATE INDEX pp_deadline_unalerted ON process_projections (deadline_at)
    WHERE deadline_at IS NOT NULL AND deadline_alerted_at IS NULL;
-- KPI and Gleichbehandlungsbericht buckets: both are keyed on started_at, never
-- on updated_at. A report grouped by updated_at moves rows between calendar
-- years as later events touch them, so re-running last year's filing produces
-- different numbers — which is the one property an annual filing must not have.
CREATE INDEX pp_started      ON process_projections (tenant, started_at DESC);
CREATE INDEX pp_pid_started  ON process_projections (tenant, pid, started_at);
CREATE INDEX pp_completed_at ON process_projections (completed_at)
    WHERE completed_at IS NOT NULL;
-- § 7a parity report: affiliate-initiated processes in a period
CREATE INDEX pp_affiliate    ON process_projections (tenant, initiator_is_affiliate, started_at DESC);

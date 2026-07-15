-- mabis-syncd: UTILTS aggregation submission tracking
--
-- submission_runs: one row per triggered aggregation and submission attempt.
-- Tracks: trigger, source period, destination BIKO, submission status, UTILTS message ref.

CREATE TABLE IF NOT EXISTS submission_runs (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Source data
    bilanzierungsgebiet_id  TEXT    NOT NULL,
    period_from     DATE        NOT NULL,
    period_to       DATE        NOT NULL,
    version         TEXT        NOT NULL DEFAULT 'vorlaeufig'   -- 'vorlaeufig' | 'endgueltig'
                    CHECK (version IN ('vorlaeufig', 'endgueltig')),

    -- Counterparties
    sender_mp_id    TEXT        NOT NULL,   -- ÜNB / NB BDEW code
    receiver_mp_id  TEXT        NOT NULL,   -- BIKO BDEW code

    -- Aggregation result
    malo_count      INTEGER     NOT NULL DEFAULT 0,
    interval_count  INTEGER     NOT NULL DEFAULT 0,
    total_kwh       TEXT,                   -- Decimal as text for audit
    has_substituted BOOLEAN     NOT NULL DEFAULT false,

    -- Submission lifecycle
    status          TEXT        NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending','aggregating','submitted','acked','rejected','failed')),
    triggered_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    submitted_at    TIMESTAMPTZ,
    acked_at        TIMESTAMPTZ,

    -- EDIFACT reference
    message_ref     TEXT,                   -- UTILTS message reference from makod
    process_id      UUID,                   -- makod workflow process ID

    -- Error tracking
    error_msg       TEXT,
    attempt_count   INTEGER     NOT NULL DEFAULT 0,

    -- Tenant isolation
    tenant          TEXT        NOT NULL DEFAULT 'default'
);

CREATE INDEX IF NOT EXISTS sr_period    ON submission_runs (bilanzierungsgebiet_id, period_from, period_to);
CREATE INDEX IF NOT EXISTS sr_status    ON submission_runs (status) WHERE status NOT IN ('acked','rejected');
CREATE INDEX IF NOT EXISTS sr_tenant    ON submission_runs (tenant);
CREATE INDEX IF NOT EXISTS sr_triggered ON submission_runs (triggered_at DESC);

COMMENT ON TABLE submission_runs IS
    'MaBiS UTILTS submission tracking. One row per Summenzeitreihe submission attempt. '
    'Preliminary (vorlaeufig) submissions are triggered monthly on day 3; '
    'final (endgueltig) on day 8 after T+2 settlement (BK6-22-024 Anlage 3 MaBiS).';

-- Track which MaLos contributed to each submission
CREATE TABLE IF NOT EXISTS submission_malo_log (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id          UUID        NOT NULL REFERENCES submission_runs(id) ON DELETE CASCADE,
    malo_id         TEXT        NOT NULL,
    interval_count  INTEGER     NOT NULL DEFAULT 0,
    total_kwh       TEXT,
    has_gaps        BOOLEAN     NOT NULL DEFAULT false,
    substituted_count INTEGER   NOT NULL DEFAULT 0,
    fetched_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS sml_run_id ON submission_malo_log (run_id);
CREATE INDEX IF NOT EXISTS sml_malo   ON submission_malo_log (malo_id);

COMMENT ON TABLE submission_malo_log IS
    'Per-MaLo contribution log for each UTILTS submission. '
    'Used for audit trail and gap analysis.';

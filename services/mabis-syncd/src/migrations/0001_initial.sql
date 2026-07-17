-- mabis-syncd schema — MaBiS UTILTS Aggregation & Submission Tracking
--
-- `submission_runs`: one row per triggered aggregation and submission attempt.
--   Tracks: trigger, source period, BIKO destination, submission status, UTILTS ref.
-- `submission_malo_log`: per-MaLo contribution log for audit trail.
--
-- Regulatory: BK6-22-024 Anlage 3 MaBiS.
--   Preliminary (vorlaeufig) submission: day 3 after T+2.
--   Final (endgueltig) submission: day 8 after T+2.

CREATE TABLE submission_runs (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    bilanzierungsgebiet_id  TEXT        NOT NULL,
    period_from             DATE        NOT NULL,
    period_to               DATE        NOT NULL,
    version                 TEXT        NOT NULL DEFAULT 'vorlaeufig'
                            CHECK (version IN ('vorlaeufig', 'endgueltig')),
    sender_mp_id            TEXT        NOT NULL,
    receiver_mp_id          TEXT        NOT NULL,
    malo_count              INTEGER     NOT NULL DEFAULT 0,
    interval_count          INTEGER     NOT NULL DEFAULT 0,
    -- NUMERIC(18,5) serialized as TEXT for lossless audit trail
    total_kwh               TEXT,
    has_substituted         BOOLEAN     NOT NULL DEFAULT false,
    status                  TEXT        NOT NULL DEFAULT 'pending'
                            CHECK (status IN ('pending','aggregating','submitted','acked','rejected','failed')),
    triggered_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    submitted_at            TIMESTAMPTZ,
    acked_at                TIMESTAMPTZ,
    message_ref             TEXT,
    process_id              UUID,
    error_msg               TEXT,
    attempt_count           INTEGER     NOT NULL DEFAULT 0,
    tenant                  TEXT        NOT NULL
);

COMMENT ON TABLE submission_runs IS
    'MaBiS UTILTS submission tracking. One row per Summenzeitreihe submission attempt. '
    'Preliminary (vorlaeufig) triggered day 3; final (endgueltig) triggered day 8 '
    'after T+2 settlement per BK6-22-024 Anlage 3 MaBiS.';

CREATE INDEX sr_period    ON submission_runs (bilanzierungsgebiet_id, period_from, period_to);
CREATE INDEX sr_status    ON submission_runs (status) WHERE status NOT IN ('acked','rejected');
CREATE INDEX sr_tenant    ON submission_runs (tenant);
CREATE INDEX sr_triggered ON submission_runs (triggered_at DESC);

-- ── Per-MaLo contribution log ─────────────────────────────────────────────────

CREATE TABLE submission_malo_log (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id              UUID        NOT NULL REFERENCES submission_runs(id) ON DELETE CASCADE,
    malo_id             TEXT        NOT NULL,
    interval_count      INTEGER     NOT NULL DEFAULT 0,
    total_kwh           TEXT,
    has_gaps            BOOLEAN     NOT NULL DEFAULT false,
    substituted_count   INTEGER     NOT NULL DEFAULT 0,
    fetched_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE submission_malo_log IS
    'Per-MaLo contribution log for each UTILTS submission. '
    'Used for gap analysis and submission audit trail.';

CREATE INDEX sml_run_id ON submission_malo_log (run_id);
CREATE INDEX sml_malo   ON submission_malo_log (malo_id);

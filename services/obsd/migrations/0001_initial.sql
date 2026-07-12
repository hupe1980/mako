-- 0001_initial_schema.sql
-- obsd: process projection read-model.
--
-- One row per MaKo business process, updated on every de.mako.* CloudEvent.
--
-- All timestamps are TIMESTAMPTZ (UTC).

CREATE TABLE IF NOT EXISTS process_projections (
    process_id    UUID        PRIMARY KEY,
    pid           INTEGER     NOT NULL,
    family        TEXT        NOT NULL DEFAULT '',
    workflow_name TEXT        NOT NULL DEFAULT '',
    state         TEXT        NOT NULL DEFAULT 'initiated',
    malo_id       TEXT,
    partner_mp_id   TEXT,
    mdm_role      TEXT,
    deadline_at   TIMESTAMPTZ,
    deadline_risk TEXT        NOT NULL DEFAULT 'green',
    started_at    TIMESTAMPTZ NOT NULL,
    last_event_at TIMESTAMPTZ NOT NULL,
    erc_code      TEXT,
    tenant        TEXT        NOT NULL DEFAULT '',
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS pp_pid_state  ON process_projections (pid, state);
CREATE INDEX IF NOT EXISTS pp_malo_id    ON process_projections (malo_id) WHERE malo_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS pp_partner    ON process_projections (partner_mp_id) WHERE partner_mp_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS pp_tenant     ON process_projections (tenant) WHERE tenant <> '';
CREATE INDEX IF NOT EXISTS pp_deadline   ON process_projections (deadline_at) WHERE deadline_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS pp_started    ON process_projections (started_at DESC);

-- 0002_affiliate_parity.sql
-- obsd: §20 EnWG Diskriminierungsfreiheitspflicht parity column.
--
-- initiator_is_affiliate: true when the initiating LF MP-ID matches the
-- operator's own MP-ID (vertically integrated utility deployment).
-- Populated on de.mako.process.initiated for Lieferbeginn PIDs (55001, 55016, 44001).
--
-- Used for BNetzA audit evidence: allows structured query of all processes
-- where the auto-acceptance guard fired (§20 EnWG non-discrimination proof).

ALTER TABLE process_projections
    ADD COLUMN IF NOT EXISTS initiator_is_affiliate BOOLEAN NOT NULL DEFAULT false;

-- Index for §20 parity report: "show all affiliate Anmeldungen in period X"
CREATE INDEX IF NOT EXISTS pp_affiliate
    ON process_projections (tenant, initiator_is_affiliate, started_at DESC)
    WHERE initiator_is_affiliate = true;

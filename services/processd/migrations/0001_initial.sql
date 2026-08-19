-- processd schema — Process Decision Engine
--
-- `approval_queue`: every role's decisions an operator must make — the NB's
--   escalated and held-back Anmeldungen, the LF's GPKE answers, the MSB's
--   escalated MSB-Wechsel and § 14a Steuerungsaufträge. `expires_at` carries the
--   *business* answer Frist of the process (24 h GPKE / 3-5-7-1 WT WiM /
--   10 WT GeLi Gas), never the 45-minute APERAK clock, which makod owns.
-- `anmeldung_decisions`: NB module — Anmeldung STP audit log (Accept/Reject/Escalate).
--
-- Both tables use (process_id, tenant) as idempotency key;
-- duplicate CloudEvents from the at-least-once EventBus are silently ignored.
--
-- Regulatory: §20 Abs. 1 S. 3 EnWG parity (initiator_is_affiliate).

-- ── Operator approval queue (all roles) ───────────────────────────────────────

CREATE TABLE approval_queue (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    process_id  UUID        NOT NULL,
    pid         INTEGER     NOT NULL,
    malo_id     TEXT,
    reason      TEXT        NOT NULL,
    status      TEXT        NOT NULL CHECK (status IN ('Pending','Approved','Rejected','Expired')),
    -- makod command dispatched on operator approve/reject, resolved from the
    -- trigger PID at enqueue time. NULL means this decision carries no market
    -- message.
    approve_command TEXT,
    reject_command  TEXT,
    marktrolle      TEXT,
    expires_at  TIMESTAMPTZ NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    decided_at  TIMESTAMPTZ,
    -- `sub` of the principal who approved or rejected. § 20 Abs. 1 EnWG parity
    -- evidence and the GoBD trail both have to say *who* decided.
    decided_by  TEXT,
    tenant      TEXT        NOT NULL,
    UNIQUE (process_id, tenant)
);

COMMENT ON TABLE approval_queue IS
    'Market processes this deployment could not answer automatically, from any '
    'compiled role. One row per process awaiting an operator decision, carrying '
    'the makod command to dispatch and the business Frist it must be dispatched '
    'within. A background worker expires Pending rows past expires_at.';

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

-- ── EoG gap-closure case log (§36/§38 EnWG) ───────────────────────────────────
--
-- One row per Marktlokation currently (or last) in the Ersatz-/Grundversorgung
-- pipeline. Written by the eog_module on de.markt.versorgung.gap-detected,
-- promoted on de.markt.versorgung.eog-begonnen, expired by the daily §38
-- timer worker (3 months from eog_seit — the possibly retroactive
-- Zuordnungsbeginn, §38 Abs. 4 S. 1 EnWG).

CREATE TABLE eog_activations (
    id              BIGSERIAL   PRIMARY KEY,
    tenant          TEXT        NOT NULL,
    malo_id         TEXT        NOT NULL,
    sparte          TEXT        NOT NULL CHECK (sparte IN ('STROM', 'GAS')),
    status          TEXT        NOT NULL DEFAULT 'detected' CHECK (status IN (
                        'detected',    -- gap seen, no GV found / auto_activate off
                        'angemeldet',  -- gpke.eog.anmelden dispatched to makod
                        'active',      -- EoG running (eog-begonnen received)
                        'expiring',    -- §38 3-month maximum approaching (warned)
                        'expired',     -- 3 months elapsed — operator action required
                        'closed'       -- regular supply resumed
                    )),
    gv_mp_id        TEXT,
    eog_art         TEXT CHECK (eog_art IN ('ERSATZVERSORGUNG', 'GRUNDVERSORGUNG') OR eog_art IS NULL),
    eog_seit        DATE,
    haushaltskunde  BOOLEAN,
    detail          TEXT,
    warned_at       TIMESTAMPTZ,
    expired_at      TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant, malo_id)
);

COMMENT ON TABLE eog_activations IS
    'NB EoG gap-closure automation case log. Timer scans active Ersatzversorgung '
    'rows daily against eog_seit + 3 months (§38 Abs. 4 S. 1 EnWG). '
    'Grundversorgung rows have no statutory maximum and never expire.';

CREATE INDEX eog_tenant_status ON eog_activations (tenant, status);
CREATE INDEX eog_timer ON eog_activations (tenant, eog_seit)
    WHERE status IN ('active', 'expiring') AND eog_art = 'ERSATZVERSORGUNG';

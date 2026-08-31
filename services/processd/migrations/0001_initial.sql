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

-- ── heute() — the business date ───────────────────────────────────────────────
--
-- Every date this schema compares against is a German calendar date — the day a
-- Frist runs out, a validity window opens, an obligation falls due.
-- PostgreSQL's own `current_date` answers the *session* time zone's date, which
-- on a UTC server is still yesterday between 23:00 and midnight Berlin time
-- (22:00 in summer). `heute()` states the conversion once, so it holds however
-- the connection was opened. The Rust side reads the same date through
-- `mako_fristen::heute`.
CREATE OR REPLACE FUNCTION heute() RETURNS date
    LANGUAGE sql STABLE
    AS $$ SELECT (now() AT TIME ZONE 'Europe/Berlin')::date $$;

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
    -- A Meldepflicht the approved answer carries with it: a message the
    -- Festlegung obliges the operator to send „unverzüglich nach dem ÜZ" of an
    -- answer an operator may hold for hours. Dispatched only after the answer
    -- itself reached makod, so it never states an outcome the market did not
    -- see. Its body is frozen at enqueue time, because the facts it states are
    -- the ones that were true when the decision was taken.
    followup_command TEXT,
    followup_payload JSONB,
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
    -- The BDEW **Antwortcode** the decision landed on, and the Entscheidungsbaum
    -- that publishes it. Not an ERC: `ERC` is the APERAK/CONTRL processability
    -- segment. `A02` means three different things across E_0607, E_0622 and the
    -- LF's E_0609, so the code alone does not identify the finding.
    antwortcode            TEXT,
    antwortcode_ebd        TEXT,
    detail                 TEXT,
    -- §20 EnWG parity: TRUE when lf_mp_id = operator's own GLN
    initiator_is_affiliate BOOLEAN     NOT NULL DEFAULT false,
    decided_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    tenant                 TEXT        NOT NULL,
    UNIQUE (process_id, tenant)
);

COMMENT ON TABLE anmeldung_decisions IS
    'NB STP decision audit log. Written before POST /api/v1/commands to makod. '
    'decision=Escalate when mako-pruefung requires operator review — a missing grid '
    'record, or a fact the EBD needs that the projection does not carry. '
    'Used for BNetzA §20 EnWG parity reports and STP rate KPI.';

-- STP rate KPI
CREATE INDEX ad_tenant_decided ON anmeldung_decisions (tenant, decided_at DESC);
-- §20 parity report
CREATE INDEX ad_affiliate      ON anmeldung_decisions (tenant, initiator_is_affiliate, decided_at DESC)
    WHERE initiator_is_affiliate = true;

-- ── NB Abmeldeanfragen: the Anmeldung decision that is waiting on an LFA ──────
--
-- GPKE Teil 2 § 2.1.2 SD Lieferbeginn Nr. 1 Prüfschritt 4 makes the NB's answer
-- two-phase whenever the Marktlokation is already assigned: ask the incumbent
-- LFA to release it (55010, Nr. 3), then decide (Nr. 5/6) once the LFA answers
-- or its 09:00 window lapses. `E_0623` Prüfschritte 20-50 read that answer.
--
-- One row per waiting Anmeldung. `anfrage` is the serialised
-- `mako_pruefung::AnmeldungAnfrage`, because phase two re-runs the same pure
-- evaluation with one more fact — reconstructing it from columns would put a
-- second, drifting copy of the Anwendungsfall in SQL.
CREATE TABLE abmeldeanfragen (
    anmeldung_process_id   UUID        NOT NULL,
    tenant                 TEXT        NOT NULL,
    malo_id                TEXT        NOT NULL,
    lfn_mp_id              TEXT        NOT NULL,
    -- Every LFA the Anfrage went to. More than one at Geschäftsvorfall 3, where
    -- the Marktlokation is split across Tranchen and Nr. 3 asks all of them.
    lfa_mp_ids             TEXT[]      NOT NULL,
    pid                    INTEGER     NOT NULL,
    -- The replayable `AnmeldungAnfrage`; phase two adds only the LFA's answer.
    anfrage                JSONB       NOT NULL,
    -- What phase two needs to discharge the Meldepflicht zur Beendigung der
    -- Zuordnung (55037 / 44037, Nr. 10 resp. Nr. 6): the Altlieferant, the
    -- Zuordnungsende and the Sparte, as of the Anmeldung. Not derivable from
    -- `anfrage`, which carries neither the incumbent nor the
    -- Transaktionsgrundergänzung that says the object is a Tranche.
    meldung                JSONB       NOT NULL,
    -- One answer per LFA, keyed by MP-ID: {"9911111111111": {...}}.
    --
    -- A Geschäftsvorfall 3 asks every Tranchen-LFA and `E_0623` Prüfschritte
    -- 510/520 count over *all* their answers, so the row cannot be resolved by
    -- the first one to arrive. It resolves when every MP-ID in `lfa_mp_ids`
    -- has answered, or when the 09:00 window lapses and the rest are silence —
    -- which the Festlegung reads as a Zustimmung.
    --
    -- The ordinary one-LFA case is the same rule with a one-element array.
    antworten              JSONB       NOT NULL DEFAULT '{}'::jsonb,
    -- When the Anmeldung arrived, so the operator queue can size the *answer*
    -- window (11:00) rather than the Anfrage's (09:00).
    received_at            TIMESTAMPTZ NOT NULL,
    -- When the 55010 actually reached makod. The row is written *before* the
    -- Anfrage goes out, so this is what tells a redelivered Anmeldung apart from
    -- a genuine duplicate: NULL means the Anfrage never went out, nothing
    -- registered the LFA's 09:00 window, and nobody will ever resolve this row —
    -- so the redelivery must send it rather than return.
    anfrage_gesendet_at    TIMESTAMPTZ,
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Set when phase two ran, so a redelivered answer is a no-op rather than a
    -- second Bestätigung. Silence and an answer both resolve it.
    resolved_at            TIMESTAMPTZ,
    PRIMARY KEY (anmeldung_process_id, tenant)
);

COMMENT ON TABLE abmeldeanfragen IS
    'NB two-phase Anmeldung decisions waiting on the LFA''s answer to a 55010. '
    'Written when mako-pruefung answers AnfrageErforderlich (GPKE Teil 2 § 2.1.2 '
    'Nr. 1 Prüfschritt 4); resolved when the LFA answers 55011/55012 or its 09:00 '
    'window lapses — which the Festlegung reads as a Zustimmung, not a timeout.';

-- The resume path looks the row up by the MaLo the answer names.
CREATE INDEX aa_tenant_malo ON abmeldeanfragen (tenant, malo_id) WHERE resolved_at IS NULL;

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

-- ── NB Neuanlage — the 60-Werktage Prüflauf (E_0608) ─────────────────────────
--
-- GPKE Teil 2 § 2.2.2 and `E_0608` Prüfschritte 110 / 590: an Anmeldung whose
-- newly commissioned Marktlokation cannot yet be identified must **not** be
-- refused. The NB re-checks it daily for 60 Werktage and only then answers
-- `A07` / `A16` — which is why the answer window is „00:00 Uhr des 61. WT nach
-- dem ÜT" and not a day.
--
-- One row per inbound 55600 / 55601. The row exists precisely because the
-- decision is *not* a single evaluation: it is a loop with a deadline, and a
-- process that carries no state between runs has no way to remember either.

CREATE TABLE neuanlage_faelle (
    id                    UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant                TEXT        NOT NULL,
    process_id            UUID        NOT NULL,
    pid                   INTEGER     NOT NULL CHECK (pid IN (55600, 55601)),
    lf_mp_id              TEXT        NOT NULL,
    -- Which `E_0608` branch answers. The two share no Antwortcode.
    marktlokationsart     TEXT        NOT NULL
                          CHECK (marktlokationsart IN ('VERBRAUCHEND', 'ERZEUGEND')),
    -- SG10 CCI+Z22 DE 7037, on an erzeugende Neuanlage: it decides whether the
    -- Vorlauffrist is the month (Direktvermarktung ab Inbetriebnahmedatum) or
    -- the Tag-vor-dem-letzten-WT rule.
    veraeusserungsform    TEXT        CHECK (veraeusserungsform IN ('Z90','Z91','Z92','Z94')),

    -- The ÜT starts the 60-Werktage clock; the Zuordnungsbeginn is what the
    -- Vorlauffrist is measured against.
    uebertragungstag      DATE        NOT NULL,
    zuordnungsbeginn      DATE        NOT NULL,
    -- Computed once from the ÜT with the BDEW-MaKo calendar: the last day on
    -- which a refusal for non-identification is still premature.
    letzter_pruefungstag  DATE        NOT NULL,

    status                TEXT        NOT NULL DEFAULT 'offen' CHECK (status IN (
                              'offen',        -- in the Prüflauf, not identified yet
                              'beantwortet',  -- answered (Bestätigung or Ablehnung)
                              'eskaliert'     -- a fact the tree needs is missing
                          )),
    -- Written by the operator or an NIS integration once the Marktlokation is
    -- identified. Until it is, the daily run defers.
    malo_id               TEXT,
    -- How many daily Prüfläufe have run. Operator-visible evidence that the
    -- obligation was met, not just a counter.
    pruefungen            INTEGER     NOT NULL DEFAULT 0,
    letzte_pruefung_am    DATE,

    antwortcode           TEXT,
    detail                TEXT,
    beantwortet_at        TIMESTAMPTZ,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- A redelivered ORDERS/UTILMD must not open a second case.
    UNIQUE (tenant, process_id),
    -- An answered case states what it answered.
    CHECK (status <> 'beantwortet' OR antwortcode IS NOT NULL)
);

COMMENT ON TABLE neuanlage_faelle IS
    'NB Neuanlage (UTILMD 55600/55601) case log and the E_0608 60-Werktage '
    'Prüflauf. A case stays `offen` while the Marktlokation cannot be '
    'identified; the daily worker re-evaluates it and answers A07/A16 only once '
    'letzter_pruefungstag has passed.';

COMMENT ON COLUMN neuanlage_faelle.letzter_pruefungstag IS
    'ÜT + 60 Werktage (BDEW-MaKo calendar). A refusal for non-identification '
    'before this date contradicts E_0608 Prüfschritt 110/590.';

-- The daily sweep: open cases not yet checked today, oldest deadline first.
CREATE INDEX nf_pruflauf ON neuanlage_faelle (tenant, letzter_pruefungstag)
    WHERE status = 'offen';
CREATE INDEX nf_tenant_status ON neuanlage_faelle (tenant, status);

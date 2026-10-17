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
    -- The bodies the two commands are dispatched with, frozen at enqueue. A
    -- held Accept carries exactly what the automatic path would have sent; an
    -- escalated decision leaves the Antwortcode to the operator, who states it
    -- on approve or reject. `makod` refuses an answer command without one.
    answer_payload JSONB,
    reject_payload JSONB,
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
    -- Gleichbehandlung: TRUE when lf_mp_id = operator's own GLN
    initiator_is_affiliate BOOLEAN     NOT NULL DEFAULT false,
    decided_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    tenant                 TEXT        NOT NULL,
    UNIQUE (process_id, tenant)
);

COMMENT ON TABLE anmeldung_decisions IS
    'NB STP decision audit log. Written before POST /api/v1/commands to makod. '
    'decision=Escalate when mako-pruefung requires operator review — a missing grid '
    'record, or a fact the EBD needs that the projection does not carry. '
    'Evidence for the § 7a Abs. 5 EnWG Gleichbehandlungsbericht (the § 20 Abs. 1 Satz 1 duty it shows compliance with mandates no report of its own), and the STP rate KPI.';

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

-- ── process_projections — the process read model ──────────────────────────────
--
-- One row per business process, updated on every de.mako.* CloudEvent received
-- from marktd, and rebuildable from marktd's replay log (GET /admin/events)
-- through the same projection function.
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

-- ── sperr_orders — Sperr-/Entsperrauftrag execution queue (NB role) ──────────
--
-- One row per order the Netzbetreiber has to physically carry out, and the
-- record of what was reported back to the Lieferant.
--
-- Lifecycle:
--   pending → executed   (field confirmation → IFTSTA 21039 STS Z14 erfolgreich)
--   pending → failed     (field failure      → IFTSTA 21039 STS Z13 gescheitert)
--   pending → cancelled  (withdrawn before execution; no IFTSTA)
--
-- Sources, verified against the documents rather than paraphrased:
--   * ORDERS AHB 1.1b §4.3 — Sperrauftrag 17115 / Anfrage Sperrung 17116 /
--     Entsperrauftrag 17117: BGM+Z51/Z52, DTM+203 Ausführungsdatum XOR DTM+469
--     frühestes Startdatum, IMD+7081 Z53/Z54 Arbeitszeit (Entsperrauftrag),
--     SG2 NAD+Z24 Treffpunkt, SG29 FTX+ACB freier Text.
--   * IFTSTA AHB 2.1 §7.2 — Auftragsstatus 21039: SG15 STS+Z37 (Sperren) or
--     STS+Z38 (Entsperren), DE4405 Z13 gescheitert / Z14 erfolgreich / Z32
--     abgelehnt, DE1131 EBD number, DE9013 Code des Prüfschritts, and
--     DTM+293 Fertigstellungsdatum (Muss on Z14, and ≤ the document date).
--
-- Three regulatory clocks run on one order, and GPKE Teil 2 §§ 3.5.1.2 / 3.5.2.2
-- (BK6-24-174) states all three:
--
--   1. The **ORDRSP** answering the order: „spätester ÜT ist der 1. WT nach dem
--      ÜT" (Prozessschritt 2). Tracked by makod from `mako_fristen::antwort`.
--   2. The **physical act**: „Die Sperrung der Marktlokation ist durch den NB
--      spätestens innerhalb von 6 WT nach dem frühestmöglichen Sperrtermin
--      durchzuführen" (Prozessschritt 1 Hinweis) → `ausfuehrung_faellig_am`.
--   3. The **IFTSTA 21039**: „spätester ÜT ist der 1. WT nach dem Abschluss des
--      Sperrauftrags" (Prozessschritt 5 / Entsperren Prozessschritt 4)
--      → `iftsta_faellig_am`.
--
-- Prozessschritt 5 also caps the field work: „Der NB führt bis zu zwei
-- Sperrversuche innerhalb eines Sperrauftrags durch" → `sperrversuche`.

CREATE TABLE sperr_orders (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant              TEXT        NOT NULL,
    malo_id             TEXT        NOT NULL,
    -- Ordering counterparty: the LF that sent the ORDERS (SG2 NAD+MS).
    lf_mp_id            TEXT        NOT NULL,

    -- BGM 1001: Z51 Sperrung | Z52 Entsperrung.
    order_type          TEXT        NOT NULL
                        CHECK (order_type IN ('sperrung', 'entsperrung')),

    -- ── Market provenance ────────────────────────────────────────────────────
    -- The ORDERS Prüfidentifikator this order came in on, and the makod process
    -- the IFTSTA 21039 must be reported into. `process_id` is NULL only for an
    -- order an operator created by hand, which has no market correspondent and
    -- therefore no IFTSTA to send.
    pruefidentifikator  INTEGER     CHECK (pruefidentifikator IN (17115, 17117)),
    process_id          TEXT,

    -- ── When it may be carried out (ORDERS AHB, conditions [55]/[56]) ────────
    -- DTM+203 Ausführungsdatum — a fixed date the LF requires (hint [533]: a
    -- Gerichtsvollzieher may have set it), XOR DTM+469 "frühestes Startdatum",
    -- meaning "at the next possible date, but not before this one". Exactly one
    -- of the two is present on a 17115; a 17117 carries neither.
    ausfuehrung_am      DATE,
    fruehestens_am      DATE,
    CHECK (ausfuehrung_am IS NULL OR fruehestens_am IS NULL),

    -- IMD 7081 on the Entsperrauftrag (Muss): Z53 innerhalb der Arbeitszeit,
    -- Z54 auch außerhalb. § 41f Abs. 7 EnWG makes restoration *unverzüglich*,
    -- and this is the lever the LF has to pay for out-of-hours work.
    arbeitszeit         TEXT        CHECK (arbeitszeit IN ('innerhalb', 'auch_ausserhalb')),

    -- ── SG2 NAD+Z24 Treffpunkt — where the technician actually goes ──────────
    -- Without this the queue told the field team which Marktlokation to
    -- disconnect and not where it is.
    treffpunkt_hinweis  TEXT,
    treffpunkt_strasse  TEXT,
    treffpunkt_plz      TEXT,
    treffpunkt_ort      TEXT,
    treffpunkt_land     TEXT        CHECK (treffpunkt_land IS NULL
                                           OR treffpunkt_land ~ '^[A-Z]{2}$'),
    -- SG29 FTX+ACB — the LF's free-text hints.
    hinweis             TEXT,

    -- ── The regulatory execution window (GPKE Teil 2 § 3.5.1.2 Nr. 1) ────────
    -- „…spätestens innerhalb von 6 WT nach dem frühestmöglichen Sperrtermin."
    -- Computed on insert from COALESCE(ausfuehrung_am, fruehestens_am) with the
    -- BDEW-MaKo Werktag calendar; NULL when the order names no date at all
    -- (an Entsperrauftrag carries neither), where „unverzüglich" governs.
    ausfuehrung_faellig_am DATE,
    -- Set when `de.sperr.ausfuehrung.ueberfaellig` was announced, so a missed
    -- window is reported once instead of on every sweep.
    ausfuehrung_eskaliert_at TIMESTAMPTZ,

    -- ── The *Lieferant's* Vorlauffrist (GPKE Teil 2 § 3.5.1.2 Nr. 1) ─────────
    -- Two windows, and the DTM the order carries tells them apart: 6 WT before
    -- the frühestmöglicher Sperrtermin for a `DTM+469` order, **12 WT** before
    -- the Sperrtermin for a `DTM+203` one, which fixes date, time and place
    -- (the Festlegung's example is a Gerichtsvollzieher) and so leaves the NB
    -- no room to reschedule.
    --
    -- Recorded, never enforced: Prozessschritt 2 lists what the NB checks
    -- before it answers, and the Vorlauffrist is not among them. Refusing on a
    -- ground the Festlegung does not publish is the § 20 EnWG-unsafe
    -- direction, so a short lead surfaces to the operator instead. NULL when
    -- the order names no Sperrtermin at all.
    vorlauffrist_eingehalten BOOLEAN,
    -- The latest ÜT the order could have carried, so the operator sees by how
    -- much rather than only that.
    vorlauffrist_spaetester_ut DATE,

    -- ── Execution ────────────────────────────────────────────────────────────
    status              TEXT        NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending', 'executed', 'failed', 'cancelled')),
    -- „Der NB führt bis zu zwei Sperrversuche innerhalb eines Sperrauftrags
    -- durch" (GPKE Teil 2 § 3.5.1.2 Nr. 5). A first unsuccessful attempt is
    -- recorded and the order stays `pending`: reporting Z13 gescheitert after
    -- one try ends a process the Festlegung says still owes a second visit.
    sperrversuche       INTEGER     NOT NULL DEFAULT 0
                        CHECK (sperrversuche BETWEEN 0 AND 2),
    letzter_versuch_am  TIMESTAMPTZ,
    letzter_versuch_grund TEXT,
    -- DTM+293 Fertigstellungsdatum. Muss when the outcome is Z14 erfolgreich,
    -- and condition [495] requires it to be ≤ the IFTSTA document date — so a
    -- future timestamp is refused at the API boundary, not discovered by the
    -- recipient's AHB validator.
    executed_at         TIMESTAMPTZ,
    execution_note      TEXT,
    fail_reason         TEXT,
    -- SG15 STS DE9013 "Code des Prüfschritts" from the EBD cluster matching the
    -- outcome. Muss in the IFTSTA, so a failure reported as free text alone
    -- cannot be turned into a valid message.
    pruefschritt_code   TEXT,
    CHECK (status <> 'executed' OR executed_at IS NOT NULL),
    CHECK (status <> 'failed'   OR fail_reason IS NOT NULL),

    -- ── IFTSTA 21039 dispatch ────────────────────────────────────────────────
    -- `iftsta_dispatched_at IS NULL` on a terminal order is the one state that
    -- needs work: the LF's gpke-sperrung-lf process cannot close without it. The
    -- retry worker drains these; `iftsta_attempts` bounds it and
    -- `iftsta_last_error` says why it is stuck.
    -- „…spätester ÜT ist der 1. WT nach dem Abschluss des Sperrauftrags"
    -- (GPKE Teil 2 § 3.5.1.2 Nr. 5 / § 3.5.2.2 Nr. 4). Set when the order turns
    -- terminal; an outstanding IFTSTA past this date is a missed Frist, not
    -- merely a queue backlog.
    iftsta_faellig_am   DATE,
    iftsta_ref          TEXT,
    iftsta_dispatched_at TIMESTAMPTZ,
    iftsta_attempts     INTEGER     NOT NULL DEFAULT 0,
    iftsta_last_error   TEXT,
    -- The claim lease *and* the retry backoff, in one column. A worker takes an
    -- order by pushing this forward, so a second replica's `<= now()` no longer
    -- matches it and cannot dispatch the same 21039 — a disconnection order the
    -- customer's Lieferant would receive twice. NOT NULL with a `now()` default,
    -- so a newly terminal order is due at once and the column is never a
    -- sentinel: `iftsta_attempts >= IFTSTA_MAX_ATTEMPTS` is the only unselectable
    -- state.
    iftsta_next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    iftsta_escalated_at TIMESTAMPTZ,

    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- One order per inbound market process. An ORDERS redelivered over AS4 must
    -- not put a second disconnection in front of the field team.
    UNIQUE (tenant, process_id)
);

COMMENT ON TABLE sperr_orders IS
    'Sperr-/Entsperrauftrag execution queue (NB role). Fed by inbound ORDERS '
    '17115/17117 and by operators; drained by the field team. Terminal orders '
    'are reported to the LF with IFTSTA 21039 (AHB 2.1 §7.2).';

COMMENT ON COLUMN sperr_orders.iftsta_next_attempt_at IS
    'When this order may next be claimed for an IFTSTA dispatch. The claim '
    'pushes it forward, which is what stops two replicas sending the same '
    'Sperrauftrag-Status to the Lieferant.';

COMMENT ON COLUMN sperr_orders.iftsta_dispatched_at IS
    'When the IFTSTA 21039 command reached makod. NULL on a terminal order means '
    'the Lieferant has not been told the outcome — the retry worker is still '
    'trying, or gave up and emitted de.sperr.iftsta.ausstehend.';

CREATE INDEX so_tenant_status ON sperr_orders (tenant, status);
CREATE INDEX so_malo_status   ON sperr_orders (malo_id, status);
-- Field-dispatch scan: what the LF asked for, soonest first. COALESCE folds the
-- fixed and the earliest date into the one ordering key a dispatcher works from.
CREATE INDEX so_pending_due   ON sperr_orders (tenant, COALESCE(ausfuehrung_am, fruehestens_am))
    WHERE status = 'pending';
-- The § 3.5.1.2 Nr. 1 execution-window sweep: which pending orders are past the
-- 6-Werktage deadline the Festlegung sets, soonest first.
-- Orders the LF sent too late — the operator queue, and the number a
-- Lieferantenrahmenvertrag review asks about.
CREATE INDEX so_vorlauf_verletzt ON sperr_orders (tenant, created_at DESC)
    WHERE vorlauffrist_eingehalten = false;
CREATE INDEX so_pending_frist ON sperr_orders (tenant, ausfuehrung_faellig_am)
    WHERE status = 'pending' AND ausfuehrung_faellig_am IS NOT NULL
      AND ausfuehrung_eskaliert_at IS NULL;
-- The retry worker's queue, in the order it is claimed: due first.
CREATE INDEX so_iftsta_open   ON sperr_orders (tenant, iftsta_next_attempt_at)
    WHERE status IN ('executed', 'failed') AND iftsta_dispatched_at IS NULL;

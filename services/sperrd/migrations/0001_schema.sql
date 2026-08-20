-- ── sperrd schema — Sperr-/Entsperrauftrag execution queue (NB role) ──────────
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
-- There is deliberately **no execution deadline in Werktagen** here. GPKE fixes
-- a 24-wall-clock-hour window for the NB's *ORDRSP* (BK6-22-024 §5); the timing
-- of the physical act is whatever the Lieferant states in DTM+203/DTM+469.

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

    -- ── Execution ────────────────────────────────────────────────────────────
    status              TEXT        NOT NULL DEFAULT 'pending'
                        CHECK (status IN ('pending', 'executed', 'failed', 'cancelled')),
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
    iftsta_ref          TEXT,
    iftsta_dispatched_at TIMESTAMPTZ,
    iftsta_attempts     INTEGER     NOT NULL DEFAULT 0,
    iftsta_last_error   TEXT,
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

COMMENT ON COLUMN sperr_orders.iftsta_dispatched_at IS
    'When the IFTSTA 21039 command reached makod. NULL on a terminal order means '
    'the Lieferant has not been told the outcome — the retry worker is still '
    'trying, or gave up and emitted de.sperr.iftsta.ausstehend.';

CREATE INDEX so_tenant_status ON sperr_orders (tenant, status);
CREATE INDEX so_malo_status   ON sperr_orders (malo_id, status);
-- Field-dispatch scan: what is due, soonest first. COALESCE folds the fixed and
-- the earliest date into the one ordering key a dispatcher works from.
CREATE INDEX so_pending_due   ON sperr_orders (tenant, COALESCE(ausfuehrung_am, fruehestens_am))
    WHERE status = 'pending';
-- The retry worker's queue.
CREATE INDEX so_iftsta_open   ON sperr_orders (tenant, updated_at)
    WHERE status IN ('executed', 'failed') AND iftsta_dispatched_at IS NULL;

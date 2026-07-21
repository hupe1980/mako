-- mabis-syncd schema — MaBiS Summenzeitreihe aggregation and submission tracking.
--
-- `submission_runs`: one row per aggregation and submission attempt.
-- `submission_malo_log`: per-MaLo contribution log for the audit trail.
-- `pruefmitteilung`: inbound BIKO objections that require a correction.
--
-- Regulatory basis: BK6-24-174 Anlage 3 (MaBiS), Lesefassung.
--
-- ## Versionierung (§3.8.2)
--
-- A Summenzeitreihe is identified by the 3-tuple
-- (MaBiS-Zählpunkt, Bilanzierungsmonat, Version). The version is not a
-- lifecycle state: "Die Version einer Summenzeitreihe ist jeweils aufsteigend
-- zu vergeben und ist über die gesamte BKA beizubehalten." A correction is the
-- same Zählpunkt and month resent under a higher version, so a period may carry
-- arbitrarily many versions.
--
-- MSCONS AHB 3.2 carries it as SG6 DTM+293 (Fertigstellungsdatum/-zeit,
-- format 304, CCYYMMDDHHMMSSZZZ) — a timestamp, which is what makes "ascending"
-- well defined. `BGM 1225` is always `9` (Original); there is no replace or
-- correction qualifier, so the version is the only thing distinguishing a
-- correction from the original.
--
-- ## Datenstatus (§3.8.3)
--
-- Assigned exclusively by the BIKO — "Der Datenstatus wird ausschließlich vom
-- BIKO vergeben" — and received inbound via IFTSTA (SG7 STS+Z04, PID 21003 to
-- NB/ÜNB and 21004 to BKV/NB). It is therefore nullable here: it is unknown
-- until the BIKO responds, and this service must never set it itself.
--
-- Settlement uses the highest version carrying 'Abrechnungsdaten' or
-- 'Abrechnungsdaten KBKA'.
--
-- ## Abrechnungslauf
--
-- BKA is the ordinary Bilanzkreisabrechnung; KBKA the
-- Korrekturbilanzkreisabrechnung that runs afterwards (§3.10, Tabelle 2). They
-- have separate clearing windows, so which run a submission belongs to decides
-- which deadline applies.

CREATE TABLE submission_runs (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    bilanzierungsgebiet_id  TEXT        NOT NULL,
    period_from             DATE        NOT NULL,
    period_to               DATE        NOT NULL,
    -- Ascending version within (bilanzierungsgebiet_id, period). Sent as
    -- MSCONS SG6 DTM+293, so the wire value derives from this timestamp.
    version                 TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Which settlement run this submission belongs to.
    abrechnungslauf         TEXT        NOT NULL DEFAULT 'BKA'
                            CHECK (abrechnungslauf IN ('BKA', 'KBKA')),
    -- Phase the submission was made in (§3.10, Tabelle 2). Determines the
    -- Datenstatus the BIKO assigns: within Erstaufschlag a new version becomes
    -- 'Abrechnungsdaten' automatically, afterwards it is 'Prüfdaten' until a
    -- positive Prüfmitteilung promotes it.
    phase                   TEXT        NOT NULL DEFAULT 'ERSTAUFSCHLAG'
                            CHECK (phase IN ('ERSTAUFSCHLAG', 'CLEARING')),
    -- BIKO-assigned; NULL until an IFTSTA Datenstatus message arrives.
    datenstatus             TEXT
                            CHECK (datenstatus IN (
                                'PRUEFDATEN',
                                'ABRECHNUNGSDATEN',
                                'ABRECHNUNGSDATEN_KBKA',
                                'ABGERECHNETE_DATEN',
                                'ABGERECHNETE_DATEN_KBKA'
                            )),
    datenstatus_at          TIMESTAMPTZ,
    -- Run this one corrects, when it was sent in response to a negative
    -- Prüfmitteilung (§9.8.1).
    corrects_run_id         UUID        REFERENCES submission_runs(id),
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
    tenant                  TEXT        NOT NULL,

    -- The identity of a Summenzeitreihe. Two submissions for the same territory
    -- and month must differ in version, or the BIKO cannot tell which one
    -- settles.
    CONSTRAINT sr_version_unique
        UNIQUE (tenant, bilanzierungsgebiet_id, period_from, period_to, version),
    -- A Datenstatus without a timestamp loses when the BIKO assigned it, which
    -- the § 60 Abs. 6 MsbG audit trail needs.
    CONSTRAINT sr_datenstatus_timed CHECK (
        (datenstatus IS NULL) = (datenstatus_at IS NULL)
    ),
    -- The KBKA statuses belong to the KBKA run and nowhere else.
    CONSTRAINT sr_datenstatus_matches_lauf CHECK (
        datenstatus IS NULL
        OR datenstatus = 'PRUEFDATEN'
        OR (abrechnungslauf = 'KBKA') =
           (datenstatus IN ('ABRECHNUNGSDATEN_KBKA', 'ABGERECHNETE_DATEN_KBKA'))
    )
);

COMMENT ON TABLE submission_runs IS
    'MaBiS Summenzeitreihe submission tracking (MSCONS PID 13003). One row per '
    'submission attempt. Identified by (Bilanzierungsgebiet, Bilanzierungsmonat, '
    'Version) per BK6-24-174 Anlage 3 §3.8.2; Datenstatus is assigned by the BIKO '
    'and arrives via IFTSTA 21003/21004.';

CREATE INDEX sr_period    ON submission_runs (bilanzierungsgebiet_id, period_from, period_to);
CREATE INDEX sr_status    ON submission_runs (status) WHERE status NOT IN ('acked','rejected');
CREATE INDEX sr_tenant    ON submission_runs (tenant);
CREATE INDEX sr_triggered ON submission_runs (triggered_at DESC);

-- Finding the version that settles: the highest one the BIKO has marked
-- 'Abrechnungsdaten' (§3.8.3).
CREATE INDEX sr_settling_version ON submission_runs
    (tenant, bilanzierungsgebiet_id, period_from, period_to, version DESC)
    WHERE datenstatus IN ('ABRECHNUNGSDATEN', 'ABRECHNUNGSDATEN_KBKA');

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
    'Per-MaLo contribution log for each Summenzeitreihe submission. '
    'Used for gap analysis and submission audit trail.';

CREATE INDEX sml_run_id ON submission_malo_log (run_id);
CREATE INDEX sml_malo   ON submission_malo_log (malo_id);

-- ── Inbound Prüfmitteilung (IFTSTA 21000/21001) ───────────────────────────────
--
-- A negative Prüfmitteilung signals Korrekturbedarf: "Der ÜNB übermittelt bei
-- Erhalt einer negativen Prüfmitteilung ggf. eine korrigierte BG-SZR"
-- (§9.8.1). Recording it is what lets a correction be traced back to the
-- objection that prompted it.

CREATE TABLE pruefmitteilung (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id          UUID        REFERENCES submission_runs(id) ON DELETE SET NULL,
    -- The 3-tuple the Prüfmitteilung refers to, kept independently of `run_id`
    -- so a message naming a version this service never sent is still recorded.
    bilanzierungsgebiet_id TEXT NOT NULL,
    period_from     DATE        NOT NULL,
    period_to       DATE        NOT NULL,
    version         TIMESTAMPTZ NOT NULL,
    positiv         BOOLEAN     NOT NULL,
    sender_mp_id    TEXT        NOT NULL,
    pid             INTEGER     NOT NULL,
    begruendung     TEXT,
    received_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Set when a correcting submission has been sent in response.
    corrected_by_run_id UUID    REFERENCES submission_runs(id),
    tenant          TEXT        NOT NULL
);

COMMENT ON TABLE pruefmitteilung IS
    'Inbound Prüfmitteilung (IFTSTA PID 21000/21001) per BK6-24-174 Anlage 3 §9.8.1. '
    'A negative one requires a corrected Summenzeitreihe under a higher version.';

CREATE INDEX pm_run       ON pruefmitteilung (run_id);
CREATE INDEX pm_tenant    ON pruefmitteilung (tenant, received_at DESC);
-- Negative Prüfmitteilungen still awaiting a correcting submission.
CREATE INDEX pm_open_negative ON pruefmitteilung
    (tenant, bilanzierungsgebiet_id, period_from)
    WHERE NOT positiv AND corrected_by_run_id IS NULL;

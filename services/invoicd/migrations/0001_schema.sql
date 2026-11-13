-- ── invoicd schema — INVOIC plausibility & settlement ────────────────────────
--
-- § 147 AO / § 14b UStG: a received INVOIC is a received invoice — a
-- Buchungsbeleg — and must be retained for 8 years (§ 147 Abs. 3 AO as amended
-- by BEG IV, in force 01.01.2025). Every INVOIC is persisted here **before** the
-- REMADV/COMDIS command is dispatched to makod (persist-before-dispatch).
--
-- Supported PIDs: 31001 31002 31003 31004 31005 31006 31007 31008 31009 31011

-- ── Business dates ───────────────────────────────────────────────────────────
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


CREATE TABLE invoic_receipts (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Business key: one row per billing process, per tenant.
    --
    -- The uniqueness is the pair, never `process_id` alone: it is read from the
    -- inbound CloudEvent `subject`, so a globally unique key lets one sender's
    -- event reach another tenant's receipt through the upsert. Same rule as
    -- `invoic_dlq_process` below.
    process_id              UUID        NOT NULL,

    -- The EDIFACT INVOIC message reference (BGM 1004) — the business key makod
    -- routes an answer command by, so a re-dispatch that has only the receipt
    -- row still needs it. A receipt without one could never be answered.
    invoice_ref             TEXT        NOT NULL,
    -- The Rechnungsnummer the document states — the duplicate check reads it.
    rechnungsnummer         TEXT,

    -- BDEW Prüfidentifikator that triggered this receipt.
    pid                     SMALLINT    NOT NULL,

    -- BDEW-Codenummer / GLN of the sender.
    sender_mp_id            TEXT        NOT NULL,
    -- Receiver MP-ID: this tenant.
    receiver_gln            TEXT,

    -- MaLo-ID extracted from the Rechnung at ingest (indexed; no JSONB scan).
    malo_id                 TEXT,

    -- The BO4E Rechnung exactly as received, with the schema version it was
    -- read under. Server-derived provenance, never a client-supplied value.
    rechnung                JSONB       NOT NULL,
    bo4e_version            TEXT        NOT NULL,
    -- `SG1 RFF+ACE` and `IMD+7081` — the two INVOIC facts BO4E has no field
    -- for. The Entscheidungsbaum reads both (the order, the Use-Case), so a
    -- held invoice walks again from the receipt alone.
    bestellung_ref          TEXT,
    rechnungstyp            TEXT,
    -- The Prüfschritte the walk reached and no record answered, while
    -- `outcome = 'Offen'`: what a person has to state.
    offen                   JSONB       NOT NULL DEFAULT '[]',
    -- What a person stated for this invoice's open Prüfschritte (310, 320,
    -- 330 per Positionsnummer, 80), with the Begründung — the answers the
    -- walk reads when it runs again.
    feststellungen          JSONB       NOT NULL DEFAULT '{}',
    festgestellt_von        TEXT,
    festgestellt_am         TIMESTAMPTZ,
    feststellung_begruendung TEXT,
    -- The round of the Rechnungsprüfung: 1 `E_0264`/`E_0270`/`E_0273`, 2 the
    -- second look `E_0266`/`E_0276`/`E_0277` after the MSB answered our
    -- refusal with a COMDIS 29001 (`comdis_ref`). Opening round 2 keeps round
    -- 1's answer and its dispatch time in `erste_runde_*`, so both REMADVs
    -- stay on the § 147 AO record.
    runde                   SMALLINT    NOT NULL DEFAULT 1 CHECK (runde IN (1, 2)),
    comdis_ref              TEXT,
    erste_runde_antwort     JSONB,
    erste_runde_dispatched_at TIMESTAMPTZ,
    CONSTRAINT invoic_zweite_runde_hat_comdis CHECK ((runde = 2) = (comdis_ref IS NOT NULL)),

    -- Plausibility outcome.
    outcome                 TEXT        NOT NULL CHECK (outcome IN (
                                'Ok',               -- accepted
                                'AcceptedPartial',  -- accepted with remarks (Storno)
                                'Warn',             -- warnings, auto-approved
                                'Dispute',          -- rejected
                                'Offen',            -- held: the Entscheidungsbaum left a Prüfschritt unanswered
                                'Resolved'          -- dispute closed by an operator
                            )),
                            -- No 'Paid': `outcome` is the plausibility verdict on
                            -- the invoice, and settlement is a different axis —
                            -- an invoice can be disputed and still paid, or Ok
                            -- and unpaid. Payment lives in `payment_confirmed_at`
                            -- below, which also records *when*. Writing 'Paid'
                            -- here would overwrite the check result.

    -- invoic_checker findings (Vec<Finding> as JSONB).
    findings                JSONB       NOT NULL DEFAULT '[]',

    -- Zahlungsziel from INVOIC SG8 DTM+265 (rubo4e Rechnung.faelligkeitsdatum).
    -- The INVOIC MIG publishes no DTM qualifier 92 at all (that is UTILMD's
    -- Vertragsbeginn); the Fälligkeitsdatum is Nr. 00033 in the SG8 PYT group,
    -- and its DE 2379 admits only 303 (CCYYMMDDHHMMZZZ).
    pay_by                  TIMESTAMPTZ,

    -- Dispute resolution: when, by whom (the verified caller), and why.
    dispute_resolved_at     TIMESTAMPTZ,
    dispute_resolved_by     TEXT,
    dispute_resolution_note TEXT,

    received_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    checked_at              TIMESTAMPTZ,
    dispatched_at           TIMESTAMPTZ,
    -- The answer command's payload as built at the check — Antwortcode, EBD,
    -- Befunde, Ablehnungsgrund. A re-dispatch sends exactly this, so a REMADV
    -- sent by an operator says what the automatic one would have.
    antwort                 JSONB,
    payment_confirmed_at    TIMESTAMPTZ,

    -- Set once `de.invoic.payment.overdue` has gone out for this receipt, so a
    -- passed Zahlungsziel is announced once rather than every six hours until
    -- somebody acts on it.
    overdue_notified_at     TIMESTAMPTZ,

    tenant                  TEXT        NOT NULL,

    -- A dispute resolution has its time and its decider, or neither.
    CONSTRAINT invoic_resolution_timed CHECK (
        (outcome = 'Resolved') = (dispute_resolved_at IS NOT NULL)
        AND (outcome = 'Resolved') = (dispute_resolved_by IS NOT NULL)
    )
);

COMMENT ON TABLE invoic_receipts IS
    '§ 147 AO / § 14b UStG: INVOIC receipts are Buchungsbelege — 8-year retention. '
    'Written before REMADV/COMDIS dispatch (persist-before-dispatch).';

COMMENT ON COLUMN invoic_receipts.invoice_ref IS
    'EDIFACT INVOIC message reference (BGM 1004) — the business key makod routes the answer command by.';

-- The business key the ingest upserts on.
CREATE UNIQUE INDEX invoic_receipts_tenant_process
    ON invoic_receipts (tenant, process_id);
-- Per-counterparty billing history.
CREATE INDEX invoic_sender_received   ON invoic_receipts (sender_mp_id, received_at DESC);
-- Prüfschritt 50 („Rechnungsnummer wurde bereits verwendet", `A05`) of every
-- invoice tree in `mako_pruefung::rechnung`. § 14 Abs. 4 Nr. 4 UStG makes the
-- Rechnungsnummer einmalig **per Rechnungssteller**, so that pair is the key;
-- the partial predicate skips the rows that never carried a number.
CREATE INDEX invoic_sender_rechnungsnummer
    ON invoic_receipts (tenant, sender_mp_id, rechnungsnummer)
    WHERE rechnungsnummer IS NOT NULL;
-- Tenant-scoped dashboard.
CREATE INDEX invoic_tenant_received   ON invoic_receipts (tenant, received_at DESC);
-- MaLo payment lifecycle.
CREATE INDEX invoic_malo_tenant       ON invoic_receipts (tenant, malo_id)
    WHERE malo_id IS NOT NULL;
-- Exception queue (skips the majority of settled rows).
CREATE INDEX invoic_outcome           ON invoic_receipts (tenant, outcome)
    WHERE outcome NOT IN ('Ok', 'Resolved');
-- Zahlungsziel approaching without a dispatched answer.
CREATE INDEX invoic_pay_by_pending    ON invoic_receipts (tenant, pay_by)
    WHERE pay_by IS NOT NULL AND dispatched_at IS NULL;
-- The overdue-payment sweep.
CREATE INDEX invoic_overdue_pending   ON invoic_receipts (tenant, pay_by)
    WHERE pay_by IS NOT NULL
      AND payment_confirmed_at IS NULL
      AND overdue_notified_at IS NULL
      AND dispatched_at IS NOT NULL;

-- ── Dead-letter queue ─────────────────────────────────────────────────────────
--
-- An INVOIC that could not be turned into a receipt: an unparseable Rechnung, a
-- payload with no message reference, a Rechnung makod cannot supply. These are
-- Buchungsbelege that arrived and were not processed, so they are kept and
-- alerted on rather than dropped in a log line.

-- ── Leistungsnachweise ────────────────────────────────────────────────────────
--
-- `E_0264`/`E_0270`/`E_0273` Prüfschritt 310: „Wurde die abzurechnende
-- Leistung vom MSB durchgeführt?" (EBD 4.3 S. 628, 657, 667). No INVOIC states
-- it; the recipient knows it — the values arrived, the technical change was
-- made. Recorded once per order, Artikel-ID and period, it answers every
-- position it covers, before or after the invoice arrives.
CREATE TABLE leistungsnachweise (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant                  TEXT        NOT NULL,
    -- The MSB the Leistung was ordered from (NAD+MS of its INVOIC).
    rechnungssteller_mp_id  TEXT        NOT NULL,
    -- The order (`SG1 RFF+ACE` of the INVOIC).
    bestellung_ref          TEXT        NOT NULL,
    artikel_id              TEXT        NOT NULL,
    von                     DATE        NOT NULL,
    bis                     DATE        NOT NULL CHECK (von <= bis),
    erbracht                BOOLEAN     NOT NULL,
    -- What the statement rests on — never empty.
    nachweis                TEXT        NOT NULL CHECK (btrim(nachweis) <> ''),
    festgestellt_von        TEXT        NOT NULL,
    festgestellt_am         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant, rechnungssteller_mp_id, bestellung_ref, artikel_id, von, bis)
);

CREATE TABLE invoic_dlq (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    -- The process the event named, when it had one. Redelivery of the same
    -- event must not pile up rows.
    process_id      UUID,
    pid             SMALLINT,
    malo_id         TEXT,
    raw_event       JSONB       NOT NULL,
    failure_reason  TEXT        NOT NULL,
    failed_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at     TIMESTAMPTZ,
    tenant          TEXT        NOT NULL
);

COMMENT ON TABLE invoic_dlq IS
    'INVOIC events that could not be processed. Alert when unresolved entries are older than 1 hour.';

-- Redelivery of the same failing event updates the row instead of adding one.
CREATE UNIQUE INDEX invoic_dlq_process ON invoic_dlq (tenant, process_id)
    WHERE process_id IS NOT NULL;
CREATE INDEX invoic_dlq_open ON invoic_dlq (tenant, failed_at DESC)
    WHERE resolved_at IS NULL;

-- ── netzbilanzd schema — NNE / MMM / MSB / AWH billing engine (NB role) ──────
--
-- `invoice_drafts`     one settled invoice per MaLo × period × Prüfidentifikator.
-- `invoice_number_seq` per-tenant, per-Rechnungskreis, per-year running number.
-- `abschlag_verrechnungen` which invoice settled which Abschlagsrechnung.
-- `kostenblatt_records` Redispatch 2.0 Kostenblatt per BK6-20-061 §4.2.
-- `fremdkosten_records` typed BO4E Fremdkosten attached to a draft.
-- `submission_runs`, `submission_series`, `submission_malo_log`, `pruefmitteilung`
--                      MaBiS Summenzeitreihen (MSCONS 13003) filed with the BIKO.
--
-- Every table is tenant-scoped and every read path filters on `tenant`.

-- ── Invoice numbering (§14 Abs. 4 Nr. 4 UStG) ─────────────────────────────────
--
-- An invoice number must be *einmalig vergeben* and run consecutively. The
-- caller therefore does not supply one: it supplies at most a Rechnungskreis
-- (a short series prefix) and the running number is allocated here, under a row
-- lock, in the same transaction as the draft.

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


CREATE TABLE invoice_number_seq (
    tenant          TEXT     NOT NULL,
    -- Series prefix, e.g. 'NNE'. Empty string when the caller names none.
    rechnungskreis  TEXT     NOT NULL,
    -- Numbering restarts per calendar year, as German invoice series do.
    year            SMALLINT NOT NULL,
    -- Last number handed out. The next allocation is `last_number + 1`.
    last_number     BIGINT   NOT NULL DEFAULT 0,
    PRIMARY KEY (tenant, rechnungskreis, year)
);

COMMENT ON TABLE invoice_number_seq IS
    'Per-tenant consecutive invoice numbering (§14 Abs. 4 Nr. 4 UStG). '
    'Allocated under a row lock inside the drafting transaction, so a rolled-back '
    'run leaves no gap and a retried run cannot reuse a number.';

-- ── Invoice drafts ────────────────────────────────────────────────────────────

CREATE TABLE invoice_drafts (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant              TEXT        NOT NULL,
    malo_id             TEXT        NOT NULL,

    -- Invoice parties, named for the role they play rather than the role that
    -- usually fills it. For PID 31009 (MSB-Rechnung) the sender is the MSB and
    -- the recipient is the NB, LF or ESA — the inverse of every other PID here.
    sender_mp_id        TEXT        NOT NULL,   -- NB/GNB, or MSB for 31009
    recipient_mp_id     TEXT        NOT NULL,   -- LF, or NB/LF/ESA for 31009

    pid                 INTEGER     NOT NULL
                        CHECK (pid IN (31001, 31002, 31005, 31009, 31011)),
    -- NN-Rechnung Strom and Gas share PID 31002, so the Prüfidentifikator does
    -- not identify the Sparte and the Sparte has to be stored beside it.
    sparte              TEXT        NOT NULL CHECK (sparte IN ('STROM', 'GAS')),
    -- `grid_billing::SettlementType`, for reporting without parsing the JSONB.
    settlement_type     TEXT        NOT NULL,

    period_from         DATE        NOT NULL,
    period_to           DATE        NOT NULL,
    CONSTRAINT id_period_ordered CHECK (period_from <= period_to),

    -- The invoice number this draft was issued under. Unique per tenant.
    rechnungsnummer     TEXT        NOT NULL,
    -- The document's own dates. Both are §14 UStG mandatory content, and both
    -- are asked for outside the document: `invoice_date` is the Rechnungsdatum
    -- an Abschlagsrechnung is referenced by (`SG51 DTM+3`), and `due_date` is
    -- the Zahlungsziel an overdue report is measured against. Keeping them only
    -- inside the Rechnung JSONB meant neither could be queried.
    invoice_date        DATE        NOT NULL,
    due_date            DATE        NOT NULL,
    CONSTRAINT id_due_after_invoice CHECK (due_date >= invoice_date),

    -- The settlement *input* — a `netzbilanzd::request::SettlementRequest`.
    -- Storing the input rather than only the rendered document is what makes a
    -- Stornorechnung a recomputation (`reverse(settle(input))`) instead of a
    -- JSON edit of a rendered invoice, and what lets an audit replay the figure
    -- rather than merely read a description of it.
    settlement_input    JSONB       NOT NULL,
    -- The rendered document — `rubo4e::current::Rechnung`.
    rechnung            JSONB       NOT NULL,
    bo4e_version        TEXT        NOT NULL DEFAULT '202607.1.0',

    -- The three amounts an invoice states, each × 10⁻⁵ EUR as an integer, so
    -- reporting never rounds through a float. `netto + steuer = brutto` is
    -- enforced rather than assumed: the tax is what §14 Abs. 4 Nr. 8 UStG
    -- requires on the document, and a total that does not add up is the one
    -- error nobody spots by reading the invoice.
    netto_eur_units     BIGINT      NOT NULL,
    steuer_eur_units    BIGINT      NOT NULL,
    brutto_eur_units    BIGINT      NOT NULL,
    CONSTRAINT id_totals_add_up CHECK (netto_eur_units + steuer_eur_units = brutto_eur_units),
    -- What the recipient actually pays: the gross less every Abschlagsrechnung
    -- this invoice settles (§14 Abs. 5 UStG — the Anzahlung was taxed on
    -- receipt, so only what is *owed* moves). Equal to `brutto_eur_units` when
    -- no Abschlag is deducted. Stored rather than derived, because it is the
    -- figure the payment run collects and the dunning report measures.
    zu_zahlen_eur_units BIGINT      NOT NULL,
    -- A deduction only ever *reduces* what is owed. It may take it past zero —
    -- an Abschlussrechnung settling for less than the Anzahlungen leaves a
    -- Guthaben the Netzbetreiber owes back, which is ordinary — so the bound is
    -- directional rather than clamped at zero.
    CONSTRAINT id_deduction_only_reduces CHECK (
        (brutto_eur_units >= 0 AND zu_zahlen_eur_units <= brutto_eur_units)
     OR (brutto_eur_units <  0 AND zu_zahlen_eur_units >= brutto_eur_units)
    ),
    -- UNCL 5305 category: 'S' taxed, 'AE' reverse charge (§13b UStG).
    steuer_kategorie    TEXT        NOT NULL CHECK (steuer_kategorie IN ('S', 'AE')),
    -- The rate in percent — 19, 7, or 0 under a reverse charge.
    steuer_satz_prozent NUMERIC(5, 2) NOT NULL,
    -- A reverse charge states no tax and an ordinary supply states a rate.
    CONSTRAINT id_reverse_charge_states_no_tax CHECK (
        (steuer_kategorie = 'AE' AND steuer_eur_units = 0 AND steuer_satz_prozent = 0)
     OR (steuer_kategorie = 'S'  AND steuer_satz_prozent > 0)
    ),

    rechnungsart        TEXT        NOT NULL DEFAULT 'RECHNUNG'
                        CHECK (rechnungsart IN ('RECHNUNG', 'STORNORECHNUNG', 'KORREKTURRECHNUNG')),
    -- Storno/Korrektur audit chain. The original is never modified.
    original_draft_id   UUID        REFERENCES invoice_drafts(id) ON DELETE RESTRICT,
    -- Why the recalculation happened — `grid_billing::KorrekturGrund`.
    korrektur_grund     TEXT,
    CONSTRAINT id_correction_is_linked CHECK (
        (rechnungsart = 'RECHNUNG'  AND original_draft_id IS NULL AND korrektur_grund IS NULL)
     OR (rechnungsart <> 'RECHNUNG' AND original_draft_id IS NOT NULL AND korrektur_grund IS NOT NULL)
    ),

    -- invoic-checker outcome, and the findings that produced it. Storing only
    -- the outcome left an operator staring at 'Warn' with no way to learn why.
    check_outcome       TEXT        NOT NULL
                        CHECK (check_outcome IN ('Ok', 'Warn', 'Dispute')),
    check_findings      JSONB       NOT NULL DEFAULT '[]',
    -- `grid_billing::SettlementWarning`s — what the engine could not do (a levy
    -- with no published rate, a KA above the KAV ceiling).
    settlement_warnings JSONB       NOT NULL DEFAULT '[]',

    status              TEXT        NOT NULL DEFAULT 'draft'
                        CHECK (status IN ('draft', 'dispatched', 'paid', 'disputed', 'rejected')),
    dispatch_ref        TEXT,       -- makod process UUID (when dispatched)
    dispatched_at       TIMESTAMPTZ,
    -- REMADV 33001 reference, kept for the § 14b UStG / § 147 AO audit trail.
    remadv_ref          TEXT,
    -- REMADV 33002/33003/33004: the EDIFACT ERC code and the LF's reason. Kept
    -- separate from `reject_reason` — a dispute by the counterparty and an
    -- operator's own rejection are different events with different consequences.
    dispute_erc_code    TEXT,
    dispute_reason      TEXT,
    reject_reason       TEXT,

    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE invoice_drafts IS
    'NNE/MMM/MSB/AWH invoices settled by the grid-billing crate. '
    'Lifecycle: draft → dispatched → paid | disputed; draft → rejected. '
    'Storno/Korrektur chains via original_draft_id; originals are never mutated.';

COMMENT ON COLUMN invoice_drafts.settlement_input IS
    'The request the settlement was computed from. Replaying it reproduces the '
    'invoice exactly, which is what a Storno negates and what an audit checks.';

COMMENT ON COLUMN invoice_drafts.brutto_eur_units IS
    'What is actually owed: netto + steuer, × 10⁻⁵ EUR as an integer.';

-- Every read path is tenant-scoped, so every index leads with the tenant.
-- The listing orders by `(created_at DESC, id DESC)` and pages by that same
-- pair, so a keyset cursor walks this index instead of re-reading the prefix
-- `OFFSET n` would discard.
CREATE INDEX id_tenant_created ON invoice_drafts (tenant, created_at DESC, id DESC);
CREATE INDEX id_tenant_malo    ON invoice_drafts (tenant, malo_id, created_at DESC);
CREATE INDEX id_tenant_status  ON invoice_drafts (tenant, status, pid);
CREATE INDEX id_tenant_period  ON invoice_drafts (tenant, period_from DESC, created_at DESC);
CREATE INDEX id_tenant_sender  ON invoice_drafts (tenant, sender_mp_id, created_at DESC);
CREATE INDEX id_original_draft ON invoice_drafts (original_draft_id)
    WHERE original_draft_id IS NOT NULL;

-- §14 Abs. 4 Nr. 4 UStG: an invoice number identifies exactly one invoice.
CREATE UNIQUE INDEX id_rechnungsnummer_unique
    ON invoice_drafts (tenant, rechnungsnummer);

-- Double-billing guard: one live RECHNUNG per MaLo × period × PID × tenant.
-- Storno/Korrektur are excluded (they reference an original by design), and so
-- is a rejected draft — rejecting is how an operator reopens a period.
--
-- **Abschlagsrechnungen are excluded here.** A period legitimately carries
-- several: an Abschlag is a payment on account, and a monthly Abschlag against a
-- yearly period is the ordinary case. Its invoice number keeps them distinct,
-- and the Abschlussrechnung reconciles them by that number. They get their own,
-- looser guard below rather than none at all.
CREATE UNIQUE INDEX id_no_double_billing
    ON invoice_drafts (tenant, malo_id, period_from, period_to, pid)
    WHERE rechnungsart = 'RECHNUNG' AND status <> 'rejected' AND pid <> 31001;

COMMENT ON INDEX id_no_double_billing IS
    'One live RECHNUNG per MaLo, period and PID per tenant. '
    'Storno/Korrektur and rejected drafts are excluded; Abschlagsrechnungen are '
    'guarded by id_one_abschlag_per_invoice_date instead.';

-- Abschlagsrechnungen are excluded above because a period legitimately carries
-- several — but "several" is not "unbounded", and without a guard a replayed
-- `POST /billing/run` produces a second Abschlag under a fresh invoice number
-- that the Abschlussrechnung then deducts twice.
--
-- The Rechnungsdatum separates them: instalments are billed on a cadence and so
-- differ by it, while a replay of one run does not. Refusing a same-day
-- duplicate is recoverable; a silent second Abschlag is not.
CREATE UNIQUE INDEX id_one_abschlag_per_invoice_date
    ON invoice_drafts (tenant, malo_id, period_from, period_to, invoice_date)
    WHERE rechnungsart = 'RECHNUNG' AND status <> 'rejected' AND pid = 31001;

COMMENT ON INDEX id_one_abschlag_per_invoice_date IS
    'One Abschlagsrechnung per MaLo, period and Rechnungsdatum. The cadence '
    'separates instalments; a replayed billing run does not.';

-- One *live* reversal per invoice. A second Stornorechnung credits the
-- counterparty twice, and nothing downstream notices: both are well-formed
-- documents that reference the same original.
--
-- A **rejected** Storno is excluded, for the same reason a rejected RECHNUNG is
-- excluded from `id_no_double_billing`: rejecting is how an operator discards a
-- draft that never left the house. Counting one left the original stranded —
-- it could not be reversed (this index refused a second attempt) and it could
-- not be corrected honestly either (`has_storno` said it was already reversed),
-- so a dispatched invoice had no way forward at all.
CREATE UNIQUE INDEX id_one_storno_per_original
    ON invoice_drafts (tenant, original_draft_id)
    WHERE rechnungsart = 'STORNORECHNUNG' AND status <> 'rejected';

-- ── Abschlag consumption ──────────────────────────────────────────────────────
--
-- An Abschlagsrechnung is a payment on account and is settled **once**: the
-- Abschlussrechnung that names it deducts its gross from what is owed
-- (§14 Abs. 5 UStG — the Anzahlung was taxed on receipt, so only the balance
-- moves). Nothing tied the two documents together, so two consecutive monthly
-- NN-Rechnungen could each name the same Abschlag and each deduct it: both
-- well-formed, both passing INVOIC rule [526] on their own, and the second one
-- collecting money the Netzbetreiber already had.
--
-- This table is that tie. One row per consumed Abschlag, and the primary key is
-- the guard: a second invoice naming it is refused rather than stored.
--
-- Rows are **released**, not kept, when the consuming invoice stops standing —
-- a Storno cancels its deductions with it, and rejecting a draft reopens the
-- period — so the Abschlag becomes available to the Korrekturrechnung that
-- replaces it.
CREATE TABLE abschlag_verrechnungen (
    tenant            TEXT        NOT NULL,
    -- The Abschlagsrechnung (PID 31001) being settled.
    abschlag_draft_id UUID        NOT NULL REFERENCES invoice_drafts(id) ON DELETE RESTRICT,
    -- The invoice that deducts it.
    invoice_draft_id  UUID        NOT NULL REFERENCES invoice_drafts(id) ON DELETE CASCADE,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT av_one_invoice_per_abschlag PRIMARY KEY (tenant, abschlag_draft_id)
);

COMMENT ON TABLE abschlag_verrechnungen IS
    'Which invoice settled which Abschlagsrechnung. One row per Abschlag: an '
    'Anzahlung is deducted once, and the primary key refuses a second deduction. '
    'Released when the consuming invoice is reversed or rejected.';

-- The release path deletes by consuming invoice.
CREATE INDEX av_invoice ON abschlag_verrechnungen (tenant, invoice_draft_id);

-- ── Redispatch 2.0 Kostenblatt ────────────────────────────────────────────────
-- BK6-20-061 §4.2: the VNB submits a monthly Kostenblatt to the ÜNB by the 15th
-- of the following month. One row per activation per TechnischeRessource.

CREATE TABLE kostenblatt_records (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant                  TEXT        NOT NULL,

    activation_id           TEXT        NOT NULL,   -- mako-engine process UUID
    tr_id                   TEXT        NOT NULL,   -- TechnischeRessource-ID
    malo_id                 TEXT,

    period_year             SMALLINT    NOT NULL,
    period_month            SMALLINT    NOT NULL CHECK (period_month BETWEEN 1 AND 12),

    uenb_mp_id              TEXT        NOT NULL,   -- ÜNB receiving the Kostenblatt
    vnb_mp_id               TEXT        NOT NULL,   -- VNB (our NB / sender)

    dispatch_kwh            NUMERIC(18, 3) NOT NULL DEFAULT 0,
    arbeitspreis_eur_per_kwh NUMERIC(12, 6) NOT NULL DEFAULT 0,
    einsatzkosten_eur       NUMERIC(16, 5)
        GENERATED ALWAYS AS (dispatch_kwh * arbeitspreis_eur_per_kwh) STORED,

    activation_start_utc    TIMESTAMPTZ,
    activation_end_utc      TIMESTAMPTZ,
    CONSTRAINT kb_window_ordered CHECK (
        activation_start_utc IS NULL
     OR activation_end_utc   IS NULL
     OR activation_end_utc > activation_start_utc
    ),

    -- Provenance of dispatch_kwh for § 147 AO / GoBD auditability:
    --   'lastgang_sum'    — summed from edmd 15-min intervals (most precise)
    --   'einspeisung_tag' — fallback: edmd EINSPEISUNG over the Berlin delivery day
    --   'manual_override' — supplied by the operator
    dispatch_source         TEXT        CHECK (dispatch_source IN (
                                'lastgang_sum', 'einspeisung_tag', 'manual_override'
                            )),

    kosten_json             JSONB,      -- typed BO4E Kosten for CIM export

    -- The record's lifecycle as this service drives it: quantified, then
    -- submitted to the ÜNB. `confirmed` / `disputed` / `paid` are deliberately
    -- absent — they are the ÜNB's answer, and netzbilanzd has no leg that
    -- receives one. Listing a state nothing can write makes the schema claim a
    -- lifecycle the code does not have, and a reader cannot tell the difference
    -- between "not implemented" and "never reached".
    status                  TEXT        NOT NULL DEFAULT 'pending'
                            CHECK (status IN ('pending', 'submitted')),
    submitted_at            TIMESTAMPTZ,
    dispatch_ref            TEXT,

    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (tenant, activation_id, tr_id)
);

COMMENT ON TABLE kostenblatt_records IS
    'Redispatch 2.0 Kostenblatt (BK6-20-061 §4.2). '
    'One row per activation per TechnischeRessource; due the 15th of the following month.';

COMMENT ON COLUMN kostenblatt_records.einsatzkosten_eur IS
    'GENERATED: dispatch_kwh × arbeitspreis_eur_per_kwh — cannot drift from its factors.';

CREATE INDEX kb_period            ON kostenblatt_records (tenant, period_year, period_month, status);
CREATE INDEX kb_activation        ON kostenblatt_records (tenant, activation_id);
CREATE INDEX kb_activation_window ON kostenblatt_records (activation_start_utc, activation_end_utc)
    WHERE activation_start_utc IS NOT NULL;
-- Gap detection: an activation registered but never quantified.
CREATE INDEX kb_gaps              ON kostenblatt_records (tenant, period_year, period_month)
    WHERE dispatch_kwh = 0 AND dispatch_source IS NULL AND status = 'pending';

-- ── Fremdkosten (external cost pass-through) ──────────────────────────────────
-- Typed BO4E Fremdkosten linked to a draft. Merged into the `Rechnung`'s own
-- `fremdkosten` field at dispatch — BO4E models this as a first-class field, so
-- it does not travel as a free-text ZusatzAttribut.

CREATE TABLE fremdkosten_records (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant              TEXT        NOT NULL,
    draft_id            UUID        NOT NULL REFERENCES invoice_drafts(id) ON DELETE CASCADE,
    fremdkosten_json    JSONB       NOT NULL,   -- rubo4e::current::Fremdkosten
    bezeichnung         TEXT,
    total_eur           NUMERIC(16, 5) NOT NULL DEFAULT 0,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant, draft_id)
);

COMMENT ON TABLE fremdkosten_records IS
    '§ 147 AO / GoBD typed external-cost pass-through, merged into Rechnung.fremdkosten on dispatch.';

CREATE INDEX fk_tenant ON fremdkosten_records (tenant, created_at DESC);

-- ══ MaBiS — Summenzeitreihe aggregation and submission tracking ══════════════
--
-- `submission_runs`: one row per aggregation and submission attempt.
-- `submission_series`: one MSCONS 13003 per Bilanzierungsgebiet within a run.
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
    -- The tenant's own Bilanzierungsgebiet, as configured. A run may span
    -- several — what actually went out per territory is `submission_series`.
    -- This column keys the scheduler's duplicate guard and the inbound
    -- Datenstatus/Prüfmitteilung lookups, which name a territory the BIKO
    -- echoes back.
    bilanzierungsgebiet_id  TEXT        NOT NULL,
    period_from             DATE        NOT NULL,
    period_to               DATE        NOT NULL,
    -- Ascending version within (bilanzierungsgebiet_id, period). Sent as
    -- MSCONS SG6 DTM+293 (format 304), which carries whole seconds and no more
    -- — so the stored value is truncated to seconds. A microsecond-precision
    -- version can never be matched again when the BIKO echoes it back in a
    -- Datenstatus or Prüfmitteilung.
    version                 TIMESTAMPTZ NOT NULL DEFAULT date_trunc('second', now()),
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
    -- Only the four states the service actually writes: a state with no
    -- producer is one a reader filters on and that can never occur.
    status                  TEXT        NOT NULL DEFAULT 'pending'
                            CHECK (status IN ('pending','submitted','acked','failed')),
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
CREATE INDEX sr_status    ON submission_runs (status) WHERE status <> 'acked';
CREATE INDEX sr_tenant    ON submission_runs (tenant);
-- What a retry must not re-file: the territories acked for a Bilanzierungsmonat
-- within the same filing lineage (`corrects_run_id`). Keyed on the period rather
-- than the run, because a retry is a new run with no series rows of its own.
CREATE INDEX sr_tenant_period ON submission_runs (tenant, period_from, period_to);
CREATE INDEX sr_triggered ON submission_runs (triggered_at DESC);

-- Finding the version that settles: the highest one the BIKO has marked
-- 'Abrechnungsdaten' (§3.8.3).
CREATE INDEX sr_settling_version ON submission_runs
    (tenant, bilanzierungsgebiet_id, period_from, period_to, version DESC)
    WHERE datenstatus IN ('ABRECHNUNGSDATEN', 'ABRECHNUNGSDATEN_KBKA');

-- ── Per-territory submission ──────────────────────────────────────────────────
--
-- A run aggregates every MaLo the tenant holds, and MaBiS settles **per
-- Bilanzierungsgebiet**, so one run files one MSCONS 13003 per territory.
--
-- This table is what actually went out, per territory; `submission_runs` keeps
-- the aggregate. The distinction matters because an acked Summenzeitreihe
-- cannot be withdrawn: when one territory of a run fails, the others are
-- already filed, and a retry has to know which.

CREATE TABLE submission_series (
    id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id                  UUID        NOT NULL REFERENCES submission_runs(id) ON DELETE CASCADE,
    -- The territory this series settles, resolved from marktd master data.
    bilanzierungsgebiet_id  TEXT        NOT NULL,
    -- The MaBiS-Zählpunkt it is filed under (MSCONS SG6 LOC+172).
    mabis_zp_id             TEXT        NOT NULL,
    malo_count              INTEGER     NOT NULL DEFAULT 0,
    interval_count          INTEGER     NOT NULL DEFAULT 0,
    total_kwh               TEXT,
    -- 'pending' until the submission is attempted, then 'acked' or 'failed'.
    -- A territory that reached the BIKO stays 'acked' even when a later one in
    -- the same run fails: recording it as failed would send a correction for a
    -- submission that was accepted.
    status                  TEXT        NOT NULL DEFAULT 'pending'
                            CHECK (status IN ('pending', 'acked', 'failed')),
    message_ref             TEXT,
    process_id              UUID,
    error_msg               TEXT,
    submitted_at            TIMESTAMPTZ,

    -- One series per territory per run.
    CONSTRAINT ss_one_per_territory UNIQUE (run_id, bilanzierungsgebiet_id),
    -- An acked series names what was filed; a failed one names why it was not.
    CONSTRAINT ss_acked_has_reference CHECK (
        status <> 'acked' OR message_ref IS NOT NULL
    )
);

COMMENT ON TABLE submission_series IS
    'One MSCONS 13003 per Bilanzierungsgebiet within a run. A run files one per territory; '
    'this records which of them reached the BIKO.';

CREATE INDEX ss_run     ON submission_series (run_id);
CREATE INDEX ss_gebiet  ON submission_series (bilanzierungsgebiet_id);
-- Territories of a run that did not reach the BIKO — the retry list.
CREATE INDEX ss_open    ON submission_series (run_id) WHERE status <> 'acked';

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

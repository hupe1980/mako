-- ── billingd schema — Energy Billing Engine ──────────────────────────────────
--
-- `billing_records`: append-only audit log of every issued document.
--   Full rubo4e::current::Rechnung JSONB plus the EN 16931 semantic model, for
--   § 14b UStG / § 147 AO / GoBD. An invoice is a Buchungsbeleg: **8 years**,
--   reduced from 10 by BEG IV with effect from 01.01.2025. GoBD additionally
--   requires Unveraenderbarkeit, which is why an issued row is never rewritten
--   — see the `outcome` guard on the upsert in `pg::insert_billing_record`.
--   Covers Einzelrechnung, Storno/Korrektur chains, B2B Sammelrechnungen,
--   § 42b GGV bundles and § 41e VPP settlements.
--
-- `invoice_number_series`: § 14 Abs. 4 Nr. 4 UStG fortlaufende Rechnungsnummer.
-- `billing_run_log`: § 40b EnWG monthly batch-run audit.
-- `abrechnungsinfo_log`: § 40b Abs. 3 EnWG monthly info dispatch claim.
-- `vpp_dispatch_ledger`: idempotency guard for de.mako.vpp.dispatch.confirmed.

-- ── § 14 Abs. 4 Nr. 4 UStG — the number series ───────────────────────────────
--
-- The law wants a **fortlaufende Nummer, einmalig vergeben**. Deriving one from
-- the billed facts (`BILL-{malo}-{product}-{period_from}`) satisfies neither:
-- it is not sequential, and — the defect that made the documented
-- Storno-und-Neuberechnung flow impossible — re-billing a cancelled period
-- produced the *same* string as the cancelled original, which
-- `br_unique_rechnungsnummer` refuses. The store told callers to do something
-- the store forbade.
--
-- One counter per (tenant, series, year): `RE` ordinary invoice, `SR`
-- consolidated document, `ST` Storno/Korrektur, `VG` § 41e Gutschrift. Numbers
-- render as `RE-2026-000123`.
--
-- Gaps are legal and expected — a number is allocated before the engine runs,
-- so a refused calculation burns one (UStAE 14.5 Abs. 11: a lückenlose Abfolge
-- is not required, only that no number is issued twice).

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


CREATE TABLE invoice_number_series (
    tenant      TEXT     NOT NULL,
    series      TEXT     NOT NULL,
    year        SMALLINT NOT NULL,
    -- The most recently issued value; the next allocation returns this + 1.
    last_value  BIGINT   NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant, series, year)
);

COMMENT ON TABLE invoice_number_series IS
    '§ 14 Abs. 4 Nr. 4 UStG: per-tenant, per-series, per-year counter behind the '
    'fortlaufende Rechnungsnummer. Allocation is an upsert returning last_value, '
    'so concurrent runs serialise on the row and never issue a number twice.';

-- ── Invoice records ───────────────────────────────────────────────────────────

CREATE TABLE billing_records (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id             TEXT        NOT NULL,
    lf_mp_id            TEXT        NOT NULL,
    product_code        TEXT        NOT NULL,
    -- Billing calculation template; determines which billingd calculator is invoked
    category            TEXT        NOT NULL CHECK (category IN (
                            'STROM', 'GAS', 'WAERME', 'WASSER', 'SOLAR', 'EEG',
                            'EINSPEISUNG', 'WAERMEPUMPE', 'WALLBOX', 'HEMS',
                            'EMOBILITY', 'ENERGIEDIENSTLEISTUNG', 'SHARING',
                            'SAMMEL', 'VPP', 'TARIFWECHSEL'
                        )),
    -- § 14 Abs. 4 Nr. 4 UStG: the invoice number is einmalig. Enforced by
    -- `br_unique_rechnungsnummer` rather than left to the JSONB, so a
    -- collision is a database error at write time and not a finding an
    -- auditor makes years later.
    rechnungsnummer     TEXT        NOT NULL,
    period_from         DATE        NOT NULL,
    period_to           DATE        NOT NULL,

    -- Full rubo4e::current::Rechnung JSONB (§ 14b UStG / § 147 AO: 8 years)
    rechnung_json       JSONB       NOT NULL,
    bo4e_version        TEXT        NOT NULL DEFAULT '202607.1.0',
    -- EN 16931 semantic invoice model (serde JSON) — the source for XRechnung /
    -- CII / PEPPOL-UBL rendering, mapped at bill time with full per-line VAT.
    en16931_json        JSONB,
    -- Why this record carries no `en16931_json`, when the reason is a property
    -- of the invoice rather than a missing step. Today one reason exists: a
    -- document mixing a not-subject-to-VAT line (EN 16931 category `O` — a
    -- hoheitliche Abwassergebühr) with any other category, which BR-O-11 ff.
    -- forbid. The paper invoice is lawful and still issued; there is simply no
    -- valid e-invoice of it, and the render endpoints say so instead of
    -- telling the operator to re-run a calculation that would refuse again.
    en16931_blocked     TEXT,
    -- What kind of document this is. A GEBUEHRENBESCHEID fixes a public-law
    -- Gebühr (EN 16931 category `O`), issued beside the supply invoice of the
    -- same period because BR-O-11 ff. forbid the two on one e-invoice; it is
    -- no invoice and carries no EN 16931 model. `bescheid_json` is the view
    -- outputd's GEBUEHRENBESCHEID kind renders, and only a Bescheid has one.
    dokumentart         TEXT        NOT NULL DEFAULT 'RECHNUNG'
                        CHECK (dokumentart IN ('RECHNUNG', 'GEBUEHRENBESCHEID')),
    bescheid_json       JSONB,
    CONSTRAINT br_bescheid_view CHECK ((dokumentart = 'GEBUEHRENBESCHEID') = (bescheid_json IS NOT NULL)),
    -- § 14 Abs. 2 Satz 2 Nr. 1 UStG: whether the Leistungsempfänger is a
    -- Unternehmer receiving for his Unternehmen, as the contract's customer
    -- segment said at billing. NULL = not recorded; the paper path refuses the
    -- § 27 Abs. 38 UStG grace on it rather than assume either answer.
    empfaenger_unternehmer BOOLEAN,
    -- The Rechnungsaussteller's tax identity (§ 14 Abs. 4 Nr. 2 UStG) the
    -- record was issued under — what selects its § 27 Abs. 38 Nr. 2 UStG
    -- Gesamtumsatz. NULL in both = not recorded, which refuses that grace.
    aussteller_ust_id      TEXT,
    aussteller_steuernummer TEXT,

    -- Monetary summary for fast reporting (avoids JSONB parse).
    --
    -- NOT NULL: every write path computes both, and a Stornorechnung is booked
    -- by negating them. A nullable column lets a correction of a record with no
    -- totals book a zero reversal against an invoice that carries an amount,
    -- and the negated JSON would still reconcile — so the constraint is what
    -- keeps the ledger and the document from disagreeing.
    total_netto_eur     NUMERIC(16, 5) NOT NULL,
    total_brutto_eur    NUMERIC(16, 5) NOT NULL,

    outcome             TEXT        NOT NULL DEFAULT 'generated' CHECK (outcome IN (
                            'generated',    -- withheld: calculated, not released
                            'dispatched',   -- issued and released for delivery
                            'paid',         -- payment confirmed
                            'partial',      -- partial payment
                            'disputed',     -- dispute raised
                            'cancelled'     -- fully reversed by a Stornorechnung
                        )),

    -- Correction invoice fields (§ 147 AO / GoBD Stornorechnung / Korrekturrechnung)
    is_correction       BOOLEAN     NOT NULL DEFAULT false,
    original_record_id  UUID        REFERENCES billing_records(id) ON DELETE SET NULL,
    correction_reason   TEXT,

    -- B2B Sammelrechnung / GGV bundle: NULL = standalone Einzelrechnung
    sammelrechnung_id   UUID        REFERENCES billing_records(id) ON DELETE SET NULL,

    -- ── Risk scoring (deterministic release gate) ────────────────────────────
    -- Score 0–100 from the coded findings in risk_findings; band decides the
    -- action: AUTO_RELEASED/SAMPLE dispatch immediately, REVIEW dispatches but
    -- queues for analysts until reviewed_by/-at is set via
    -- POST /api/v1/billing/{id}/reviewed, HELD blocks dispatch until
    -- released_by/-at is set via POST /api/v1/billing/{id}/release.
    risk_score        SMALLINT,
    risk_band         TEXT CHECK (risk_band IN ('AUTO_RELEASED','SAMPLE','REVIEW','HELD')),
    risk_findings     JSONB,
    released_by       TEXT,
    released_at       TIMESTAMPTZ,
    reviewed_by       TEXT,
    reviewed_at       TIMESTAMPTZ,
    -- Each stamp has its decider, and only its own band carries it.
    CONSTRAINT br_release_attributed CHECK ((released_by IS NULL) = (released_at IS NULL)),
    CONSTRAINT br_review_attributed  CHECK ((reviewed_by IS NULL) = (reviewed_at IS NULL)),
    CONSTRAINT br_review_is_review   CHECK (reviewed_at IS NULL OR risk_band = 'REVIEW'),

    -- An issued document records the template that produced it, so its
    -- appearance is reproducible for as long as the document must be kept.
    -- Templates live in outputd (content-addressed, append-only); the hash is
    -- outputd's X-Mako-Template-Hash response header, pinned here after
    -- dispatch. It crosses a service boundary, so no foreign key can guard it
    -- — outputd's append-only store policy is what keeps it resolvable.
    template_hash       TEXT,

    dispatched_at       TIMESTAMPTZ,

    -- § 40c Abs. 2 EnWG: the last day this invoice may reach the customer,
    -- from `mako_fristen::vorlauf::abrechnung_faellig`. Set on every invoice
    -- of a supply period (the run and `/calculate`); NULL on documents the
    -- paragraph does not govern (a § 41e VPP settlement, a Sammelrechnung
    -- wrapper, a GGV bundle, a Gebührenbescheid).
    due_on              DATE,

    -- Rechnung nachrechnen. `seal_sha256` is the SHA-256 over the
    -- RFC 8785 (JCS) canonical form of `rechnung_json` at issue; a verdict
    -- states whether the stored document still hashes to it. The verify token
    -- lets anyone holding the invoice recompute it without signing in; only
    -- its SHA-256 is stored, so a database read does not yield a working link.
    seal_sha256         TEXT,
    verify_token_sha256 TEXT,
    verify_revoked_at   TIMESTAMPTZ,
    -- Who revoked the token (the OIDC subject).
    verify_revoked_by   TEXT,
    CONSTRAINT br_revocation_attributed
        CHECK ((verify_revoked_at IS NULL) = (verify_revoked_by IS NULL)),

    tenant              TEXT        NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX br_verify_token ON billing_records (verify_token_sha256)
    WHERE verify_token_sha256 IS NOT NULL;

COMMENT ON TABLE billing_records IS
    '§ 14b UStG / § 147 AO / GoBD: 8-year audit ledger for all issued documents. '
    'Originals, Storno/Korrektur chains, B2B Sammelrechnungen, § 42b GGV bundles '
    'and § 41e VPP settlements.';

COMMENT ON COLUMN billing_records.rechnungsnummer IS
    '§ 14 Abs. 4 Nr. 4 UStG: einmalige Rechnungsnummer, unique per tenant.';

COMMENT ON COLUMN billing_records.is_correction IS
    'TRUE = Stornorechnung / Korrekturrechnung (rubo4e istOriginal=false). '
    'Positions are negated relative to original_record_id.';

COMMENT ON COLUMN billing_records.outcome IS
    'generated = the risk gate withheld the document; it exists but has not been '
    'released. dispatched = issued and released for delivery, whether or not an '
    'ERP webhook is configured — issuance is a property of the document, not of '
    'the operator''s integrations, and tying it to one left every invoice a '
    'permanent draft (rewritable, never template-pinned) for operators without '
    'an ERP. cancelled = fully reversed by a Stornorechnung; a cancelled period '
    'drops out of br_unique_original, so the corrected amounts can be re-billed '
    'as a fresh original — the Storno-und-Neuberechnung flow German accounting '
    'expects (with a fresh number from invoice_number_series).';

COMMENT ON COLUMN billing_records.sammelrechnung_id IS
    'FK to the consolidated document (category=SAMMEL) this per-MaLo record '
    'belongs to. NULL = standalone Einzelrechnung.';

COMMENT ON COLUMN billing_records.template_hash IS
    'The outputd template hash that rendered this invoice''s PDF. NULL until '
    'the document has been rendered, and for XML-only records.';

-- § 14 Abs. 4 Nr. 4 UStG — the number series is unique per tenant.
CREATE UNIQUE INDEX br_unique_rechnungsnummer
    ON billing_records (tenant, rechnungsnummer);

-- One live original per (malo, lf, period, product, tenant).
--
-- Corrections, per-MaLo children of a bundle and cancelled periods are
-- excluded: a Storno releases the period so the corrected amounts can be
-- re-billed, and per-dispatch VPP settlements are guarded by
-- `vpp_dispatch_ledger` instead — several dispatches legitimately settle
-- within the same calendar day.
CREATE UNIQUE INDEX br_unique_original
    ON billing_records (malo_id, lf_mp_id, period_from, period_to, product_code, tenant)
    WHERE is_correction = false
      AND sammelrechnung_id IS NULL
      AND outcome <> 'cancelled'
      AND category <> 'VPP';

-- No two live originals of one MaLo, Lieferant and product cover the same
-- day. `br_unique_original` only matches an identical period, so without this
-- a Schlussrechnung could be issued over days an earlier invoice already
-- billed, and the customer would pay them twice.
CREATE EXTENSION IF NOT EXISTS btree_gist;
ALTER TABLE billing_records ADD CONSTRAINT br_no_overlapping_original
    EXCLUDE USING gist (
        tenant WITH =, malo_id WITH =, lf_mp_id WITH =, product_code WITH =,
        daterange(period_from, period_to, '[]') WITH &&
    ) WHERE (is_correction = false AND sammelrechnung_id IS NULL
             AND outcome <> 'cancelled' AND category <> 'VPP'
             AND dokumentart = 'RECHNUNG');

-- An issued document is what the customer holds: its stored form and its
-- seal never change afterwards. A draft (`generated`) may be replaced by a
-- re-run; once issued only the status, the derived columns and the verify
-- token's revocation move. The seal is written once, by the issuing
-- transaction, after the record turned `dispatched`.
CREATE OR REPLACE FUNCTION billing_records_immutable_after_issue() RETURNS trigger
    LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.outcome <> 'generated' AND (
           NEW.rechnung_json IS DISTINCT FROM OLD.rechnung_json
        OR (OLD.seal_sha256 IS NOT NULL AND NEW.seal_sha256 IS DISTINCT FROM OLD.seal_sha256)
        OR (OLD.verify_token_sha256 IS NOT NULL
            AND NEW.verify_token_sha256 IS DISTINCT FROM OLD.verify_token_sha256)
        OR NEW.total_netto_eur IS DISTINCT FROM OLD.total_netto_eur
        OR NEW.total_brutto_eur IS DISTINCT FROM OLD.total_brutto_eur) THEN
        RAISE EXCEPTION 'billing record % is issued; its document and seal are immutable', OLD.id
            USING ERRCODE = 'integrity_constraint_violation';
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER billing_records_immutable_after_issue
    BEFORE UPDATE ON billing_records
    FOR EACH ROW EXECUTE FUNCTION billing_records_immutable_after_issue();

-- Period lookup per MaLo
CREATE INDEX br_malo_period   ON billing_records (malo_id, lf_mp_id, period_from DESC);
-- Outcome workflow
CREATE INDEX br_outcome       ON billing_records (outcome, lf_mp_id);
-- Analyst work list: what is still undecided
CREATE INDEX br_review_queue  ON billing_records (tenant, risk_score DESC)
    WHERE risk_band IN ('REVIEW','HELD') AND released_at IS NULL AND reviewed_at IS NULL;
-- Correction chain lookup
CREATE INDEX br_corrections   ON billing_records (original_record_id)
    WHERE is_correction = true;
-- Sammelrechnung group
CREATE INDEX br_sammel_group  ON billing_records (sammelrechnung_id)
    WHERE sammelrechnung_id IS NOT NULL;
-- Tenant-scoped reporting
CREATE INDEX br_tenant_period ON billing_records (tenant, period_from DESC);

-- ── Unbilled periods (§ 40c Abs. 2 EnWG watch) ───────────────────────────────

-- A billing period the scheduled run attempted and could not bill, with the
-- refusal it got and the § 40c Abs. 2 EnWG date it is owed by. A log line and
-- a counter do not tell a person which customer is about to miss a Frist; a
-- row does. The run writes it with one upsert per attempt; the trigger below
-- closes it when any path writes the period's invoice, and re-opens it when
-- that invoice is cancelled by a Storno.
CREATE TABLE unbilled_periods (
    tenant          TEXT        NOT NULL,
    malo_id         TEXT        NOT NULL,
    lf_mp_id        TEXT        NOT NULL,
    period_from     DATE        NOT NULL,
    period_to       DATE        NOT NULL,
    -- The period ends with the supply: it is owed an Abschlussrechnung.
    final_invoice   BOOLEAN     NOT NULL DEFAULT false,
    -- The stable refusal code (`NO_METER_DATA`, `RATE_REFUSED`,
    -- `ABSCHLAEGE_UNBEKANNT`, `STORNIERT`, …); the Desk words it from its
    -- catalogue.
    reason_code     TEXT        NOT NULL,
    -- The refusal's developer message; shown only under „Details".
    message         TEXT        NOT NULL,
    due_on          DATE        NOT NULL,
    attempts        INTEGER     NOT NULL DEFAULT 1 CHECK (attempts > 0),
    first_seen_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- The invoice that discharged the period; NULL while it is open.
    settled_by      UUID        REFERENCES billing_records(id) ON DELETE SET NULL,
    settled_at      TIMESTAMPTZ,
    CONSTRAINT up_settled_together CHECK ((settled_by IS NULL) = (settled_at IS NULL)),
    CONSTRAINT up_period CHECK (period_from <= period_to),
    PRIMARY KEY (tenant, malo_id, period_from, period_to)
);

CREATE INDEX up_open_by_due ON unbilled_periods (tenant, due_on)
    WHERE settled_by IS NULL;

-- An issued invoice for the period discharges it, whichever path wrote it —
-- the run, `/calculate`, a manual re-bill after a Storno. A record the risk
-- gate holds (`generated`) has reached nobody, so it discharges nothing until
-- it is released.
CREATE OR REPLACE FUNCTION unbilled_periods_settle() RETURNS trigger
    LANGUAGE plpgsql AS $$
BEGIN
    UPDATE unbilled_periods
       SET settled_by = NEW.id, settled_at = now()
     WHERE tenant = NEW.tenant AND malo_id = NEW.malo_id
       AND period_from = NEW.period_from AND period_to = NEW.period_to
       AND settled_by IS NULL;
    RETURN NULL;
END $$;

CREATE TRIGGER billing_records_settle_unbilled
    AFTER INSERT OR UPDATE OF outcome ON billing_records
    FOR EACH ROW
    WHEN (NEW.outcome = 'dispatched' AND NEW.is_correction = false
          AND NEW.dokumentart = 'RECHNUNG')
    EXECUTE FUNCTION unbilled_periods_settle();

-- A Storno takes the period's invoice back, so the period is owed one again
-- by the same date. Only an invoice that carries a § 40c date re-opens a watch.
CREATE OR REPLACE FUNCTION unbilled_periods_reopen() RETURNS trigger
    LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO unbilled_periods
        (tenant, malo_id, lf_mp_id, period_from, period_to, final_invoice,
         reason_code, message, due_on)
    VALUES (NEW.tenant, NEW.malo_id, NEW.lf_mp_id, NEW.period_from, NEW.period_to,
            NEW.rechnung_json @> '{"zusatzAttribute": [{"name": "mako:rechnungsart", "wert": "SCHLUSSRECHNUNG"}]}',
            'STORNIERT', 'invoice ' || NEW.rechnungsnummer || ' was cancelled by a Storno',
            NEW.due_on)
    ON CONFLICT (tenant, malo_id, period_from, period_to) DO UPDATE
       SET reason_code = 'STORNIERT', message = EXCLUDED.message,
           settled_by = NULL, settled_at = NULL, last_seen_at = now();
    RETURN NULL;
END $$;

CREATE TRIGGER billing_records_reopen_unbilled
    AFTER UPDATE OF outcome ON billing_records
    FOR EACH ROW
    WHEN (NEW.outcome = 'cancelled' AND OLD.outcome <> 'cancelled'
          AND NEW.is_correction = false AND NEW.due_on IS NOT NULL)
    EXECUTE FUNCTION unbilled_periods_reopen();

-- ── Strom- and Energiesteuer lines ────────────────────────────────────────────

-- One typed tax line of an issued document, written by the statement that
-- writes its billing_records row. The Steueranmeldung (§ 8 StromStG, § 39
-- EnergieStG) is a sum over these, grouped by ground; a Stornorechnung carries
-- its original's lines negated, so a cancelled document nets to zero.
CREATE TABLE steuerzeilen (
    id                UUID          PRIMARY KEY DEFAULT gen_random_uuid(),
    billing_record_id UUID          NOT NULL REFERENCES billing_records(id) ON DELETE CASCADE,
    -- The line's place among its document's tax lines, from 1.
    nr                INTEGER       NOT NULL CHECK (nr > 0),
    tenant            TEXT          NOT NULL,
    steuer            TEXT          NOT NULL CHECK (steuer IN ('STROMSTEUER', 'ENERGIESTEUER')),
    art               TEXT          NOT NULL CHECK (art IN ('REGELSATZ', 'ERMAESSIGUNG', 'BEFREIUNG')),
    -- The provision the line is taxed or exempted under, as the invoice cites it.
    rechtsgrundlage   TEXT          NOT NULL,
    menge             NUMERIC(18, 6) NOT NULL,
    einheit           TEXT          NOT NULL CHECK (einheit IN ('kWh', 'kWh_Hs')),
    satz_ct           NUMERIC(12, 6) NOT NULL,
    betrag_eur        NUMERIC(16, 5) NOT NULL,
    period_from       DATE          NOT NULL,
    period_to         DATE          NOT NULL,
    created_at        TIMESTAMPTZ   NOT NULL DEFAULT now(),
    -- A tax-free quantity carries no tax.
    CONSTRAINT sz_befreiung_ohne_betrag
        CHECK (art <> 'BEFREIUNG' OR (satz_ct = 0 AND betrag_eur = 0)),
    -- Checked at the end of the statement: the upsert replacing a draft
    -- deletes the old lines and inserts the new in one statement.
    CONSTRAINT sz_unique_nr UNIQUE (billing_record_id, nr) DEFERRABLE INITIALLY IMMEDIATE
);

CREATE INDEX sz_tenant_period ON steuerzeilen (tenant, steuer, period_from);

-- ── Batch run log ─────────────────────────────────────────────────────────────

CREATE TABLE billing_run_log (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    lf_mp_id        TEXT        NOT NULL,
    tenant          TEXT        NOT NULL,
    billing_year    SMALLINT    NOT NULL,
    billing_month   SMALLINT    NOT NULL CHECK (billing_month BETWEEN 1 AND 12),
    run_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    records_count   INTEGER     NOT NULL DEFAULT 0,
    -- Periods the sweep deliberately did not bill (a JAEHRLICH settlement it
    -- cannot supply Abschläge for). Not a fault: kept apart from errors_count
    -- so a normally configured operator does not read `failed` every month.
    skipped_count   INTEGER     NOT NULL DEFAULT 0,
    errors_count    INTEGER     NOT NULL DEFAULT 0,
    status          TEXT        NOT NULL DEFAULT 'completed'
                    CHECK (status IN ('running', 'completed', 'failed')),
    UNIQUE (tenant, lf_mp_id, billing_year, billing_month)
);

COMMENT ON TABLE billing_run_log IS
    '§40b EnWG billing-run audit: one row per (tenant, lf, calendar month), '
    'accumulated across the daily worker sweeps of that month. Per-invoice '
    'idempotency lives in billing_records (unique malo+period+product); this '
    'table answers "did the scheduled runs of month X happen, and how did '
    'they go".';

-- ── § 40b Abs. 3 EnWG — monthly Abrechnungsinformation log ────────────────────
-- Customers with remote-readable meters (iMSys) receive a free monthly
-- consumption/cost information. One row per delivered info; the UNIQUE guard
-- makes the daily worker idempotent per month.

CREATE TABLE abrechnungsinfo_log (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant        TEXT        NOT NULL,
    malo_id       TEXT        NOT NULL,
    info_year     SMALLINT    NOT NULL,
    info_month    SMALLINT    NOT NULL CHECK (info_month BETWEEN 1 AND 12),
    sent_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant, malo_id, info_year, info_month)
);

COMMENT ON TABLE abrechnungsinfo_log IS
    '§ 40b Abs. 3 EnWG: monthly Abrechnungsinformation dispatch log for '
    'iMSys/fernauslesbare MaLos. UNIQUE guard = one info per MaLo and month. '
    'A claim is released again when the delivery it guards does not happen, '
    'so a transient failure postpones the info rather than cancelling it.';

-- ── VPP dispatch idempotency ──────────────────────────────────────────────────
-- Prevents double-billing when the outbox retries a de.mako.vpp.dispatch.confirmed
-- delivery. The §41e EnWG Aggregatorvertrag itself is Contract-context master
-- data and lives in `vertragd.aggregatorvertraege`; billingd reads it over HTTP.

CREATE TABLE vpp_dispatch_ledger (
    tx_id           TEXT        NOT NULL,
    tenant          TEXT        NOT NULL,
    -- The billing_records row generated for this dispatch, when one was.
    record_id       UUID        REFERENCES billing_records(id) ON DELETE SET NULL,
    processed_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tx_id, tenant)
);

COMMENT ON TABLE vpp_dispatch_ledger IS
    'Idempotency guard for de.mako.vpp.dispatch.confirmed webhook delivery. '
    'Prevents double-billing on outbox retry, and is the reason per-dispatch '
    'VPP settlements are exempt from br_unique_original.';

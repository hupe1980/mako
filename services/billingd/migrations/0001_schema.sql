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
-- `vpp_dispatch_ledger`: idempotency guard for de.vpp.dispatch.confirmed.

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

    -- Monetary summary for fast reporting (avoids JSONB parse)
    total_netto_eur     NUMERIC(16, 5),
    total_brutto_eur    NUMERIC(16, 5),

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
    -- queues for analysts, HELD blocks dispatch until released_by/-at is set
    -- via POST /api/v1/billing/{id}/release.
    risk_score        SMALLINT,
    risk_band         TEXT CHECK (risk_band IN ('AUTO_RELEASED','SAMPLE','REVIEW','HELD')),
    risk_findings     JSONB,
    released_by       TEXT,
    released_at       TIMESTAMPTZ,

    -- An issued document records the template that produced it, so its
    -- appearance is reproducible for as long as the document must be kept.
    -- Templates live in outputd (content-addressed, append-only); the hash is
    -- outputd's X-Mako-Template-Hash response header, pinned here after
    -- dispatch. It crosses a service boundary, so no foreign key can guard it
    -- — outputd's append-only store policy is what keeps it resolvable.
    template_hash       TEXT,

    dispatched_at       TIMESTAMPTZ,
    tenant              TEXT        NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

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

-- Period lookup per MaLo
CREATE INDEX br_malo_period   ON billing_records (malo_id, lf_mp_id, period_from DESC);
-- Outcome workflow
CREATE INDEX br_outcome       ON billing_records (outcome, lf_mp_id);
-- Analyst work list
CREATE INDEX br_review_queue  ON billing_records (tenant, risk_score DESC)
    WHERE risk_band IN ('REVIEW','HELD');
-- Correction chain lookup
CREATE INDEX br_corrections   ON billing_records (original_record_id)
    WHERE is_correction = true;
-- Sammelrechnung group
CREATE INDEX br_sammel_group  ON billing_records (sammelrechnung_id)
    WHERE sammelrechnung_id IS NOT NULL;
-- Tenant-scoped reporting
CREATE INDEX br_tenant_period ON billing_records (tenant, period_from DESC);

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
-- Prevents double-billing when the outbox retries a de.vpp.dispatch.confirmed
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
    'Idempotency guard for de.vpp.dispatch.confirmed webhook delivery. '
    'Prevents double-billing on outbox retry, and is the reason per-dispatch '
    'VPP settlements are exempt from br_unique_original.';

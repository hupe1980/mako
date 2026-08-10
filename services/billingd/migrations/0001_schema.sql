-- ── billingd schema — Energy Billing Engine ──────────────────────────────────
--
-- `billing_records`: immutable audit log of every generated invoice.
--   Full rubo4e::current::Rechnung JSONB for § 14b UStG / § 147 AO / GoBD
--   compliance. An invoice is a Buchungsbeleg: 8 years, reduced from 10 by
--   BEG IV with effect from 01.01.2025. GoBD additionally requires
--   Unveraenderbarkeit, which is why this table is append-only.
--   Supports Einzelrechnung, Korrektur/Storno (is_correction), and B2B Sammelrechnung.
--
-- `billing_run_log`: monthly batch run audit + idempotency guard.
--
-- `vpp_dispatch_ledger`: idempotency guard for de.vpp.dispatch.confirmed deliveries.

-- ── Invoice records ───────────────────────────────────────────────────────────

CREATE TABLE billing_records (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id             TEXT        NOT NULL,
    lf_mp_id            TEXT        NOT NULL,
    product_code        TEXT        NOT NULL,
    -- Billing calculation template; determines which billingd calculator is invoked
    category            TEXT        NOT NULL CHECK (category IN (
                            'STROM', 'GAS', 'WAERME', 'SOLAR', 'EEG', 'EINSPEISUNG',
                            'WAERMEPUMPE', 'WALLBOX', 'HEMS', 'EMOBILITY',
                            'ENERGIEDIENSTLEISTUNG', 'BUNDLE', 'SAMMEL',
                            'SHARING', 'VPP', 'WASSER', 'TARIFWECHSEL'
                        )),
    period_from         DATE        NOT NULL,
    period_to           DATE        NOT NULL,

    -- Full rubo4e::current::Rechnung JSONB (§ 14b UStG / § 147 AO: 8 years)
    rechnung_json       JSONB       NOT NULL,
    bo4e_version        TEXT        NOT NULL DEFAULT 'v202607.0.0',
    -- EN 16931 semantic invoice model (serde JSON) — the source for XRechnung /
    -- CII / PEPPOL-UBL rendering, mapped at bill time with full per-line VAT.
    en16931_json        JSONB,

    -- Monetary summary for fast reporting (avoids JSONB parse)
    total_netto_eur     NUMERIC(16, 5),
    total_brutto_eur    NUMERIC(16, 5),

    outcome             TEXT        NOT NULL DEFAULT 'generated' CHECK (outcome IN (
                            'generated',    -- created, not yet dispatched
                            'dispatched',   -- sent to accountingd / ERP
                            'paid',         -- payment confirmed
                            'partial',      -- partial payment
                            'disputed',     -- dispute raised
                            'cancelled'     -- cancelled before dispatch
                        )),

    -- Correction invoice fields (§ 147 AO / GoBD Stornorechnung / Korrekturrechnung)
    is_correction       BOOLEAN     NOT NULL DEFAULT false,
    original_record_id  UUID        REFERENCES billing_records(id) ON DELETE SET NULL,
    correction_reason   TEXT,

 -- B2B Sammelrechnung: NULL = standalone Einzelrechnung
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
    
    -- CloudEvent ID of the emitted de.billing.rechnung.erstellt
    ce_id               UUID,
    dispatched_at       TIMESTAMPTZ,
    tenant              TEXT        NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE billing_records IS
    '§ 14b UStG / § 147 AO / GoBD: 8-year audit ledger for all generated invoices. '
    'Supports original invoices, Storno/Korrektur chains, and B2B Sammelrechnungen.';

COMMENT ON COLUMN billing_records.is_correction IS
    'TRUE = Stornorechnung / Korrekturrechnung (rubo4e istOriginal=false). '
    'Positions are negated relative to original_record_id.';

COMMENT ON COLUMN billing_records.sammelrechnung_id IS
    'FK to the consolidated Sammelrechnung (category=SAMMEL) for B2B Rahmenvertrag '
    'portfolio billing. NULL = standalone Einzelrechnung.';

-- Period lookup per MaLo
CREATE INDEX br_malo_period   ON billing_records (malo_id, lf_mp_id, period_from DESC);
-- Outcome workflow
CREATE INDEX br_outcome       ON billing_records (outcome, lf_mp_id);
-- Pending CE dispatch
CREATE INDEX br_ce_pending    ON billing_records (lf_mp_id, created_at DESC)
    WHERE ce_id IS NULL AND outcome = 'generated';
-- Unique: one original per (malo, lf, period, product, tenant) — corrections excluded
CREATE INDEX br_review_queue ON billing_records (tenant, risk_score DESC)
    WHERE risk_band IN ('REVIEW','HELD');

CREATE UNIQUE INDEX br_unique_original
    ON billing_records (malo_id, lf_mp_id, period_from, period_to, product_code, tenant)
    WHERE is_correction = false AND sammelrechnung_id IS NULL;
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

-- ── §40b Abs. 2 EnWG — monthly Abrechnungsinformation log ────────────────────
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
    '§40b Abs. 2 EnWG: monthly Abrechnungsinformation dispatch log for '
    'iMSys/fernauslesbare MaLos. UNIQUE guard = one info per MaLo and month.';

-- ── VPP dispatch idempotency ──────────────────────────────────────────────────
-- Prevents double-billing when the outbox retries a de.vpp.dispatch.confirmed
-- delivery. The §41e EnWG Aggregatorvertrag itself is Contract-context master
-- data and lives in `vertragd.aggregatorvertraege`; billingd reads it over HTTP.

CREATE TABLE vpp_dispatch_ledger (
    tx_id           TEXT        NOT NULL,
    tenant          TEXT        NOT NULL,
    -- FK to the billing_records row generated for this dispatch
    record_id       UUID,
    processed_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tx_id, tenant)
);

COMMENT ON TABLE vpp_dispatch_ledger IS
    'Idempotency guard for de.vpp.dispatch.confirmed webhook delivery. '
    'Prevents double-billing on outbox retry.';

-- ── Document templates ───────────────────────────────────────────────────────
--
-- The operator owns the *visual* layout of an invoice (logo, Briefkopf, where
-- the Pflichtangaben sit); the embedded CII XML is always rendered from the
-- EN 16931 semantic model, never from a template. See `billingd::document`.
--
-- **Content-addressed and append-only.** An invoice is a Buchungsbeleg kept for
-- 8 years (§ 14b UStG / § 147 AO), and GoBD requires Unveraenderbarkeit — a
-- document issued today must still be explicable in 2034. A mutable template row
-- would silently rewrite the history of how documents looked, so a template is
-- identified by the hash of its source and never updated in place. Publishing a
-- change means inserting a new row and moving the pointer.

CREATE TABLE document_templates (
    -- SHA-256 of `source`, lowercase hex. The identity of the template.
    hash            TEXT        PRIMARY KEY,
    tenant          TEXT        NOT NULL,
    -- Which document this renders. Textform kinds share the engine and the
    -- store with the invoice kind so an operator maintains one template system.
    kind            TEXT        NOT NULL CHECK (kind IN (
                        'INVOICE',          -- ZUGFeRD PDF/A-3 carrier
                        'MAHNUNG',          -- Textform (§ 126b BGB)
                        'PREISANPASSUNG'    -- § 41 Abs. 5 EnWG notice, Textform
                    )),
    -- The template source. Typst.
    source          TEXT        NOT NULL,
    -- PDF/A conformance level the publish gate enforced, in Typst's spelling
    -- (`a-3b`). NULL for the Textform kinds, which have no PDF/A to meet.
    pdf_standard    TEXT,
    -- What the publish gate actually established about this template. Recorded
    -- rather than assumed: RENDERED_PDFA means it produced a conformant carrier
    -- whose embedded invoice was extracted again and matched, PARSED means only
    -- that it compiles and exports the contract function. An INVOICE row is
    -- always RENDERED_PDFA; the Textform kinds have no view to render against
    -- yet, and a column saying so beats a comment implying otherwise.
    proof           TEXT        NOT NULL CHECK (proof IN ('RENDERED_PDFA', 'PARSED')),
    published_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    published_by    TEXT,
    -- An invoice template is only ever stored after the full proof.
    CONSTRAINT dt_invoice_is_rendered
        CHECK (kind <> 'INVOICE' OR (proof = 'RENDERED_PDFA' AND pdf_standard IS NOT NULL))
);

COMMENT ON TABLE document_templates IS
    'Append-only, content-addressed store of operator document templates. '
    'Never UPDATE or DELETE: an issued document pins the hash that rendered it, '
    'and § 147 AO / GoBD require that to stay resolvable for 8 years.';

-- One template per (tenant, kind) is "the current one". This pointer moves;
-- the rows it points at do not.
CREATE TABLE document_template_current (
    tenant          TEXT        NOT NULL,
    kind            TEXT        NOT NULL,
    hash            TEXT        NOT NULL REFERENCES document_templates(hash),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant, kind)
);

COMMENT ON TABLE document_template_current IS
    'Which published template each tenant renders with now. The pointer is '
    'mutable; the templates it references are not.';

-- An issued document records the template that produced it, so its appearance
-- is reproducible for as long as the document must be kept.
ALTER TABLE billing_records
    ADD COLUMN template_hash TEXT REFERENCES document_templates(hash);

COMMENT ON COLUMN billing_records.template_hash IS
    'The document_templates.hash that rendered this invoice''s PDF. NULL for '
    'records issued before a template was configured, or XML-only records.';

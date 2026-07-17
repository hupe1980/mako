-- ── billingd schema — Energy Billing Engine ──────────────────────────────────
--
-- `billing_records`: immutable audit log of every generated invoice.
--   Full rubo4e::current::Rechnung JSONB for §22 MessZV compliance (3-year retention).
--   Supports Einzelrechnung, Korrektur/Storno (is_correction), and B2B Sammelrechnung.
--
-- `billing_run_log`: monthly batch run audit + idempotency guard.
--
-- `vpp_contracts`: VPP (Virtual Power Plant) billing configuration per SR-ID.
--   Enables auto-settlement of §41b EnWG Steuerungsauftrag confirmations.
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
                            'ENERGIEDIENSTLEISTUNG', 'BUNDLE', 'SAMMEL'
                        )),
    period_from         DATE        NOT NULL,
    period_to           DATE        NOT NULL,

    -- Full rubo4e::current::Rechnung JSONB (§22 MessZV 3-year retention)
    rechnung_json       JSONB       NOT NULL,
    bo4e_version        TEXT        NOT NULL DEFAULT 'v202607.0.0',

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

    -- Correction invoice fields (§22 MessZV Stornorechnung / Korrekturrechnung)
    is_correction       BOOLEAN     NOT NULL DEFAULT false,
    original_record_id  UUID        REFERENCES billing_records(id) ON DELETE SET NULL,
    correction_reason   TEXT,

    -- B2B Sammelrechnung: NULL = standalone Einzelrechnung
    sammelrechnung_id   UUID        REFERENCES billing_records(id) ON DELETE SET NULL,

    -- CloudEvent ID of the emitted de.billing.rechnung.erstellt
    ce_id               UUID,
    dispatched_at       TIMESTAMPTZ,
    tenant              TEXT        NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE billing_records IS
    '§22 MessZV: 3-year audit ledger for all generated invoices. '
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
    'Monthly automated batch run audit trail and idempotency guard. '
    'UNIQUE (tenant, lf_mp_id, year, month) prevents double-runs.';

-- ── VPP (Virtual Power Plant) contracts ──────────────────────────────────────
-- Maps SteuerbareRessource-ID → billing parameters for auto-settlement of
-- de.vpp.dispatch.confirmed CloudEvents (§41b EnWG / RED III Art. 17).

CREATE TABLE vpp_contracts (
    id                          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    -- SteuerbareRessource-ID (C…) or NeLo-ID from marktd
    sr_id                       TEXT        NOT NULL,
    -- Operator-assigned VPP portfolio identifier
    vpp_id                      TEXT        NOT NULL,
    malo_id                     TEXT        NOT NULL,
    lf_mp_id                    TEXT        NOT NULL,
    -- Agreed capacity price in EUR/kWh (Einsatzkosten)
    capacity_price_eur_per_kwh  NUMERIC(12, 6) NOT NULL CHECK (capacity_price_eur_per_kwh >= 0),
    valid_from                  DATE        NOT NULL,
    valid_to                    DATE,
    -- MwSt override; NULL = use billingd default (0.19)
    mwst_rate_override          NUMERIC(5, 4),
    tenant                      TEXT        NOT NULL,
    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (sr_id, tenant, valid_from)
);

COMMENT ON TABLE vpp_contracts IS
    '§41b EnWG / RED III Art. 17: VPP dispatch billing configuration per SR-ID. '
    'Enables auto-settlement when de.vpp.dispatch.confirmed is received.';

CREATE INDEX vpp_sr_tenant    ON vpp_contracts (sr_id, tenant, valid_from DESC);

-- ── VPP dispatch idempotency ──────────────────────────────────────────────────
-- Prevents double-billing when the outbox retries a de.vpp.dispatch.confirmed delivery.

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

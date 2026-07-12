-- ── accountingd schema — Massenkontokorrent / Customer Account Ledger ─────────
--
-- `accounts`: one row per (malo_id, lf_mp_id) — the Kundenkonto record.
--
-- `ledger_entries`: immutable debit/credit log.
--   amount_ct > 0 = debit  (Rechnung, Mahngebühr)
--   amount_ct < 0 = credit (Zahlung, Gutschrift, EEG credit)
--   Balance = SUM(amount_ct) — negative means overpaid / credit balance.
--
-- `sepa_mandates`: SEPA direct-debit mandate registry per account.
--
-- `dunning_cases`: escalating Mahnwesen (Mahnstufe 1–3).

-- ── Kundenkonto ───────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS accounts (
    account_id     UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id        TEXT        NOT NULL,
    lf_mp_id       TEXT        NOT NULL,
    tenant         TEXT        NOT NULL,
    -- SEPA mandate IBAN (copied from sepa_mandates for fast lookup)
    iban           TEXT,
    -- SEPA mandate reference
    mandatsref     TEXT,
    -- Monthly advance payment (Abschlag) in ct × 10⁻² EUR  (i.e. cents)
    -- billingd uses this when generating monthly pre-payment invoices.
    abschlag_ct    BIGINT      NOT NULL DEFAULT 0,
    -- Day of month for automated Abschlag booking (1–28)
    billing_day    SMALLINT    NOT NULL DEFAULT 1,
    -- Current cumulative balance (cached from ledger; updated on every write).
    -- Negative = credit balance (customer overpaid); positive = outstanding debt.
    balance_ct     BIGINT      NOT NULL DEFAULT 0,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (malo_id, lf_mp_id)
);

CREATE INDEX IF NOT EXISTS acct_tenant ON accounts (tenant, lf_mp_id);
CREATE INDEX IF NOT EXISTS acct_overdue ON accounts (tenant)
    WHERE balance_ct > 0;

-- ── Ledger entries (immutable) ────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS ledger_entries (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id     UUID        NOT NULL REFERENCES accounts (account_id) ON DELETE CASCADE,
    tenant         TEXT        NOT NULL,
    entry_type     TEXT        NOT NULL
                   CHECK (entry_type IN (
                       'RECHNUNG',       -- debit: invoice created by billingd/netzbilanzd
                       'ZAHLUNG',        -- credit: payment received (CAMT.054 or ERP confirm)
                       'GUTSCHRIFT',     -- credit: credit note
                       'EEG_GUTSCHRIFT', -- credit: EEG settlement payment (de.eeg.verguetung.berechnet)
                       'BANKRUECKLAST',  -- debit: returned direct debit
                       'MAHNGEBUEHR',    -- debit: dunning fee
                       'ABSCHLAG',       -- debit: monthly advance payment
                       'KORREKTUR'       -- signed: manual correction
                   )),
    -- Amount in ct × 10⁻² EUR.  Positive = debit; negative = credit.
    amount_ct      BIGINT      NOT NULL,
    -- Reference to the originating record (invoice_id, payment_ref, CloudEvent id, …)
    reference_id   TEXT,
    -- CE type that triggered this entry (for audit trail)
    ce_type        TEXT,
    -- CE id from the source event
    ce_id          TEXT,
    booking_date   DATE        NOT NULL,
    value_date     DATE        NOT NULL,
    description    TEXT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS le_account_date ON ledger_entries (account_id, booking_date DESC);
CREATE INDEX IF NOT EXISTS le_tenant_date  ON ledger_entries (tenant, booking_date DESC);
CREATE INDEX IF NOT EXISTS le_ce_id        ON ledger_entries (ce_id) WHERE ce_id IS NOT NULL;

-- ── SEPA direct-debit mandates ────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS sepa_mandates (
    mandate_id     UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id     UUID        NOT NULL REFERENCES accounts (account_id) ON DELETE CASCADE,
    tenant         TEXT        NOT NULL,
    iban           TEXT        NOT NULL,
    bic            TEXT,
    kontoinhaber   TEXT,
    -- SEPA mandatsreferenz (unique creditor-assigned ID)
    mandatsref     TEXT        NOT NULL UNIQUE,
    -- FRST = first collection; RCUR = recurring; FNAL = final; OOFF = one-off
    sequence_type  TEXT        NOT NULL
                   CHECK (sequence_type IN ('FRST', 'RCUR', 'FNAL', 'OOFF')),
    signed_at      DATE        NOT NULL,
    revoked_at     DATE,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS sm_account ON sepa_mandates (account_id);
CREATE INDEX IF NOT EXISTS sm_active  ON sepa_mandates (account_id)
    WHERE revoked_at IS NULL;

-- ── Dunning cases (Mahnwesen) ─────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS dunning_cases (
    id             UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id     UUID        NOT NULL REFERENCES accounts (account_id) ON DELETE CASCADE,
    tenant         TEXT        NOT NULL,
    stufe          SMALLINT    NOT NULL CHECK (stufe BETWEEN 1 AND 3),
    amount_due_ct  BIGINT      NOT NULL,
    issued_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    due_date       DATE        NOT NULL,
    resolved_at    TIMESTAMPTZ,
    -- Sperrauftrag triggered for Mahnstufe 3 (de.accounting.sperrauftrag CloudEvent)
    sperrauftrag_ce_id TEXT
);

CREATE INDEX IF NOT EXISTS dc_account   ON dunning_cases (account_id, stufe);
CREATE INDEX IF NOT EXISTS dc_overdue   ON dunning_cases (tenant, due_date)
    WHERE resolved_at IS NULL;

-- ── Processed events (idempotency) ───────────────────────────────────────────

CREATE TABLE IF NOT EXISTS processed_events (
    ce_id          TEXT        PRIMARY KEY,
    processed_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- accountingd migration 0002: BO4E Vorauszahlung typed advance-payment schedule
--
-- Adds a typed `vorauszahlung JSONB` column to `accounts` for structured
-- advance-payment schedule data (betrag, datum, referenz) stored as
-- `rubo4e::current::Vorauszahlung` camelCase.
--
-- Regulatory: §40 Abs. 1 EnWG — Abschlag must match estimated consumption.
-- The existing `abschlag_ct` column is kept and kept in sync by the handler
-- so the monthly Abschlagslauf scheduler continues to work unchanged.

ALTER TABLE accounts ADD COLUMN IF NOT EXISTS vorauszahlung JSONB;

COMMENT ON COLUMN accounts.vorauszahlung IS
    'BO4E Vorauszahlung COM — typed advance payment schedule: betrag (EUR), datum (next due), '
    'referenz. Validated on PUT /accounts/{malo_id}/vorauszahlung. '
    'abschlag_ct is updated atomically from vorauszahlung.betrag.wert × 100.';

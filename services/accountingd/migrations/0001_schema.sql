-- ── accountingd schema — Massenkontokorrent / Customer Account Ledger ─────────
--
-- `accounts`: one row per (malo_id, lf_mp_id, tenant) — the Kundenkonto.
-- `ledger_entries`: immutable debit/credit log (positive = debit, negative = credit).
-- `sepa_mandates`: SEPA direct-debit mandate registry.
-- `dunning_cases`: Mahnwesen escalation (Mahnstufe 1–3).
-- `processed_events`: idempotency guard for CloudEvent ingest.
-- `anonymization_log`: GDPR Art. 17 erasure audit trail (INSERT-only).
-- `auto_dunning_runs`: daily auto-dunning idempotency + audit.
--
-- Regulatory: §40 EnWG (Abschlag), §238 HGB (Buchführungspflicht 10y),
--             GDPR Art. 15/17/20, SEPA Regulation 260/2012.

-- ── Kundenkonto ───────────────────────────────────────────────────────────────

CREATE TABLE accounts (
    account_id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id             TEXT        NOT NULL,
    lf_mp_id            TEXT        NOT NULL,
    tenant              TEXT        NOT NULL,

    -- SEPA mandate IBAN (denormalized for fast payment-matching lookup)
    iban                TEXT,
    mandatsref          TEXT,

    -- Monthly Abschlag in EUR-cent (1 ct = 0.01 EUR)
    -- §40 Abs. 1 EnWG: Abschlag must reflect estimated consumption
    abschlag_ct         BIGINT      NOT NULL DEFAULT 0,
    -- Day of month for automated Abschlag booking (1–28)
    billing_day         SMALLINT    NOT NULL DEFAULT 1,

    -- Cached cumulative balance (negative = credit, positive = outstanding debt)
    -- Updated atomically on every ledger write
    balance_ct          BIGINT      NOT NULL DEFAULT 0,

    -- BO4E Vorauszahlung COM: typed advance-payment schedule (§40 EnWG)
    vorauszahlung       JSONB,
    -- BO4E Zahlungsinformation COM: IBAN/BIC/Zahlungsart for SEPA batch export
    zahlungsinformation JSONB,

    -- GDPR Art. 17: set when PII was anonymized; financial records are retained
    anonymized_at       TIMESTAMPTZ,

    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- Tenant isolation: one Kundenkonto per (MaLo, LF, tenant)
    UNIQUE (malo_id, lf_mp_id, tenant)
);

COMMENT ON TABLE accounts IS
    'Customer account ledger (Massenkontokorrent). '
    'One row per (MaLo, Lieferant, tenant). Balance is a cached aggregate from ledger_entries. '
    'Regulatory: §40 EnWG, §238 HGB (10y retention), GDPR Art. 17.';

COMMENT ON COLUMN accounts.balance_ct IS
    'Cached SUM(amount_ct) from ledger_entries. '
    'Negative = credit balance (customer overpaid). Positive = outstanding debt.';

COMMENT ON COLUMN accounts.anonymized_at IS
    'GDPR Art. 17: set when PII (IBAN, mandatsref, zahlungsinformation) was '
    'replaced with anonymized placeholders. Financial records retained per §238 HGB.';

CREATE INDEX acct_tenant       ON accounts (tenant, lf_mp_id);
CREATE INDEX acct_malo_tenant  ON accounts (malo_id, tenant);
CREATE INDEX acct_overdue      ON accounts (tenant)
    WHERE balance_ct > 0;

-- ── Ledger entries (immutable) ────────────────────────────────────────────────

CREATE TABLE ledger_entries (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id      UUID        NOT NULL REFERENCES accounts (account_id) ON DELETE CASCADE,
    tenant          TEXT        NOT NULL,

    -- Buchungsart (§238 HGB: every entry must have a clear type)
    entry_type      TEXT        NOT NULL CHECK (entry_type IN (
        'RECHNUNG',         -- debit:  invoice from billingd / netzbilanzd
        'STORNO',           -- debit:  Stornorechnung / billing reversal
        'ZAHLUNG',          -- credit: incoming payment (CAMT.054, ERP confirm, SEPA)
        'GUTSCHRIFT',       -- credit: credit note (de.billing.gutschrift.erstellt)
        'EEG_GUTSCHRIFT',   -- credit: EEG Einspeisevergütung (de.eeg.verguetung.berechnet)
        'EEG_MARKTPRAEMIE', -- credit: EEG Direktvermarktung Marktprämie
        'BANKRUECKLAST',    -- debit:  returned SEPA direct debit (R-transaction)
        'MAHNGEBUEHR',      -- debit:  dunning fee (Mahnstufe escalation)
        'ABSCHLAG',         -- debit:  monthly advance payment (§40 Abs. 1 EnWG)
        'JAHRESABSCHLUSS',  -- signed: annual Mehr-/Mindermengenabrechnung settlement
        'KORREKTUR'         -- signed: manual operator correction (audit-trailed)
    )),

    -- Amount in EUR-cent. Positive = Debit (Forderung). Negative = Credit (Gutschrift).
    amount_ct       BIGINT      NOT NULL,

    -- Reference to the originating record (invoice_id, payment_ref, CE id, …)
    reference_id    TEXT,
    -- CloudEvent type and id for audit trail
    ce_type         TEXT,
    ce_id           TEXT,

    booking_date    DATE        NOT NULL,
    value_date      DATE        NOT NULL,
    description     TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
    -- No updated_at — ledger entries are immutable (INSERT-only)
);

COMMENT ON TABLE ledger_entries IS
    'Immutable debit/credit ledger. INSERT-only — no UPDATE or DELETE. '
    '§238 HGB: Buchführungspflicht. Positive amount_ct = Debit; negative = Credit.';

COMMENT ON COLUMN ledger_entries.entry_type IS
    'STORNO replaces the former KORREKTURRECHNUNG type (billing system reversal). '
    'EEG_MARKTPRAEMIE is for Direktvermarktung Marktprämie payments from einsd.';

CREATE INDEX le_account_date ON ledger_entries (account_id, booking_date DESC);
CREATE INDEX le_tenant_date  ON ledger_entries (tenant, booking_date DESC);
CREATE INDEX le_ce_id        ON ledger_entries (ce_id) WHERE ce_id IS NOT NULL;

-- ── SEPA direct-debit mandates ────────────────────────────────────────────────

CREATE TABLE sepa_mandates (
    mandate_id      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id      UUID        NOT NULL REFERENCES accounts (account_id) ON DELETE CASCADE,
    tenant          TEXT        NOT NULL,
    iban            TEXT        NOT NULL,
    bic             TEXT,
    kontoinhaber    TEXT,
    -- Unique creditor-assigned Mandatsreferenz (SEPA SDD, ISO 20022)
    mandatsref      TEXT        NOT NULL UNIQUE,
    -- FRST = first collection; RCUR = recurring; FNAL = final; OOFF = one-off
    sequence_type   TEXT        NOT NULL CHECK (sequence_type IN ('FRST', 'RCUR', 'FNAL', 'OOFF')),
    signed_at       DATE        NOT NULL,
    revoked_at      DATE,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX sm_account ON sepa_mandates (account_id);
CREATE INDEX sm_active  ON sepa_mandates (account_id)
    WHERE revoked_at IS NULL;

-- ── Dunning cases (Mahnwesen) ─────────────────────────────────────────────────

CREATE TABLE dunning_cases (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id      UUID        NOT NULL REFERENCES accounts (account_id) ON DELETE CASCADE,
    tenant          TEXT        NOT NULL,
    stufe           SMALLINT    NOT NULL CHECK (stufe BETWEEN 1 AND 3),
    amount_due_ct   BIGINT      NOT NULL,
    issued_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    due_date        DATE        NOT NULL,
    resolved_at     TIMESTAMPTZ,
    -- CloudEvent ID of the de.accounting.sperrauftrag emitted at Mahnstufe 3
    sperrauftrag_ce_id TEXT
);

COMMENT ON TABLE dunning_cases IS
    'Mahnwesen escalation (Mahnstufe 1–3). '
    'Mahnstufe 3 triggers de.accounting.sperrauftrag CloudEvent → sperrd.';

CREATE INDEX dc_account ON dunning_cases (account_id, stufe);
CREATE INDEX dc_overdue ON dunning_cases (tenant, due_date)
    WHERE resolved_at IS NULL;

-- ── Processed events (idempotency guard) ─────────────────────────────────────

CREATE TABLE processed_events (
    ce_id           TEXT        PRIMARY KEY,
    processed_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ── GDPR Art. 17 anonymization log (INSERT-only) ──────────────────────────────

CREATE TABLE anonymization_log (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id          UUID        NOT NULL,
    tenant              TEXT        NOT NULL,
    malo_id             TEXT        NOT NULL,
    requested_by        TEXT        NOT NULL,
    legal_basis         TEXT        NOT NULL,
    -- JSON array of anonymized column names
    anonymized_fields   JSONB       NOT NULL DEFAULT '[]',
    anonymized_at       TIMESTAMPTZ NOT NULL DEFAULT now()
    -- No updated_at — this table is INSERT-only (immutable audit log)
);

COMMENT ON TABLE anonymization_log IS
    'GDPR Art. 17 erasure audit trail (INSERT-only). '
    'Proves compliance per GDPR Art. 5(2) accountability principle.';

CREATE INDEX anon_log_account ON anonymization_log (account_id);
CREATE INDEX anon_log_tenant  ON anonymization_log (tenant, anonymized_at DESC);

-- ── Auto-dunning run log ──────────────────────────────────────────────────────

CREATE TABLE auto_dunning_runs (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant              TEXT        NOT NULL,
    run_date            DATE        NOT NULL,
    accounts_checked    INTEGER     NOT NULL DEFAULT 0,
    dunning_created     INTEGER     NOT NULL DEFAULT 0,
    dunning_escalated   INTEGER     NOT NULL DEFAULT 0,
    run_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- One run per (tenant, day) — prevents double-dunning from worker restarts
    UNIQUE (tenant, run_date)
);

COMMENT ON TABLE auto_dunning_runs IS
    'Idempotency guard and audit trail for automatic Mahnwesen escalation. '
    'One row per (tenant, calendar day) — prevents double-dunning on restart.';

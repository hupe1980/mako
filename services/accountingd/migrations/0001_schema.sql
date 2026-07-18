-- ── accountingd schema — Massenkontokorrent / Customer Account Ledger ─────────
--
-- Tables:
--   accounts             — one row per (malo_id, lf_mp_id, tenant) — the Kundenkonto
--   ledger_entries       — immutable debit/credit log (positive = debit, negative = credit)
--   journal_lines        — double-entry shadow: balanced debit/credit pairs per SKR 03/04
--   sepa_mandates        — SEPA direct-debit mandate registry
--   dunning_cases        — Mahnwesen escalation (Mahnstufe 1–3)
--   interest_charges     — Verzugszinsen §288 BGB (default interest on overdue invoices)
--   payment_plans        — Zahlungsvereinbarung (structured installment agreements)
--   payment_plan_installments — individual installments per plan
--   bank_import_log      — CAMT.054 deduplication (bank transaction IDs already imported)
--   processed_events     — idempotency guard for CloudEvent ingest
--   anonymization_log    — GDPR Art. 17 erasure audit trail (INSERT-only)
--   auto_dunning_runs    — daily auto-dunning idempotency + audit
--   eeg_payout_orders    — EEG SCT/SCT Inst payout pipeline
--   sepa_collection_runs — pain.008 XML archive for audit + replay
--   abschlag_runs        — monthly Abschlag idempotency
--   jahresabschluss_runs — annual settlement idempotency
--   account_audit_log    — §238 HGB master-data change trail
--   processed_events     — CloudEvent idempotency
--
-- Regulatory: §40 EnWG (Abschlag), §238 HGB (Buchführungspflicht 10y),
--             §288 BGB (Verzugszinsen), GDPR Art. 15/17/20, SEPA Regulation 260/2012.

-- pgcrypto: for IBAN SHA-256 hash (lookup key for encrypted IBAN columns)
CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- ── Kundenkonto ───────────────────────────────────────────────────────────────

CREATE TABLE accounts (
    account_id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id             TEXT        NOT NULL,
    lf_mp_id            TEXT        NOT NULL,
    tenant              TEXT        NOT NULL,

    -- SEPA mandate IBAN (denormalized for fast payment-matching lookup)
    -- Stored as plaintext; for encrypted deployments set iban_encrypted=true
    -- and store pgp_sym_encrypt(iban, key) here instead.
    iban                TEXT,
    -- SHA-256 hash of normalised IBAN (uppercase, no spaces).
    -- Used as an indexed lookup key in CAMT.054 matching even when IBAN is encrypted.
    iban_hash           TEXT GENERATED ALWAYS AS (
                            CASE WHEN iban IS NOT NULL
                            THEN encode(digest(upper(replace(iban,' ','')), 'sha256'), 'hex')
                            ELSE NULL END
                        ) STORED,
    -- Set to true when `iban` column stores pgp_sym_encrypt(...) ciphertext.
    iban_encrypted      BOOLEAN     NOT NULL DEFAULT false,
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
    '§238 HGB: Buchführungspflicht. Positive amount_ct = Debit; negative = Credit. '
    'amount_ct != 0 is enforced: zero-amount entries are semantically invalid.';

-- P2-6: zero-amount entries are invalid and indicate bugs.
ALTER TABLE ledger_entries ADD CONSTRAINT le_amount_nonzero CHECK (amount_ct != 0);

COMMENT ON COLUMN ledger_entries.entry_type IS
    'STORNO replaces the former KORREKTURRECHNUNG type (billing system reversal). '
    'EEG_MARKTPRAEMIE is for Direktvermarktung Marktprämie payments from einsd.';

CREATE INDEX le_account_date ON ledger_entries (account_id, booking_date DESC);
CREATE INDEX le_tenant_date  ON ledger_entries (tenant, booking_date DESC);
CREATE INDEX le_ce_id        ON ledger_entries (ce_id) WHERE ce_id IS NOT NULL;
CREATE INDEX le_reference_id ON ledger_entries (reference_id) WHERE reference_id IS NOT NULL;

-- ── SEPA direct-debit mandates ────────────────────────────────────────────────

CREATE TABLE sepa_mandates (
    mandate_id      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id      UUID        NOT NULL REFERENCES accounts (account_id) ON DELETE CASCADE,
    tenant          TEXT        NOT NULL,
    iban            TEXT        NOT NULL,
    bic             TEXT,
    kontoinhaber    TEXT,
    -- Unique creditor-assigned Mandatsreferenz (SEPA SDD, ISO 20022).
    -- P1-1 fix: UNIQUE per tenant (not globally) to avoid cross-tenant namespace collisions.
    mandatsref      TEXT        NOT NULL,
    -- FRST = first collection; RCUR = recurring; FNAL = final; OOFF = one-off
    sequence_type   TEXT        NOT NULL CHECK (sequence_type IN ('FRST', 'RCUR', 'FNAL', 'OOFF')),
    signed_at       DATE        NOT NULL,
    revoked_at      DATE,
    -- P2-14: track mandate creation date for SEPA audit trail
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- P1-4: track when the first successful collection occurred for FRST→RCUR auto-transition
    first_collected_at TIMESTAMPTZ
);

-- P1-1 fix: mandatsref unique per tenant, not globally
CREATE UNIQUE INDEX sm_mandatsref_tenant ON sepa_mandates (tenant, mandatsref);
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

-- ── EEG Einspeisevergütung payout orders (§25 EEG 2023) ─────────────────────
--
-- Every EEG Vergütung settlement triggers one row here.  The row tracks the
-- full lifecycle from ledger credit → pain.001 generation → bank submission →
-- pain.002 confirmation.
--
-- Two payment types:
--   SCT_INST — SEPA Credit Transfer Instant (pain.001.001.09)
--              Settles in <10 seconds (EU 2024/886 mandatory from Oct 2025).
--              Preferred for monthly EEG payouts where plant operators rely on
--              immediate liquidity (§25 Abs. 1 EEG 2023: "unverzüglich").
--   SCT_CORE — Standard SEPA Credit Transfer (pain.001.003.03)
--              D+1 settlement. Fallback when bank does not support SCT Inst.
--
-- `end_to_end_ref` carries the ISO 20022 EndToEndId used in the pain.001 XML.
-- It is constructed as: EEG-{malo_id_short}-{year}-{month}-{ce_id_short}
-- Unique per payout order; used to correlate pain.002 status reports.
--
-- `pain001_xml` stores the generated XML verbatim for bank audit and replay.
-- `pain002_status`:  PDNG = submitted, awaiting confirmation
--                    ACCP = accepted (funds credited)
--                    RJCT = rejected (see pain002_reason for EPC reason code)
--                    CANC = cancelled before submission

CREATE TABLE eeg_payout_orders (
    payout_id       UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id         TEXT        NOT NULL,
    account_id      UUID        NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE,
    tr_id           TEXT,               -- EEG plant ID (Anlagen-ID from einsd)
    billing_year    SMALLINT    NOT NULL,
    billing_month   SMALLINT    NOT NULL,
    -- Amount in EUR-cent (positive = payout to plant operator)
    amount_ct       BIGINT      NOT NULL CHECK (amount_ct > 0),
    creditor_iban   TEXT        NOT NULL,
    creditor_name   TEXT        NOT NULL,
    -- SCT_INST or SCT_CORE
    payment_type    TEXT        NOT NULL CHECK (payment_type IN ('SCT_INST', 'SCT_CORE')),
    -- ISO 20022 EndToEndId — unique per order, correlates pain.001 + pain.002
    end_to_end_ref  TEXT        NOT NULL,
    -- Verbatim pain.001 XML (for audit, bank replay, and debugging)
    pain001_xml     TEXT,
    -- pain.002 status: PDNG (pending) | ACCP (accepted) | RJCT (rejected) | CANC (cancelled)
    pain002_status  TEXT        CHECK (pain002_status IN ('PDNG','ACCP','RJCT','CANC')),
    -- EPC SEPA reason code from pain.002 (e.g. AC01 = invalid IBAN, AM04 = insufficient funds)
    pain002_reason  TEXT,
    -- When pain.001 XML was submitted to the bank adapter
    submitted_at    TIMESTAMPTZ,
    -- When ACCP confirmation was received (funds credited to plant operator)
    settled_at      TIMESTAMPTZ,
    -- Source CloudEvent ID for idempotency (de.eeg.verguetung.berechnet ce_id)
    source_ce_id    TEXT,
    tenant          TEXT        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE eeg_payout_orders IS
    'EEG Einspeisevergütung SEPA Credit Transfer orders. '
    'One row per settlement CE. Full pain.001 XML + pain.002 lifecycle. '
    'Regulatory: §25 Abs. 1 EEG 2023 (unverzüglich), EU Reg 2024/886 (SCT Inst).';

-- Idempotency: one payout per source CE
CREATE UNIQUE INDEX eeg_payout_source_ce   ON eeg_payout_orders (source_ce_id) WHERE source_ce_id IS NOT NULL;
-- Also unique on EndToEndId (ISO 20022 requirement)
CREATE UNIQUE INDEX eeg_payout_e2e         ON eeg_payout_orders (end_to_end_ref);
-- Fast status monitoring (bank integration health dashboard)
CREATE INDEX eeg_payout_status    ON eeg_payout_orders (tenant, payment_type, pain002_status);
CREATE INDEX eeg_payout_malo      ON eeg_payout_orders (malo_id, billing_year, billing_month, tenant);
-- Pending orders awaiting pain.002 confirmation (retry worker)
CREATE INDEX eeg_payout_pending   ON eeg_payout_orders (tenant, created_at)
    WHERE pain002_status IS NULL OR pain002_status = 'PDNG';

-- ── SEPA pain.008 collection runs (P1-6: persist for audit + replay) ─────────
--
-- Every pain.008 batch is stored here.  Provides full audit trail per SEPA
-- Rulebook DS-01 requirements and allows replay if the ERP webhook fails.
-- One row per scheduled batch (one per billing_day per day).

CREATE TABLE sepa_collection_runs (
    run_id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant          TEXT        NOT NULL,
    -- Date the pain.008 batch targets (collection date at debtor bank)
    collection_date DATE        NOT NULL,
    -- Verbatim pain.008 XML (for audit, ERP replay, bank resubmission)
    pain008_xml     TEXT        NOT NULL,
    -- Total amount in ct across all entries
    total_ct        BIGINT      NOT NULL,
    mandate_count   INTEGER     NOT NULL DEFAULT 0,
    -- Status of ERP webhook delivery
    -- PENDING = generated, not yet confirmed by ERP
    -- DISPATCHED = ERP acknowledged
    -- FAILED = ERP webhook error (manual retry required)
    dispatch_status TEXT        NOT NULL DEFAULT 'PENDING'
                    CHECK (dispatch_status IN ('PENDING', 'DISPATCHED', 'FAILED')),
    dispatched_at   TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE sepa_collection_runs IS
    'SEPA SDD pain.008 batch archive. One row per scheduled collection run. '
    'Pain.008 XML persisted for regulatory audit (SEPA Rulebook DS-01) and ERP replay.';

-- Prevent duplicate batches for the same collection date per tenant
CREATE UNIQUE INDEX scr_tenant_date ON sepa_collection_runs (tenant, collection_date);
CREATE INDEX scr_tenant_status ON sepa_collection_runs (tenant, dispatch_status, created_at);

-- ── Abschlag run idempotency (P1-2: prevent duplicate ABSCHLAG on restart) ───

CREATE TABLE abschlag_runs (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant          TEXT        NOT NULL,
    malo_id         TEXT        NOT NULL,
    -- Month this Abschlag covers (YYYY-MM-01 first-of-month for consistent key)
    period_month    DATE        NOT NULL,
    amount_ct       BIGINT      NOT NULL,
    ledger_entry_id UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- One Abschlag per (tenant, malo_id, period_month) — prevents duplicate postings
    UNIQUE (tenant, malo_id, period_month)
);

COMMENT ON TABLE abschlag_runs IS
    'Idempotency guard for monthly Abschlag (advance payment) bookings. '
    'Prevents duplicate ABSCHLAG ledger entries when the scheduler restarts mid-day.';

CREATE INDEX ar_tenant_period ON abschlag_runs (tenant, period_month);

-- ── Jahresabschluss idempotency (P1-10) ──────────────────────────────────────

CREATE TABLE jahresabschluss_runs (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant          TEXT        NOT NULL,
    malo_id         TEXT        NOT NULL,
    billing_year    SMALLINT    NOT NULL,
    annual_bill_ct  BIGINT      NOT NULL,
    sum_abschlage_ct BIGINT     NOT NULL,
    zahlbetrag_ct   BIGINT      NOT NULL, -- positive = customer owes; negative = LF refunds
    ledger_entry_id UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- One annual settlement per (tenant, malo_id, year)
    UNIQUE (tenant, malo_id, billing_year)
);

COMMENT ON TABLE jahresabschluss_runs IS
    'Idempotency guard for annual settlement (Jahresabschluss / Schlussabrechnung §40 EnWG). '
    'Prevents double-posting when POST /jahresabschluss is called more than once per year.';

-- ── Account master-data audit log (P2-5: §238 HGB traceability) ──────────────
--
-- Records every change to account master data (IBAN, billing_day, abschlag_ct, etc.)
-- Required per §238 HGB: "wer, wann, was gebucht hat" for financial records.

CREATE TABLE account_audit_log (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id      UUID        NOT NULL,
    tenant          TEXT        NOT NULL,
    malo_id         TEXT        NOT NULL,
    -- JWT sub of the operator who made the change (from OIDC claims)
    operator_sub    TEXT,
    -- HTTP endpoint that triggered the change
    action          TEXT        NOT NULL,
    -- Previous values (for rollback analysis)
    old_values      JSONB,
    -- New values written
    new_values      JSONB,
    changed_at      TIMESTAMPTZ NOT NULL DEFAULT now()
    -- INSERT-only: never UPDATE or DELETE
);

COMMENT ON TABLE account_audit_log IS
    'Account master-data change audit trail (INSERT-only). '
    'Tracks IBAN changes, Abschlag updates, and mandate registrations. '
    'Regulatory: §238 HGB Buchführungspflicht (traceability requirement).';

CREATE INDEX aal_account   ON account_audit_log (account_id, changed_at DESC);
CREATE INDEX aal_tenant    ON account_audit_log (tenant, changed_at DESC);
CREATE INDEX aal_operator  ON account_audit_log (operator_sub) WHERE operator_sub IS NOT NULL;

-- ── processed_events retention index (P1-9) ──────────────────────────────────
-- Add index to support periodic cleanup of old idempotency records.
-- Cleanup job: DELETE FROM processed_events WHERE processed_at < now() - INTERVAL '10 years'
CREATE INDEX pe_processed_at ON processed_events (processed_at);

-- ── IBAN hash index (fast lookup even when IBAN is encrypted) ─────────────────
CREATE INDEX acct_iban_hash ON accounts (iban_hash, tenant) WHERE iban_hash IS NOT NULL;

-- ── Double-entry journal shadow (§238 HGB Buchführungspflicht) ───────────────
--
-- Each ledger_entry produces exactly two journal_lines: one debit and one credit.
-- Account codes follow the German chart of accounts (SKR 03 / SKR 04):
--
--   SKR 03  |  SKR 04  |  Description
--   1200    |  1800    |  Bank (cash received / paid)
--   1400    |  1400    |  Forderungen aus Lieferungen und Leistungen (AR)
--   4000    |  8000    |  Erlöse (Revenue / Einspeisung)
--   4003    |  8003    |  Mahngebühren / Verzugszinsen
--   3000    |  1500    |  Verbindlichkeiten (Liabilities — overpaid by customer)
--
-- The mapping per entry_type:
--   RECHNUNG      → Debit 1400  / Credit 4000
--   STORNO        → Debit 4000  / Credit 1400  (reversal)
--   ZAHLUNG       → Debit 1200  / Credit 1400
--   GUTSCHRIFT    → Debit 4000  / Credit 1400
--   EEG_GUTSCHRIFT→ Debit 3000  / Credit 4000  (LF owes to plant operator)
--   EEG_MARKTPRAEMIE → same as EEG_GUTSCHRIFT
--   BANKRUECKLAST → Debit 1400  / Credit 1200
--   MAHNGEBUEHR   → Debit 1400  / Credit 4003
--   ABSCHLAG      → Debit 1400  / Credit 4000  (prepayment receivable)
--   JAHRESABSCHLUSS → signed: same as RECHNUNG if positive, GUTSCHRIFT if negative
--   KORREKTUR     → operator-defined (same mapping as RECHNUNG/GUTSCHRIFT by sign)
--
-- This table is INSERT-only (immutable).  One pair per ledger_entry_id.

CREATE TABLE journal_lines (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    ledger_entry_id UUID        NOT NULL REFERENCES ledger_entries (id) ON DELETE CASCADE,
    account_id      UUID        NOT NULL REFERENCES accounts (account_id) ON DELETE CASCADE,
    tenant          TEXT        NOT NULL,
    -- 'D' = Debit (Soll), 'C' = Credit (Haben)
    side            CHAR(1)     NOT NULL CHECK (side IN ('D', 'C')),
    -- SKR 03 or SKR 04 account code (text, e.g. '1400', '4000')
    skr_account     TEXT        NOT NULL,
    -- Human-readable account description (e.g. 'Forderungen aus L+L', 'Erlöse')
    skr_description TEXT        NOT NULL,
    -- Amount in ct (always positive — the sign is conveyed by `side`)
    amount_ct       BIGINT      NOT NULL CHECK (amount_ct > 0),
    booking_date    DATE        NOT NULL,
    description     TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
    -- INSERT-only — never UPDATE or DELETE
);

COMMENT ON TABLE journal_lines IS
    'Double-entry journal shadow (SKR 03/04). '
    'Two rows per ledger_entry: one Soll (D) and one Haben (C). '
    'SUM(amount_ct WHERE side=D) = SUM(amount_ct WHERE side=C) per ledger_entry. '
    'Regulatory: §238 HGB Buchführungspflicht.';

CREATE INDEX jl_entry      ON journal_lines (ledger_entry_id);
CREATE INDEX jl_account    ON journal_lines (account_id, booking_date DESC);
CREATE INDEX jl_skr        ON journal_lines (skr_account, tenant, booking_date DESC);
CREATE INDEX jl_tenant     ON journal_lines (tenant, booking_date DESC);

-- ── Verzugszinsen §288 BGB (default interest on overdue invoices) ─────────────
--
-- When a customer invoice is not paid by the due date, the creditor is entitled
-- to default interest per §288 BGB:
--   B2C (§288 Abs. 1 BGB): ECB base rate + 5 percentage points
--   B2B (§288 Abs. 2 BGB): ECB base rate + 9 percentage points
--
-- Interest accrues daily from the day after the due date.
-- A MAHNGEBUEHR ledger entry is created when interest is booked.

CREATE TABLE interest_charges (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id      UUID        NOT NULL REFERENCES accounts (account_id) ON DELETE CASCADE,
    tenant          TEXT        NOT NULL,
    -- The overdue invoice reference (ledger_entry.reference_id of the RECHNUNG)
    invoice_reference TEXT,
    -- Principal amount that is overdue (from the RECHNUNG ledger entry)
    principal_ct    BIGINT      NOT NULL CHECK (principal_ct > 0),
    -- Calculated interest amount in ct
    interest_ct     BIGINT      NOT NULL CHECK (interest_ct > 0),
    -- Interest rate applied: e.g. 12.12 (9% + ECB base 3.12%)
    rate_pct        NUMERIC(6,3) NOT NULL,
    -- ECB base rate used (for audit trail)
    ecb_base_rate_pct NUMERIC(6,3) NOT NULL,
    -- B2C or B2B (+5pp vs +9pp above base rate)
    customer_type   TEXT        NOT NULL CHECK (customer_type IN ('B2C', 'B2B')),
    -- Period for which interest is calculated
    period_from     DATE        NOT NULL,
    period_to       DATE        NOT NULL,
    -- Legal basis
    legal_basis     TEXT        NOT NULL DEFAULT '§288 Abs. 1 BGB',
    -- Linked ledger entry (MAHNGEBUEHR type, created when interest is booked)
    ledger_entry_id UUID        REFERENCES ledger_entries (id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE interest_charges IS
    'Verzugszinsen per §288 BGB. '
    'B2C: ECB base rate + 5pp (§288 Abs. 1). '
    'B2B: ECB base rate + 9pp (§288 Abs. 2). '
    'Linked to a MAHNGEBUEHR ledger entry when booked.';

CREATE INDEX ic_account ON interest_charges (account_id, created_at DESC);
CREATE INDEX ic_tenant  ON interest_charges (tenant, created_at DESC);

-- ── ECB base rate history (for Verzugszinsen §288 BGB calculation) ────────────
--
-- The ECB base rate (Basiszinssatz, §247 BGB) changes twice per year (Jan 1 + Jul 1).
-- This table stores the historical values for audit-accurate interest calculations.
-- Initial rows must be seeded by the operator; the service reads the current rate
-- by selecting the row with valid_from <= date ORDER BY valid_from DESC LIMIT 1.

CREATE TABLE ecb_base_rates (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    valid_from      DATE        NOT NULL UNIQUE,
    rate_pct        NUMERIC(6,3) NOT NULL,
    source          TEXT        NOT NULL DEFAULT 'BAnz AT',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE ecb_base_rates IS
    'ECB Basiszinssatz history per §247 BGB. '
    'Updated twice per year (Jan 1 + Jul 1). '
    'Used for §288 BGB Verzugszinsen calculation. '
    'Seed with current rates from Bundesbank / BAnz AT.';

-- Seed current rates (as of 2026-07-01; update as ECB changes rates)
INSERT INTO ecb_base_rates (valid_from, rate_pct, source) VALUES
    ('2025-01-01', 3.15, 'BAnz AT 2025-01-02'),
    ('2025-07-01', 2.65, 'BAnz AT 2025-07-01'),
    ('2026-01-01', 2.15, 'BAnz AT 2026-01-02'),
    ('2026-07-01', 1.65, 'BAnz AT 2026-07-01');

-- ── Payment plans / Zahlungsvereinbarung ──────────────────────────────────────
--
-- A Zahlungsvereinbarung (payment plan) allows a customer in financial difficulty
-- to pay an overdue balance in structured installments without triggering Sperrung.
--
-- Lifecycle:
--   ACTIVE     → installments are due per schedule
--   COMPLETED  → all installments paid → auto-resolve related dunning cases
--   CANCELLED  → operator cancelled before completion
--   DEFAULTED  → installment missed → auto-escalate to next Mahnstufe
--
-- Creating a plan:  POST /api/v1/accounts/{malo_id}/payment-plans
-- Listing plans:    GET  /api/v1/accounts/{malo_id}/payment-plans
-- Cancelling:       DELETE /api/v1/payment-plans/{id}

CREATE TABLE payment_plans (
    plan_id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id      UUID        NOT NULL REFERENCES accounts (account_id) ON DELETE CASCADE,
    tenant          TEXT        NOT NULL,
    -- Total amount covered by this plan (usually = balance_ct at plan creation)
    total_ct        BIGINT      NOT NULL CHECK (total_ct > 0),
    -- Amount per scheduled installment
    installment_ct  BIGINT      NOT NULL CHECK (installment_ct > 0),
    -- Number of installments (total_ct / installment_ct, possibly with final adjustment)
    installment_count INTEGER   NOT NULL CHECK (installment_count >= 1),
    -- Day of month for recurring installments (1–28)
    billing_day     SMALLINT    NOT NULL CHECK (billing_day BETWEEN 1 AND 28),
    status          TEXT        NOT NULL DEFAULT 'ACTIVE'
                    CHECK (status IN ('ACTIVE', 'COMPLETED', 'CANCELLED', 'DEFAULTED')),
    -- Optional reference to the dunning_case this plan resolves
    dunning_case_id UUID        REFERENCES dunning_cases (id),
    operator_sub    TEXT,
    note            TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE payment_plans IS
    'Zahlungsvereinbarung: structured payment plans for overdue balances. '
    'ACTIVE plans suppress automatic Sperrung escalation. '
    'DEFAULTED when an installment is missed (auto-escalates dunning).';

CREATE INDEX pp_account ON payment_plans (account_id, status);
CREATE INDEX pp_tenant  ON payment_plans (tenant, status, created_at);

CREATE TABLE payment_plan_installments (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    plan_id         UUID        NOT NULL REFERENCES payment_plans (plan_id) ON DELETE CASCADE,
    tenant          TEXT        NOT NULL,
    -- Installment number (1 = first)
    installment_no  INTEGER     NOT NULL,
    due_date        DATE        NOT NULL,
    amount_ct       BIGINT      NOT NULL CHECK (amount_ct > 0),
    status          TEXT        NOT NULL DEFAULT 'PENDING'
                    CHECK (status IN ('PENDING', 'PAID', 'OVERDUE', 'WAIVED')),
    -- Linked to the ZAHLUNG ledger entry when paid
    ledger_entry_id UUID        REFERENCES ledger_entries (id),
    paid_at         TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- One installment per (plan, number)
    UNIQUE (plan_id, installment_no)
);

COMMENT ON TABLE payment_plan_installments IS
    'Individual installments belonging to a payment_plan. '
    'Status OVERDUE is set by the daily worker when due_date passes without payment. '
    'WAIVED = operator manually waived the installment (operator_sub logged in plan).';

CREATE INDEX ppi_plan    ON payment_plan_installments (plan_id, installment_no);
CREATE INDEX ppi_due     ON payment_plan_installments (tenant, due_date)
    WHERE status = 'PENDING';

-- ── Bank import deduplication log (CAMT.054 dedup) ───────────────────────────
--
-- Every CAMT.054 bank transaction import records the bank's own transaction ID
-- here.  Re-importing the same bank file (e.g. operator error or ERP retry)
-- is detected and rejected without creating duplicate ledger entries.
--
-- The `bank_transaction_id` comes from:
--   CAMT.054 `<Ntry><NtryRef>` (entry reference) or
--   CAMT.054 `<Ntry><Dtls><Refs><EndToEndId>` (end-to-end reference)
--   Fallback: SHA-256(iban + amount + value_date + reference)

CREATE TABLE bank_import_log (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant              TEXT        NOT NULL,
    -- Stable bank-side transaction identifier (NtryRef or EndToEndId)
    bank_transaction_id TEXT        NOT NULL,
    -- Amount in ct (for audit; not used for dedup — only bank_transaction_id is used)
    amount_ct           BIGINT      NOT NULL,
    -- IBAN of the debtor/creditor involved
    iban                TEXT,
    -- Value date of the transaction
    value_date          DATE        NOT NULL,
    -- Linked ledger entry (ZAHLUNG or BANKRUECKLAST)
    ledger_entry_id     UUID        REFERENCES ledger_entries (id),
    imported_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- One import per (tenant, bank_transaction_id)
    UNIQUE (tenant, bank_transaction_id)
);

COMMENT ON TABLE bank_import_log IS
    'CAMT.054 bank transaction deduplication log. '
    'Prevents duplicate ZAHLUNG/BANKRUECKLAST entries on re-import of the same bank file. '
    'bank_transaction_id = NtryRef or EndToEndId from the CAMT.054 <Ntry>.';

CREATE INDEX bil_tenant      ON bank_import_log (tenant, imported_at DESC);
CREATE INDEX bil_value_date  ON bank_import_log (tenant, value_date DESC);


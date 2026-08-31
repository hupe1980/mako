-- ── accountingd schema — customer master data + payments satellites ──────────
--
-- The double-entry ledger itself — the journal, per-account balances, the
-- append-only Merkle log, period seals, and open-item clearing — lives in the
-- `doubleentry` crate's own schema (see `ledger.rs`), NOT here. This file holds
-- only the domain state around it: customer master data and the SEPA / dunning /
-- audit machinery.
--
-- Tables:
--   accounts             — one row per (malo_id, lf_mp_id, tenant) — Kundenstammdaten
--   sepa_mandates        — SEPA direct-debit mandate registry
--   dunning_cases        — Mahnwesen escalation (Mahnstufe 1–3)
--   interest_charges     — Verzugszinsen §288 BGB (default interest on overdue invoices)
--   ecb_base_rates       — Basiszinssatz history (§247 BGB)
--   payment_plans        — Zahlungsvereinbarung (structured installment agreements)
--   payment_plan_installments — individual installments per plan
--   bank_import_log      — CAMT.054 deduplication (bank transaction IDs already imported)
--   anonymization_log    — GDPR Art. 17 erasure audit trail (INSERT-only)
--   auto_dunning_runs    — daily auto-dunning idempotency + audit
--   eeg_payout_orders    — EEG SCT/SCT Inst payout pipeline
--   sepa_collection_runs — pain.008 XML archive for audit + replay
--   jahresabschluss_runs — annual settlement idempotency
--   account_audit_log    — §238 HGB master-data change trail
--
-- Regulatory: §40 EnWG (Abschlag), §238 HGB (Buchführungspflicht 10y),
--             §288 BGB (Verzugszinsen), GDPR Art. 15/17/20, SEPA Regulation 260/2012.
--
-- No pgcrypto: the IBAN lookup hash is computed in the application (keyed BLAKE3),
-- so pgcrypto's digest() is no longer needed. gen_random_uuid() is core PostgreSQL
-- (>= 13), not an extension, so surrogate keys keep their server-side default.

-- ── Kundenstammdaten ─────────────────────────────────────────────────────────

-- ── heute() — the business date ───────────────────────────────────────────────
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

CREATE TABLE accounts (
    account_id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    malo_id             TEXT        NOT NULL,
    lf_mp_id            TEXT        NOT NULL,
    tenant              TEXT        NOT NULL,

    -- Business-partner key (Geschäftspartner) linking to vertragd.kunden.kunden_nr.
    -- One customer may hold N market-location accounts; grouping by this key gives
    -- FI-CA-style contract-account aggregation (cross-MaLo balance & dunning).
    kunden_nr           TEXT,

    -- SEPA mandate IBAN (denormalized for fast payment-matching lookup).
    -- Stored as plaintext; for encrypted deployments set iban_encrypted=true and
    -- store ciphertext here — the lookup still works via iban_hash.
    iban                TEXT,
    -- Keyed BLAKE3 hash of the normalised IBAN (uppercase, no spaces), computed in
    -- the application (see ledger/iban_hash). Used as an indexed lookup key in
    -- CAMT.054 matching even when the IBAN is encrypted. A keyed hash — not plain
    -- SHA-256 — so the small IBAN keyspace cannot be enumerated offline from it.
    iban_hash           TEXT,
    -- Set to true when `iban` column stores ciphertext.
    iban_encrypted      BOOLEAN     NOT NULL DEFAULT false,
    mandatsref          TEXT,

    -- Monthly Abschlag in EUR-cent (1 ct = 0.01 EUR)
    -- §40 Abs. 1 EnWG: Abschlag must reflect estimated consumption
    abschlag_ct         BIGINT      NOT NULL DEFAULT 0,
    -- Day of month for automated Abschlag booking (1–28)
    billing_day         SMALLINT    NOT NULL DEFAULT 1,
    -- The USt rate this account's Abschlagsforderungen are raised at, as a
    -- fraction (0.19 = 19 %). § 14 Abs. 5 Satz 2 UStG: the settling invoice
    -- deducts the advances *and the tax attributable to them*, and one raised
    -- at 19 % takes a different amount of tax out of the same gross sum than
    -- one at 7 %. Copied onto each demand, so a later rate change does not
    -- rewrite what was already demanded.
    abschlag_ust_satz   NUMERIC(5,4) NOT NULL DEFAULT 0.19
                            CHECK (abschlag_ust_satz >= 0 AND abschlag_ust_satz < 1),

    -- Ledger-DERIVED balance projection (negative = credit, positive = debt),
    -- in EUR-cent. NOT the system of record: the authoritative balance is the
    -- signed net of this customer's Kontokorrent in the doubleentry ledger. This
    -- column is a materialized cache for portfolio queries (open receivables,
    -- aging, dunning candidate selection) that need set-based SUMs over all
    -- accounts. It is set ABSOLUTELY from the ledger net after every post — not
    -- incremented — so it cannot drift by arithmetic, and `reconcile_balance`
    -- re-derives it from the ledger.
    balance_ct          BIGINT      NOT NULL DEFAULT 0,

    -- The **§ 41f Abs. 3 Zahlungsverzug**, likewise ledger-derived and set
    -- absolutely. Deliberately not the same number as `balance_ct`:
    --
    --   * it counts only *open* debit residuals after FIFO clearing, so an
    --     unallocated credit cannot net an unpaid invoice out of sight;
    --   * it excludes Verzugsschaden (Mahngebühren, Verzugszinsen — see
    --     `pg::VERZUGSSCHADEN_KINDS`), which arise *because* of the default and
    --     may not count toward the threshold that authorises a disconnection;
    --   * it subtracts open `forderungs_einwaende` (§41f Abs. 3 S. 3–5).
    --
    -- Cached because deriving it walks the account's whole posting history.
    -- Refreshed on every posting, clearing and objection — the cadence of the
    -- facts it depends on, not of the questions asked about it.
    verzug_ct           BIGINT      NOT NULL DEFAULT 0,

    -- BO4E Vorauszahlung COM: typed advance-payment schedule (§40 EnWG)
    vorauszahlung       JSONB,
    -- BO4E Zahlungsinformation COM: IBAN/BIC/Zahlungsart for SEPA batch export
    zahlungsinformation JSONB,

    -- ── Postal address (ISO 20022 PstlAdr) ───────────────────────────────────
    -- The counterparty's own address: `Cdtr/PstlAdr` when accountingd pays this
    -- account (EEG Vergütung, Jahresabschluss-Erstattung), and the fallback for
    -- `Dbtr/PstlAdr` when a mandate carries none. BO4E's Zahlungsinformation COM
    -- has no address, so it cannot come from `zahlungsinformation`.
    --
    -- Version 1.1 of the 2025 SEPA rulebooks ends the unstructured address on
    -- 2026-11-15; `town` + `country` are what the schemes then require. Nullable
    -- until the cut-over, refused half-filled at build time.
    -- BO4E Sparte, learned from the `de.billing.rechnung.erstellt` CloudEvent.
    -- Drives the ISO 20022 `Purp/Cd` on a direct debit: `ELEC` for an
    -- electricity bill, `GASB` for gas, `WTER` for water. The code is
    -- informational — it instructs no bank — but it is what the debtor's
    -- statement and their accounting software read to categorise the
    -- collection, and an energy supplier collecting with no purpose at all is
    -- indistinguishable from any other Lastschrift on the statement.
    --
    -- Nullable: an account that has never been billed has no Sparte yet, and
    -- STROM_UND_GAS has no single ISO code, so both simply emit none.
    sparte              TEXT CHECK (sparte IS NULL OR sparte IN
                            ('STROM', 'GAS', 'FERNWAERME', 'NAHWAERME', 'WASSER', 'ABWASSER')),

    addr_town           TEXT,
    addr_country        TEXT CHECK (addr_country IS NULL OR addr_country ~ '^[A-Z]{2}$'),
    addr_street         TEXT,
    addr_building_number TEXT,
    addr_post_code      TEXT,
    addr_country_subdivision TEXT,

    -- GDPR Art. 17: set when PII was anonymized; financial records are retained
    anonymized_at       TIMESTAMPTZ,

    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- Tenant isolation: one Kundenkonto per (MaLo, LF, tenant)
    UNIQUE (malo_id, lf_mp_id, tenant)
);

COMMENT ON TABLE accounts IS
    'Customer master data (Kundenstammdaten). One row per (MaLo, Lieferant, tenant). '
    'balance_ct is a ledger-derived read cache, NOT the system of record — the '
    'authoritative balance is the signed net of this customer''s Kontokorrent in '
    'the doubleentry ledger. Regulatory: §40 EnWG, §238 HGB (10y retention), GDPR Art. 17.';

COMMENT ON COLUMN accounts.anonymized_at IS
    'GDPR Art. 17: set when PII (IBAN, mandatsref, zahlungsinformation) was '
    'replaced with anonymized placeholders. Financial records retained per §238 HGB.';

CREATE INDEX acct_tenant       ON accounts (tenant, lf_mp_id);
CREATE INDEX acct_malo_tenant  ON accounts (malo_id, tenant);
CREATE INDEX acct_bp           ON accounts (tenant, kunden_nr) WHERE kunden_nr IS NOT NULL;
-- Supports the portfolio open-receivables / dunning-candidate scans.
CREATE INDEX acct_overdue      ON accounts (tenant) WHERE balance_ct > 0;
-- The §41f candidate scan: accounts actually in Verzug on the supply debt.
CREATE INDEX acct_verzug       ON accounts (tenant) WHERE verzug_ct > 0;

-- ── Abschlagsforderungen (advance-payment register) ──────────────────────────
--
-- One row per advance the operator has **demanded**. The ledger holds the money
-- fact (a Kontokorrent debit against Erhaltene Anzahlungen); this table holds
-- the two document facts it does not carry: the USt rate the advance was raised
-- at (§ 14 Abs. 5 Satz 2 UStG makes it part of the deduction), and which
-- settling invoice absorbed it, so the same advance cannot be deducted twice.
--
-- Not a second ledger. Whether an advance was **received** (§ 14 Abs. 5: „die
-- vereinnahmten Teilentgelte") is never stored here — it is the residual of
-- `entry_id` after FIFO clearing, so payment state has one home.

CREATE TABLE abschlag_forderungen (
    tenant          TEXT        NOT NULL,
    -- `ABSCHLAG-{malo}-{YYYY}-{MM}`, and also the ledger idempotency key of the
    -- debit — so a re-run of the Abschlagslauf is a no-op on both.
    reference       TEXT        NOT NULL,
    account_id      UUID        NOT NULL REFERENCES accounts (account_id) ON DELETE CASCADE,
    malo_id         TEXT        NOT NULL,
    lf_mp_id        TEXT        NOT NULL,
    -- First day of the month the advance covers.
    periode         DATE        NOT NULL,
    -- When payment is owed. The SEPA collection and the Mahnwesen both read it.
    faellig_am      DATE        NOT NULL,
    -- Gross amount demanded, in EUR-cent. Always positive: a negative advance
    -- is a Gutschrift and is booked as one.
    betrag_ct       BIGINT      NOT NULL CHECK (betrag_ct > 0),
    -- The rate contained in `betrag_ct`, copied from `accounts.abschlag_ust_satz`
    -- when the demand was raised.
    ust_satz        NUMERIC(5,4) NOT NULL CHECK (ust_satz >= 0 AND ust_satz < 1),
    -- The Kontokorrent debit. The join key for "was it paid?".
    entry_id        UUID        NOT NULL,
    -- The Rechnungsnummer of the settling invoice that deducted this advance,
    -- set when the ABSCHLAG_VERRECHNUNG for it is booked. NULL = still open
    -- towards a future Jahres-/Schlussrechnung.
    verrechnet_mit  TEXT,
    verrechnet_at   TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant, reference),
    CHECK ((verrechnet_mit IS NULL) = (verrechnet_at IS NULL))
);

COMMENT ON TABLE abschlag_forderungen IS
    'Register of demanded advance payments (Abschlagsforderungen). Carries the '
    'USt rate each advance was raised at (§ 14 Abs. 5 Satz 2 UStG) and which '
    'settling invoice absorbed it. Payment state is NOT stored here — it is the '
    'residual of entry_id in the doubleentry ledger.';

-- The billingd read path: advances of one MaLo in a period, oldest first.
CREATE INDEX af_malo_periode ON abschlag_forderungen (tenant, malo_id, lf_mp_id, periode);
-- Advances still awaiting a settling invoice.
CREATE INDEX af_offen ON abschlag_forderungen (tenant, malo_id) WHERE verrechnet_mit IS NULL;

-- ── The double-entry ledger lives in the `doubleentry` schema ─────────────────
--
-- The former `ledger_entries` (immutable debit/credit log) and `journal_lines`
-- (SKR 03/04 double-entry shadow) tables are gone. Both are now one authoritative
-- structure owned by the `doubleentry` crate: an append-only journal with an
-- immutable Merkle log, per-account balances (no cached `balance_ct` to drift),
-- period seals, and open-item clearing — see `ledger.rs`. accountingd maps each
-- Buchungsart (RECHNUNG, ZAHLUNG, EEG_GUTSCHRIFT, …) to a balanced entry there.
-- doubleentry's tables live in their own PostgreSQL schema in this same database.

-- ── SEPA direct-debit mandates ────────────────────────────────────────────────

CREATE TABLE sepa_mandates (
    mandate_id      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id      UUID        NOT NULL REFERENCES accounts (account_id) ON DELETE CASCADE,
    tenant          TEXT        NOT NULL,
    iban            TEXT        NOT NULL,
    bic             TEXT,
    kontoinhaber    TEXT,
    -- Unique creditor-assigned Mandatsreferenz (SEPA SDD AT-01, ISO 20022
    -- Max35Text — also reused as the pain.008 EndToEndId).
    -- UNIQUE per tenant (not globally) to avoid cross-tenant namespace collisions.
    mandatsref      TEXT        NOT NULL CHECK (char_length(mandatsref) BETWEEN 1 AND 35),
    -- FRST = first collection; RCUR = recurring; FNAL = final; OOFF = one-off
    sequence_type   TEXT        NOT NULL CHECK (sequence_type IN ('FRST', 'RCUR', 'FNAL', 'OOFF')),
    signed_at       DATE        NOT NULL,
    revoked_at      DATE,
    -- ── Debtor postal address (ISO 20022 PstlAdr) ────────────────────────────
    -- Version 1.1 of the 2025 SEPA rulebooks ends the unstructured address on
    -- 2026-11-15, so from then a scheme message must carry TwnNm + Ctry. Both
    -- are nullable until the cut-over; a *partially* filled address is refused
    -- at pain.008 build time rather than silently emitting nothing.
    debtor_town             TEXT,
    debtor_country          TEXT CHECK (debtor_country IS NULL OR debtor_country ~ '^[A-Z]{2}$'),
    debtor_street           TEXT,
    debtor_building_number  TEXT,
    debtor_post_code        TEXT,
    debtor_country_subdivision TEXT,
    -- track mandate creation date for SEPA audit trail
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- track when the first successful collection occurred for FRST→RCUR auto-transition
    first_collected_at TIMESTAMPTZ,

    -- ── EPC dormancy (SDD Core Rulebook) ─────────────────────────────────────
    -- A mandate unused for **36 consecutive months** must be cancelled by the
    -- creditor. The clock resets on every *presentation*, including one later
    -- rejected or refunded, so this is stamped on submission rather than on
    -- settlement. The debtor banks do not enforce it; the creditor must.
    last_presented_at  TIMESTAMPTZ,

    -- ── Scheme ───────────────────────────────────────────────────────────────
    -- `CORE` (consumers; 8-week no-questions-asked refund) or `B2B` (business
    -- only; no refund right, and the debtor bank must hold the mandate).
    -- Different rulebooks and different `LclInstrm` codes, so collecting a B2B
    -- mandate as CORE grants a refund right the mandate does not carry.
    scheme          TEXT        NOT NULL DEFAULT 'CORE'
                    CHECK (scheme IN ('CORE', 'B2B'))
);

COMMENT ON COLUMN sepa_mandates.last_presented_at IS
    'EPC SDD Core Rulebook: a mandate unused for 36 consecutive months must be '
    'cancelled by the creditor. Stamped on presentation (not settlement) because '
    'the rulebook resets the clock on rejected and refunded collections too.';

-- mandatsref unique per tenant, not globally
CREATE UNIQUE INDEX sm_mandatsref_tenant ON sepa_mandates (tenant, mandatsref);

-- The Mandatsreferenz with separators stripped, for resolving an incoming
-- payment from its Verwendungszweck. A customer keys `MND-000123` back in as
-- `MND 000123`, `mnd000123`, or with the hyphen intact, and all three have to
-- find the same mandate. Generated by the database rather than the application
-- so the two spellings cannot drift, and indexed so the lookup stays a single
-- index probe instead of a sequential scan over every mandate in the tenant.
--
-- NOT unique: two distinct Mandatsreferenzen can normalise to the same string
-- (`A-1` and `A1`). `resolve_account_for_payment` refuses to book when a lookup
-- returns more than one account, which is the correct answer for that case.
ALTER TABLE sepa_mandates
    ADD COLUMN mandatsref_norm TEXT
    GENERATED ALWAYS AS (upper(regexp_replace(mandatsref, '[^A-Za-z0-9]', '', 'g'))) STORED;
CREATE INDEX sm_mandatsref_norm ON sepa_mandates (tenant, mandatsref_norm);
CREATE INDEX sm_account ON sepa_mandates (account_id);
CREATE INDEX sm_active  ON sepa_mandates (account_id)
    WHERE revoked_at IS NULL;
-- The dormancy sweep: mandates approaching or past the 36-month limit.
CREATE INDEX sm_dormant ON sepa_mandates (tenant, last_presented_at)
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
    -- ── §§41f/41g EnWG Versorgungsunterbrechung sequence ────────────────────
    -- Three phase marks and two dispatch references. What **halts** the sequence
    -- is deliberately not here: a halt is an account-level fact with a reason and
    -- a review date, so it lives in `dunning_locks`.
    sperrandrohung_at       TIMESTAMPTZ,
    sperrankuendigung_at    TIMESTAMPTZ,
    geplantes_sperrdatum    DATE,
    -- makod process id of the dispatched ORDERS 17115 Sperrauftrag.
    -- Non-NULL is the idempotency guard: the candidate query selects on NULL, so
    -- a second §41f disconnection order can never be placed for the same case.
    sperrauftrag_ce_id      TEXT,
    -- §41f Abs. 7 — makod process id of the ORDERS 17117 Entsperrauftrag issued
    -- once the grounds fell away. Restoration is *unverzüglich* and owed without
    -- being asked, so it follows automatically from the grounds ending.
    entsperrauftrag_ce_id   TEXT,
    -- The `outputd` document that communicated this Mahnung, and when. NULL
    -- means the customer has not been told: the escalation is in this table and
    -- nothing left the building — which at Mahnstufe 3 is a §§ 41f/41g sequence
    -- with no notice behind it.
    --
    -- A plain value, not a foreign key: outputd owns the document in its own
    -- database, and its append-only store keeps the reference resolvable
    -- (§ 147 AO).
    dokument_id             UUID,
    dokument_issued_at      TIMESTAMPTZ,
    CHECK ((dokument_id IS NULL) = (dokument_issued_at IS NULL)),
    -- §41g Abs. 1 S. 2 — when the Grundversorger's Abwendungsvereinbarung offer
    -- went out. Due within one week of a demand made after the Androhung, and at
    -- the latest with the Ankündigung. Recorded separately from acceptance: a
    -- supplier that never offered and one that offered and was refused are
    -- otherwise indistinguishable.
    abwendung_angeboten_at  TIMESTAMPTZ,
    -- Phase 2 and 3 read this: the disconnection announced on `geplantes_sperrdatum`
    -- may only proceed if the announcement is still the current one.
    CHECK (geplantes_sperrdatum IS NULL OR sperrankuendigung_at IS NOT NULL)
);

COMMENT ON TABLE dunning_cases IS
    'Mahnwesen escalation (Mahnstufe 1-3). At Mahnstufe 3 a §§41f/41g EnWG '
    'disconnection sequence runs: Sperrandrohung (4 Wochen) -> Sperrankuendigung '
    '(8 Werktage) -> Sperrauftrag (ORDERS 17115) -> Entsperrauftrag (17117). '
    'What halts it lives in dunning_locks; what reduces the Verzug lives in '
    'forderungs_einwaende.';

CREATE INDEX dc_account ON dunning_cases (account_id, stufe);
-- The document sweep: open cases nobody has been told about.
CREATE INDEX dc_undocumented ON dunning_cases (tenant, issued_at)
    WHERE resolved_at IS NULL AND dokument_id IS NULL;
CREATE INDEX dc_overdue ON dunning_cases (tenant, due_date)
    WHERE resolved_at IS NULL;
-- §41f Abs. 7 candidate scan: disconnected cases awaiting restoration.
CREATE INDEX dc_entsperr ON dunning_cases (tenant)
    WHERE sperrauftrag_ce_id IS NOT NULL AND entsperrauftrag_ce_id IS NULL;

-- CloudEvent idempotency no longer needs a table here: the ledger post is keyed
-- by the CloudEvent id, so a redelivery replays as a store-level no-op. There is
-- no separate `processed_events` guard to keep in sync.

-- ── Mahnsperren (dunning locks) ───────────────────────────────────────────────
--
-- A reason to stop dunning an account, with a validity period and an audit
-- trail. The shape is FI-CA's dunning lock: a reason code, a scope, a validity.
-- § 41f Abs. 2 makes the Schutzbeduerftigkeit 'auf Verlangen glaubhaft zu
-- machen', i.e. reviewable, so a flag that can only ever be set is the wrong
-- shape for a halt.
--
-- **Account-scoped**: disconnection is per supply point, and auto-dunning opens
-- a fresh case per Mahnstufe.

CREATE TABLE dunning_locks (
    lock_id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant          TEXT        NOT NULL,
    account_id      UUID        NOT NULL REFERENCES accounts (account_id) ON DELETE CASCADE,

    grund           TEXT        NOT NULL CHECK (grund IN (
                        -- §41g Abs. 1 S. 10 — accepted in Textform before the
                        -- disconnection was carried out; bars it outright.
                        'abwendungsvereinbarung',
                        -- §41f Abs. 2 — konkrete Gefahr fuer Leib oder Leben by
                        -- reason of personal or health circumstances.
                        'schutzbeduerftigkeit',
                        -- §41f Abs. 1 S. 2 — the customer showed hinreichende
                        -- Aussicht that they will meet their obligations.
                        'zahlungsaussicht',
                        -- Anything else an operator decides, always with a note.
                        'operator')),
    -- The citation the lock rests on, e.g. '§41g Abs. 1 S. 10 EnWG'. Free text
    -- rather than an enum: the same ground can rest on different Saetze.
    rechtsgrundlage TEXT        NOT NULL,
    note            TEXT,
    CHECK (grund <> 'operator' OR note IS NOT NULL),

    valid_from      DATE        NOT NULL DEFAULT heute(),
    -- NULL = open-ended. Permitted (a Schutzbeduerftigkeit may have no
    -- foreseeable end) but surfaced for review, so it stays a decision.
    valid_to        DATE,
    CHECK (valid_to IS NULL OR valid_to >= valid_from),

    -- Lifting is an act with its own reason, not a column set back to NULL.
    -- `vereinbarung_gebrochen` is §41g Abs. 1 S. 11: the sequence may resume, but
    -- §41f Abs. 1 S. 2 and Abs. 5 must be re-observed, so lifting for that reason
    -- also clears the Ankuendigung state on the account's open cases.
    aufgehoben_at    TIMESTAMPTZ,
    aufhebung_grund  TEXT,
    CHECK ((aufgehoben_at IS NULL) = (aufhebung_grund IS NULL)),

    -- `sub` of the operator who set it (§238 HGB traceability).
    created_by      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE dunning_locks IS
    'Reasons to stop dunning an account (FI-CA Mahnsperre): §41g Abs. 1 S. 10 '
    'Abwendungsvereinbarung, §41f Abs. 2 Schutzbeduerftigkeit, §41f Abs. 1 S. 2 '
    'Zahlungsaussicht, or an operator decision. Account-scoped, dated, and lifted '
    'with a reason.';

-- The hot path: "is this account locked today?".
CREATE INDEX dl_active ON dunning_locks (account_id, valid_from, valid_to)
    WHERE aufgehoben_at IS NULL;
CREATE INDEX dl_tenant ON dunning_locks (tenant, created_at DESC);
-- Open-ended locks that nobody has revisited.
CREATE INDEX dl_review ON dunning_locks (tenant, valid_from)
    WHERE aufgehoben_at IS NULL AND valid_to IS NULL;

-- ── Forderungseinwände (§ 41f Abs. 3 S. 3–5 EnWG) ────────────────────────────
--
-- Amounts that must stay **out of the Verzug calculation** when deciding whether
-- a disconnection is permitted. Not locks: they do not halt the sequence, they
-- reduce the number it is measured against, and the sequence stops by itself
-- when what is left falls below the Abs. 3 gates. The list is the statute's.

CREATE TABLE forderungs_einwaende (
    einwand_id      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant          TEXT        NOT NULL,
    account_id      UUID        NOT NULL REFERENCES accounts (account_id) ON DELETE CASCADE,
    -- The disputed booking, where one can be pointed at. NULL when the objection
    -- is against a part of the account rather than one document.
    ledger_entry_id UUID,

    art             TEXT        NOT NULL CHECK (art IN (
                        -- S. 3 — form- und fristgerecht, schluessig bestritten,
                        -- and not titled. Whether it qualifies is a human call;
                        -- recording it is the operator's act.
                        'forderung_bestritten',
                        -- S. 4 — a disputed price increase.
                        'preiserhoehung_bestritten',
                        -- S. 5 — the claim is before a §111b EnWG Schlichtung.
                        'schlichtung',
                        -- S. 3 — instalments under an agreement, not yet due.
                        'ratenzahlung_nicht_faellig')),
    betrag_ct       BIGINT      NOT NULL CHECK (betrag_ct > 0),
    erhoben_am      DATE        NOT NULL DEFAULT heute(),
    note            TEXT,

    -- Resolved: upheld, withdrawn, or decided against the customer. Either way
    -- the amount re-enters the Verzug from that point.
    erledigt_at     TIMESTAMPTZ,
    erledigung      TEXT        CHECK (erledigung IN
                        ('stattgegeben', 'zurueckgenommen', 'zurueckgewiesen')),
    CHECK ((erledigt_at IS NULL) = (erledigung IS NULL)),

    created_by      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE forderungs_einwaende IS
    'Amounts excluded from the § 41f Abs. 3 Zahlungsverzug: bestrittene '
    'Forderungen und Preiserhoehungen, §111b Schlichtungsverfahren, and '
    'noch nicht faellige Ratenzahlungsrueckstaende. They reduce the arrears the '
    'disconnection threshold is measured against; they do not halt the sequence.';

CREATE INDEX fe_open   ON forderungs_einwaende (account_id)
    WHERE erledigt_at IS NULL;
CREATE INDEX fe_tenant ON forderungs_einwaende (tenant, erhoben_am DESC);

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
-- `pain002_status` carries the ISO 20022 status the bank reported, verbatim:
--   ACTC accepted technical validation · ACCP accepted customer profile
--   ACSP accepted, in clearing        · ACSC settlement completed (funds moved)
--   ACWC accepted with change         · PART partially accepted
--   PDNG pending                      · RJCT rejected (see pain002_reason)
--   CANC cancelled before submission (accountingd's own, not an ISO code)
-- ACSC is the only status that means the money has actually moved; `settled_at`
-- is stamped on the first accepted status and is a submission milestone, not a
-- settlement proof.
--
-- Verification of Payee (mandatory for euro credit transfers since 2025-10-09)
-- reports on a *different axis* and is stored separately in `vop_outcome`: RCVC
-- says a payee name matched, which is not a statement about acceptance.

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
    -- Creditor postal address (ISO 20022 PstlAdr) — see the note on
    -- `sepa_mandates.debtor_*`; mandatory from the EPC cut-over on 2026-11-15.
    creditor_town             TEXT,
    creditor_country          TEXT CHECK (creditor_country IS NULL OR creditor_country ~ '^[A-Z]{2}$'),
    creditor_street           TEXT,
    creditor_building_number  TEXT,
    creditor_post_code        TEXT,
    creditor_country_subdivision TEXT,
    -- SCT_INST or SCT_CORE
    payment_type    TEXT        NOT NULL CHECK (payment_type IN ('SCT_INST', 'SCT_CORE')),
    -- ISO 20022 EndToEndId — unique per order, correlates pain.001 + pain.002
    end_to_end_ref  TEXT        NOT NULL,
    -- Verbatim pain.001 XML (for audit, bank replay, and debugging)
    pain001_xml     TEXT,
    -- ISO 20022 payment status reported by the bank (see the note above).
    pain002_status  TEXT        CHECK (pain002_status IN
                        ('ACTC','ACCP','ACSP','ACSC','ACWC','PART','PDNG','RJCT','CANC')),
    -- EPC SEPA reason code from pain.002 (e.g. AC01 = invalid IBAN, AM04 = insufficient funds)
    pain002_reason  TEXT,
    -- Verification of Payee outcome, a separate axis from acceptance:
    --   MATCH · CLOSE_MATCH · NO_MATCH · NOT_APPLICABLE
    -- On CLOSE_MATCH the payee's actual name, as the payee's PSP holds it,
    -- arrives in the pain.002 AddtlInf and is kept in `vop_name` so an operator
    -- can compare it before releasing the payment.
    vop_outcome     TEXT        CHECK (vop_outcome IN
                        ('MATCH','CLOSE_MATCH','NO_MATCH','NOT_APPLICABLE')),
    vop_name        TEXT,
    vop_reported_at TIMESTAMPTZ,
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

-- ── SEPA pain.008 collection runs (persist for audit + replay) ─────────
--
-- Every pain.008 batch is stored here.  Provides full audit trail per SEPA
-- Rulebook DS-01 requirements and allows replay if the ERP webhook fails.
-- One row per scheduled batch (one per billing_day per day).

CREATE TABLE sepa_collection_runs (
    run_id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant          TEXT        NOT NULL,
    -- Date the pain.008 batch targets (collection date at debtor bank)
    collection_date DATE        NOT NULL,
    -- GrpHdr/MsgId of the submitted message — the key a pain.002 reply quotes in
    -- OrgnlMsgId and a pain.007 reversal must reference in its own GrpHdr.
    msg_id          TEXT        NOT NULL,
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
-- A pain.002 reply quotes OrgnlMsgId; a camt Btch block quotes MsgId + PmtInfId.
CREATE UNIQUE INDEX scr_msg_id ON sepa_collection_runs (tenant, msg_id);

-- ── What each collection run actually collected ──────────────────────────────
--
-- The run row stores the XML; this table stores what is *in* it, one row per
-- collected mandate. Without it a bank reply cannot be attributed: a pain.002
-- rejection names an EndToEndId, a camt booking names a PmtInfId in its `Btch`
-- block, and a pain.007 reversal has to restate the original amount, mandate
-- and collection date exactly as submitted. Re-parsing the archived XML for
-- each of those would make the file the system of record.
--
-- No IBAN or account holder is duplicated here: both live on `sepa_mandates`
-- and are reached through `mandate_id`, so GDPR Art. 17 erasure keeps working
-- from one place. A reversal for an erased mandate is correctly impossible.

CREATE TABLE sepa_collection_entries (
    entry_id        UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id          UUID        NOT NULL REFERENCES sepa_collection_runs (run_id) ON DELETE CASCADE,
    tenant          TEXT        NOT NULL,
    -- NULL once the mandate is deleted; the audit row survives it.
    mandate_id      UUID        REFERENCES sepa_mandates (mandate_id) ON DELETE SET NULL,
    account_id      UUID        REFERENCES accounts (account_id) ON DELETE SET NULL,
    -- Mandatsreferenz (AT-01) — accountingd also uses it as the EndToEndId.
    mandatsref      TEXT        NOT NULL,
    end_to_end_id   TEXT        NOT NULL,
    -- PmtInfId of the group this entry sat in (`<MsgId>-<SEQ>`).
    payment_info_id TEXT        NOT NULL,
    sequence_type   TEXT        NOT NULL CHECK (sequence_type IN ('FRST', 'RCUR', 'FNAL', 'OOFF')),
    -- The scheme this collection went out under. Recorded on the entry rather
    -- than looked up from the mandate later, because a pain.007 reversal must
    -- restate the original **as submitted** — and the mandate may have been
    -- migrated between schemes since.
    scheme          TEXT        NOT NULL DEFAULT 'CORE' CHECK (scheme IN ('CORE', 'B2B')),
    amount_ct       BIGINT      NOT NULL CHECK (amount_ct > 0),
    -- Lifecycle, driven by the bank's own replies:
    --   SUBMITTED  written when the pain.008 is generated
    --   SETTLED    an accepted pain.002 status, or a matching camt booking
    --   REJECTED   pain.002 RJCT (never left the bank)
    --   RETURNED   camt.054 Rückläufer after settlement (R-transaction)
    --   REVERSED   the creditor sent it back via pain.007
    status          TEXT        NOT NULL DEFAULT 'SUBMITTED'
                    CHECK (status IN ('SUBMITTED', 'SETTLED', 'REJECTED', 'RETURNED', 'REVERSED')),
    status_reason   TEXT,
    status_at       TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE sepa_collection_entries IS
    'One row per mandate collected in a pain.008 run. The attribution key for '
    'pain.002 replies (EndToEndId), camt bookings (Btch/PmtInfId) and pain.007 '
    'reversals. Holds no IBAN — that stays on sepa_mandates for GDPR erasure.';

-- EndToEndId is the key a pain.002 rejection names.
CREATE UNIQUE INDEX sce_e2e     ON sepa_collection_entries (tenant, end_to_end_id, run_id);
CREATE INDEX sce_run            ON sepa_collection_entries (run_id);
CREATE INDEX sce_mandate        ON sepa_collection_entries (mandate_id, created_at DESC);
-- Btch/PmtInfId matches a booked collection back to the group submitted.
CREATE INDEX sce_pmtinf         ON sepa_collection_entries (tenant, payment_info_id);
CREATE INDEX sce_open           ON sepa_collection_entries (tenant, status)
    WHERE status = 'SUBMITTED';

-- ── pain.007 SEPA Direct Debit reversals ─────────────────────────────────────
--
-- A reversal is the creditor giving a *settled* collection back — the
-- counterpart to a debtor-initiated refund (which arrives as camt.054) and to a
-- reject (which arrives as pain.002). One row per reversed entry; the XML is
-- stored on each row of the batch it belongs to, keyed by `msg_id`.

CREATE TABLE sepa_reversals (
    reversal_id     UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant          TEXT        NOT NULL,
    -- The collected entry being given back.
    collection_entry_id UUID    REFERENCES sepa_collection_entries (entry_id) ON DELETE SET NULL,
    -- GrpHdr/MsgId of this reversal message, and the pain.008 it reverses.
    msg_id                   TEXT NOT NULL,
    original_msg_id          TEXT NOT NULL,
    original_payment_info_id TEXT NOT NULL,
    original_end_to_end_id   TEXT NOT NULL,
    original_amount_ct       BIGINT NOT NULL CHECK (original_amount_ct > 0),
    -- Equal to the original for a full reversal; the crate refuses more.
    reversed_amount_ct       BIGINT NOT NULL CHECK (reversed_amount_ct > 0),
    -- ISO 20022 ExternalReversalReason1Code (MS02, AM05, DUPL, CUST, …).
    reason_code     TEXT        NOT NULL,
    -- Verbatim pain.007 XML (audit + bank resubmission).
    pain007_xml     TEXT        NOT NULL,
    -- doubleentry EntryId of the compensating SEPA_STORNO (ledger schema; no FK).
    ledger_entry_id UUID,
    -- OIDC subject that authorised the reversal.
    created_by      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (reversed_amount_ct <= original_amount_ct)
);

COMMENT ON TABLE sepa_reversals IS
    'pain.007 SDD reversals — the creditor returning a settled collection. '
    'OrgnlTxRef is mandatory under the DK validation subset, so every field is '
    'restated from sepa_collection_entries rather than typed by an operator.';

-- One reversal per collected entry: a second attempt is a correction, not a
-- silent double refund.
CREATE UNIQUE INDEX srv_entry  ON sepa_reversals (collection_entry_id)
    WHERE collection_entry_id IS NOT NULL;
CREATE INDEX srv_tenant        ON sepa_reversals (tenant, created_at DESC);
CREATE INDEX srv_msg           ON sepa_reversals (tenant, msg_id);

-- Monthly Abschlag idempotency needs no table of its own: each Abschlag posts
-- through the ledger with a deterministic idempotency key
-- (`ABSCHLAG-{malo}-{YYYY}-{MM}`), so a scheduler restart mid-day replays as a
-- no-op in the store rather than double-booking.

-- ── Jahresabschluss idempotency ──────────────────────────────────────

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

-- ── Account master-data audit log (§238 HGB traceability) ──────────────
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

-- ── IBAN hash index (fast lookup even when IBAN is encrypted) ─────────────────
CREATE INDEX acct_iban_hash ON accounts (iban_hash, tenant) WHERE iban_hash IS NOT NULL;

-- The SKR 03/04 double-entry journal is not a shadow table here: it is the
-- doubleentry ledger itself. accountingd's `ledger.rs` posts each Buchungsart as a
-- balanced two-leg entry — the customer Kontokorrent (SKR 1400 subledger) against
-- a GL contra account (SKR 1200 Bank / 4000 Erlöse / 4003 Mahnerlöse / EEG-Aufwand).
-- doubleentry enforces the Soll=Haben invariant in-engine AND in its own schema
-- (a deferred constraint trigger), and additionally makes it tamper-evident.

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
    -- doubleentry EntryId of the MAHNGEBUEHR entry (in the ledger schema; no FK).
    ledger_entry_id UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- One interest charge per account and period. The MAHNGEBUEHR ledger entry
    -- is already idempotent on `interest:{malo}:{from}:{to}`, so without this
    -- the ledger stayed correct on a retry while this satellite grew a second
    -- row — and `GET /interest-charges` then showed the customer the same
    -- Verzugszinsen twice.
    UNIQUE (tenant, account_id, period_from, period_to)
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
    -- Total amount covered by this plan (usually = the Kontokorrent balance at plan creation)
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
    -- doubleentry EntryId of the ZAHLUNG entry when paid (ledger schema; no FK).
    ledger_entry_id UUID,
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
    -- `NtryDtls/Btch/PmtInfId` — the bank's own assertion of which submitted
    -- PmtInf group this booking aggregates. It is what matches a booked
    -- collection back to a `sepa_collection_runs` group without guessing from
    -- amounts and dates; NULL for a booking that is not a batch.
    payment_info_id     TEXT,
    -- EndToEndId of the underlying transaction, when the bank itemised it.
    end_to_end_id       TEXT,
    -- doubleentry EntryId of the ZAHLUNG/BANKRUECKLAST entry (ledger schema; no FK).
    ledger_entry_id     UUID,
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


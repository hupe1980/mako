---
layout: default
title: accountingd Operator Guide
nav_order: 33
parent: Services
mermaid: true
description: >
  accountingd operator guide — Massenkontokorrent / Customer Account Ledger (LF role).
  11 entry types, FIFO open-item management, CAMT.054 import, SEPA pain.008 + pain.001 XML
  (sepa 0.3.0), Mahnwesen automatic rule engine (Mahnstufe 1–3), GDPR Art. 17 pseudonymization,
  balance reconciliation, EEG Gutschrift + Marktprämie ingest, Jahresabschluss §40 EnWG,
  71 unit tests.
---

# `accountingd` — Massenkontokorrent / Customer Account Ledger

`accountingd` provides the **FI-CA equivalent** for the mako retail billing stack.
Without it, `billingd` invoices are fire-and-forget — no Offene-Posten tracking,
no automated dunning, no SEPA collection.

Port: **`:9380`**

---

## Why a dedicated ledger?

SAP IS-U calls this module **FI-CA** (Financial Contract Accounting). powercloud and
Wilken ENER:GY both include it natively. `accountingd` provides the same capabilities
as a standalone microservice with CloudEvents integration.

**The ledger is event-driven and idempotent.** CloudEvents from `billingd`, `einsd`,
and `invoicd` drive entries atomically — re-delivering the same CloudEvent produces
no duplicate entry (idempotency via `processed_events` table + DB lock).

---

## Event flow

```mermaid
graph TB
    billingd["billingd :9280"]
    einsd["einsd :9180"]
    invoicd["invoicd :8280"]
    accountingd["accountingd :9380"]
    erp["ERP webhook"]
    sperrd["sperrd :8780"]
    portald["portald :9480"]

    billingd -->|"de.billing.rechnung.erstellt → RECHNUNG debit\n(is_correction=true → STORNO)"| accountingd
    billingd -->|"de.billing.gutschrift.erstellt → GUTSCHRIFT credit"| accountingd
    einsd -->|"de.eeg.verguetung.berechnet → EEG_GUTSCHRIFT credit"| accountingd
    einsd -->|"de.eeg.marktpraemie.berechnet → EEG_MARKTPRAEMIE credit"| accountingd
    invoicd -->|"de.invoic.receipt.settled → ZAHLUNG credit"| accountingd

    accountingd -->|"de.accounting.mahnung.issued (Mahnstufe 1–3)"| erp
    accountingd -->|"de.accounting.sperrauftrag (Mahnstufe 3)"| sperrd
    accountingd -->|"GET /kontoauszug"| portald
```

---

## Ledger entry types

| `entry_type` | Sign | Trigger |
|---|---|---|
| `RECHNUNG` | +debit | `de.billing.rechnung.erstellt` (`is_correction=false`) |
| `STORNO` | ±signed | `de.billing.rechnung.erstellt` (`is_correction=true`) — billing reversal |
| `ZAHLUNG` | -credit | CAMT.054 import or `de.invoic.receipt.settled` |
| `GUTSCHRIFT` | -credit | `de.billing.gutschrift.erstellt` — credit note |
| `EEG_GUTSCHRIFT` | -credit | `de.eeg.verguetung.berechnet` — §21 EEG Einspeisevergütung |
| `EEG_MARKTPRAEMIE` | -credit | `de.eeg.marktpraemie.berechnet` — §20 EEG Direktvermarktung |
| `BANKRUECKLAST` | +debit | Returned SEPA direct debit |
| `MAHNGEBUEHR` | +debit | Dunning fee per Mahnstufe (configurable) |
| `ABSCHLAG` | +debit | Monthly advance payment (Abschlagslauf scheduler) |
| `JAHRESABSCHLUSS` | ±signed | Annual Mehr-/Mindermengenabrechnung (§40 EnWG) |
| `KORREKTUR` | ±signed | Manual operator correction via `POST /buchen` |

**Balance** = `SUM(amount_ct)` — negative = credit balance (customer overpaid); positive = outstanding debt.

**No f64 money.** All amounts use `i64` cents (1 ct = 0.01 EUR). The pain.008 XML
generator uses integer arithmetic — no floating-point rounding errors.

---

## Mahnwesen (dunning) lifecycle

The dunning engine operates in two modes: **automatic** (background worker) and **manual** (operator-triggered).

```mermaid
graph LR
    subgraph auto ["Auto-dunning worker (daily, dunning_auto_enabled=true)"]
        trigger["balance_ct > 0\n+ oldest RECHNUNG > grace_days\n+ no active dunning case"]
        a1["Auto: Mahnstufe 1\ncreated + fee1 (\u20ac0)"]
        a2["Auto: Mahnstufe 2\n+ fee2 (\u20ac5.00)"]
        a3["Auto: Mahnstufe 3\n+ fee3 (\u20ac10.00)\n+ Sperrauftrag"]
        trigger -->|"grade_days elapsed"| a1
        a1 -->|"due_date passed"| a2
        a2 -->|"due_date passed"| a3
    end

    subgraph manual ["Manual operator path"]
        m1["POST /dunning/{id}/escalate\
stufe=1|2|3"]
    end

    resolved["POST /dunning/{id}/resolve"]
    a1 -->|"payment received"| resolved
    a2 -->|"payment received"| resolved
    a3 -->|"payment received"| resolved
    m1 -->|"payment received"| resolved
```

**Automatic escalation** (P1-5 fix): set `dunning_auto_enabled = true` in config.
The worker runs daily, is idempotent (`auto_dunning_runs` UNIQUE guard), and emits
`de.accounting.sperrauftrag.batch` when Mahnstufe 3 cases are created.

**Manual escalation**: `POST /api/v1/dunning/{account_id}/escalate` remains available
for operator override (e.g. grace extensions, special B2B arrangements).

---

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/webhook` | Ingest CloudEvents (billingd, einsd, invoicd) |
| `GET/PUT` | `/api/v1/accounts/{malo_id}` | Account CRUD (IBAN, Abschlag, billing_day) |
| `GET` | `/api/v1/accounts/{malo_id}/balance` | Current balance in ct; status: overdue/credit/settled |
| `GET` | `/api/v1/accounts/{malo_id}/ledger` | Paged ledger entries |
| `GET` | `/api/v1/accounts/{malo_id}/kontoauszug` | Account statement (portald-consumable) |
| `GET` | `/api/v1/accounts/{malo_id}/open-items` | **FIFO open-item list** — individual unpaid/partial invoices |
| `PUT` | `/api/v1/accounts/{malo_id}/abschlag` | Update monthly advance payment |
| `GET/PUT` | `/api/v1/accounts/{malo_id}/vorauszahlung` | Typed `rubo4e::current::Vorauszahlung` (§40 EnWG) |
| `GET/PUT` | `/api/v1/accounts/{malo_id}/zahlungsinformation` | Typed `rubo4e::current::Zahlungsinformation` |
| `POST` | `/api/v1/accounts/{malo_id}/buchen` | **Manual booking** (operator-authorised ledger entry) |
| `POST` | `/api/v1/accounts/{malo_id}/reconcile` | **Balance reconciliation** — detect/repair `balance_ct` cache drift |
| `POST` | `/api/v1/accounts/{malo_id}/anonymize` | **GDPR Art. 17** pseudonymization (preserves ledger) |
| `POST` | `/api/v1/payments/import` | Ingest CAMT.054 bank statement (JSON array) |
| `GET` | `/api/v1/offene-posten` | Overdue accounts |
| `GET` | `/api/v1/dunning` | Open dunning cases |
| `POST` | `/api/v1/dunning/{account_id}/escalate` | Manual Mahnstufe escalation |
| `POST` | `/api/v1/dunning/{id}/resolve` | Mark dunning case resolved |
| `POST` | `/api/v1/sepa/mandates` | Register SEPA mandate (IBAN validated via mod-97) |
| `GET` | `/api/v1/sepa/mandates/{id}` | Fetch mandate |
| `DELETE` | `/api/v1/sepa/mandates/{id}` | **Revoke mandate** (§58 ZAG) |
| `POST` | `/api/v1/sepa/run` | Generate pain.008 XML for all active Abschlag mandates |
| `POST` | `/api/v1/jahresabschluss/{malo_id}` | **Annual settlement** (§40 EnWG Mehr-/Mindermengenabrechnung) |
| `GET` | `/health` · `/health/ready` | Liveness / readiness |

---

## Manual booking (`POST /api/v1/accounts/{malo_id}/buchen`)

For operator-authorised bookings not driven by CloudEvents:

```bash
curl -X POST "http://accountingd:9380/api/v1/accounts/51238696780/buchen" \
  -H "Content-Type: application/json" \
  -d '{
    "entry_type":   "ZAHLUNG",
    "amount_ct":    -5000,
    "reference_id": "BANK-TXN-2026-07-10",
    "description":  "Überweisung Kunde (ausserhalb SEPA)"
  }'
```

Allowed `entry_type` values: `RECHNUNG`, `ZAHLUNG`, `GUTSCHRIFT`, `EEG_GUTSCHRIFT`,
`EEG_MARKTPRAEMIE`, `BANKRUECKLAST`, `MAHNGEBUEHR`, `ABSCHLAG`, `JAHRESABSCHLUSS`, `KORREKTUR`, `STORNO`.

`amount_ct`: positive = debit (increases outstanding debt); negative = credit (reduces debt).

---

## Jahresabschluss (§40 Abs. 1 EnWG)

The annual settlement compares actual billed amounts against advance payments collected:

```bash
# Preview (dry_run=true)
curl "http://accountingd:9380/api/v1/jahresabschluss/51238696780?year=2025&dry_run=true"

# Commit
curl -X POST "http://accountingd:9380/api/v1/jahresabschluss/51238696780?year=2025"
```

Response:
```json
{
  "malo_id":                  "51238696780",
  "year":                     2025,
  "rechnung_sum_ct":          120000,
  "abschlag_paid_ct":         -108000,
  "settlement_ct":            12000,
  "settlement_eur":           "120.00",
  "new_monthly_abschlag_ct":  10000,
  "action":                   "NACHZAHLUNG",
  "committed":                true,
  "ce_id":                    "3fa85f64-..."
}
```

When committed, writes a `JAHRESABSCHLUSS` entry (positive = Nachzahlung; negative = Erstattung)
and updates the monthly `abschlag_ct` to `actual_annual ÷ 12` (§40 Abs. 1 EnWG).

The annual sum includes: `RECHNUNG` + `STORNO` + `MAHNGEBUEHR` (net billed amounts including reversals).

---

## Vorauszahlung (§40 Abs. 1 EnWG)

```bash
curl -X PUT "http://accountingd:9380/api/v1/accounts/51238696780/vorauszahlung" \
  -H "Content-Type: application/json" \
  -d '{
    "_typ": "VORAUSZAHLUNG",
    "betrag": { "_typ": "BETRAG", "wert": "75.00", "waehrung": "EUR" },
    "gueltigkeit": { "_typ": "ZEITRAUM", "startdatum": "2026-08-01" }
  }'
```

Syncs `abschlag_ct = 7500` atomically. GET returns the stored BO4E object or synthesises
from `abschlag_ct` when no typed value has been stored.

---

## IBAN validation

Every SEPA mandate PUT validates the IBAN using **ISO 13616 mod-97** via the
[`sepa`](https://crates.io/crates/sepa) 0.3.0 crate (`sepa::validate_iban`).
Covered by **21 tests** in `unit_tests.rs` (DE, GB, NL, AT, CH, checksum failures, length, lowercase).

---

## Open-item management (FIFO clearing)

`GET /api/v1/accounts/{malo_id}/open-items` returns individual unpaid or partially-paid
invoice debits using **FIFO clearing** against available credits:

```json
{
  "malo_id": "51238696780",
  "balance_ct": 15000,
  "open_item_count": 2,
  "open_items": [
    { "entry_type": "RECHNUNG", "amount_ct": 8000, "outstanding_ct": 0,
      "reference_id": "R2026-05", "booking_date": "2026-05-15" },
    { "entry_type": "RECHNUNG", "amount_ct": 12000, "outstanding_ct": 15000,
      "reference_id": "R2026-06", "booking_date": "2026-06-15" }
  ]
}
```

The oldest debits are cleared first. This matches SAP FI-CA “oldest-first” default and
§252 HGB Abs. 1 Nr. 4 (Vorsichtsprinzip — individual receivables must be tracked separately).

`balance_ct` remains the authoritative balance; open-items add invoice-level transparency.

---

## Balance integrity (`POST /reconcile`)

`balance_ct` is a cached sum updated atomically with every ledger write (`SELECT FOR UPDATE`).
A crash between the INSERT and the UPDATE could leave it inconsistent:

```bash
# Check only
curl -X POST "http://accountingd:9380/api/v1/accounts/51238696780/reconcile"

# Detect + repair
curl -X POST "http://accountingd:9380/api/v1/accounts/51238696780/reconcile?repair=true"
```

Response:
```json
{
  "is_consistent": true,
  "cached_balance_ct": 5000,
  "recomputed_balance_ct": 5000,
  "drift_ct": 0
}
```

When `drift_ct != 0`, the `repair=true` flag atomically resets `balance_ct` to `SUM(amount_ct)`.
Schedule this as a weekly health check in your monitoring pipeline.

---

## GDPR Art. 17 — Pseudonymization

```bash
curl -X POST "http://accountingd:9380/api/v1/accounts/51238696780/anonymize" \
  -H "Content-Type: application/json" \
  -d '{ "requested_by": "operator-1", "legal_basis": "GDPR Art. 17 - customer request #42" }'
```

**What is anonymized**: `accounts.iban` → `ANONYMIZED`, `mandatsref`/`zahlungsinformation`/`vorauszahlung` → `NULL`; `sepa_mandates.iban` → `ANONYMIZED`, `kontoinhaber` → `ANONYMIZED`, `bic` → `NULL`.

**What is preserved**: All `ledger_entries` (amounts, dates, types, references) — exempt from
GDPR Art. 17 under Art. 17(3)(b) and §238 HGB / §147 AO retention requirements (10 years).

**Audit trail**: An immutable record is written to `anonymization_log` (GDPR Art. 5(2)).

The operation is idempotent — returns `409 Conflict` if already anonymized.

---

## CAMT.054 payment import

```bash
curl -X POST "http://accountingd:9380/api/v1/payments/import" \
  -H "Content-Type: application/json" \
  -d '[{ "iban": "DE89 3704 0044 0532 0130 00", "amount_eur": "155.42",
          "reference": "Rechnung R2026-06-001", "date": "2026-07-10" }]'
```

Matches by IBAN → writes `ZAHLUNG` credit (or `BANKRUECKLAST` for returned direct debits) → updates balance. Returns `{ "accepted": 1, "total": 1 }`.

Amount parsing uses `sepa::ct_from_eur_str` (sepa 0.3.0) — integer arithmetic only, **no f64**.

---

## SEPA payments (sepa 0.3.0)

`accountingd` uses the [`sepa`](https://crates.io/crates/sepa) crate v0.3.0 which adds several capabilities beyond the 0.2 pain.008-only API:

```mermaid
graph LR
    subgraph out ["Outgoing payments"]
        pain008["pain.008 SDD\nDirect Debit\n(N-5 scheduler + /sepa/run)"]
        pain001["pain.001 SCT\nCredit Transfer\n(EEG Vergütung refunds)"]
    end
    subgraph in ["Bank responses"]
        pain002["pain.002 parser\nPayment Status Report\n(bank rejection handling)"]
        camt053["camt.053 parser\nEnd-of-day statement\n(reconciliation)"]
    end
    creditor["Creditor Identifier\n(EPC AT-02)"]
    creditor --> pain008
    creditor --> pain001
```

### pain.008 Direct Debit

```bash
curl -X POST "http://accountingd:9380/api/v1/sepa/run" -H "Accept: application/xml" > abschlag-2026-07.xml
```

Generates ISO 20022 pain.008.003.02 via `sepa::Pain008Builder` for all active mandates with `abschlag_ct > 0`.

- **Typed `SequenceType`**: FRST/RCUR/FNAL/OOFF dispatch per mandate
- **`with_description`**: Each entry carries `"Abschlag YYYY-MM"` as RemittanceInfo (`Ustrd`) — visible on debtor’s bank statement
- **Hard error**: missing or invalid `creditor_iban` returns HTTP 503 (no silent placeholder IBAN)
- **N-5 scheduler**: Background worker auto-generates and dispatches pain.008 5 days before each `billing_day`

To revoke a mandate (§58 ZAG — customer right to revoke before cut-off):
```bash
curl -X DELETE "http://accountingd:9380/api/v1/sepa/mandates/{mandate_id}"
```

### pain.001 Credit Transfer (sepa 0.3.0 — new)

For outgoing payments (EEG Vergütung payout to plant operators, Jahresabschluss Erstattung):

```rust
use accountingd::sepa::build_pain_001;
// 200.00 EUR Erstattung to plant operator
let xml = build_pain_001(
    "DE89370400440532013000",  // LF's own IBAN (debit side)
    &[("DE29100500005001065004", "Franz Huber", 20_000, "ERSTATTUNG-2025")],
    false, // false = standard SCT; true = SCT Instant
)?;
```

Supports **SCT Instant** (`LocalInstrument::Inst`, pain.001.001.09) for real-time payments.

### pain.002 + camt.053 parsers (sepa 0.3.0 — new)

| Parser | Use case |
|---|---|
| `sepa::parse_pain002` | Bank rejection report → auto-create `BANKRUECKLAST` entries |
| `sepa::parse_camt053` | End-of-day bank statement → full automated reconciliation |

---

## Idempotency

Every CloudEvent `ce_id` is written to `processed_events` atomically with the ledger entry.
Re-delivering produces no duplicate. The `/buchen` endpoint has no idempotency guard —
supply `reference_id` for audit trails.

---

## Database schema

### `accounts`

| Column | Notes |
|--------|-------|
| `account_id` | UUID primary key |
| `malo_id`, `lf_mp_id` | Customer + LF identity |
| `balance_ct` | Cached balance (i64 ct) — updated atomically on every write |
| `abschlag_ct` | Monthly advance payment in ct |
| `billing_day` | Day of month for advance payment (1–28) |
| `iban`, `mandatsref` | Active SEPA mandate link (fast lookup) |
| `vorauszahlung` | `rubo4e::current::Vorauszahlung` JSONB |
| `zahlungsinformation` | `rubo4e::current::Zahlungsinformation` JSONB |
| `anonymized_at` | GDPR Art. 17 timestamp — set when account is pseudonymized |

**Tenant isolation**: `(malo_id, lf_mp_id, tenant)` UNIQUE constraint (migration 0005).

### `ledger_entries` (immutable)

`amount_ct > 0` = debit; `amount_ct < 0` = credit. Balance = `SUM(amount_ct)`.
Includes `booking_date` (Buchungsdatum) and `value_date` (Wertstellung) — may differ
for backdated corrections (§238 HGB).

### `sepa_mandates`

| Column | Notes |
|--------|-------|
| `mandatsref` | UNIQUE creditor-assigned mandate reference |
| `sequence_type` | `FRST` / `RCUR` / `FNAL` / `OOFF` |
| `signed_at` | Datum der Unterzeichnung |
| `revoked_at` | Set by `DELETE /api/v1/sepa/mandates/{id}` |

### `dunning_cases`, `processed_events`

Standard schema — see `migrations/0001_initial.sql`.

### `anonymization_log`

Immutable audit trail for GDPR Art. 17 operations. Stores `requested_by`, `legal_basis`, `anonymized_fields` (JSON array), `anonymized_at`. Required by GDPR Art. 5(2) accountability principle.

### `auto_dunning_runs`

Idempotency guard for the auto-dunning background worker. One row per `(tenant, run_date)` — prevents double-escalation from crash+restart on the same calendar day.

### Migrations summary

| Migration | Content |
|---|---|
| `0001_initial.sql` | `accounts`, `ledger_entries`, `sepa_mandates`, `dunning_cases`, `processed_events` |
| `0003_zahlungsinformation.sql` | `accounts.zahlungsinformation JSONB` (typed BO4E payment info) |
| `0004_entry_types.sql` | Extended CHECK: `STORNO`, `EEG_MARKTPRAEMIE`, `JAHRESABSCHLUSS` |
| `0005_tenant_unique.sql` | `(malo_id, lf_mp_id, tenant)` UNIQUE constraint (tenant isolation) |
| `0006_open_items_gdpr.sql` | `accounts.anonymized_at`, `anonymization_log`, `auto_dunning_runs` |

---

## Configuration

```toml
database_url          = "postgresql://accountingd:secret@db:5432/accountingd"
port                  = 9380
tenant                = "9910000000002"
erp_webhook_url       = "http://erp:8000/webhooks/accounting"
sperrd_url            = "http://sperrd:8780"

# Dunning fees per Mahnstufe
dunning_fee_stufe1_ct = 0     # no fee for first reminder
dunning_fee_stufe2_ct = 500   # 5.00 EUR
dunning_fee_stufe3_ct = 1000  # 10.00 EUR
dunning_grace_days    = 30

# Auto-dunning rule engine (opt-in, default false)
dunning_auto_enabled  = true   # set to true to enable daily auto-escalation

# SEPA creditor IBAN (required for pain.008 generation; hard error if missing/invalid)
creditor_iban         = "DE89370400440532013000"

# SEPA N-5 pre-notification window (default: 5 calendar days)
sepa_pre_notification_days = 5
```

> **`creditor_iban` is now required.** Missing or invalid `creditor_iban` causes `POST /sepa/run`
> to return HTTP 503. The N-5 background worker also blocks (no silent placeholder IBAN fallback).

---

## MCP server

10 tools at `/mcp` (Streamable HTTP 2025-11-25):

`get_balance` · `list_ledger` · `list_dunning` · `list_overdue` · `update_abschlag` ·
`import_payments` · `run_sepa_collection` · `trigger_jahresabschluss` ·
`run_abschlag_cycle` · `compute_bilanzielle_abgrenzung` · `suggest_payment_match` ·
`post_manual_booking`

The `payment-reconciliation-agent` in `agentd` uses these tools for automated payment
matching (powercloud-equivalent >98% match rate).

---

## Testing

**71 unit tests** (`cargo test -p accountingd --test unit_tests`):

- IBAN validation (21 tests): DE/GB/NL/AT/CH, checksum, length, lowercase
- Entry type coverage: all 11 types, sign conventions, STORNO vs KORREKTUR semantics
- Jahresabschluss: Nachzahlung/Erstattung/Ausgeglichen, STORNO inclusion in annual sum
- **Decimal precision** (P0-1 fix): `f64` vs `Decimal` rounding correctness (`1.99 EUR = 199 ct`, not 198 ct)
- **Open-item FIFO** (P1-3): formula verification for FIFO clearing across 4 scenarios
- **GDPR anonymization** (P1-4): field list completeness, required-field validation
- **Auto-dunning rules** (P1-5): grace period logic, default fee schedule
- pain.008 formatting: integer arithmetic, no f64, CtrlSum validation
- SEPA sequence types (FRST/RCUR/FNAL/OOFF), mandate revocation

```bash
cargo test -p accountingd --all-features
```

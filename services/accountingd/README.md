# accountingd — Massenkontokorrent / Customer Account Ledger

`accountingd` is the **FI-CA equivalent** for the mako retail billing stack. Without it,
`billingd` invoices are fire-and-forget — no Offene-Posten tracking, no automated dunning,
no SEPA collection.

| Feature | Detail |
|---|---|
| **HTTP port** | `:9380` |
| **Database** | PostgreSQL (sqlx 0.8) — customer/SEPA satellites in `public`, the ledger in the `doubleentry` schema of the same database |
| **Auth** | OIDC/JWT on write endpoints + inbound webhook HMAC-SHA256 |
| **Ledger** | The [`doubleentry`](https://github.com/hupe1980/doubleentry) crate: an **immutable, tamper-evident double-entry engine** — balanced by construction, an append-only Merkle log with inclusion/consistency proofs, period seals (GoBD/§ 146 AO), open-item clearing, and store-level idempotency. accountingd owns the chart of accounts (`ledger::Chart`) and the `entry_type → postings` mapping; every money movement flows through `pg::post_entry` → `ledger.post`. |
| **Double-entry** | Each Buchungsart is one balanced entry: the per-MaLo **Kontokorrent** (SKR 1400 subledger, an Asset leaf whose signed net *is* the balance) against a GL contra leaf (Bank 1200 / Erlöse 4000 / Mahnerlöse 4003 / EEG-Aufwand / Erstattungen). Soll = Haben is enforced in-engine **and** by a deferred DB trigger; §238 HGB. `accounts.balance_ct` is a ledger-derived read cache (set absolutely from the ledger net — never incremented, so it cannot drift) backing the portfolio SUM queries. |
| **Vorauszahlung** | `PUT/GET /api/v1/accounts/{malo_id}/vorauszahlung` — typed `rubo4e::current::Vorauszahlung` (§40 Abs. 1 EnWG) |
| **Aging analysis** | `GET /api/v1/aging` — receivables by 0–30d / 31–60d / 61–90d / >90d |
| **Verzugszinsen** | `GET/POST /api/v1/accounts/{malo_id}/interest-charges` — §288 BGB B2C/B2B |
| **Payment plans** | `GET/POST /api/v1/accounts/{malo_id}/payment-plans` — Zahlungsvereinbarung |
| **SEPA mandates** | IBAN validated via **ISO 13616 mod-97** on PUT; `sepa_mandates` table (UNIQUE per tenant) |
| **SEPA scheduler** | N-5 background worker generates **one pain.008 message per collection date** (one `PmtInf` group per SequenceType, mandatory Gläubiger-ID); persisted in `sepa_collection_runs` |
| **SEPA Gläubiger-ID** | `creditor_id` config field (EPC AT-02); validated via `sepa::validate_creditor_id`; included as `<CdtrSchmeId>` |
| **FRST→RCUR transition** | Auto-transitions FRST mandate to RCUR after first successful collection |
| **CAMT.054 import** | `POST /api/v1/payments/import` — deduplicated by `bank_transaction_id` (prevents re-import) |
| **IBAN encryption ready** | `iban_hash` is an app-computed **keyed BLAKE3** lookup key (no pgcrypto); `iban_encrypted` flag; CAMT.054 matching uses the hash even when the IBAN is ciphertext |
| **Abschlag model** | ABSCHLAG booked as advance-payment **credit** (negative); full-cost Jahresrechnung as debit — the balance nets to the Nachzahlung/Erstattung |
| **Mahnwesen** | Mahnstufe 1→2→3; auto-dunning worker (advisory-locked, opt-in) |
| **Sperrung handoff** | Mahnstufe-3 arrears ≥ `sperrung_threshold_ct` (§19 Abs. 2 StromGVV, default 100 EUR) → `POST sperrd /api/v1/sperr-orders` (idempotent) |
| **Business partner** | `kunden_nr` links accounts to `vertragd.kunden`; `GET /api/v1/business-partners/{kunden_nr}/{accounts,balance}` aggregate cross-MaLo |
| **Refund payout** | Jahresabschluss Erstattung → **pain.001** to the customer IBAN (credit balance carried forward when no IBAN) |
| **Metrics** | `GET /metrics` — Prometheus gauges (open receivables, credit balances, dunning by Mahnstufe, pending SEPA runs) |
| **Worker safety** | Abschlag/dunning workers hold a PostgreSQL advisory lock; all money workers are idempotent (per-run guards) |
| **Jahresabschluss** | Annual settlement (§40 EnWG); idempotent per year via `jahresabschluss_runs`; recalibrates the monthly Abschlag |
| **Festschreibung** | `POST /api/v1/periods/{id}/seal` closes + seals a period (GoBD / § 146 AO / § 239 HGB) — after which a backdated booking into it is refused; `GET /api/v1/periods/seals` lists the chained seals and verifies the chain |
| **Audit proofs** | `GET /api/v1/entries/{id}/proof` returns an `O(log n)` Merkle inclusion proof (content hash + tree head) — an auditor can verify an entry is committed without this service |
| **Offene-Posten (OP-Verwaltung)** | Authoritative open items via recorded **FIFO Zahlungszuordnung** (doubleentry clearing) — every post matches open credits against the oldest open debits, so `GET .../open-items` shows real residuals (§ 252 HGB). `POST .../clear` re-runs matching; `POST /api/v1/clearings/{id}/reset` releases a mis-assignment |
| **Summen- und Saldenliste** | `GET /api/v1/trial-balance` — GL trial balance (§ 238 HGB): gross Soll/Haben turnover + Saldo per account, Σ debits = Σ credits (Kontokorrent leaves aggregated into one Debitoren line) |
| **MCP** | 12 tools at `/mcp` |
| **Tests** | pure unit + integration tests + DB-backed scenario tests (`tests/db_scenarios.rs`, `just test-accountingd-db` — idempotency, netting, reconcile, **period seal + backdate rejection**, **Merkle inclusion proof against Postgres**) |
| **Health** | `GET /health/live`, `GET /health/ready` |

## Security

- **OIDC/JWT**: all financial write endpoints require a valid Bearer token; dev mode emits `[WARN]`
- **Inbound HMAC**: `POST /webhook` verifies `X-Mako-Signature: sha256=...`; constant-time comparison
- **SecretString**: `erp_hmac_secret` never appears in logs or debug output

## IBAN validation

Every SEPA mandate PUT validates the IBAN via the ISO 13616 mod-97 checksum algorithm.
Malformed IBANs are rejected at the API boundary with HTTP 422.
The validation logic is covered by unit tests (DE/GB/NL/AT/CH checksums) without a database.

## SEPA pain.008

`POST /api/v1/sepa/run` returns a JSON array of XML batches — one per `SequenceType`.
FRST and RCUR mandates are in separate batches (EPC SDD Core Rulebook §3.8 compliance).
Each batch is stored in `sepa_collection_runs` for audit and ERP webhook replay.

The XML schema version defaults to the current EPC releases (`pain.008.001.08`,
`pain.001.001.09`) and can be pinned per bank with the optional `pain008_schema` /
`pain001_schema` config keys (e.g. `pain.008.001.02` for the pre-2023 version).
Unknown values fail at startup rather than on a rejected batch.

## Configuration

```toml
# accountingd.toml
port                  = 9380
tenant                = "9900357000004"
creditor_iban         = "DE89370400440532013000"
creditor_id           = "DE74ZZZ09999999999"   # SEPA Gläubiger-ID (EPC AT-02)
creditor_name         = "Muster Energie GmbH"
# pain008_schema      = "pain.008.001.02"       # optional; default pain.008.001.08
# pain001_schema      = "pain.001.001.03"       # optional; default pain.001.001.09
# Outbound de.accounting.* CloudEvents. Delivery is durable: each event is
# written to `event_outbox` in the same transaction as the ledger change
# (persist-before-dispatch) and drained by a background worker with retry +
# dead-letter — a crash never drops an event.
erp_webhook_url       = "http://erp:8000/events"
erp_hmac_secret       = "env:ACCOUNTINGD_INBOUND_HMAC_SECRET"
dunning_auto_enabled  = true
dunning_grace_days    = 30

[database]
url = "postgresql://accountingd:secret@db:5432/accountingd"
# pool_size = 10   # optional pool tuning (min_connections, acquire/idle/max_lifetime)

[oidc]
issuer   = "https://keycloak:8080/realms/mako"
audience = "accountingd"

[eeg]
sepa_instant   = true
auto_payout    = true
debtor_iban    = "env:LF_BANK_IBAN"
bank_submit_url = "https://banking-adapter.internal/api/v1/pain001"
bank_api_key   = "env:BANK_API_KEY"
```

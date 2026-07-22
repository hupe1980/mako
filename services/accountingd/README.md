# accountingd — Massenkontokorrent / Customer Account Ledger

`accountingd` is the **FI-CA equivalent** for the mako retail billing stack. Without it,
`billingd` invoices are fire-and-forget — no Offene-Posten tracking, no automated dunning,
no SEPA collection.

| Feature | Detail |
|---|---|
| **HTTP port** | `:9380` |
| **Database** | PostgreSQL (sqlx 0.8, 16 tables) |
| **Auth** | OIDC/JWT on write endpoints + inbound webhook HMAC-SHA256 |
| **Ledger** | Immutable `ledger_entries`; `amount_ct != 0` CHECK; idempotent on CloudEvent `ce_id` |
| **Double-entry** | `journal_lines` shadow — balanced Soll/Haben per entry (SKR 03/04) |
| **Vorauszahlung** | `PUT/GET /api/v1/accounts/{malo_id}/vorauszahlung` — typed `rubo4e::current::Vorauszahlung` (§40 Abs. 1 EnWG) |
| **Aging analysis** | `GET /api/v1/aging` — receivables by 0–30d / 31–60d / 61–90d / >90d |
| **Verzugszinsen** | `GET/POST /api/v1/accounts/{malo_id}/interest-charges` — §288 BGB B2C/B2B |
| **Payment plans** | `GET/POST /api/v1/accounts/{malo_id}/payment-plans` — Zahlungsvereinbarung |
| **SEPA mandates** | IBAN validated via **ISO 13616 mod-97** on PUT; `sepa_mandates` table (UNIQUE per tenant) |
| **SEPA scheduler** | N-5 background worker generates **FRST/RCUR-separated** pain.008 batches; each persisted in `sepa_collection_runs` |
| **SEPA Gläubiger-ID** | `creditor_id` config field (EPC AT-02); validated via `sepa::validate_creditor_id`; included as `<CdtrSchmeId>` |
| **FRST→RCUR transition** | Auto-transitions FRST mandate to RCUR after first successful collection |
| **CAMT.054 import** | `POST /api/v1/payments/import` — deduplicated by `bank_transaction_id` (prevents re-import) |
| **IBAN encryption ready** | `iban_hash` generated column (pgcrypto SHA-256); `iban_encrypted` flag; CAMT.054 matching uses hash |
| **Mahnwesen** | Mahnstufe 1→2→3; auto-dunning worker (opt-in); `de.accounting.sperrauftrag` → `sperrd` |
| **Jahresabschluss** | Annual reconciliation (§40 EnWG); idempotent per year via `jahresabschluss_runs` |
| **MCP** | 12 tools at `/mcp` |
| **Tests** | 107 tests (75 unit + 16 integration, no DB required; additional DB-backed tests via `integration-tests` feature) |
| **Health** | `GET /health/live`, `GET /health/ready` |

## Security

- **OIDC/JWT**: all financial write endpoints require a valid Bearer token; dev mode emits `[WARN]`
- **Inbound HMAC**: `POST /webhook` verifies `X-Mako-Signature: sha256=...`; constant-time comparison
- **SecretString**: `erp_hmac_secret` never appears in logs or debug output

## IBAN validation

Every SEPA mandate PUT validates the IBAN via the ISO 13616 mod-97 checksum algorithm.
Malformed IBANs are rejected at the API boundary with HTTP 422.
The validation logic is covered by **21 unit tests** without a database.

## SEPA pain.008

`POST /api/v1/sepa/run` returns a JSON array of XML batches — one per `SequenceType`.
FRST and RCUR mandates are in separate batches (EPC SDD Core Rulebook §3.8 compliance).
Each batch is stored in `sepa_collection_runs` for audit and ERP webhook replay.

## Configuration

```toml
# accountingd.toml
database_url          = "postgresql://accountingd:secret@db:5432/accountingd"
port                  = 9380
tenant                = "9900357000004"
creditor_iban         = "DE89370400440532013000"
creditor_id           = "DE74ZZZ09999999999"   # SEPA Gläubiger-ID (EPC AT-02)
creditor_name         = "Muster Energie GmbH"
erp_webhook_url       = "http://erp:8000/events"
erp_hmac_secret       = "env:ACCOUNTINGD_INBOUND_HMAC_SECRET"
dunning_auto_enabled  = true
dunning_grace_days    = 30

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

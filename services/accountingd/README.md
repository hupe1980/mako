# accountingd — Massenkontokorrent / Customer Account Ledger

`accountingd` is the **FI-CA equivalent** for the mako retail billing stack. Without it,
`billingd` invoices are fire-and-forget — no Offene-Posten tracking, no automated dunning,
no SEPA collection.

| Feature | Detail |
|---|---|
| **HTTP port** | `:9380` |
| **Database** | PostgreSQL (sqlx 0.8, dynamic queries) |
| **Auth** | OIDC/JWT + Cedar ABAC + webhook HMAC-SHA256 |
| **Ledger** | Immutable `ledger_entries`; `amount_ct > 0` = debit, `< 0` = credit; idempotent on CloudEvent `ce_id` |
| **Vorauszahlung** | `PUT/GET /api/v1/accounts/{malo_id}/vorauszahlung` — typed `rubo4e::current::Vorauszahlung`; syncs `abschlag_ct` column (§40 Abs. 1 EnWG) |
| **SEPA mandates** | IBAN validated via **ISO 13616 mod-97 algorithm** on PUT; `sepa_mandates` table; pain.008 XML generation |
| **SEPA scheduler** | Background worker runs daily; generates pain.008 on `billing_day - 5`; emits `de.accounting.payment.due` |
| **CAMT.054 import** | `POST /api/v1/payments/import` — matches transfers to open items by IBAN + Verwendungszweck |
| **Mahnwesen** | Mahnstufe 1→2→3; Mahnstufe 3 emits `de.accounting.sperrauftrag` → `sperrd` |
| **Jahresabschluss** | Annual reconciliation: actual bill − Σ(Abschläge) |
| **Health** | `GET /health/live`, `GET /health/ready` |

## IBAN validation

Every SEPA mandate PUT validates the IBAN via the ISO 13616 mod-97 checksum algorithm.
Malformed IBANs are rejected at the API boundary with HTTP 422.
The validation logic is covered by **21 unit tests** without a database:

```bash
cargo test -p accountingd --test unit_tests
```

## Configuration

```toml
# accountingd.toml
database_url       = "postgresql://accountingd:secret@db:5432/accountingd"
port               = 9380
tenant             = "9900357000004"
creditor_iban      = "DE89370400440532013000"  # LF SEPA creditor account
creditor_name      = "Muster Energie GmbH"

[erp]
webhook_url  = "http://erp:8000/events"
hmac_secret  = "${ERP_HMAC_SECRET}"
```

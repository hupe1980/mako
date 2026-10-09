# accountingd

`accountingd` (`:9380`) is the customer account ledger (Massenkontokorrent) of
the retail stack: open items, Abschläge, dunning and the §§ 41f/41g EnWG
Sperr-Sequenz, SEPA collections and payouts, bank-statement import and the
Jahresabschluss. Every money movement is one balanced entry in the
[`doubleentry`](https://github.com/hupe1980/doubleentry) ledger — append-only,
Merkle-logged and sealable per period (GoBD / § 146 AO).

## Run it

```bash
just dev accountingd   # dev/config/accountingd.toml against the dev PostgreSQL
```

## At a glance

| | |
|---|---|
| **Database** | PostgreSQL — satellites (accounts, mandates, dunning, SEPA runs) in `public`, the ledger in the `doubleentry` schema |
| **Auth** | OIDC + Cedar `policies/accountingd.cedar` on every route; reads split into `read-account`, `read-banking`, `read-books`. `/webhook` is HMAC-verified |
| **Ledger** | accountingd owns the chart of accounts (`ledger::Chart`) and the Buchungsart → postings map; everything posts through `pg::post_entry` |
| **SEPA** | pain.008 per collection date (CORE/B2B × sequence type), pain.001 payouts, pain.002 status (incl. Verification of Payee), pain.007 reversal, camt.055/029 recall; structured `PstlAdr` for the EPC cut-over on 15 Nov 2026 |
| **Bank import** | camt.052/053/054 and a flat JSON import; only `BOOK` entries post, deduplicated on the bank reference |
| **Dunning** | Mahnstufe 1→3, then Sperrandrohung → Sperrankündigung → Sperrauftrag (ORDERS 17115 via `makod`) → Entsperrauftrag (17117); Mahnsperren and Forderungseinwände are recorded acts |
| **Events** | `de.accounting.*` through the transactional outbox to `erp_webhook_url` |
| **MCP** | 13 tools at `/mcp`, each authorized as its REST twin |
| **Metrics** | `GET /accountingd/metrics` (the runner owns `/metrics`) |

## Routes

| Area | Routes |
|---|---|
| Accounts | `/api/v1/accounts/{malo_id}` and its `/balance`, `/ledger`, `/kontoauszug`, `/open-items`, `/clear`, `/buchen`, `/reconcile`, `/abschlag(e)`, `/vorauszahlung`, `/zahlungsinformation`, `/interest-charges`, `/payment-plans`, `/business-partner`, `/anonymize` |
| Reports | `GET /api/v1/aging`, `/offene-posten`, `/trial-balance`, `/business-partners/{kunden_nr}/{accounts,balance}` |
| Dunning | `/api/v1/dunning`, `/{id}/locks`, `/{id}/einwaende`, `/{id}/abwendung/angebot`, `/{id}/resolve`, `/{account_id}/escalate` |
| SEPA | `/api/v1/sepa/mandates`, `/run`, `/pain002`, `/reversals`, `/recalls`, `/camt029`, `/collections/{run_id}/entries` |
| Payments | `POST /api/v1/payments/import`, `/import/camt052`, `/camt053`, `/camt054` |
| Periods & proofs | `POST /api/v1/periods/{id}/seal`, `GET /periods/{id}/balance-proof`, `/entries/{id}/proof`, `/entries/consistency-proof` |
| Jahresabschluss · EEG | `POST /api/v1/jahresabschluss/{malo_id}`; `/api/v1/eeg/payouts` |

## Configuration

Top-level: `tenant`, `port`, `creditor_iban`, `creditor_id` (Gläubiger-ID),
`creditor_name`, `pain008_schema` / `pain001_schema`, `erp_webhook_url`,
`erp_hmac_secret`, `makod_url` / `makod_api_key` (without them the Sperr-Sequenz
does not run), `sperrkosten_ct` / `entsperrkosten_ct`, `dunning_auto_enabled`,
`dunning_grace_days`. Tables: `[database]`, `[oidc]`, `[creditor_address]`,
`[eeg]`. DB-backed scenario tests: `just test-accountingd-db`.

The [operator guide](https://hupe1980.github.io/mako/docs/services/accountingd/)
covers the ledger model, SEPA, dunning, the emitted events and the full
configuration reference.

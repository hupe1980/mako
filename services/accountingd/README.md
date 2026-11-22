# accountingd

`accountingd` is the Lieferant's customer account ledger (Massenkontokorrent):
open items, Abschläge, dunning and the §§ 41f/41g EnWG Sperr-Sequenz, SEPA
collections and payouts, bank-statement import and the Jahresabschluss. For the
Netzbetreiber it pays the EEG Gutschriften `einsd` settles. Every money movement
is one balanced entry in the [`doubleentry`](https://github.com/hupe1980/doubleentry)
ledger, committed with the business change that causes it; it prices nothing.

| | |
|---|---|
| **Port** | 9380 (`deploy/ports.toml`) |
| **Database** | PostgreSQL — satellites in `public`, the journal in schema `ledger` |
| **MCP** | 20 tools, 1 prompt at `/mcp` |
| **Emits** | `de.mako.accounting.*` (`payment.imported`, `mahnung.issued`, `sperrauftrag`, `sepa.collection-rejected`, …) to the ERP webhook |
| **Consumes** | `de.mako.billing.rechnung.erstellt`, `de.mako.eeg.verguetung.berechnet`, `de.mako.eeg.marktpraemie.berechnet` at `POST /webhook` |
| **Depends on** | `makod` (ORDERS 17115/17117), `outputd` (Mahnung), `vertragd` (Mahnung recipient) |

## Run

```bash
just dev    # the whole estate, accountingd on deploy/demo/accountingd.toml, seeded
ACCOUNTINGD_CONFIG=deploy/demo/accountingd.toml cargo run -p accountingd -- --check-config
```

`just test-accountingd-db` runs the PostgreSQL scenario suite.

## API

| Route | Purpose |
|---|---|
| `/api/v1/accounts/{malo_id}/…` | balance, ledger, Kontoauszug, open items, Abschläge, payment plans, locks |
| `GET /api/v1/aging` · `/offene-posten` · `/trial-balance` | reports |
| `/api/v1/dunning/…` | open cases, Mahnstufen, Einwände, Abwendung, escalation into the Sperr-Sequenz |
| `/api/v1/sepa/…` | mandates, pain.008 runs, pain.002, reversals, recalls |
| `POST /api/v1/payments/import` · `/import/camt05{2,3,4}` | bank-statement import |
| `GET /api/v1/payments/unmatched` · `POST /api/v1/payments/{payment_id}/assign` | bank lines no account claimed, and their assignment |
| `/api/v1/credit-payouts…` | a Schlussrechnung's Guthaben owed back (§ 40c Abs. 3 EnWG), overdue and paid |
| `/api/v1/periods/…` · `/api/v1/entries/…` | period seals and audit proofs |
| `POST /api/v1/jahresabschluss/{malo_id}` | the annual settlement |
| `/api/v1/eeg/…` | EEG payouts, Rückforderungen, Aufrechnungen, EnFG payments |

`GET /api/v1/openapi.json` serves the OpenAPI 3.1 document of every `/api/v1`
route, committed as [`openapi.json`](openapi.json); the reads shared with MCP
answer the same type on both doors.

The full route list with each Cedar action is the guide's
[Endpoints](https://hupe1980.github.io/mako/docs/services/accountingd/#endpoints).

## Accounts

An account is keyed `(malo_id, mp_id, tenant)`, where `mp_id` is the MP-ID of
the own Marktrolle it is kept under: the Lieferant's (`lf_mp_id`) for a
customer, the Netzbetreiber's (`[eeg] nb_mp_id`) for an EEG plant. The EEG
payout is the Netzbetreiber's — § 19 Abs. 1 EEG 2023: Anlagenbetreiber „haben
für den in diesen Anlagen erzeugten Strom gegen den Netzbetreiber einen
Anspruch auf 1. die Marktprämie nach § 20, 2. eine Einspeisevergütung …, 3.
einen Mieterstromzuschlag …" — so the Netzbetreiber pays, the Anlagenbetreiber
is paid, and neither the Direktvermarktungsunternehmer nor a Lieferant is a
party to it. The routes that serve every account take `?mp_id=`; the
Lieferant's customer features (Verbraucher status, Abschlag, payment plans,
locks, interest, payment assignment) keep `?lf_mp_id=`. Both default to
`lf_mp_id`, and a key a route does not take is refused.

## Configuration

`tenant` (the isolation key); `lf_mp_id`, the Lieferant whose customer accounts
the ledger keeps and the default of every `lf_mp_id` / `mp_id` a request or
event leaves out (required, an MP-ID); `[database]` (`url`, and `migration_url` for the role that may write
the journal); `[oidc]`; the SEPA creditor `creditor_iban`, `creditor_id`,
`creditor_name` (without it every pain.008 and Mahnung is
refused — `503 creditor_name_not_configured` on a REST door), `[creditor_address]`;
`[eeg] nb_mp_id`, the Netzbetreiber the EEG plant accounts are kept under
(without it every EEG receipt is refused `503 eeg_nb_not_configured` and
redelivered), `[eeg] debtor_iban` / `debtor_name` / `[eeg.debtor_address]`, the
Netzbetreiber as SEPA debtor of every EEG pain.001 (without the first two a
payout is refused `503 eeg_debtor_not_configured`; `auto_payout = true` refuses
the start without `nb_mp_id`, `debtor_iban` or `debtor_name`); `makod_url` / `makod_api_key` (without
them the Sperr-Sequenz does not run); `outputd_url`, `vertragd_url`;
`erp_webhook_url` / `erp_hmac_secret`. `dunning_auto_enabled` requires
`dunning_fee_stufe{1,2,3}_ct` and `dunning_zahlungsfrist_stufe{1,2,3}_tage`.
accountingd has no `[policy]` reading.

The [operator guide](https://hupe1980.github.io/mako/docs/services/accountingd/)
covers the ledger model, SEPA, dunning, the events and the full configuration
reference.

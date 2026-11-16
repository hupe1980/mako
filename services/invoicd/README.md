# invoicd

`invoicd` checks every INVOIC a market partner sends this deployment — as
Lieferant by default, or as Netzbetreiber or ESA — records it as a
Buchungsbeleg (§ 147 AO) and answers the counterparty, accept or dispute,
through `makod`. It handles PIDs 31001–31009 and 31011; 31010
(Kapazitätsrechnung) is refused into `invoic_dlq`.

| | |
|---|---|
| **Port** | 8280 (`dev/ports.toml`) |
| **Database** | PostgreSQL — `invoic_receipts`, `leistungsnachweise`, `invoic_dlq` |
| **MCP** | 9 tools, 4 prompts at `/mcp` |
| **Emits** | `de.invoic.receipt.settled`, `de.invoic.receipt.disputed`, `de.invoic.payment.overdue` to the ERP webhook |
| **Consumes** | `de.mako.process.initiated` from `marktd` at `POST /webhook` (HMAC) |
| **Depends on** | `makod` (answer commands, the Rechnung), `marktd` (Preisblätter, MMM prices, ESA-Angebote) |

## Run

```bash
just dev invoicd       # dev/config/invoicd.toml against the dev PostgreSQL
INVOICD_CONFIG=dev/config/invoicd.toml cargo run -p invoicd -- --check-config
```

`just test-invoicd-db` runs the PostgreSQL suite (needs Docker).

## API

| Route | Purpose |
|---|---|
| `POST /webhook` | inbound process events from `marktd` |
| `GET /api/v1/receipts` · `/receipts/{id}` · `/receipts/{id}/rechnung` | receipts and the BO4E Rechnung |
| `GET /api/v1/disputes` · `/overdue-remadv` · `/zahlungsstatus/{malo_id}` | disputes, missing REMADV, payment state |
| `POST /api/v1/receipts/{id}/confirm-payment` · `/dispatch-answer` · `/resolve-dispute` | the Lieferant's decisions |
| `POST /api/v1/receipts/{id}/feststellungen` · `/api/v1/leistungsnachweise` | the statements that decide a held invoice |

Each route's Cedar action is in the guide's
[Routes and authorization](https://hupe1980.github.io/mako/docs/services/invoicd/#routes-and-authorization).

## Configuration

`[identity]` `tenant` and `marktrolle` (`lieferant`, `netzbetreiber` or `esa`
— the role this deployment receives invoices in); `[makod]` and `[marktd]`
`url` / `api_key`; `[webhook] inbound_secret`; `[subscription] webhook_url`;
`[erp] webhook_url` / `hmac_secret`; `[database]`, `[oidc]`. `[check]` holds the
tolerances and `auto_dispute_threshold_eur`. invoicd has no `[policy]` reading.

The [operator guide](https://hupe1980.github.io/mako/docs/services/invoicd/)
covers the checks, the answer trees, ERP delivery, metrics and the full
configuration reference.

# invoicd

`invoicd` checks every INVOIC a market partner sends this deployment — as
Lieferant by default, or as Netzbetreiber or ESA — records it as a
Buchungsbeleg (§ 147 AO) and answers the counterparty, accept or dispute,
through `makod`. It handles PIDs 31001–31009 and 31011; 31010
(Kapazitätsrechnung) is refused into `invoic_dlq`.

| | |
|---|---|
| **Port** | 8280 (`deploy/ports.toml`) |
| **Database** | PostgreSQL — `invoic_receipts`, `leistungsnachweise`, `invoic_dlq` |
| **MCP** | 11 tools, 4 prompts at `/mcp` |
| **Emits** | `de.mako.invoic.receipt.settled`, `de.mako.invoic.receipt.disputed`, `de.mako.invoic.payment.overdue` to the ERP (the webhook endpoint `default`) and every registered endpoint |
| **Consumes** | `de.mako.engine.process.initiated` from `marktd` at `POST /webhook` (HMAC) |
| **Depends on** | `makod` (answer commands, the Rechnung), `marktd` (Preisblätter, MMM prices, ESA-Angebote) |

## Run

```bash
just dev    # the whole estate, invoicd on deploy/demo/invoicd.toml, seeded
INVOICD_CONFIG=deploy/demo/invoicd.toml cargo run -p invoicd -- --check-config
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
| `/api/v1/webhooks/endpoints/**` · `GET /api/v1/events` | webhook endpoints, deliveries, replay, the event feed |
| `GET /api/v1/openapi.json` | the OpenAPI document; `openapi.json` here is its committed copy |

Each route's Cedar action is in the guide's
[Routes and authorization](https://hupe1980.github.io/mako/docs/services/invoicd/#routes-and-authorization).

## Configuration

`[identity]` `tenant` and `marktrolle` (`lieferant`, `netzbetreiber` or `esa`
— the role this deployment receives invoices in); `[makod]` and `[marktd]`
`url` / `api_key`; `[webhook] inbound_secret`; `[erp] webhook_url` /
`hmac_secret` (the webhook endpoint `default`); `[subscriptions]
retention_days`; `[database]`, `[oidc]`. `[check]` holds the
tolerances and `auto_dispute_threshold_eur`. invoicd has no `[policy]` reading.

The [operator guide](https://hupe1980.github.io/mako/docs/services/invoicd/)
covers the checks, the answer trees, ERP delivery, metrics and the full
configuration reference.

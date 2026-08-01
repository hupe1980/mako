# portald — Customer Portal Read-Model Gateway (LF role)

`portald` is a **headless REST + SSE aggregation gateway** for customer-facing portals.
It never decodes JWTs or maintains its own MaLo maps — all authorization flows through
`vertragd`.

| Feature | Detail |
|---|---|
| **HTTP port** | `:9480` |
| **Auth** | OIDC/JWT (Bearer); routes to `vertragd` for sub → malo_id resolution |
| **Dashboard** | `GET /api/v1/portal/{malo_id}/dashboard` — aggregated snapshot |
| **Lastgang** | `GET /api/v1/portal/{malo_id}/lastgang` — interval time-series from `edmd` |
| **Invoices** | `GET /api/v1/portal/{malo_id}/invoices` — billing history from `billingd` |
| **Balance** | `GET /api/v1/portal/{malo_id}/balance` — Kundenkonto balance from `accountingd` |
| **Kontoauszug** | `GET /api/v1/portal/{malo_id}/kontoauszug` — account statement |
| **EEG** | `GET /api/v1/portal/{malo_id}/eeg` — EEG plant + settlement from `einsd` |
| **VersorgungsStatus** | `GET /api/v1/portal/{malo_id}/versorgung` — supply status from `marktd` |
| **Vorauszahlung** | `GET /api/v1/portal/{malo_id}/vorauszahlung` — advance-payment schedule (§40 Abs. 1 EnWG) |
| **Contract view** | `GET /api/v1/portal/{malo_id}/vertrag` — active contract (prerequisite for Tarifwechsel/Kündigung UI) |
| **Invoice download** | `GET /api/v1/portal/{malo_id}/invoices/{record_id}/download` — XRechnung 3.0 CII XML (EN 16931) |
| **SSE stream** | `GET /api/v1/portal/{malo_id}/events` — live event stream |
| **Self-service writes** | `POST /api/v1/portal/{malo_id}/tarifwechsel`, `POST /api/v1/portal/{malo_id}/kuendigen`, `PUT /api/v1/portal/{malo_id}/kontakt`, `PUT /api/v1/portal/{malo_id}/sepa` |
| **MCP** | `/mcp` — 8 read tools + 3 guided prompts (see below) |
| **Health** | `GET /health/live`, `GET /health/ready` |

## MCP server

`portald` exposes a read-only MCP server at `/mcp` (Streamable HTTP) so an LLM client
can answer customer-service questions over the aggregated read model.

| Tool | Purpose |
|---|---|
| `get_dashboard` | Aggregated snapshot: MaLo metadata, latest invoice, balance, supply status, EEG plants |
| `get_lastgang` | MSCONS consumption time-series (15-min / hourly), optional ISO-8601 range |
| `get_invoices` | Billing history (newest-first, `limit` default 10) |
| `get_balance` | Open-items balance in EUR cents (positive = owed, negative = credit) |
| `get_kontoauszug` | Full account statement — all ledger entries (§666 BGB transparency) |
| `get_vorauszahlung` | Advance-payment (Abschlag) schedule, next due date (§40 Abs. 1 EnWG) |
| `get_eeg_status` | EEG/KWKG plant list + latest settlement (Förderungsende, model, capacity) |
| `get_versorgung` | Supply status (Beliefert / Unbeliefert / Gesperrt) and effective date |

Guided prompts: `customer-overview`, `billing-dispute`, `eeg-foerderung-check`.

## Configuration

```toml
# portald.toml
port           = 9480
tenant         = "9900357000004"

vertragd_url   = "http://vertragd:9780"
edmd_url       = "http://edmd:8380"
edmd_api_key   = "env:PORTALD_EDMD_SERVICE_KEY"  # opaque Bearer; register in edmd [[oidc.service_keys]]
billingd_url   = "http://billingd:9280"
accountingd_url = "http://accountingd:9380"
einsd_url      = "http://einsd:9180"
marktd_url     = "http://marktd:8180"
```

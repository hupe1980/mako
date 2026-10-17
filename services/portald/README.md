# portald

`portald` (`:9480`) is the customer portal gateway of the LF side: one
customer-facing REST API assembled on the request path from `vertragd`, `edmd`,
`billingd`, `accountingd`, `einsd`, `marktd` and `outputd`. It has no database,
cache or session store — scale replicas freely.

## Run it

```bash
just dev portald       # dev/config/portald.toml
```

## Authorization

`portald` verifies no tokens itself. Every route forwards the customer's bearer
token to `vertragd GET /api/v1/kunden/authenticate?malo_id=…` and serves only on
its verdict; each handler takes the resulting `PortalAuthCtx`, and
`tests/it/authorization_guard.rs` drives every route against a refusing
`vertragd`.

The invoice download additionally checks the record's own `malo_id`. Startup
refuses without `vertragd_url` unless `allow_insecure_no_auth = true`.

## Endpoints

All paths are prefixed `/api/v1/portal/{malo_id}`.

| Method | Path | Upstream |
|---|---|---|
| `GET` | `/dashboard` | all five, concurrently; a field is `null` when its upstream has no data |
| `GET` | `/lastgang?from=&to=` | `edmd` |
| `GET` | `/invoices?limit=&outcome=` | `billingd` |
| `GET` | `/invoices/{record_id}/download` | `billingd` — XRechnung 3.0 CII XML (EN 16931) |
| `GET` | `/dokumente?kind=&limit=` | `outputd` — the document inbox: what was issued and sent |
| `GET` | `/dokumente/{document_id}` | `outputd` — the bytes as issued; opening it records the portal read receipt |
| `GET` | `/balance` · `/kontoauszug` · `/vorauszahlung` | `accountingd` |
| `GET` | `/eeg` | `einsd` |
| `GET` | `/versorgung` | `marktd` |
| `GET` | `/vertrag` · `/kuendigungsfrist` | `vertragd` |
| `POST` | `/tarifwechsel` · `/kuendigen` | `vertragd` |
| `GET` · `PUT` | `/kontakt` | `vertragd` (GDPR Art. 16) |
| `PUT` | `/sepa` | `accountingd` |

Notice periods and tariff rules live in `vertragd`: `portald` validates date
format only and relays `vertragd`'s `422`, which names the rule applied. Health
and `/metrics` come from the service runner.

## Configuration

```toml
# portald.toml
tenant = "9900357000004"

# Required — the authorization authority.
vertragd_url     = "http://vertragd:9780"
vertragd_api_key = "env:PORTALD_VERTRAGD_SERVICE_KEY"   # sent as X-Api-Key

edmd_url        = "http://edmd:8380"
billingd_url    = "http://billingd:9280"
accountingd_url = "http://accountingd:9380"
einsd_url       = "http://einsd:9180"
marktd_url      = "http://marktd:8180"
outputd_url     = "http://outputd:9880"   # the document inbox behind /dokumente
# …_api_key = "env:…"  — opaque service Bearer tokens

[http]
addr = "0.0.0.0:9480"
```

## No MCP server

`portald` serves no `/mcp`. An MCP tool takes a `malo_id` and no customer
token, so it could not pass the `vertragd` ownership check every route here
makes. Agents read the same data from the service that owns it, behind that
service's Cedar policy: `billingd` `list_billing_records`, `accountingd`
`get_balance` · `list_ledger` · `get_vorauszahlung`, `edmd` `get_timeseries`,
`einsd` `list_plants`, `marktd` `get_versorgungsstatus`.

An LF-role service (§§ 6a, 7a EnWG): it reads `marktd` only for the LF's own
VersorgungsStatus; `netzbilanzd` and `processd` (which holds the Sperrauftrag
queue) are not reachable through it.

The [operator guide](https://hupe1980.github.io/mako/docs/services/portald/)
has the full reference.

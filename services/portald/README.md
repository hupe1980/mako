# portald

`portald` is the LF's customer portal gateway: one customer-facing REST API
assembled on the request path from `vertragd`, `edmd`, `billingd`,
`accountingd`, `einsd`, `marktd` and `outputd`. It verifies no tokens itself —
every route forwards the customer's bearer token to `vertragd` and serves only
on its verdict. It holds no state, so replicas scale freely.

| | |
|---|---|
| **Port** | 9480 (`deploy/ports.toml`) |
| **Database** | none |
| **MCP** | none — a tool carries no customer token, so it could not pass the `vertragd` ownership check |
| **Emits** | none |
| **Consumes** | none |
| **Depends on** | vertragd (required, authorization), edmd, billingd, accountingd, einsd, marktd, outputd |

## Run

```bash
just dev    # the whole estate, portald on deploy/demo/portald.toml, seeded
PORTALD_CONFIG=deploy/demo/portald.toml cargo run -p portald -- --check-config
```

## API

Every path is prefixed `/api/v1/portal/{malo_id}`.

| Route | Upstream |
|---|---|
| `GET /dashboard` | marktd · accountingd · billingd · edmd, concurrently |
| `GET /lastgang?from=&to=` | edmd |
| `GET /invoices`, `GET /invoices/{record_id}/download` | billingd — the EN 16931 CII XML |
| `GET /dokumente`, `GET /dokumente/{document_id}` | outputd — the document inbox; opening one records the read receipt |
| `GET /balance` · `/kontoauszug` · `/vorauszahlung`, `PUT /sepa` | accountingd |
| `GET /eeg` | einsd |
| `GET /versorgung` | marktd |
| `GET /vertrag` · `/kuendigungsfrist`, `POST /tarifwechsel` · `/kuendigen`, `GET\|PUT /kontakt` | vertragd |

Notice periods and tariff rules live in `vertragd`; `portald` relays its `422`,
which names the rule applied.

## Configuration

`tenant`, `vertragd_url` / `vertragd_api_key` (required — startup refuses
without it unless `allow_insecure_no_auth = true`), and each upstream's `_url`
and `_api_key`. `lf_mp_id` (the MP-ID a self-service SEPA mandate is registered
under) defaults to `tenant`. No `[policy]`, no `[oidc]`.

The [operator guide](https://hupe1980.github.io/mako/docs/services/portald/)
covers authorization, every endpoint and its failures, the dashboard and
deployment.

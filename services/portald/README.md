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
| **SSE stream** | `GET /api/v1/portal/{malo_id}/events` — live event stream |
| **Self-service writes** | `POST /portal/{malo_id}/tarifwechsel`, `POST /portal/{malo_id}/kuendigen`, `PUT /portal/{malo_id}/kontakt` |
| **Health** | `GET /health/live`, `GET /health/ready` |

## Configuration

```toml
# portald.toml
port           = 9480
tenant         = "9900357000004"

vertragd_url   = "http://vertragd:9780"
edmd_url       = "http://edmd:8380"
billingd_url   = "http://billingd:9280"
accountingd_url = "http://accountingd:9380"
einsd_url      = "http://einsd:9180"
marktd_url     = "http://marktd:8180"
```

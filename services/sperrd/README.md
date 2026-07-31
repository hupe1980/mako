# sperrd — Sperrung Execution Tracking (NB role)

`sperrd` tracks physical Sperrung/Entsperrung execution and auto-dispatches IFTSTA 21039
on field-service confirmation. Without it, a missed IFTSTA 21039 leaves the Sperrung
permanently unresolved in the LF's system — a GPKE protocol violation.

| Feature | Detail |
|---|---|
| **HTTP port** | `:8780` |
| **Database** | PostgreSQL (sperr_orders) |
| **Auth** | OIDC/JWT + Cedar ABAC |
| **Status machine** | `pending` → `executed` / `failed` / `cancelled` |
| **IFTSTA 21039** | Auto-dispatched via MakodClient on `PUT /api/v1/sperr-orders/{id}/execute` |
| **Failure escalation** | `PUT /api/v1/sperr-orders/{id}/fail` → operator alert |
| **GPKE compliance** | BK6-22-024: IFTSTA 21039 within ORDERS execution window |
| **Health** | `GET /health/live`, `GET /health/ready` |

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1/sperr-orders` | List orders (filter by status) |
| `POST` | `/api/v1/sperr-orders` | Create a Sperrung/Entsperrung order |
| `GET` | `/api/v1/sperr-orders/stats` | Aggregate compliance-sweep counters |
| `GET` | `/api/v1/sperr-orders/{id}` | Fetch a single order |
| `PUT` | `/api/v1/sperr-orders/{id}/execute` | Field-service confirmation → auto-dispatch IFTSTA 21039 |
| `PUT` | `/api/v1/sperr-orders/{id}/fail` | Mark execution failed → operator alert |
| `PUT` | `/api/v1/sperr-orders/{id}/cancel` | Cancel a `pending` order (no IFTSTA dispatched) |
| `GET\|POST` | `/mcp` | MCP Streamable HTTP |

## MCP tools

| Tool | Description |
|------|-------------|
| `list_sperr_orders` | List orders, filter by status or `older_than_hours` |
| `get_sperr_order` | Fetch a single order by UUID |
| `get_sperr_stats` | Compliance counters incl. `executed_missing_iftsta` |
| `list_overdue_orders` | Pending orders past `planned_date` |
| `cancel_sperr_order` | Cancel a pending order |

Prompts: `execute-sperrung`, `compliance-sweep`.

## Configuration

```toml
# sperrd.toml
port           = 8780
tenant         = "9900357000004"

makod_url      = "http://makod:8080"
makod_api_key  = "env:SPERRD_MAKOD_API_KEY"

[database]
url       = "env:SPERRD_DATABASE_URL"   # postgresql://sperrd:secret@db:5432/sperrd
pool_size = 10
```

The service runs on the `mako-service` daemon runner (`mako_service::run::<Sperrd>()`), which
owns tracing, the tuned connection pool (`application_name = "sperrd"`), migrations, graceful
shutdown, and a real `/health/ready` (bounded `SELECT 1`). Start it with `sperrd`
(config path via `SPERRD_CONFIG`); `sperrd --check` is the container HEALTHCHECK probe.

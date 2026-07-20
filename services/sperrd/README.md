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

## Configuration

```toml
# sperrd.toml
database_url   = "postgresql://sperrd:secret@db:5432/sperrd"
port           = 8780
tenant         = "9900357000004"

makod_url      = "http://makod:8080"
makod_api_key  = "env:SPERRD_MAKOD_API_KEY"
```

# netzbilanzd — NNE/KA/MMM Billing Daemon (NB role)

`netzbilanzd` generates, self-validates, and dispatches INVOIC 31001/31002/31005 invoices
from the NB to the LF. A `ValidationFailed` INVOIC is **never sent** — `invoic-checker` is
the same library the LF uses.

| Feature | Detail |
|---|---|
| **HTTP port** | `:8680` |
| **Database** | PostgreSQL (invoice_drafts) |
| **Auth** | OIDC/JWT + Cedar ABAC |
| **INVOIC PIDs** | 31001 (Abschlagsrechnung), 31002 (Netznutzungsabrechnung), 31005 (MMM Mehr-/Mindermengensaldo) |
| **Calculation** | `mako_nne::calculate_nne_invoice()` — `billing::EuroAmount` (`i64 × 10⁻⁵`), no `f64` |
| **Draft lifecycle** | `status`: draft → dispatched / rejected; `PUT /api/v1/billing/drafts/{id}/dispatch` |
| **Pre-dispatch validation** | `invoic-checker` mandatory gate; `ValidationFailed` blocks dispatch |
| **MMM prices** | Auto-fetches `mehr_preis`/`minder_preis` from `marktd` MMMA store when not in request body |
| **REMADV handling** | 33001/33003/33004 → `Paid`; 33002 → `Disputed` + operator alert |
| **Retention** | §22 MessZV 3-year retention in `billing_records` |
| **Health** | `GET /health/live`, `GET /health/ready` |

## Configuration

```toml
# netzbilanzd.toml
database_url = "postgresql://netzbilanzd:secret@db:5432/netzbilanzd"
port         = 8680
tenant       = "9900357000004"
nb_mp_id     = "9900357000004"

makod_url    = "http://makod:8080"
marktd_url   = "http://marktd:8180"

[erp]
webhook_url = "http://erp:8000/events"
```

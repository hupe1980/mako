# sperrd

`sperrd` (`:8780`) is the Netzbetreiber's execution queue for Sperr- and
Entsperraufträge. An ORDERS **17115** Sperrauftrag or **17117** Entsperrauftrag
from a Lieferant becomes a work order for the field team; the outcome goes back
through `makod` as IFTSTA **21039**, which is what lets the LF's
`gpke-sperrung-lf` process reach a terminal state.

## Run it

```bash
just dev sperrd        # dev/config/sperrd.toml against the dev PostgreSQL
```

`sperrd --check` is the container `HEALTHCHECK`; the config file comes from
`SPERRD_CONFIG`.

## At a glance

| Feature | Detail |
|---|---|
| **Database** | PostgreSQL (`sperr_orders`) |
| **Auth** | OIDC + Cedar on every REST route; HMAC (`inbound_hmac_secret`) on the market ingest |
| **Status machine** | `pending` → `executed` / `failed` / `cancelled`; two Sperrversuche per Sperrauftrag — the first `fail` keeps the order queued, `endgueltig: true` closes it |
| **IFTSTA 21039** | sent on a terminal outcome; a leased retry queue with backoff, `de.sperr.iftsta.ausstehend` once the budget is spent. `execute`/`fail` answer `204` when `makod` took it, `202` when only recorded |
| **Events** | `de.sperr.*` through the transactional outbox |
| **MCP** | 4 read-only tools and 2 prompts at `/mcp` |

## What the queue carries

| Column | EDIFACT | Meaning |
|---|---|---|
| `order_type` | `BGM+Z51` / `BGM+Z52` | Sperrung / Entsperrung |
| `ausfuehrung_am` | `DTM+203` | A **fixed** date the LF requires (a Gerichtsvollzieher may have set it) |
| `fruehestens_am` | `DTM+469` | Execute at the next opportunity, but **not before** this date |
| `arbeitszeit` | `IMD+7081` `Z53`/`Z54` | Entsperrauftrag: within working hours, or also outside |
| `treffpunkt_*` | `SG2 NAD+Z24` | Where the technician goes — street, PLZ, Ort, or a Zusatzinformation |
| `hinweis` | `SG29 FTX+ACB` | The LF's free-text hints |
| `pruefschritt_code` | `SG15 STS DE9013` | The EBD Prüfschritt code the IFTSTA reports (**Muss**) |
| `executed_at` | `DTM+293` | Fertigstellungsdatum — **Muss** on `Z14`, and must be ≤ the message date |

`ausfuehrung_am` and `fruehestens_am` are mutually exclusive (AHB conditions
[55]/[56]), in the API and in a `CHECK`.

## Three published clocks

BK6-24-174 GPKE Teil 2 §§ 3.5.1.2 / 3.5.2.2:

| Clock | Frist | Tracked by |
|---|---|---|
| ORDRSP 19116 / 19117 answering the order | spätester ÜT ist der 1. WT nach dem ÜT | `makod` |
| The **physical act** | 6 WT nach dem frühestmöglichen Sperrtermin | `ausfuehrung_faellig_am` |
| IFTSTA 21039 | 1. WT nach dem Abschluss des Sperrauftrags | `iftsta_faellig_am` |

`/stats` reports `overdue_pending` (the LF's requested date) apart from
`frist_ueberschritten` (the regulatory window).

## Routes

| Method | Path | Cedar action |
|--------|------|---|
| `POST` | `/webhook` | *(HMAC, not OIDC)* |
| `GET` | `/api/v1/sperr-orders` | `read-sperr-order` |
| `POST` | `/api/v1/sperr-orders` | `create-sperr-order` |
| `GET` | `/api/v1/sperr-orders/stats` | `read-sperr-order` |
| `GET` | `/api/v1/sperr-orders/{id}` | `read-sperr-order` |
| `PUT` | `/api/v1/sperr-orders/{id}/execute` | `execute-sperr-order` |
| `PUT` | `/api/v1/sperr-orders/{id}/fail` | `execute-sperr-order` |
| `PUT` | `/api/v1/sperr-orders/{id}/cancel` | `cancel-sperr-order` |
| `GET\|POST` | `/mcp` | *(McpAuth; read-only)* |

`?status=` refuses an unknown value with `400`; `?due=true` is the field-dispatch
list.

## Configuration

```toml
# sperrd.toml
port           = 8780
tenant         = "9900357000004"

makod_url      = "http://makod:8080"
makod_api_key  = "env:SPERRD_MAKOD_API_KEY"

# The only authentication on POST /webhook; required unless allow_insecure_no_auth.
inbound_hmac_secret = "env:SPERRD_INBOUND_HMAC_SECRET"

# Drains the de.sperr.* events from the transactional outbox.
erp_webhook_url = "http://erp.internal/hooks/mako"
erp_hmac_secret = "env:SPERRD_ERP_HMAC_SECRET"

[database]
url       = "env:SPERRD_DATABASE_URL"
pool_size = 10

[oidc]
issuer   = "https://keycloak:8080/realms/mako"
audience = "sperrd"
```

DB suite: `just test-sperrd-db`.

The [operator guide](https://hupe1980.github.io/mako/docs/services/sperrd/)
has the full reference.

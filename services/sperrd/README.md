# sperrd — Sperr-/Entsperrauftrag execution queue (NB role)

The Netzbetreiber's work queue for the physical acts GPKE orders it to perform.
An **ORDERS 17115 Sperrauftrag** or **17117 Entsperrauftrag** from a Lieferant
becomes a job for the field team; the outcome goes back as **IFTSTA 21039**
(Auftragsstatus Sperren/Entsperren). Without that message the LF's
`gpke-sperrung-lf` process never reaches a terminal state, and GPKE gives them no
way to find out what happened but to ask.

| Feature | Detail |
|---|---|
| **HTTP port** | `:8780` |
| **Database** | PostgreSQL (`sperr_orders`) |
| **Auth** | OIDC/JWT **+ Cedar ABAC** on every REST route; HMAC on the market ingest |
| **Market ingest** | `POST /webhook` — `de.mako.process.initiated` for PIDs 17115/17117 becomes a work order, deduplicated per market process |
| **Status machine** | `pending` → `executed` / `failed` / `cancelled` |
| **IFTSTA 21039** | Dispatched via `makod` on a terminal outcome; **retried** by a background worker and escalated once the budget is spent |
| **Events** | `de.sperr.{auftrag.eingegangen,ausgefuehrt,fehlgeschlagen,storniert,iftsta.ausstehend}` via the transactional outbox |
| **Health** | `GET /health/live`, `GET /health/ready` |

## What the queue carries

The row is shaped by what the ORDERS AHB actually sends, so the field team has
what it needs:

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

`ausfuehrung_am` and `fruehestens_am` are mutually exclusive, in the API and in a
database `CHECK` — the AHB's conditions [55]/[56] make them alternatives.

## Timing

GPKE fixes **no execution deadline in Werktagen** for the physical act. The only
date is the Lieferant's own, from `DTM+203` or `DTM+469`. BK6-22-024 §5's
**24 wall-clock hours** is the deadline for the NB's ORDRSP, which `makod`
tracks — not this service. A guard test keeps invented Werktage windows from
creeping back in.

## Endpoints

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

`?status=` accepts `pending`/`executed`/`failed`/`cancelled` — an unknown value is
a 400, not a filter that silently matches nothing. `?due=true` is the
field-dispatch list.

`execute` and `fail` answer **204** when the IFTSTA reached `makod` and **202**
when it did not: the outcome is recorded either way — the field team's report is
not discarded because a downstream service was unreachable — but only the first
means the Lieferant has been told.

## The IFTSTA retry queue

A terminal order whose `iftsta_dispatched_at` is NULL is an order whose Lieferant
does not know the outcome, so it is a queue rather than a one-shot. A worker
re-sends under the same idempotency key `makod` deduplicates on, up to
`IFTSTA_MAX_ATTEMPTS`, then announces `de.sperr.iftsta.ausstehend` once and
leaves the order alone — a dispatch that fails eight times is not a transport
problem but a `makod` process in the wrong state, and retrying forever only hides
it. `/stats` reports `iftsta_outstanding` (in flight) apart from `iftsta_stuck`
(needs someone).

## MCP tools

Read-only by construction; mutating actions stay on the authenticated REST
routes.

| Tool | Description |
|------|-------------|
| `list_sperr_orders` | The queue — filter by status, MaLo, or `due` |
| `get_sperr_order` | One order, with its ORDERS provenance and IFTSTA state |
| `get_sperr_stats` | Counters, including `iftsta_outstanding` / `iftsta_stuck` |
| `list_due_orders` | The field-dispatch list, with the Treffpunkt |

Prompts: `execute-sperrung`, `iftsta-sweep`.

## Configuration

```toml
# sperrd.toml
port           = 8780
tenant         = "9900357000004"

makod_url      = "http://makod:8080"
makod_api_key  = "env:SPERRD_MAKOD_API_KEY"

# Verifies X-Mako-Signature on the market ingest. Absent → unsigned events are
# accepted with a startup warning; the webhook queues physical disconnections,
# so that is a development setting.
inbound_hmac_secret = "env:SPERRD_INBOUND_HMAC_SECRET"

[database]
url       = "env:SPERRD_DATABASE_URL"
pool_size = 10

[oidc]
issuer   = "https://keycloak:8080/realms/mako"
audience = "sperrd"
```

The service runs on the `mako-service` daemon runner (`mako_service::run::<Sperrd>()`),
which owns tracing, the tuned connection pool (`application_name = "sperrd"`),
migrations, graceful shutdown, and a real `/health/ready`. Start it with `sperrd`
(config path via `SPERRD_CONFIG`); `sperrd --check` is the container HEALTHCHECK.

## Tests

`cargo test -p sperrd` runs the unit and guard tests; `just test-sperrd-db` runs
10 scenarios against real PostgreSQL — redelivery, the claim guard, the retry
queue, tenant isolation, the mutually-exclusive ORDERS dates.

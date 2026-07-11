# invoicd

**INVOIC plausibility-check daemon for the Lieferant (LF) role.**

`invoicd` is an autonomous microservice that receives incoming INVOIC billing
events from [`makod`](../makod/README.md) via the [`marktd`](../marktd/README.md)
event bus, runs a five-check plausibility pipeline against price sheets fetched
from `marktd`, and either accepts or disputes each invoice by issuing a command
back to `makod` — which then emits the corresponding REMADV or COMDIS to the
counterparty.

Every checked invoice is persisted to PostgreSQL **before** the command is
dispatched, in a single transaction that also records the payment deadline
(`pay_by`) from the invoice's `DTM+faelligkeitsdatum` field. This satisfies the
**3-year retention requirement under §22 MessZV and §41 EnWG** and enables
automated REMADV deadline tracking.

---

## What it does

```
marktd ──(POST /webhook)──► invoicd
                              │
                   de.mako.process.initiated
                   makopid in {31001, 31002, 31005, 31006}
                              │
               ┌──────────────▼───────────────┐
               │ fetch PreisblattNetznutzung   │◄── GET marktd :8180
               │   1-hour cache · CB(3/30s)   │    /api/v1/preisblaetter/{nb_mp_id}
               └──────────────┬───────────────┘
                              │
               ┌──────────────▼───────────────┐
               │  invoic-checker               │
               │  ① period validity            │
               │  ② position arithmetic (1%)   │
               │  ③ document total (1%)        │
               │  ④ tariff match (SLP, 3%)     │
               │  ⑤ tariff found               │
               └──────────────┬───────────────┘
                              │
               ┌──────────────▼───────────────┐
               │  PostgreSQL — invoic_receipts │  ← atomic write:
               │  outcome · findings · pay_by  │    receipt + pay_by in one TX
               │  sender_mp_id · erp_attempts  │    before dispatching command
               └──────────────┬───────────────┘
                              │
           ┌──────────────────┴──────────────────┐
           ▼                                     ▼
     Ok / Warn (accepted)              Dispute findings present
           │                                     │
  POST /api/v1/commands            POST /api/v1/commands
  gpke.abrechnung.annehmen         gpke.abrechnung.ablehnen
  → makod → REMADV 33001           → makod → COMDIS 29001
```

### Supported INVOIC PIDs

| PID   | Process name                              | Direction | Status |
|-------|-------------------------------------------|-----------|--------|
| 31001 | Abschlagsrechnung (Netznutzung)           | NB → LF   | ✅     |
| 31002 | NN-Rechnung (Netznutzungsabrechnung)      | NB → LF   | ✅     |
| 31005 | MMM-Rechnung (Mehr-/Mindermengensaldo)    | NB → LF   | ✅     |
| 31006 | MMM-Rechnung (selbst ausgestellt)         | LF → NB   | Schema ✅ · API M16 |
| 31009 | MSB-Rechnung                              | MSB → LF  | ⏳ M16 gap |

> **31009 (M16 gap).** MSB invoices do not embed the `Rechnung` in the
> `process.initiated` payload — add `GET /api/v1/invoic/{id}/rechnung` to `makod`
> and a `Wim31009Ingestor` in `invoicd` triggering on `makoworkflow == "wim-rechnung"`.
>
> **PIDs 31003, 31004, 31007, 31008, 31010, 31011** are Gas or GaBi domain billing
> and are handled by their own workflows. They do not trigger `invoicd`.

---

## Persistence schema

`invoicd` runs SQLx migrations at startup (`migrations/0001_initial.sql`). The schema:

```sql
CREATE TABLE invoic_receipts (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    process_id    UUID        NOT NULL UNIQUE,
    pid           SMALLINT    NOT NULL,           -- 31001 | 31002 | 31005 | 31006 | 31009
    direction     TEXT        NOT NULL,           -- 'Inbound' | 'Outbound'
    sender_mp_id  TEXT        NOT NULL,           -- NB/MSB MP-ID (Inbound) or tenant MP-ID (Outbound)
    receiver_gln  TEXT,                           -- tenant MP-ID (Inbound) or NB MP-ID (Outbound)
    rechnung      JSONB       NOT NULL,           -- rubo4e::v202607::Rechnung
    bo4e_version  TEXT        NOT NULL DEFAULT 'v202607.0.0',
    outcome       TEXT        NOT NULL CHECK (outcome IN (
                                  'Ok',              -- accepted; REMADV 33001
                                  'AcceptedPartial', -- accepted with remarks; REMADV 33003/33004
                                  'Warn',            -- tolerance warning; auto-approved
                                  'Dispute',         -- disputed; COMDIS 29001
                                  'Dispatched',      -- outbound 31006 sent, awaiting REMADV
                                  'Paid'             -- outbound 31006 settled
                              )),
    findings      JSONB       NOT NULL DEFAULT '[]',
    pay_by        TIMESTAMPTZ,                    -- Zahlungsziel from Rechnung.faelligkeitsdatum

    received_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    checked_at           TIMESTAMPTZ,
    dispatched_at        TIMESTAMPTZ,

    -- ERP notification tracking — durable at-least-once delivery
    -- erp_notified_at: set when ERP webhook returns 2xx; NULL = pending or failed
    -- erp_attempts: total delivery attempts (inline + outbox worker retries)
    -- erp_next_attempt_at: backoff schedule for background retries
    erp_notified_at      TIMESTAMPTZ,
    erp_attempts         SMALLINT    NOT NULL DEFAULT 0,
    erp_next_attempt_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    tenant        TEXT        NOT NULL DEFAULT 'default'
);
```

**Atomicity guarantee.** `direction`, `outcome`, `findings`, `pay_by`, and
`rechnung` are written in a single PostgreSQL transaction before any command is
dispatched to `makod`. A crash between the two would violate §22 MessZV
retention — so persistence always comes first.

**ERP notification.** After REMADV dispatch, `invoicd` POSTs a `de.invoic.receipt.*`
CloudEvent to the configured ERP webhook.  Delivery is **durable at-least-once**:
the initial attempt runs inline; failures are retried by the background outbox worker
with exponential backoff (30 s → 5 min → 30 min → 2 h → dead-letter at attempt 5).
HTTP 4xx = permanent failure (dead-lettered immediately); 5xx/transport = retried.
Signed with `X-Mako-Signature: sha256=<hex>` when `[erp] hmac_secret` is configured.
Dead-lettered receipts: `SELECT * FROM invoic_receipts WHERE erp_notified_at IS NULL AND erp_attempts >= 5`.

**REMADV deadline tracking.** Alert query (run every 6 h):

```sql
SELECT process_id, pid, sender_mp_id, pay_by
FROM invoic_receipts
WHERE outcome IN ('Ok', 'AcceptedPartial', 'Warn')
  AND pay_by < now() + interval '3 days'
  AND dispatched_at IS NULL;
```

**Dead-letter queue.** Events that fail HMAC verification or deserialization
are written to `invoic_dlq`. An operator alert fires when entries are older
than 1 hour. Events are never silently dropped.

---

## Endpoints

### `POST /webhook` — inbound from marktd

Receives CloudEvents 1.0 JSON from `marktd`. Signature verified via
`X-Mako-Signature: sha256=<hex>` (HMAC-SHA256 over the raw body) when
`--inbound-secret` is set. Rejected signatures return `401 Unauthorized`
before the event body is deserialized.

The daemon auto-subscribes to `de.mako.process.initiated` at startup via
`PUT /api/v1/subscriptions/invoicd` on `marktd`. No manual setup required.

### `GET /api/v1/receipts` — list receipts

Query receipts for the caller's tenant. Supports filtering:
`?direction=Inbound|Outbound`, `?outcome=Dispute`, `?pid=31001`,
`?from=2026-01-01`, `?to=2026-12-31`.

### `GET /api/v1/receipts/{id}` — fetch receipt

Returns the full receipt including `findings` JSONB and `pay_by`.

### `POST /api/v1/receipts/{id}/confirm-payment` — ERP payment confirmation

Called by the ERP when a bank transfer is confirmed. Sets `payment_confirmed_at`
on the receipt row, closing the §22 MessZV payment audit trail.

```bash
curl -X POST http://invoicd:8280/api/v1/receipts/<uuid>/confirm-payment \
  -H "Authorization: Bearer <token>"
# → 204 No Content
```

### `GET /api/v1/zahlungsstatus/{malo_id}` — payment status per MaLo

Returns `overdue_count`, `pending_count`, `settled_count` and a list of receipts
with `zahlungsstatus` values: `settled` / `pending` / `overdue` / `undispatched`.
Use this for accounts-payable dashboards and dunning workflows.

### `GET /api/v1/disputes` — list open disputes

Returns all receipts with `outcome = 'Dispute'` for the caller's tenant.
Shorthand for `GET /api/v1/receipts?outcome=Dispute`.

### `GET /health/live` / `GET /health/ready`

Standard Kubernetes probes. `/health/ready` checks PostgreSQL connectivity.

### `POST|GET /mcp` — MCP Streamable HTTP

MCP server for LLM tooling. Requires `Authorization: Bearer <token>` (same
OIDC+Cedar layer as REST endpoints).

**MCP tools:**

| Tool | Description |
|---|---|
| `get_receipt` | Fetch a receipt by process ID |
| `list_disputes` | List all receipts with outcome = 'Dispute' |
| `get_check_result` | Return the full plausibility report for an INVOIC |
| `get_zahlungsstatus` | Payment status per MaLo (settled / pending / overdue) |

---

## Tariff data (price sheets)

`invoicd` does **not** manage its own tariff store. Price sheets
(`PreisblattNetznutzung`) are fetched from `marktd` at check time:

```
GET marktd :8180 /api/v1/preisblaetter/{nb_mp_id}?date={billing_date}
```

**1-hour TTL cache** keyed by `(nb_mp_id, billing_date)` avoids redundant calls
for high-volume billing periods.

**Circuit breaker** (3 consecutive failures → open for 30 s):
- `CB_FAILURE_THRESHOLD = 3` (in `src/preisblatt_client.rs`)
- `CB_COOLDOWN_SECS = 30`

While open, `invoicd` returns `None` for the price sheet and falls back to
structural checks only (period, arithmetic, total). It **never** dispatches a
REMADV without having confirmed the price sheet — open circuit → the invoice
is held in the queue until `marktd` recovers.

To upload a price sheet to `marktd`:

```bash
curl -X PUT http://marktd:8180/api/v1/preisblaetter/9904234560001 \
  -H "Authorization: Bearer ${TOKEN}" \
  -H "Content-Type: application/json" \
  -d @preisblatt.json   # rubo4e::v202607::PreisblattNetznutzung
```

---

## Service topology

```
                 ┌─────────────────────────────────────┐
                 │            makod :8080               │
                 │  EDIFACT → GpkeAbrechnungWorkflow    │
                 │  outbox: de.mako.process.initiated   │
                 └──────────────┬──────────────────────┘
                                │ CloudEvents (HMAC-signed)
                 ┌──────────────▼──────────────────────┐
                 │            marktd :8180                │
                 │  fan-out to registered subscribers   │
                 │  preisblaetter (price sheets)        │
                 └──────┬───────────────────────────────┘
                        │ POST /webhook (CloudEvents)
         ┌──────────────▼──────────────────────┐
         │           invoicd :8280              │
         │  ① fetch PreisblattNetznutzung       │◄── GET marktd :8180
         │  ② invoic-checker (5 checks)         │
         │  ③ persist receipt + pay_by (atomic) │
         │  ④ dispatch annehmen / ablehnen      │
         └──────────────┬──────────────────────┘
                        │ POST /api/v1/commands
                 ┌──────▼──────────────────────────────┐
                 │            makod :8080               │
                 │  gpke.abrechnung.annehmen →REMADV   │
                 │  gpke.abrechnung.ablehnen →COMDIS   │
                 └─────────────────────────────────────┘
```

`invoicd` is stateless between requests — all business state lives in `makod`'s
event store. `invoicd` only persists what it has personally checked (the
`invoic_receipts` table).

---

## Configuration

All settings can be provided as CLI flags or environment variables.

| Flag                          | Env var                           | Default                              |
|-------------------------------|-----------------------------------|--------------------------------------|
| `--listen`                    | `INVOICD_LISTEN`                  | `0.0.0.0:8280`                       |
| `--makod-url`                 | `INVOICD_MAKOD_URL`               | `http://localhost:8080`              |
| `--marktd-url`                  | `INVOICD_MARKTD_URL`                | `http://localhost:8180`              |
| `--subscriber-id`             | `INVOICD_SUBSCRIBER_ID`           | `invoicd`                            |
| `--webhook-url`               | `INVOICD_WEBHOOK_URL`             | *(required)*                         |
| `--webhook-secret`            | `INVOICD_WEBHOOK_SECRET`          | *(optional)*                         |
| `--inbound-secret`            | `INVOICD_INBOUND_SECRET`          | *(optional)*                         |
| `--database-url`              | `DATABASE_URL`                    | *(optional — disables DB if absent)* |
| `--db-max-connections`        | —                                 | `5`                                  |
| `--tenant`                    | `INVOICD_TENANT`                  | `default`                            |
| `--arithmetic-tolerance`      | `INVOICD_ARITHMETIC_TOLERANCE`    | `0.01`                               |
| `--total-tolerance`           | `INVOICD_TOTAL_TOLERANCE`         | `0.01`                               |
| `--tariff-tolerance`          | `INVOICD_TARIFF_TOLERANCE`        | `0.03`                               |
| `--require-tariff`            | `INVOICD_REQUIRE_TARIFF`          | `false`                              |
| `--auto-dispute-threshold`    | `INVOICD_AUTO_DISPUTE_THRESHOLD`  | `0.0` (dispute on any finding)       |

`--auto-dispute-threshold` (euros): when all dispute findings are below this
amount `invoicd` accepts anyway — useful for rounding tolerances that do not
warrant a formal dispute.

When `--database-url` is omitted, migrations are skipped and no receipt is
persisted. The plausibility check still runs but §22 MessZV compliance is not
met — only acceptable in CI / development.

---

## Quick start (Docker Compose)

```yaml
invoicd:
  image: ghcr.io/hupe1980/invoicd:latest
  environment:
    INVOICD_MAKOD_URL:       http://makod:8080
    INVOICD_MARKTD_URL:        http://marktd:8180
    INVOICD_WEBHOOK_URL:     http://invoicd:8280/webhook
    INVOICD_INBOUND_SECRET:  "${MARKTD_OUTBOUND_SECRET}"
    INVOICD_REQUIRE_TARIFF:  "true"
    DATABASE_URL:            postgres://invoicd:secret@postgres/invoicd
    INVOICD_TENANT:          "${TENANT}"
  ports:
    - "8280:8280"
  depends_on: [postgres, marktd]
```

---

## Regulatory basis

- **§22 MessZV / §41 EnWG** — 3-year billing receipt retention (PostgreSQL persistence)
- **BK6-24-174** — GPKE Teil 1–3 (Lieferantenwechsel, Netznutzungsabrechnung)
- INVOIC AHB for PIDs 31001, 31002, 31005, 31006
- REMADV AHB (outbound via `makod` after `gpke.abrechnung.annehmen`)
- COMDIS AHB (outbound via `makod` after `gpke.abrechnung.ablehnen`)

## See Also

- [`marktd` README](../marktd/README.md) — price sheets, subscriptions, partner registry
- [`makod` README](../makod/README.md) — EDIFACT workflows
- [`edmd` README](../edmd/README.md) — meter data (prerequisite for M16 RLM billing)
- [`invoic-checker`](../../crates/invoic-checker/) — pure plausibility library

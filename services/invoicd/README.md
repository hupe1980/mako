# invoicd

**INVOIC plausibility-check daemon for the Lieferant (LF) role.**

`invoicd` is an autonomous microservice that listens for incoming INVOIC
billing notifications from [`makod`](../makod/README.md) and automatically
runs a plausibility check. Based on the result, it either settles or disputes
each invoice by issuing a command back to `makod`, which then emits the
corresponding REMADV or COMDIS to the counterparty.

---

## What it does

```
mdmd ──(POST /webhook)──► invoicd
                              │
                   de.mako.process.initiated
                   + PID in {31001, 31002, 31005, 31006}
                              │
                   ┌──────────▼──────────┐
                   │  invoic-checker      │
                   │  - period validity   │
                   │  - position arith.   │
                   │  - document total    │
                   │  - tariff match      │
                   │  - tariff found      │
                   └──────────┬──────────┘
                              │
              ┌───────────────┴──────────────┐
              ▼                              ▼
         no findings              dispute findings present
         (or below threshold)           │
              │                   POST /api/v1/commands
    POST /api/v1/commands         gpke.abrechnung.ablehnen
    gpke.abrechnung.annehmen
```

`invoicd` handles the four GPKE grid-usage billing PIDs:

| PID   | Process name                              |
|-------|-------------------------------------------|
| 31001 | Abschlagsrechnung (Netznutzung)           |
| 31002 | NN-Rechnung (Netznutzungsabrechnung)      |
| 31005 | MMM-Rechnung (Mehr-/Mindermengensaldo)    |
| 31006 | MMM-Rechnung (selbst ausgestellt)         |

> Other INVOIC PIDs (31003, 31004, 31009–31011) are handled by their own
> domain workflows and do not trigger `invoicd`.

---

## Configuration

All settings can be provided as CLI flags or environment variables.

| Flag                          | Env var                           | Default                         |
|-------------------------------|-----------------------------------|---------------------------------|
| `--listen`                    | `INVOICD_LISTEN`                  | `0.0.0.0:8280`                  |
| `--makod-url`                 | `INVOICD_MAKOD_URL`               | `http://localhost:8080`         |
| `--mdmd-url`                  | `INVOICD_MDMD_URL`                | `http://localhost:8180`         |
| `--subscriber-id`             | `INVOICD_SUBSCRIBER_ID`           | `invoicd`                       |
| `--webhook-url`               | `INVOICD_WEBHOOK_URL`             | *(required)*                    |
| `--webhook-secret`            | `INVOICD_WEBHOOK_SECRET`          | *(optional)*                    |
| `--inbound-secret`            | `INVOICD_INBOUND_SECRET`          | *(optional)*                    |
| `--arithmetic-tolerance`      | `INVOICD_ARITHMETIC_TOLERANCE`    | `0.01`                          |
| `--total-tolerance`           | `INVOICD_TOTAL_TOLERANCE`         | `0.01`                          |
| `--tariff-tolerance`          | `INVOICD_TARIFF_TOLERANCE`        | `0.03`                          |
| `--require-tariff`            | `INVOICD_REQUIRE_TARIFF`          | `false`                         |
| `--auto-dispute-threshold`    | `INVOICD_AUTO_DISPUTE_THRESHOLD`  | `0.0` (dispute on any finding)  |

The `--auto-dispute-threshold` sets a minimum deviation in euros. When all
dispute findings are below this amount, `invoicd` settles the invoice even if
minor discrepancies were found — useful for rounding differences that do not
warrant a full dispute.

---

## Endpoints

### `POST /webhook` — mdmd inbound

Receives CloudEvents 1.0 JSON from `mdmd`. Signature verification is
performed when `--inbound-secret` is set (`X-Mako-Signature` header,
HMAC-SHA256 over the raw body).

The daemon subscribes to `de.mako.process.initiated` events at startup
via `PUT /api/v1/subscriptions/invoicd` on `mdmd`. No manual subscription
setup is required.

### `PUT /admin/tariff` — tariff seeding (NDJSON)

Seed the in-memory tariff store used by the plausibility check:

```
PUT /admin/tariff
Content-Type: application/json

{"malo_id": "51238696782", "artikel_id": "NT-4x20", "preis_eur_ct_per_kwh": 4.85, "valid_from": "2025-10-01", "valid_to": "2026-09-30"}
```

Returns `204 No Content` on success. The tariff store is ephemeral — entries
are lost on restart. For a persistent tariff store, implement the
`TariffStore` trait from the `invoic-checker` crate against your database.

---

## Integration with makod and mdmd

```
                 ┌─────────────────────────────────────┐
                 │            makod :8080               │
                 │  EDIFACT → GpkeAbrechnungWorkflow    │
                 │  outbox: de.mako.process.initiated   │
                 └──────────────┬──────────────────────┘
                                │ POST /api/v1/events (CloudEvents)
                 ┌──────────────▼──────────────────────┐
                 │            mdmd :8180                │
                 │  fan-out to registered subscribers   │
                 └──────────────┬──────────────────────┘
                                │ POST /webhook (CloudEvents)
                 ┌──────────────▼──────────────────────┐
                 │          invoicd :8280               │
                 │  invoic-checker plausibility run     │
                 └──────────────┬──────────────────────┘
                                │ POST /api/v1/commands
                 ┌──────────────▼──────────────────────┐
                 │            makod :8080               │
                 │  gpke.abrechnung.annehmen /          │
                 │  gpke.abrechnung.ablehnen            │
                 │  → emits REMADV / COMDIS via AS4     │
                 └─────────────────────────────────────┘
```

`invoicd` is stateless — it holds no durable data of its own. All
process state lives in `makod`'s event store. The tariff cache is the
only in-memory state; it can be re-seeded at any time via `PUT /admin/tariff`.

---

## Quick start (Docker Compose)

```yaml
invoicd:
  image: ghcr.io/hupe1980/invoicd:latest
  environment:
    INVOICD_MAKOD_URL: http://makod:8080
    INVOICD_MDMD_URL: http://mdmd:8180
    INVOICD_WEBHOOK_URL: http://invoicd:8280/webhook
    INVOICD_INBOUND_SECRET: "${MDMD_OUTBOUND_SECRET}"
    INVOICD_REQUIRE_TARIFF: "true"
  ports:
    - "8280:8280"
```

---

## Regulatory basis

- **BK6-24-174** — GPKE Teil 1–3 (Lieferantenwechsel, Netznutzungsabrechnung)
- INVOIC AHB for PIDs 31001, 31002, 31005, 31006
- REMADV AHB (outbound via `makod` after `gpke.abrechnung.annehmen`)
- COMDIS AHB (outbound via `makod` after `gpke.abrechnung.ablehnen`)

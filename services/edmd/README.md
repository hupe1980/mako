# edmd

`edmd` is mako's energy data management daemon for the Messstellenbetreiber,
Netzbetreiber and Lieferant. It stores meter readings — MSCONS delivered by
the `marktd` webhook, direct iMSys/SMGW push, IoT, bulk and Kafka intake —
validates and scores them, schedules reading orders (Ablesesteuerung) and
serves register-projected series to billing, balancing and the API-Webdienste.
Readings live in [`meterstore`](https://github.com/hupe1980/meterstore): a
recent window in PostgreSQL, settled history in Apache Iceberg V2.

| | |
|---|---|
| **Port** | 8380 (`deploy/ports.toml`) |
| **Database** | PostgreSQL — `migrations/0001_schema.sql` plus the meterstore tables created at startup; Iceberg catalog for the cold tier |
| **MCP** | 16 tools, 5 prompts at `/mcp` |
| **Emits** | `de.messwert.reading.*`, `de.messwert.esa.typ2.delivery.*`, `de.messwert.cls.*`, `de.messwert.smgw.cert.expiry-warning` |
| **Consumes** | `de.mako.process.completed` (MSCONS, GPKE Lieferbeginn/-ende), `de.markt.malo.updated` at `POST /webhook` |
| **Depends on** | `marktd` (subscription, Bundesland, Lokationszuordnung) |

## Run

```bash
just dev    # the whole estate, edmd on deploy/demo/edmd.toml, seeded
EDMD_CONFIG=deploy/demo/edmd.toml cargo run -p edmd -- --check-config
```

## API

| Route | Purpose |
|---|---|
| `POST /webhook` | inbound CloudEvents from `marktd` |
| `POST /api/v1/meter-reads/{rlm,gas,iot}/{malo_id}`, `/{malo_id}/bulk`, `/{malo_id}/substitute` | direct ingest and Ersatzwerte |
| `GET /api/v1/energy/{malo_id}` | the canonical register-projected series |
| `GET /api/v1/lastgang/{malo_id}`, `/zeitreihe/{malo_id}`, `/billing-period/{malo_id}` | raw and billing reads |
| `/api/v1/zaehlerstandsgang/{malo_id}` | Zählerstandsgang |
| `/api/v1/reading-orders` | Ablesesteuerung |
| `/api/v1/archive/*`, `POST /api/v1/query/sql`, `/api/v1/iceberg` | cold tier |
| `/api/v1/smgw`, `/api/v1/surveillance/delivery` | SMGW registry, delivery surveillance |
| `DELETE /api/v1/gdpr/erasure/{malo_id}` | Art. 17 DSGVO erasure |

## Configuration

`[database] url`, `[identity] tenant`, `[marktd] url`/`api_key`,
`[subscription] webhook_url` and `[oidc]` (or `allow_insecure_no_auth = true`
in dev). `[privacy] erasure_secret` enables erasure. `[policy]`:
[`physische_loeschung`](https://hupe1980.github.io/mako/docs/services/edmd/#policy-physische-loeschung).

The [operator guide](https://hupe1980.github.io/mako/docs/services/edmd/)
covers ingest, validation, substitutes and corrections, read windows, the cold
tier, reading orders, GDPR erasure and the full configuration reference.

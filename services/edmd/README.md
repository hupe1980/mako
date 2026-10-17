# edmd

`edmd` (`:8380`) is mako's energy data management daemon. It stores meter
readings — MSCONS from `marktd`, direct iMSys/SMGW push, bulk and Kafka intake —
validates and scores them, schedules reading orders (Ablesesteuerung), and
serves register-projected series to billing, balancing and the API-Webdienste.
Readings live in the [`meterstore`](https://github.com/hupe1980/meterstore)
crate: a recent window in PostgreSQL, settled history in Apache Iceberg V2.

## Run it

```bash
just dev edmd          # dev/config/edmd.toml against the dev PostgreSQL
```

`edmd --check` probes the running instance's `/health/ready` on loopback — the
container `HEALTHCHECK`. Every other setting lives in `edmd.toml`
(`EDMD_CONFIG`, overridable per key with `EDMD_<SECTION>__<KEY>`).

## At a glance

| | |
|---|---|
| HTTP port | `:8380` · MCP `POST\|GET /mcp` (16 tools, 5 prompts) |
| Database | PostgreSQL 17+ (`migrations/0001_schema.sql`); readings in `meter_reads`, `esa_typ2_reads`, `meter_readings` (meterstore-owned, created by `create_tables` at startup). meterstore connects over TLS — `verify-full` unless `[database] url` names an `sslmode` (`disable` for the dev and test PostgreSQL) |
| Auth | OIDC + Cedar `policies/edmd.cedar` — reads tenant-scoped, writes role-gated; service keys via `[[oidc.service_keys]]`. Refuses to start without `[oidc]` unless `allow_insecure_no_auth` |
| Validation | every door runs `metering::vee::validation`, one series per register, through `ValidatedReads`; findings annotate, never reject. Overlapping spans are refused by an `EXCLUDE` constraint (`422`) |
| Ersatzwerte | `metering::vee::substitute` — interpolation up to two hours, else the Vergleichswert of comparable days; needs `reason` and `bundesland` |
| Balancing day | Strom on the calendar day, Gas on the Gastag (06:00–06:00) — `?sparte=` |
| Events | `de.messwert.reading.*` (direct.stored, quality.warning, delivery.overdue/resumed, order.failed), `de.messwert.cls.*`, `de.messwert.smgw.cert.expiry-warning` |
| Metrics | `GET /edmd/metrics` |

## Routes

| Area | Routes |
|---|---|
| Ingest | `POST /webhook` (MSCONS CloudEvents), `POST /api/v1/meter-reads/{rlm,gas,iot}/{malo_id}`, `/{malo_id}/bulk`, `/{malo_id}/substitute`, `POST\|GET /api/v1/zaehlerstandsgang/{malo_id}`, `/esa/typ2/{malo_id}` |
| Reads | `GET /api/v1/energy/{malo_id}` (the canonical projected series), `/deliveries`, `/lastgang` (+ `/resampled`), `/zeitreihe`, `/summenzeitreihe`, `/billing-period(s)`, `/imbalance/{malo_id}/{year}/{month}?bilanziert_kwh=`, `/netzverlust`, `/forecast`, `/gas-quality` |
| Cold tier | `/api/v1/archive/*`, `POST /api/v1/query/sql`, read-only Iceberg REST catalog under `/api/v1/iceberg` |
| Reading orders | `/api/v1/reading-orders` (+ `/campaign`, `/{id}/complete`, `/cancel`, `/fail`), `GET /api/v1/compliance/jahresablesung/{year}` |
| Quality | `/api/v1/quality-assessments/{malo_id}`, `/quality-score/{malo_id}`, `/corrections/{malo_id}`, `/confirmations`, `/surveillance/delivery` (+ `/scan`) |
| SMGW · § 14a | `/api/v1/smgw`, `/smgw/{malo_id}`, `/smgw/compliance` (+ `/scan`) |
| Sharing · virtual | `/api/v1/sharing/{community_id}/allocation`, `/sharing/readiness`, `/api/v1/virtual` |
| GDPR | `DELETE /api/v1/gdpr/erasure/{malo_id}`, `GET /api/v1/gdpr/erasures` — needs `[privacy] erasure_secret` |

A materialising read defaults to the last 31 days and refuses a window wider
than 732 days; bulk history goes over Arrow IPC, SQL or Iceberg.

## Configuration

Sections: `[http]`, `[database]`, `[identity]`, `[marktd]`, `[subscription]`,
`[webhook]`, `[oidc]` / `[[oidc.service_keys]]`, `[rate_limit]`, `[mcp]`,
`[smgw]`, `[surveillance]`, `[kafka_ingest]`, `[privacy]`, `[archive]`,
`[policy]` (`physische_loeschung = false`: an Art. 17 erasure destroys the
subject mapping and records a tombstone; `true` is refused at startup because
this build removes no rows physically). The policy report is served at
`GET /api/v1/policy` and by the MCP tool `get_policy`.
`dev/config/edmd.toml` is a working example.

The [operator guide](https://hupe1980.github.io/mako/docs/services/edmd/)
covers validation rules, substitutes and corrections, the archive tier, GDPR
erasure and the full configuration reference.

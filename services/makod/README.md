# makod

`makod` is the production daemon that assembles the full `mako` process engine stack into a deployable binary. It wires together all domain modules (GPKE, WiM, GeLi Gas, WiM Gas, MaBiS, GaBi Gas, Redispatch 2.0), connects them to a durable [SlateDB](https://github.com/slatedb/slatedb) event store, and exposes three independent server ports.

For the complete operator reference — including persistence configuration, AS4 transport setup, Kubernetes deployment, and all CLI flags — see the **[`makod` Operator Guide](../../docs/makod.md)**.

---

## Port layout

```
:4080  ← AS4/ebMS3 inbound  (EDIFACT via SOAP/MTOM, WS-Security)
:8080  ← HTTP REST API       (POST /edifact, ERP Command API, admin)
:8090  ← API-Webdienste Strom (iMS REST/JSON — energy-api)
```

All three ports are optional and independently enabled via CLI flags or environment variables. `GET /health` is available on every enabled port.

---

## Domain modules

| Module | Domain | Key PIDs |
|---|---|---|
| `GpkeModule` | GPKE — Lieferbeginn/-ende Strom, Sperrung (NB), Abrechnung, Konfiguration | 55001–55002, 55016–55018, 55555, 17115–17117 (NB), 31001–31008, 17134/17135 |
| `WimModule` | WiM Strom — Messstellenbetrieb, Geräteübernahme, INSRPT | 55039, 55042, 55051, 55168, 23001/23003/23004/23008 |
| `GeliGasModule` | GeLi Gas 3.0 — Lieferantenwechsel Gas, Sperrung Gas, PARTIN Gas + AWH-Rechnung | 44001–44021, 17115–17117 (Gas NB), 37008–37014, 31011 |
| `WimGasModule` | WiM Gas — Messstellenbetrieb Gas, INVOIC Gas billing | 44022–44024, 44039–44053, 44168–44170, 31003/31004, 23005/23009 |
| `MabisModule` | MABIS — Bilanzkreisabrechnung Strom (BKV↔ÜNB) | 13003 |
| `GaBiGasModule` | GaBi Gas — Kapazitätsrechnung | 31010 |
| `RedispatchModule` | Redispatch 2.0 — congestion management (§§ 13/13a/14 EnWG) | 21037/21038 (NB/ÜNB/ANB roles only) |

---

## Quick start

### Development — volatile in-memory (data lost on restart)

```bash
cargo run -p makod -- \
  --allow-volatile \
  --http-addr 127.0.0.1:8080 \
  --tenant-id 9900357000004
```

> `--allow-volatile` is required when `--data-dir` is omitted. Without it, `makod` refuses to start and prints an error directing you to set `--data-dir` or pass the flag explicitly. This prevents accidental production deployments without persistent storage.

### Production — durable SlateDB on local disk

```bash
cargo build -p makod --release --features slatedb

./target/release/makod \
  --data-dir /var/lib/makod \
  --http-addr 0.0.0.0:8080 \
  --as4-addr  0.0.0.0:4080 \
  --tenant-id 9900357000004 \
  --erp-webhook-url https://erp.example.com/mako/events
```

### Startup validation — no workers started

```bash
./target/release/makod --check --data-dir /var/lib/makod --tenant-id 9900357000004
```

`--check` validates configuration, loads profiles, and runs all adapter startup checks, then exits with code 0 on success. Use this in deployment pipelines before starting the live process.

---

## Health checks

Every enabled port exposes `GET /health`:

```
HTTP 200  {"status":"ok","version":"0.5.0","uptime_secs":142}
HTTP 503  {"status":"degraded","reason":"deadline_scheduler not running"}
```

The response is `200 OK` when all background workers (outbox, deadline scheduler, projection worker) are running. Use this as the liveness and readiness probe in container orchestration.

---

## Graceful shutdown

`makod` handles `SIGTERM` and `SIGINT` (Ctrl-C). On receipt it:

1. Stops accepting new inbound messages on all ports.
2. Waits up to **30 seconds** for in-flight event-store writes and outbox drains to complete.
3. Exits with code 0 on clean shutdown, or code 1 if the timeout elapses with pending work.

Adjust the timeout via `--shutdown-timeout-secs <N>`.

---

## Key CLI flags

| Flag | Env var | Description |
|---|---|---|
| `--data-dir <DIR>` | `MAKOD_DATA_DIR` | Persistent SlateDB path. Omit only with `--allow-volatile`. |
| `--allow-volatile` | `MAKOD_ALLOW_VOLATILE` | Permit in-memory (non-durable) mode. Never use in production. |
| `--tenant-id <ID>` | `MAKOD_TENANT_ID` | Operator BDEW code / GLN / EIC. |
| `--http-addr <ADDR>` | `MAKOD_HTTP_ADDR` | Enable HTTP REST API on this address. |
| `--as4-addr <ADDR>` | `MAKOD_AS4_ADDR` | Enable AS4/ebMS3 inbound transport. |
| `--api-webdienste-addr <ADDR>` | `MAKOD_API_WEBDIENSTE_ADDR` | Enable API-Webdienste Strom port. |
| `--erp-webhook-url <URL>` | `MAKOD_ERP_WEBHOOK_URL` | CloudEvents 1.0 webhook for ERP integration. |
| `--check` | `MAKOD_CHECK` | Validate config/profiles, then exit. |
| `-l, --log-level` | `MAKOD_LOG_LEVEL` | Log level (`trace`/`debug`/`info`/`warn`/`error`). Default: `info`. |
| `-f, --log-format` | `MAKOD_LOG_FORMAT` | Log format (`pretty`/`json`/`compact`). Default: `pretty`. |

See `makod --help` for the full flag list including object-store backends (S3, GCS, Azure) and AS4 signing keys.

  --erp-webhook-secret "$(cat /run/secrets/makod-hmac)"
```

---

## Feature flags

| Flag | Description |
|---|---|
| `slatedb` | Enable SlateDB persistence (required for production). Never enable in library crates. |

---

## Health checks

```bash
curl http://localhost:8080/health  # → {"status":"ok"}
```

In Kubernetes, point separate liveness/readiness probes at each enabled port. A port that is not enabled has no `/health` route.

---

## Shutdown

`makod` drains in-flight store writes before exiting. The timeout defaults to 30 seconds:

| CLI flag | Environment variable | Default |
|---|---|---|
| `--shutdown-timeout-secs` | `MAKOD_SHUTDOWN_TIMEOUT_SECS` | `30` |

Increase this for cloud object-store backends (S3/GCS/Azure) that may need longer to flush large write buffers.

---

## Integration tests

End-to-end tests covering all process families live in `tests/`:

| Test | What it covers |
|---|---|
| `e2e_lieferbeginn.rs` | GPKE LF-Anmeldung bilateral (LFN ↔ NB, PIDs 55001/55003/55004) |
| `e2e_lieferende.rs` | GPKE Lieferende bilateral (PIDs 55002/55005/55006) |
| `e2e_lieferantenwechsel.rs` | Full supplier-switch saga with APERAK timeout |
| `e2e_gpke_lf_abmeldung.rs` | GPKE Kündigung Lieferbeginn (PIDs 55016/55017/55018) |
| `e2e_gpke_neuanlage.rs` | GPKE Neuanlage (new grid connection) |
| `e2e_sperrung.rs` | GPKE Sperrung/Entsperrung ORDERS/ORDRSP |
| `e2e_netznutzungsabrechnung.rs` | GPKE INVOIC billing (31001–31008) |
| `e2e_anfrage_bestellung.rs` | GPKE Anfrage individuelle Bestellung (PID 55555) |
| `e2e_wim_*.rs` | WiM Strom MSB-Wechsel, Gerätewechsel, Geräteübernahme, Stammdaten, Steuerungsauftrag, Stornierung |
| `e2e_wim_gas_anmeldung.rs` | WiM Gas Anmeldung (PIDs 44039–44053) |
| `e2e_lieferbeginn_gas.rs` | GeLi Gas bilateral (PIDs 44001/44003/44004) |
| `e2e_lieferende_gas.rs` | GeLi Gas Lieferende bilateral |
| `e2e_mabis.rs` | MaBiS Bilanzkreisabrechnung (PID 13003) |
| `e2e_ahb_conformance.rs` | Cross-PID AHB rule enforcement |
| `startup_smoke.rs` | `assert_dispatch_coverage` — every registered workflow has a deadline dispatch entry |
| `erp_response_dispatch.rs` | ERP adapter response correlation |

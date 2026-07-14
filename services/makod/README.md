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
| `GpkeModule` | GPKE — 16 workflows: Lieferbeginn/-ende Strom (LF+NB), Neuanlage, Abmeldung LF, Ankündigung ZuordnungLF, Sperrung (NB+LF-Antwort), Abrechnung, Datenabruf, Allokationsliste, Messwerte, Konfiguration, Anfrage Bestellung, Ankündigung, UTILTS, PARTIN Strom | 55001–55018/55022–55024/55555/55600–55609, ORDERS 17xxx, INVOIC 31001–31006, PARTIN 37000–37006 |
| `WimModule` | WiM Strom — 10 workflows: MSB-Wechsel, Geräteübernahme, Stammdaten, Preisanfrage/Preisliste, Abrechnung, INSRPT, Stornierung, iMS-Steuerungsauftrag | 55039/55042/55051/55168, ORDERS 17001–17133, INVOIC 31009, INSRPT 23001–23012 |
| `GeliGasModule` | GeLi Gas 3.0 — 9 workflows: UTILMD G Lieferantenwechsel, Stornierung (LF+GNB), Sperrung (LF+GNB), MSCONS Messdaten, Datenabruf, INVOIC 31011 (AWH), PARTIN Gas | 44001–44024, 17103/17104, MSCONS 13002/13007–13009, ORDERS 17115–17117 (Gas), INVOIC 31011, PARTIN 37008–37014 |
| `WimGasModule` | WiM Gas — MSB-Wechsel Gas, Stornierung WiM Gas, INVOIC Gas billing, INSRPT Gas | 44022–44024, 44039–44053, 44168–44170, INVOIC 31003/31004, INSRPT 23005/23009 |
| `MabisModule` | MABIS — Bilanzkreisabrechnung Strom (BKV↔ÜNB) + Clearingliste | 13003, 55065/55069/55070 |
| `GaBiGasModule` | GaBi Gas — 8 workflows: INVOIC 31007/31008/31010, MSCONS 13013 (Allokationsliste MMMA), ALOCAT, NOMINT/NOMRES, SCHEDL, IMBNOT, TRANOT, DELORD/DELRES | INVOIC 31007/31008/31010, ORDERS 17110, MSCONS 13013, synthetic PIDs 90001–90062 |
| `RedispatchModule` | Redispatch 2.0 — congestion management (§§ 13/13a/14 EnWG) | 21037/21038 (NB/ÜNB/ANB roles only) |

---

## Quick start

### Development — volatile in-memory (data lost on restart)

```bash
cargo run -p makod -- \
  --allow-volatile \
  --http-addr 127.0.0.1:8080 \
  --tenant-id 9900357000004 \
  --marktrollen LF
```

> `--allow-volatile` is required when `--data-dir` is omitted. Without it, `makod` refuses to start and prints an error directing you to set `--data-dir` or pass the flag explicitly. This prevents accidental production deployments without persistent storage.

### Production — durable SlateDB on local disk

```bash
# slatedb is enabled via mako-engine's feature in Cargo.toml — no --features flag needed
cargo build -p makod --release

./target/release/makod \
  --data-dir /var/lib/makod \
  --http-addr 0.0.0.0:8080 \
  --auth-key erp-prod=$(openssl rand -hex 32) \
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
HTTP 200  {"status":"ok","version":"0.9.0","uptime_secs":142}
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
| `--marktrollen <ROLES>` | `MAKOD_MARKTROLLEN` | **Required** when `--http-addr` is set. Comma-separated Marktrollen (e.g. `LF,LFG`, `NB,MSB`). An unlisted role is rejected with `422`. |
| `--http-addr <ADDR>` | `MAKOD_HTTP_ADDR` | Enable HTTP REST API on this address. |
| `--auth-key <NAME=TOKEN>` | `MAKOD_AUTH_KEYS` | Named API key for Bearer authentication. Repeatable. At least one `--auth-key` or `--oidc-issuer` is required when `--http-addr` is set. |
| `--oidc-issuer <URL>` | `MAKOD_OIDC_ISSUER` | OIDC issuer URL. `makod` fetches `<URL>/.well-known/openid-configuration` at startup and validates JWT bearer tokens. |
| `--oidc-audience <AUD>` | `MAKOD_OIDC_AUDIENCE` | Expected JWT `aud` claim (required when `--oidc-issuer` is set). |
| `--oidc-jwks-refresh-secs <N>` | `MAKOD_OIDC_JWKS_REFRESH_SECS` | JWKS key-set refresh interval in seconds (default: 300). |
| `--cedar-policy-dir <DIR>` | `MAKOD_CEDAR_POLICY_DIR` | Directory of extra `.cedar` policy files appended to the built-in default policy. |
| `--as4-addr <ADDR>` | `MAKOD_AS4_ADDR` | Enable AS4/ebMS3 inbound transport. |
| `--api-webdienste-addr <ADDR>` | `MAKOD_API_WEBDIENSTE_ADDR` | Enable API-Webdienste Strom port. |
| `--erp-webhook-url <URL>` | `MAKOD_ERP_WEBHOOK_URL` | CloudEvents 1.0 webhook for ERP integration. |
| `--check` | `MAKOD_CHECK` | Validate config/profiles, then exit. |
| `-l, --log-level` | `MAKOD_LOG_LEVEL` | Log level (`trace`/`debug`/`info`/`warn`/`error`). Default: `info`. |
| `-f, --log-format` | `MAKOD_LOG_FORMAT` | Log format (`pretty`/`json`/`compact`). Default: `pretty`. |

See `makod --help` for the full flag list including object-store backends (S3, GCS, Azure) and AS4 signing keys.

---

## Authorization (Cedar ABAC)

All non-health HTTP endpoints are protected by [Cedar](https://cedarpolicy.com)
attribute-based access control. Every request is evaluated against a Cedar policy
set. The built-in `default.cedar` policy grants all actions to every authenticated
principal — suitable for single-tenant deployments.

### Provisioning API keys

Each named key maps a caller identity to a Cedar principal:

```bash
# Single key
makod --auth-key erp-prod=$(openssl rand -hex 32) ...

# Multiple keys (one per integration)
makod \
  --auth-key erp-sap=$(openssl rand -hex 32) \
  --auth-key ops-grafana=$(openssl rand -hex 32) \
  ...
```

Via environment variable (comma-separated `NAME=TOKEN` pairs):

```bash
export MAKOD_AUTH_KEYS="erp-sap=abc123,ops-grafana=xyz456"
```

At least one `--auth-key` or `--oidc-issuer` is required when `--http-addr` is set.
`makod` refuses to start without either.

### Custom Cedar policies

Drop `.cedar` files into a directory and point `--cedar-policy-dir` at it:

```cedar
// /etc/makod/cedar/restrict_readonly.cedar
// Allow ops-grafana to read MaLo stats only; deny everything else.
// Uses the AdminMalo action group (covers all 4 AdminMalo* actions).
forbid(
  principal == MaKo::Principal::"ops-grafana",
  action    in [MaKo::Action::"AdminMalo"],
  resource
)
unless { action == MaKo::Action::"AdminMaloStats" };
```

```bash
makod --cedar-policy-dir /etc/makod/cedar ...
```

Cedar policies are validated at startup against the built-in schema using the
Cedar Validator in strict mode — a policy with type errors prevents startup. This
makes misconfigured policies visible immediately, not at first API call.

### OIDC / JWT authentication

In addition to API keys, `makod` accepts JWT bearer tokens from any
standards-compliant OIDC identity provider — Azure AD/Entra ID, Keycloak,
Okta, AWS Cognito, Google Workspace, Kubernetes workload identity, and others.

```bash
makod \
  --oidc-issuer  "https://login.microsoftonline.com/$TENANT/v2.0" \
  --oidc-audience "api://makod" \
  --http-addr    "0.0.0.0:8080"
```

Or via the TOML config file:

```toml
[oidc]
issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
audience = "api://makod"
```

The JWT `sub` claim becomes the Cedar principal entity ID — identical to an
API-key name. All existing Cedar policies work unchanged regardless of
authentication method.

**Security properties:**
- Only asymmetric algorithms are accepted: RS256/384/512, ES256/384, PS256/384/512.
- HMAC algorithms (`HS256`, `HS384`, `HS512`) are unconditionally rejected.
- `iss`, `aud`, `exp`, and `nbf` claims are validated on every token.
- JWKS public keys are cached in memory; a background task refreshes them every
  `--oidc-jwks-refresh-secs` seconds (default: 300) to handle key rotation
  without restarting.

**Coexistence:** `--auth-key` and `--oidc-issuer` can both be active at once,
enabling gradual migration from API keys to OIDC without downtime.

For the full configuration reference, Cedar policy examples, and provider-specific
setup (Azure Managed Identity, Kubernetes workload identity), see the
[Operator Guide authorization section](../../docs/makod.md#authorization).

---

## MCP server

`makod` runs an [MCP](https://modelcontextprotocol.io) server at `/mcp` on the
`--http-addr` port. LLM clients (Claude Desktop, VS Code Copilot Chat) can use
it to inspect process state and submit commands without writing integration code.

```json
// claude_desktop_config.json
{
  "mcpServers": {
    "makod": {
      "url": "http://localhost:8080/mcp",
      "headers": { "Authorization": "Bearer <token>" }
    }
  }
}
```

**Tools:**

| Tool | Description |
|---|---|
| `list_commands` | List commands available for this instance's configured Marktrollen |
| `submit_command` | Trigger a MaKo process command (GPKE, GeLi Gas, WiM, MABIS) |
| `get_malo` | Read a cached MaLo record by 11-digit ID |
| `list_partners` | List all registered trading partners |
| `get_partner` | Get a trading partner by 13-digit GLN |
| `get_health` | Daemon version, tenant ID, Marktrollen, MaLo cache stats |

**Resources:** `malo://{malo_id}`, `partner://{mp_id}`

**Prompts:** `gpke-lieferbeginn`, `geli-lieferbeginn`, `wim-geraetewechsel` — guided step-by-step workflows

The server returns dynamic instructions at connection time, including a filtered command
list for this instance's Marktrollen and the applicable regulatory deadlines.

See the [MCP section of the operator guide](../../docs/makod.md#mcp-server) for full details.

Authentication is enforced — every request to `/mcp` must carry a valid Bearer
token (same Cedar ABAC layer as the REST API). See the
[Operator Guide MCP section](../../docs/makod.md#mcp-server) for full details.

---

## API reference

When `--http-addr` is enabled, the full OpenAPI 3.1 spec and an interactive
Swagger UI are available at runtime — no separate documentation step required:

| Path | Description |
|------|-------------|
| `GET /api/v1/openapi.json` | Machine-readable OpenAPI 3.1 spec |
| `GET /api/v1/docs/` | Swagger UI — interactive API explorer |

```bash
# Download spec for client generation
curl http://localhost:8080/api/v1/openapi.json -o makod-openapi.json

# Open Swagger UI
open http://localhost:8080/api/v1/docs/
```

---

## Feature flags

| Flag | Description |
|---|---|
| `slatedb` | Enable SlateDB persistence (required for production). Never enable in library crates. |

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

# makod

`makod` is the production daemon that assembles the full `mako` process engine stack into a deployable binary. It wires together all domain modules (GPKE, WiM, GeLi Gas, WiM Gas, MaBiS, GaBi Gas, Redispatch 2.0), connects them to a durable [SlateDB](https://github.com/slatedb/slatedb) event store, and exposes three independent server ports.

For the complete operator reference — including persistence configuration, AS4 transport setup, Kubernetes deployment, and all CLI flags — see the **[`makod` Operator Guide](https://hupe1980.github.io/mako/docs/services/makod/)**.

---

## Port layout

```
:4080  ← AS4/ebMS3 inbound  (EDIFACT + Redispatch XML via SOAP/MTOM, WS-Security)
:8080  ← HTTP REST API       (POST /edifact, ERP Command API, admin)
:8090  ← API-Webdienste Strom (iMS REST/JSON — energy-api)
```

All three ports are optional and independently enabled via CLI flags or environment variables. The `/health` probes are available on every enabled port.

---

## Domain modules

| Module | Domain | Key PIDs |
|---|---|---|
| `GpkeModule` | GPKE — 16 workflows: Lieferbeginn/-ende Strom (LF+NB), Neuanlage, Abmeldung LF, Ankündigung ZuordnungLF, Sperrung (NB+LF-Antwort), Abrechnung, Datenabruf, Allokationsliste, Messwerte, Konfiguration, Anfrage Bestellung, Ankündigung, UTILTS, PARTIN Strom | 55001–55018/55022–55024/55555/55600–55609, ORDERS 17xxx, INVOIC 31001–31006, PARTIN 37000–37006 |
| `WimModule` | WiM Strom — 11 workflows: MSB-Wechsel, Geräteübernahme, Stammdaten, Technik-Änderung, Preisanfrage/Preisliste, Abrechnung, INSRPT, Stornierung, Wertebestellung, iMS-Steuerungsauftrag | 55039/55042/55051/55168, ORDERS 17001–17133, INVOIC 31009, INSRPT 23001–23012 |
| `GeliGasModule` | GeLi Gas 3.0 — 9 workflows: UTILMD G Lieferantenwechsel, Stornierung (LF+GNB), Sperrung (LF+GNB), MSCONS Messdaten, Datenabruf, INVOIC 31011 (AWH), PARTIN Gas | 44001–44024, 17103/17104, MSCONS 13002/13007–13009, ORDERS 17115–17117 (Gas), INVOIC 31011, PARTIN 37008–37014 |
| `WimGasModule` | WiM Gas — MSB-Wechsel Gas, Stornierung WiM Gas, INVOIC Gas billing, INSRPT Gas | 44022–44024, 44039–44053, 44168–44170, INVOIC 31003/31004, INSRPT 23005/23009 |
| `MabisModule` | MaBiS — 5 workflows: Bilanzkreisabrechnung Strom (BKV↔ÜNB), Clearingliste, ZP-Lifecycle (Aktivierung/Deaktivierung MaBiS-ZP, Zuordnungsermächtigung, AAÜZ/LF-AASZR), Anforderungen, Listenabgleich | MSCONS 13003/13010–13012, IFTSTA 21000–21005, UTILMD 55062–55064/55071–55072/55195–55196/55197–55214/55223–55224, 55065/55069/55070, ORDERS 17201–17208 |
| `GaBiGasModule` | GaBi Gas — 4 workflows: INVOIC 31007/31008/31010, MSCONS 13013 (Allokationsliste MMMA), ALOCAT, NOMINT/NOMRES | INVOIC 31007/31008/31010, REMADV 33001, COMDIS 29001, ORDERS 17110, MSCONS 13013, DVGW 70001–70023 (ALOCAT) / 70030–70039 (NOMINT/NOMRES) |
| `RedispatchModule` | Redispatch 2.0 — congestion management (§§ 13/13a/14 EnWG) | 21037/21038 (NB/ÜNB/ANB roles only) |

---

## Quick start

### Development — volatile in-memory (data lost on restart)

```bash
cat > makod.toml <<'TOML'
[[party]]
mp_id   = "9900357000004"
roles   = ["LF"]
primary = true

[storage]
allow_volatile = true          # in-memory; data is lost on exit

[http]
addr      = "127.0.0.1:8080"
auth_keys = ["dev=dev-token-change-me"]

[as4]
allow_no_signing = true        # no AS4 credentials: log outbound EDIFACT
TOML

cargo run -p makod -- --config makod.toml
```

Three things the daemon refuses to start without, all visible above:

- **Durability.** Omitting `[storage] data_dir` needs `allow_volatile = true`. Without it `makod` refuses to start, so a production deployment cannot lose its event store by accident.
- **A credential on every authenticated port.** `[http] addr` submits commands and triggers migrations; it never runs open. Supply `auth_keys` or an `[oidc]` issuer.
- **A path for outbound EDIFACT.** With neither AS4 signing material nor `[erp] edifact_outbox_webhook_url`, every outbound message would be logged and rescheduled forever. `allow_no_signing = true` makes that a deliberate development choice instead of a silent regulatory failure.

### Production — durable SlateDB on local disk

```bash
# slatedb is enabled via mako-engine's feature in Cargo.toml — no --features flag needed
cargo build -p makod --release

./target/release/makod \
  --config /etc/makod/makod.toml \
  --data-dir /var/lib/makod \
  --http-addr 0.0.0.0:8080 \
  --auth-key erp-prod=$(openssl rand -hex 32) \
  --as4-addr  0.0.0.0:4080 \
  --erp-webhook-url https://erp.example.com/mako/events
```

### Startup validation — no workers started

```bash
./target/release/makod --check --config /etc/makod/makod.toml --data-dir /var/lib/makod
```

`--check` runs every validation the real boot runs before it opens a socket, then exits: the config file schema, the `[[party]]` identity rules, profile and adapter coverage, dispatch completeness, the data-directory lock, AS4 key material and the partner registry, the Cedar policy set, credentials for every authenticated port, and the ingest and egress transport rules. Exit 0 means the same configuration will start; any failure exits non-zero and names the flag or field that fixes it.

Only the network round-trips are deferred — OIDC discovery and the JWKS fetch — so `--check` runs on a CI runner with no route to the identity provider. Its arguments are still validated.

`--check` changes no domain state: the process-registry reconciliation, the one startup step that writes, runs only after the check exits. It does take the exclusive data-directory lock, so it will refuse while the daemon is running.

---

## Health checks

Every enabled port exposes three routes. They are unauthenticated and exempt
from the per-peer rate limiter — a throttled probe reads as a dead container.

| Route | Answers | Fails when | Kubernetes probe |
|---|---|---|---|
| `/health/live` | Is the process running? | never, if it responds at all | `livenessProbe` |
| `/health/ready` | Can it serve traffic? | store unreachable, or a worker heartbeat is stale | `readinessProbe` |
| `/health` | alias of `/health/ready` | as above | — |

```
HTTP 200  {"status":"ok","instance_id":"mako-prod-01-12345","version":"0.16.0"}
HTTP 503  {"status":"degraded","instance_id":"mako-prod-01-12345","version":"0.16.0",
           "reason":"worker_stale:deadline-scheduler"}
```

The split matters: Kubernetes *restarts* a container that fails liveness, but
only removes one that fails readiness from Service endpoints. A stalled outbox
worker or an unreachable object store belongs on readiness — restarting the
container does not fix the object store, and doing it mid-delivery costs an AS4
retry cycle.

`reason` is a stable category (`store_unavailable`, `worker_stale:<name>`) and
never carries internal paths or store state.

---

## Graceful shutdown

`makod` handles `SIGTERM` and `SIGINT` (Ctrl-C). On receipt it:

1. Cancels the shared shutdown token. Listeners stop accepting and drain
   in-flight requests; every background worker returns at its next message or
   tick boundary.
2. **Joins** every listener and worker — this is the step that makes the next
   one safe.
3. Flushes the buffered dead-letter entries, which have no other durable home.
4. Closes the event store.

Steps 1–3 share the `--shutdown-timeout-secs` budget (default 30); the store
close keeps a 10-second floor of its own, because abandoning an unflushed
write-ahead log is worse than overrunning the grace period.

Exit code is 0 only when all three completed. A timeout anywhere exits **1** and
names which stage did not finish — a lost write must not be indistinguishable
from a clean stop.

> Joining the workers before closing the store is not tidiness. A worker still
> running when the store closes can lose an outbox `acknowledge` after the
> counterparty already has the message, and the next start delivers it again.

Set `terminationGracePeriodSeconds` above `--shutdown-timeout-secs` + 10.

---

## Key CLI flags

| Flag | Env var | Description |
|---|---|---|
| `--data-dir <DIR>` | `MAKOD_DATA_DIR` | Persistent SlateDB path. Omit only with `--allow-volatile`. |
| `--allow-volatile` | `MAKOD_ALLOW_VOLATILE` | Permit in-memory (non-durable) mode. Never use in production. |
| `--config <FILE>` | `MAKOD_CONFIG` | TOML config file. **Required** — must define at least one `[[party]]` entry (`mp_id` + `roles`); the primary entry is the operator identity. |
| `--marktrollen <ROLES>` | `MAKOD_MARKTROLLEN` | Optional override of the role allow-list (comma-separated, e.g. `LF,LFG`, `NB,MSB`). Defaults to all roles from `[[party]]`. A command for an unlisted role is rejected with `422`. |
| `--http-addr <ADDR>` | `MAKOD_HTTP_ADDR` | Enable HTTP REST API on this address. |
| `--auth-key <NAME=TOKEN>` | `MAKOD_AUTH_KEYS` | Named API key for Bearer authentication. Repeatable. At least one `--auth-key` or `--oidc-issuer` is required when `--http-addr` is set. Prefer `[http] auth_keys_file` in the config file. |
| `--oidc-issuer <URL>` | `MAKOD_OIDC_ISSUER` | OIDC issuer URL. `makod` fetches `<URL>/.well-known/openid-configuration` at startup and validates JWT bearer tokens. |
| `--oidc-audience <AUD>` | `MAKOD_OIDC_AUDIENCE` | Expected JWT `aud` claim (required when `--oidc-issuer` is set). |
| `--oidc-jwks-refresh-secs <N>` | `MAKOD_OIDC_JWKS_REFRESH_SECS` | JWKS key-set refresh interval in seconds (default: 300). |
| `--cedar-policy-dir <DIR>` | `MAKOD_CEDAR_POLICY_DIR` | Directory of extra `.cedar` policy files appended to the built-in default policy. |
| `--cedar-no-default-policy` | `MAKOD_CEDAR_NO_DEFAULT_POLICY` | Omit the built-in permit-all baseline so only `--cedar-policy-dir` grants access. |
| `--as4-addr <ADDR>` | `MAKOD_AS4_ADDR` | Enable AS4/ebMS3 inbound transport. |
| `--api-webdienste-addr <ADDR>` | `MAKOD_API_WEBDIENSTE_ADDR` | Enable API-Webdienste Strom port. |
| `--erp-webhook-url <URL>` | `MAKOD_ERP_WEBHOOK_URL` | CloudEvents 1.0 webhook for ERP integration. |
| `--check` | `MAKOD_CHECK` | Run every configuration-derived startup validation, then exit. |
| `-l, --log-level` | `MAKOD_LOG_LEVEL` | Log level (`trace`/`debug`/`info`/`warn`/`error`). Default: `info`. |
| `-f, --log-format` | `MAKOD_LOG_FORMAT` | Log format (`pretty`/`json`/`compact`). Default: `pretty`. |

See `makod --help` for the full flag list including object-store backends (S3, GCS, Azure) and AS4 signing keys.

---

## Authorization (Cedar ABAC)

All non-health HTTP endpoints are protected by [Cedar](https://cedarpolicy.com)
attribute-based access control. Every request is evaluated against a Cedar policy
set. The built-in `default.cedar` policy grants all actions to every authenticated
principal — suitable for single-tenant deployments.

A Cedar request is allowed when any `permit` matches and no `forbid` does, so an
added `permit` cannot narrow that baseline — only `forbid` can. For a
least-privilege deployment (and for §9 EnWG role separation in a combined-role
VIU install) pass `--cedar-no-default-policy`, which drops the baseline and makes
`--cedar-policy-dir` the only source of access. `conservative.cedar` ships as a
starting point. The flag requires a policy directory; without one `makod` refuses
to start rather than denying every request.

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

That example uses `forbid` because it narrows the permit-all baseline. To deny by
default and grant back only what is listed, copy `conservative.cedar` into the
directory and add `--cedar-no-default-policy`.

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
[Operator Guide authorization section](https://hupe1980.github.io/mako/docs/services/makod/#authorization).

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
| `get_partner` | Get a trading partner by Marktpartner-ID |
| `get_health` | Daemon version, tenant ID, Marktrollen, MaLo cache stats |
| `get_process` | Look up an active process by business key (MaLo/MeLo/Vorgang) — stream ID + pending deadlines |
| `list_overdue_deadlines` | Processes with expired regulatory deadlines (compliance alert) |
| `list_active_processes` | Count of active (registered) process instances |
| `get_outbox_status` | AS4 outbox delivery status — pending count and oldest-message age |
| `list_dead_letters` | 20 most recent permanently dead-lettered messages (§147 AO / GoBD) |

**Resources:** `malo://{malo_id}`, `partner://{mp_id}`

**Prompts:** `gpke-lieferbeginn`, `geli-lieferbeginn`, `wim-geraetewechsel`, `msb-preisanfrage`, `wim-gas-anmeldung`, `gpke-sperrung` — guided step-by-step workflows

The server returns dynamic instructions at connection time, including a filtered command
list for this instance's Marktrollen and the applicable regulatory deadlines.

See the [MCP section of the operator guide](https://hupe1980.github.io/mako/docs/services/makod/#mcp-server) for full details.

Authentication is enforced — every request to `/mcp` must carry a valid Bearer
token (same Cedar ABAC layer as the REST API). See the
[Operator Guide MCP section](https://hupe1980.github.io/mako/docs/services/makod/#mcp-server) for full details.

---

## EDIFACT rendering

Workflow intent becomes wire bytes in `orchestrator/edifact_renderer/` (split per message type), which dispatches on
the outbox message type and — for MSCONS — on the Prüfidentifikator.

| PID | Anwendungsfall | BGM DE 1001 |
|---|---|---|
| 13003 | Summenzeitreihe (MaBiS) | `BK` |
| 13023 | Redispatch 2.0 Ausfallarbeitssummenzeitreihe | `Z46` |
| 13015 | Arbeit + Leistungsmaximum im Kalenderjahr vor Lieferbeginn | `Z27` |
| 13016 | Energiemenge und Leistungsmaximum | `Z28` |
| 13019 | Energiemenge (Strom) | `7` |
| 13027 | Werte nach Typ 2 (MSB → ESA) | `Z83` |

`BGM` DE 1001 names the document type the receiver routes by, so it is set per
Anwendungsfall. DE 1004 is the **Dokumentennummer** — all sixteen BGM rows of
MSCONS AHB 3.2 spell it that way, and the Prüfidentifikator travels in
`SG1 RFF+Z13` instead. Inbound detection reads both locations (profile
`pid_source` first) and accepts only a plausible 5-digit code, so a numeric
Belegnummer cannot outrank the real PID; a PID-bearing message that carries none
is dead-lettered and reported as `missing_pid`, not silently accepted. An unimplemented PID is refused by name — rendering it in a
supported shape would produce a syntactically valid message stating something the
sender did not say.

`tests/mscons_conformance.rs` renders each use case, parses it back and validates
it against the registered release profile, rather than asserting on segment
substrings. See the [operator guide](https://hupe1980.github.io/mako/makod#edifact-rendering)
for the segment-level detail.

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
| `e2e_lieferbeginn.rs` | GPKE LF-Anmeldung bilateral (LFN ↔ NB, PIDs 55001/55002/55003) |
| `e2e_lieferende.rs` | GPKE Lieferende bilateral (PIDs 55002/55005/55006) |
| `e2e_lieferantenwechsel.rs` | Full supplier-switch saga with APERAK timeout |
| `e2e_gpke_lf_abmeldung.rs` | GPKE Abmeldung/Beendigung der Zuordnung (NB→LF, PIDs 55007/55008/55009) |
| `e2e_gpke_neuanlage.rs` | GPKE Neuanlage (new grid connection) |
| `e2e_gpke_ankuendigung_zuordnung_lf.rs` | GPKE Ankündigung Zuordnung (NB→LFN, PID 55607) |
| `e2e_sperrung.rs` | GPKE Sperrung/Entsperrung ORDERS/ORDRSP |
| `e2e_netznutzungsabrechnung.rs` | GPKE INVOIC billing (31001/31002/31005/31006) |
| `e2e_anfrage_bestellung.rs` | GPKE Anfrage individuelle Bestellung (PID 55555) |
| `e2e_loopback.rs` | VIU self-addressed loopback + FV coexistence |
| `e2e_wim_*.rs` | WiM Strom MSB-Wechsel, Gerätewechsel, Geräteübernahme, Stammdaten, Steuerungsauftrag, Stornierung |
| `e2e_wim_gas_anmeldung.rs` | WiM Gas Anmeldung (PIDs 44039–44053) |
| `e2e_lieferbeginn_gas.rs` | GeLi Gas bilateral (PIDs 44001/44002/44003) |
| `e2e_lieferende_gas.rs` | GeLi Gas Lieferende bilateral |
| `e2e_mabis.rs` | MaBiS Bilanzkreisabrechnung (PID 13003) |
| `e2e_ahb_conformance.rs` | Cross-PID AHB rule enforcement |
| `e2e_dispatch_coverage_guard.rs` | **Every registered PID must reach a dispatch arm.** Dispatches a real fixture per registered PID (318 of 383; fixtures resolved by filename, then by scanning content for the PID) and fails on any silent `pid_not_in_*` drop. Send-only PIDs — where mako initiates and no receiver exists — are enumerated in `SEND_ONLY_PIDS`; the guard fails if one starts being handled, so the list only shrinks. The 44 it cannot reach are the PIDs with no AHB profile entry |
| `e2e_outbox_type_coverage_guard.rs` | **Every emitted outbox `message_type` must reach a worker** — the EDIFACT renderer or the ERP adapter. A type in neither is enqueued and goes nowhere: the AS4 sender substitutes raw domain JSON for the interchange, so the partner receives something it cannot parse and nothing errors |
| `e2e_outbox_render_contract.rs` | Renders GeLi Gas answers (44002/44003/44007) and the WiM Störungsmeldung (23001) and parses the bytes back; the INSRPT case also asserts **AHB validity**, not just parseability |
| `e2e_anmeldung_answer_routing.rs` | Every answer PID on the supplier-change success paths reaches its arm — GeLi Gas `ANTWORT_PIDS_LF`, GPKE `UTILMD_ANFRAGE_PIDS`. Both arms read the module's own constant, so the dispatch table cannot drift from the router registration |
| `startup_smoke.rs` | `assert_dispatch_coverage` — every registered workflow has a deadline dispatch entry; §2.13 party registry validation |
| `as4_security.rs` | **12 AS4 security tests** — BDEW AS4-Profil v1.2 compliance: sign+encrypt defaults, tampered-signature rejection (`As4WsSecVerifier`), `require_encrypted_inbound` enforcement, 72h replay dedup, full round-trip via `MockAs4Endpoint` with decryption |
| `erp_response_dispatch.rs` | ERP adapter response correlation |

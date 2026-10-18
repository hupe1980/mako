# makod

`makod` is the market-communication daemon: it assembles the domain workflow
crates into one process engine on a durable [SlateDB](https://github.com/slatedb/slatedb)
event store, receives and sends EDIFACT over AS4, takes ERP commands over REST,
and serves the BDEW API-Webdienste Strom. Seven modules register
**71 workflows** over 469 Prüfidentifikatoren in the default build.

## Run it

```bash
just dev makod         # dev/config/makod.toml: in-memory store, permit-all, no AS4 keys
makod --check --config /etc/makod/makod.toml   # every startup validation, then exit
```

`makod` refuses to start without durability (`[storage] data_dir`, or
`allow_volatile = true`), without a credential on the HTTP port (`auth_keys` or
`[oidc]`), and without a path for outbound EDIFACT (AS4 key material,
`[erp] edifact_outbox_webhook_url`, or `allow_no_signing = true`).

## Ports

| Port | Purpose |
|---|---|
| `:8080` | REST API — `POST /edifact`, `POST /api/v1/commands`, admin, `/mcp`, `/metrics`, OpenAPI at `/api/v1/openapi.json` |
| `:4080` | AS4/ebMS3 inbound at `POST /as4/inbox/<MP-ID>` per `[[party]]` — EDIFACT and Redispatch XML, signed and encrypted (BDEW AS4-Profil) |
| `:8090` | API-Webdienste Strom (iMS REST/JSON, `energy-api`) |

Each port is enabled on its own. Every enabled port serves `/health/live`,
`/health/ready` and `/health`.

## Modules

| Module | Workflows | Scope |
|---|---|---|
| `GpkeModule` | 23 | GPKE Strom: Lieferbeginn/-ende, Sperrung, Abrechnung, Messwerte, Konfiguration, PARTIN Strom |
| `WimModule` | 14 | WiM Strom **and** Gas: MSB-Wechsel, Geräte, Preisanfrage, INVOIC 31003/31004/31009, INSRPT |
| `GeliGasModule` | 12 | GeLi Gas 3.0: UTILMD G 44001–44024, Sperrung Gas, INVOIC 31011, PARTIN Gas |
| `MabisModule` | 6 | MaBiS: Bilanzkreisabrechnung, Clearinglisten, Listenabgleich, ZP-Lifecycle, Anforderungen |
| `GaBiGasModule` | 5 | GaBi Gas: INVOIC 31007/31008/31010, MSCONS 13013, DVGW ALOCAT/NOMINT/SSQNOT |
| `EmobModule` | 3 | NZR-EMob Modell 2 |
| `RedispatchModule` | 8 | Redispatch 2.0 |

`startup::production_modules` is the one list; `tests/it/published_counts.rs`
holds every document that states the totals to it. The PID-level map is the
[PID reference](https://hupe1980.github.io/mako/docs/regulatory/pid-reference/).

## Deployment roles

`[deployment] rollen` in `makod.toml` decides which modules register:
`lf-strom`, `lf-gas`, `nb-strom`, `nb-gas`, `msb-strom`, `msb-gas`,
`esa-strom`. Startup refuses a configuration that declares none, a declared
role without exactly one `[[party]]` MP-ID, and an MP-ID for a role the
deployment does not declare. `startup::MODULE_TABLE` maps each role to its
modules. makod has no role cargo features, and SlateDB is enabled through the
`mako-engine` dependency, so a plain `cargo build -p makod` is the binary every
deployment runs.

## Security and integration

- **Authorization:** Cedar ABAC on every non-health route, default-deny;
  `--cedar-policy-dir` supplies the policy (`conservative.cedar` is a starting
  point), `--cedar-permit-all` is development-only.
- **Authentication:** named API keys (`--auth-key NAME=TOKEN`, `[http]
  auth_keys_file`) and/or OIDC JWTs (asymmetric algorithms only); the JWT `sub`
  is the Cedar principal.
- **ERP:** commands in at `POST /api/v1/commands`; CloudEvents out to
  `--erp-webhook-url`.
- **marktd:** with `[marktd] url`, the MaLo-Ident cache follows marktd's durable
  fan-out (`de.markt.malo.updated` at `POST /webhooks/marktd`, signed with
  `[marktd] webhook_secret`) and a startup backfill that `/health/ready` waits
  for.
- **MCP:** 12 tools and 6 prompts at `/mcp`, behind the same Cedar layer.

## Configuration

`makod.toml` (`--config`, `MAKOD_CONFIG`) requires `[deployment] rollen` and at
least one `[[party]]` (`mp_id`, `roles`, `primary`, and `[party.as4]` — each
MP-ID's own signing, encryption and TLS key pairs). Other blocks: `[storage]`
(+ `.s3`/`.gcs`/`.azure`), `[http]`, `[authz]`, `[oidc]`, `[engine]`, `[as4]`
(`[[as4.partner]]`), `[policy.as4] unbekannte_absender`, `[erp]`,
`[webdienste]`, `[marktd]`, `[maloid]`, `[logging]`. Precedence is CLI flag →
`MAKOD_*` environment variable → file → default; `makod --help` lists every
flag.

## Tests

`cargo test -p makod --test it` runs the end-to-end suite in `tests/it/` against
the in-memory store — process families, AHB conformance, dispatch and outbox
coverage guards, AS4 security and the published-figure checks.

The [operator guide](https://hupe1980.github.io/mako/docs/services/makod/)
covers configuration, AS4, the command registry, EDIFACT rendering, deployment,
health checks and background workers in full.

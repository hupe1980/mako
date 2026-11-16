# makod

`makod` is the market-communication daemon for every Marktrolle a deployment
declares: it assembles the domain workflow crates into one process engine on a
durable [SlateDB](https://github.com/slatedb/slatedb) event store, receives and
sends EDIFACT over AS4, takes ERP commands over REST and serves the BDEW
API-Webdienste Strom.
Seven modules register **77 workflows** over 498 Prüfidentifikatoren; which
ones run is decided by `[deployment] rollen`.

| | |
|---|---|
| **Port** | 8080 REST (`dev/ports.toml`); 4080 AS4 inbound and 8090 API-Webdienste when enabled |
| **Database** | none — SlateDB on local disk or S3/GCS/Azure (`[storage]`) |
| **MCP** | 17 tools, 6 prompts at `/mcp` |
| **Emits** | `de.mako.*` to `[erp] webhook_url` — `process.*`, `aperak.*`, `antwortfrist.abgelaufen`, `edifact.outbound` |
| **Consumes** | EDIFACT and Redispatch XML over AS4; `de.markt.malo.updated` at `POST /webhooks/marktd` |
| **Depends on** | marktd (optional: MaLo cache, ESA consent gate) |

## Run

```bash
just dev makod    # dev/config/makod.toml: in-memory store, permit-all, no AS4 keys
cargo run -p makod -- --config dev/config/makod.toml --check-config   # every finding, --json for the /admin/einrichtung body
cargo run -p makod -- --config dev/config/makod.toml --check          # every startup validation, then exit
cargo run -p makod -- --config dev/config/makod.toml migrate          # pending state migrations, then exit
```

`makod` runs its own `main` rather than the `mako-service` runner. It refuses
to start without durability (`[storage] data_dir` or `allow_volatile`), without
a credential on the HTTP port (`auth_keys` or `[oidc]`), and without a path for
outbound EDIFACT (`[party.as4]` key material, `[erp] edifact_outbox_webhook_url`
or `[as4] allow_no_signing`).

## API

| Route | Purpose |
|---|---|
| `POST /api/v1/commands` | ERP command — starts or answers a process |
| `POST /api/v1/probe/{command}` | Rehearse a command; writes nothing |
| `POST /edifact` | Raw EDIFACT interchange |
| `POST /as4/inbox/<MP-ID>` (`:4080`) | AS4 inbox per `[[party]]`, signed and encrypted |
| `GET /api/v1/invoic/{process_id}/rechnung` | BO4E billing data of an INVOIC process |
| `/admin/partners`, `/admin/malo`, `/admin/migrations` | Partner registry, MaLo cache, live format-version migration |
| `GET /admin/einrichtung`, `GET /api/v1/policy` | Configuration findings and policy report |
| `GET /api/v1/openapi.json` | OpenAPI 3.1 |

## Configuration

`makod.toml` (`--config`, `MAKOD_CONFIG`; CLI flag → `MAKOD_*` → file →
default) requires:

- `[deployment] rollen` — any of `lf-strom`, `lf-gas`, `nb-strom`, `nb-gas`,
  `msb-strom`, `msb-gas`, `esa-strom`, `bkv-strom`; each needs exactly one
  `[[party]]` MP-ID.
- `[[party]]` — `mp_id`, `roles`, `primary`, and `[party.as4]` with the signing,
  encryption **and** TLS key pairs (`tls_cert_pem_file`, `tls_key_pem_file`;
  a party without them is refused).
- `[[as4.partner]]` — `mp_id`, `endpoint` and `encryption_cert_pem_file` for a
  partner makod sends to; a receive-only partner omits both.
  `[[as4.partner]] quirks` and `[as4] lenient_receipts` are refused at load.

Policy: [`[policy.as4] unbekannte_absender`](https://hupe1980.github.io/mako/docs/services/makod/#policy-as4-unbekannte-absender),
[`[policy.ingest] unbekannter_vorgang`](https://hupe1980.github.io/mako/docs/services/makod/#policy-ingest-unbekannter-vorgang).

The [operator guide](https://hupe1980.github.io/mako/docs/services/makod/)
covers every configuration key, the deployment-role map, AS4, the command
registry, Cedar authorization and the MCP surface.

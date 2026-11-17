+++
title = "Getting Started"
description = "One command to a running, seeded mako: `just dev` builds and starts every service with the Desk and fills one Stadtwerk; `just demo` runs the released images without Rust."
weight = 2
+++
Two commands start mako. Both run the same world — **Stadtwerke Musterstadt**,
Netzbetreiber, Lieferant and Messstellenbetreiber in Strom and Gas in one
tenant — and both fill
it through the services' public APIs, so the Desk opens on work to do.

| | Needs | What runs |
|---|---|---|
| `just dev` | Docker, Rust (`rustup`), Node for the Desk | PostgreSQL in Docker; every service built by cargo and run on this machine; the Desk under Vite |
| `just demo` | Docker | The released images of every service and the Desk, one published port |

## Check the machine

```bash
git clone https://github.com/hupe1980/mako.git
cd mako
just doctor
```

`just doctor` checks what `just dev` needs and names every miss with the command
that fixes it: Docker running with Compose v2, the toolchain
`rust-toolchain.toml` pins, `curl`, `perl` and `python3`, Node for the Desk
(optional), the ports in `deploy/ports.toml` plus `5432` and `5173` free, and
the disk — `target/` grows to about 100 GB, so it warns below that. Then it
prints one line per daemon's configuration, ready or blocked
([Einrichtung](@/docs/guide/einrichtung.md) explains the findings).

## `just dev` — the estate on this machine

```bash
just dev
```

1. PostgreSQL starts in Docker (`deploy/dev/compose.yaml`), one database per
   service, created by `deploy/postgres/init.sql`.
2. Every service and the seeder are built with cargo. The first build takes
   15–30 minutes; later ones take seconds.
3. Each service runs its configuration `deploy/demo/<service>.toml`, rewritten
   into `target/dev/` so its peers and its database are reached on
   `127.0.0.1`. Every log line carries the service's coloured name, and each
   log is also written to `target/dev/<service>.log`. `marktd` starts first,
   `agentd` last, because it connects to every other service's MCP endpoint.
4. The seeder fills Stadtwerke Musterstadt (below).
5. The Desk starts under Vite on <http://localhost:5173> against the running
   services.

It ends with the URLs, the token command and the time it took. **Ctrl-C** stops
the services and keeps the data; the next `just dev` builds incrementally and
does not seed twice. `just dev-down` removes the database.

| Option | Effect |
|---|---|
| `just dev --no-desk` | without the Desk |
| `just dev --no-seed` | an empty estate |
| `just dev --ci` | headless: start, seed, verify, stop — what the CI job runs |

A service that does not become ready stops the run with the last lines of its
log. A changed service is a Ctrl-C and `just dev` away.

### The seeded Stadtwerk

`tools/mako-seed` writes the world through the same REST routes and the same
EDIFACT door an integrator uses, judged by the same checks and Cedar policies.
Every step is keyed on a natural key, so `just seed` can run again and changes
nothing.

| What | Where it shows |
|---|---|
| Marktlokationen with their Messlokationen, MSB and grid record, and the Netzbetreiber's Preisblatt | `marktd` |
| A UTILMD 55001 from Nordlicht Energie, decided `Accept` (`E_0622` → `A51`) and answered with the 55002 | `processd`, `makod` |
| A 55001 from Musterstadt Energievertrieb, the Stadtwerk's own supplier, held for a person (§ 20 Abs. 1 EnWG parity) | `processd` — the Desk's Freigaben |
| A 55001 on a Marktlokation with no supply record, escalated as a Klärfall (`E_0622` Prüfschritt 50) | `processd` — the Desk's Heute |
| A 9.8 kWp rooftop plant and its June 2026 feed-in, 2880 kWh, settled at EUR 233.568 under § 21 EEG 2023 | `einsd`, `edmd` |
| Erika Mustermann's household contract, its January invoice (111.33 netto, 132.48 brutto), the issued PDF, the Offener Posten and the payment that closes it | `vertragd`, `billingd`, `outputd`, `accountingd` |
| Clara Sommer's online order: the frozen Endkunden-Angebot, the legal documents and the contract with its Bestellnachweis | `productd`, `vertragd` |
| Twelve more households with contracts and last month's daily readings, one of whose invoices the risk gate holds for review | `vertragd`, `edmd`, `billingd` |
| The Stadtwerk in Gas on its own DVGW-Codenummer: six Gas Marktlokationen with Messlokation, MSB and supply record, and Hannah Brandt's Gas contract | `marktd`, `productd`, `vertragd` |
| A Gas NN-Rechnung (INVOIC 31002) from Gasnetz Nachbarstadt at 1.42 ct/kWh where its Preisblatt publishes 1.12 ct/kWh, disputed | `invoicd` — the Desk's invoice disputes |
| A UTILMD with a Prüfidentifikator no Anwendungsübersicht publishes, dead-lettered | `makod` — the Desk's dead letters |
| A KAV-Grenzpreis for 2024, synthetic demo data that names the seed file as its source — never a published figure | `marktd` Referenzreihen |

```bash
just seed --verify
```

`--verify` asserts the outcomes, not status codes — the decision and its
Antwortcode, the kWh and the Vergütung, each invoice line to the cent, the
document, the closed receivable — and that each Desk view the seed fills has
rows.

### Personas and tokens

The demo configurations set `[oidc] mode = "demo"`: a service accepts tokens
signed by the published development key, and its configuration check reports
that as a warning. The Desk's account menu switches between the personas; on the
command line, `just token` prints one:

| Persona | Roles |
|---|---|
| `nb` — Sachbearbeitung Netz | `NB` |
| `lf` — Sachbearbeitung Vertrieb | `LF` |
| `admin` — Administration (the default) | `NB`, `LF`, `MSB`, `ESA`, `ADMIN` |
| `agent` — Agent | `NB`, `LF`, `MSB` |

```bash
TOKEN=$(just token lf)
curl -s http://127.0.0.1:9780/api/v1/vertraege -H "Authorization: Bearer $TOKEN"
```

Each request is decided by the same Cedar policies as in production. A
configuration without `mode = "demo"` refuses to start against an issuer that
publishes the development key. `makod` is reached with its API key
(`deploy/demo/makod.toml`, `[http] auth_keys`).

### Watch the events

```bash
just listen
```

subscribes to `marktd`'s fan-out — every CloudEvent of the estate passes
through it — verifies each delivery's Standard Webhooks signature and prints it
as it arrives. Ctrl-C removes the subscription.

### Every service

Each service answers on its port from `deploy/ports.toml`, at
`http://127.0.0.1:<port>`: `/health/ready`, `/api/v1/openapi.json`, `/mcp`.

| Service | Port | | Service | Port |
|---|---|---|---|---|
| `makod` | 8080 | | `productd` | 9080 |
| `marktd` | 8180 | | `einsd` | 9180 |
| `invoicd` | 8280 | | `billingd` | 9280 |
| `edmd` | 8380 | | `accountingd` | 9380 |
| `processd` | 8580 | | `portald` | 9480 |
| `netzbilanzd` | 8680 | | `agentd` | 9580 |
| `vertragd` | 9780 | | `outputd` | 9880 |

PostgreSQL is `postgres://mako:mako@127.0.0.1:5432/<service>`.

A configuration problem is reported in one go — the file read, the environment
prefix (`MARKTD_`, nested keys joined with `__`) and every missing, unknown or
invalid key:

```bash
cargo run -p marktd -- --help                                              # usage, reads no config
MARKTD_CONFIG=deploy/demo/marktd.toml cargo run -p marktd -- --check-config  # every finding with its path
```

## `just demo` — no Rust needed

```bash
just demo
```

starts the released images of the version this checkout carries
(`ghcr.io/hupe1980/mako-<service>`): PostgreSQL, every service, the Desk and a
seed container that fills and verifies the same Stadtwerk. One port is
published: open <http://localhost:8000>. The Desk's edge routes each
`/svc/<service>/` to its service with the token of the persona chosen in the
account menu; `just token <persona>` prints the same tokens for `curl`.

`just demo --build` builds the images from this tree instead of pulling them —
budget 20–45 minutes cold. `just demo-down` stops it and removes its data.
`just listen` subscribes through the demo's edge when it is running.

## The Desk alone

```bash
just dev-ui
```

runs the Desk on its recorded answers (`desk/fixtures`) — Node only, no Rust,
no Docker.

## Next steps

| Topic | Guide |
|---|---|
| Configuration findings and the deployment checklist | [Einrichtung](@/docs/guide/einrichtung.md) |
| EDIFACT parsing and validation | [Parsing guide](@/docs/reference/parsing.md) |
| ERP integration — CloudEvents, HMAC | [ERP integration](@/docs/architecture/erp-integration.md) |
| makod operator reference | [makod guide](@/docs/services/makod.md) |
| marktd operator reference | [marktd guide](@/docs/services/marktd.md) |
| processd — NB STP + LF answer automation | [processd guide](@/docs/services/processd.md) |
| INVOIC plausibility, § 147 AO / GoBD | [invoicd guide](@/docs/services/invoicd.md) |
| Energy data, imbalance, billing periods | [edmd guide](@/docs/services/edmd.md) |
| Process read model, § 7a Abs. 5 EnWG parity | [processd guide — process read model](@/docs/services/processd.md#process-read-model) |
| Full system architecture | [Architecture](@/docs/architecture/_index.md) |
| Process catalogue (GPKE, WiM, …) | [Processes](@/docs/reference/processes.md) |
| Building, testing and the gate | [CONTRIBUTING.md](https://github.com/hupe1980/mako/blob/main/CONTRIBUTING.md) |

# netzbilanzd

`netzbilanzd` is the Netzbetreiber's outbound billing daemon. It settles,
checks and dispatches the INVOICs a network operator issues — Netznutzung and
Konzessionsabgabe (31001/31002), Mehr-/Mindermengen (31005/31006), MSB-Rechnung
as MSB (31009) and the GeLi Gas Sperrprozess fees (31011) — closes each on the
returning REMADV, computes the Redispatch 2.0 Kostenblätter and files the MaBiS
Summenzeitreihen (MSCONS 13003) with the BIKO through `makod`.

| | |
|---|---|
| **Port** | 8680 (`deploy/ports.toml`) |
| **Database** | PostgreSQL — invoice drafts and numbering, Kostenblätter, MaBiS submission runs |
| **MCP** | 15 tools, 7 prompts at `/mcp` |
| **Emits** | `de.netzbilanz.invoic.*`, `de.netzbilanz.kostenblatt.*`, `de.mabis.submission.failed`, `de.mabis.korrekturbedarf.opened` |
| **Consumes** | REMADV CloudEvents at `POST /api/v1/webhooks/remadv` |
| **Depends on** | `marktd` (MaLo, MaBiS-Zählpunkte, KAV Grenzpreis Strom), `makod` (dispatch), `edmd` (energy, MaBiS Lastgänge) |

## Run

```bash
just dev    # the whole estate, netzbilanzd on deploy/demo/netzbilanzd.toml, seeded
NETZBILANZD_CONFIG=deploy/demo/netzbilanzd.toml cargo run -p netzbilanzd -- --check-config
```

## API

| Route | Purpose |
|---|---|
| `POST /api/v1/billing/run` | settle up to 1 000 positions into drafts, one transaction |
| `GET /api/v1/billing/drafts` (+ `/{id}`) | drafts and their check outcome |
| `PUT /api/v1/billing/drafts/{id}/dispatch` · `/reject` · `/mark-paid` · `/mark-disputed` | draft lifecycle |
| `POST /api/v1/billing/drafts/{id}/storno` · `/korrektur` | reverse or correct a dispatched invoice |
| `POST /api/v1/billing/mmm-run/{malo_id}`, `/ggv-nne/{ggv_malo_id}` | Mehr-/Mindermengen, § 42b EnWG GGV |
| `/api/v1/redispatch/kostenblatt`, `/ausfallarbeit/*` | Redispatch 2.0 |
| `POST /api/v1/mabis/sync`, `GET /api/v1/mabis/runs` | MaBiS Summenzeitreihen (with `[mabis]`) |
| `POST /api/v1/webhooks/remadv` | inbound REMADV, HMAC-verified |
| `/api/v1/webhooks/endpoints` (+ `/deliveries`, `/replay`, `/test`), `GET /api/v1/events` | Webhook subscriptions and the event feed (`ADMIN`) |
| `GET /api/v1/openapi.json` | The OpenAPI document, committed as `openapi.json` |

The request body and its response are in the operator guide's
[Settling an invoice](https://hupe1980.github.io/mako/docs/services/netzbilanzd/#settling-an-invoice).
The KAV Grenzpreis for a Strom Sondervertragskunde is read from `marktd`'s
Referenzreihe `kav_grenzpreis_strom`; a Gas position states
`grenzpreis_ct_per_kwh`.

## Configuration

`tenant`, `marktd_url`/`marktd_api_key`, `makod_url`/`makod_api_key`,
`edmd_url`/`edmd_api_key`, `inbound_secret`, `[database] url`, `[oidc]` (or
`allow_insecure_no_auth = true` in dev) and `zahlungsziel_werktage` — required,
at least 10 (GPKE Teil 2 Kap. 3.3.2 Nr. 1, S. 90). `[mabis]` (`sender_mp_id`,
`receiver_mp_id`, a Y-type EIC `bilanzierungsgebiet_id`) switches the BIKO
filing on. `erp_webhook_url`/`erp_webhook_secret` name the webhook endpoint
`default`; `[subscriptions]` sets `retention_days` and, for `just dev`,
`allow_loopback`. netzbilanzd has no `[policy]` section.

The [operator guide](https://hupe1980.github.io/mako/docs/services/netzbilanzd/)
covers Umsatzsteuer, § 14a EnWG, Abschläge, corrections, Mehr-/Mindermengen,
GGV, Redispatch, MaBiS and the full configuration reference.

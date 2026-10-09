# invoicd

`invoicd` (`:8280`) checks every INVOIC a market partner sends this deployment
(the Lieferant by default), records it as a Buchungsbeleg (§ 147 AO) and answers
the counterparty — accept or dispute — through `makod`. It also issues the
self-issued Mehrmengen-Rechnung (PID 31006).

## Run it

```bash
just dev invoicd       # dev/config/invoicd.toml against the dev PostgreSQL
```

| | |
|---|---|
| **Inbound** | `de.mako.process.initiated` from `marktd` (HMAC-signed `POST /webhook`) |
| **Outbound** | answer commands to `makod`; `de.invoic.*` CloudEvents to the ERP |
| **Invariants** | the receipt is persisted before the answer is sent; an event that cannot become a receipt goes to `invoic_dlq` with its reason |
| **MCP** | 7 read-only tools and 4 prompts at `/mcp` |

## PID routing

One pipeline; the differences are a table in `src/routing.rs`.

| PID | Meaning | Check | Answer commands |
|---|---|---|---|
| 31001 | Abschlagsrechnung Netznutzung | `PreisblattNetznutzung` | `gpke.abrechnung.*` |
| 31002 | NN-Rechnung (both Sparten) | `PreisblattNetznutzung` | `gpke.abrechnung.*` |
| 31003 | WiM-Rechnung — Dienstleistungen im Messwesen, **beide Sparten** | `PreisblattMessung` + AufAbschlag | `wim.rechnung.*` |
| 31004 | Stornorechnung — Sparte-neutral, any process | arithmetic only | `invoic.stornorechnung.*` |
| 31005 | MMM-Rechnung Strom | + MMM Strom prices | `gpke.abrechnung.*` |
| 31006 | MMM Mehrmenge, selbst ausgestellt | + MMM Strom prices | `gpke.abrechnung.*` |
| 31007 | GaBi Gas MMM-Rechnung | + MMM Gas prices (THE) | `gabi.rechnung.*` |
| 31008 | GaBi Gas MMM, selbst ausgestellt | + MMM Gas prices (THE) | `gabi.rechnung.*` |
| 31009 | WiM MSB-Rechnung | `PreisblattMessung` + AufAbschlag | `wim.rechnung.*` |
| 31011 | Rechnung sonstige Leistung — Sparte-neutral (GPKE Teil 2 · AWH Sperrprozesse Gas) | `PreisblattNetznutzung` | `invoic.sonstige-leistung.*` |

A PID with no route is never answered with a default command. 31010
(Kapazitätsrechnung, NB → Kapazitätsnutzer — INVOIC AHB 1.0b Kap. 3.1.4) is
refused into `invoic_dlq`: no answer command permits that role.

31003 bills the
abgebender MSB's Dienstleistungen im Messwesen in both Sparten and prices
against `PreisblattMessung`. A Rechnung flagged `ist_storno` takes the
arithmetic-only check whatever its PID. Command names come from
`mako_markt::commands`; `cargo xtask check-answer-commands` holds the table to
`makod`'s registry.

## Outcomes

| Outcome | Meaning |
|---|---|
| `Ok` | accepted |
| `AcceptedPartial` | Storno accepted on the reduced check |
| `Warn` | warnings, auto-approved below `auto_dispute_threshold_eur` |
| `Dispute` | rejected; the answer carries the findings as `ablehnungsgrund` |
| `Resolved` | dispute closed by an operator |
| `Dispatched` | self-issued document sent; awaiting the NB's REMADV |

A `Warn` escalates to `Dispute` only above `auto_dispute_threshold_eur`
(default `0.0`: every warning is approved).

## API

| Method | Path | Cedar action |
|---|---|---|
| `POST` | `/webhook` | — (HMAC from `marktd`) |
| `GET` | `/api/v1/receipts` · `/receipts/{id}` · `/receipts/{id}/rechnung` | `read-receipt` |
| `GET` | `/api/v1/disputes` | `read-disputes` |
| `GET` | `/api/v1/overdue-remadv` | `read-overdue-remadv` |
| `GET` | `/api/v1/zahlungsstatus/{malo_id}` | `read-receipt` |
| `POST` | `/api/v1/receipts/{id}/confirm-payment` | `write-receipt` |
| `POST` | `/api/v1/receipts/{id}/dispatch-answer` | `write-receipt` |
| `POST` | `/api/v1/receipts/{id}/resolve-dispute` | `write-receipt` |
| `POST` | `/api/v1/selbstausstellen` | `dispatch-selbstausstellen` |
| `GET` | `/invoicd/metrics` | — (internal) |

Write actions are restricted to the `LF` role (`policies/invoicd.cedar`).
`/webhook` answers `204` once an event is finished with — answered, deliberately
unanswered or dead-lettered — and `503` when the reference data (Preisblatt,
Mehr-/Mindermengenpreise, ESA-Angebot) could not be read from `marktd`, so the
sender redelivers. `POST /api/v1/selbstausstellen` takes `malo_id`, `nb_mp_id`,
`year`, `month` and `bilanziert_kwh`; the measured half comes from `edmd`.

## Configuration

```toml
# invoicd.toml
[http]
addr = "0.0.0.0:8280"

[database]
url = "env:DATABASE_URL"

[identity]
tenant     = "9900357000004"
# The Marktrolle this deployment *receives* invoices in: lieferant (default),
# netzbetreiber or esa. PID 31009 carries five Use-Cases and the message names
# none; this is half the lookup and `IMD+7081` is the other half.
marktrolle = "lieferant"

[makod]
url     = "http://makod:8080"
api_key = "env:INVOICD_MAKOD_API_KEY"

[marktd]
url     = "http://marktd:8180"
api_key = "env:INVOICD_MARKTD_API_KEY"

[webhook]
inbound_secret = "env:INVOICD_INBOUND_SECRET"

[subscription]
webhook_url = "http://invoicd:8280/webhook"

[check]
arithmetic_tolerance       = 0.01   # relative; 0.01 = 1 %
total_tolerance            = 0.01
tariff_tolerance           = 0.03
require_tariff             = false  # true: a missing PRICAT entry disputes
auto_dispute_threshold_eur = 0.0    # 0.0 = never escalate a Warn
max_zahlungsziel_days      = 30     # § 7 Allgemeine Festlegungen V6.1d

# Required only for POST /api/v1/selbstausstellen.
[edmd]
url     = "http://edmd:8380"
api_key = "env:INVOICD_EDMD_API_KEY"

[erp]
webhook_url = "https://erp.example.com/webhooks/invoicd"
hmac_secret = "env:INVOICD_ERP_HMAC_SECRET"
```

## Tests

`cargo test -p invoicd` runs the unit and policy tests; `just test-invoicd-db`
runs the real-PostgreSQL suite (`tests/it/receipts_pg.rs`, needs Docker).

The [operator guide](https://hupe1980.github.io/mako/docs/services/invoicd/)
covers the checks, ERP delivery, metrics and the full configuration reference.

# invoicd — INVOIC Plausibility Check & Settlement (LF role)

Checks every INVOIC a market partner sends the Lieferant, records it, and
answers the counterparty — accept or dispute — through `makod`.

| | |
|---|---|
| **Port** | `:8280` |
| **Inbound** | `de.mako.process.initiated` from `marktd` (HMAC-signed webhook) |
| **Outbound** | accept/reject commands to `makod`; `de.invoic.*` CloudEvents to the ERP |

```text
marktd ──POST /webhook──► routing::route_for(pid) ──► check
                              │                        │
                        marktd price sheets ───────────┘
                              │
                    persist receipt (§ 147 AO)
                              │
                     makod ◄──answer command
                              │
                       ERP webhook ◄── de.invoic.receipt.*
```

## Two invariants

**Persist before dispatch.** A received INVOIC is a Buchungsbeleg (§ 147 Abs. 3
AO, § 14b UStG, 8-year retention). The receipt is written before the answer is
sent, and a failed write aborts the dispatch rather than answering an invoice
that is not in the audit trail.

**Nothing is dropped.** An event that cannot become a receipt — no message
reference, an unparseable Rechnung, a `makod` that cannot supply one — goes to
`invoic_dlq` with the reason. `invoicd_dlq_open_total` counts them.

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

A PID with no route is ignored, never answered with a default command. The
subscription PID filter is derived from the same table.

31003 is **not** a Gas Netznutzungsrechnung: it bills the Dienstleistungen im
Messwesen the abgebender MSB rendered — temporäre Fortführung, Geräteübernahme,
Zwischen- oder Kontrollablesung — in both Sparten, so it prices against
`PreisblattMessung` for the same reason 31009 does. There is no `wim.gas.*`
command; `wim.rechnung.annehmen` carries `Gnb` among its permitted roles because
the Gas NB is a payer of 31003.

### The answer commands are named, not spelled

`makod` rejects an unknown command name with HTTP 422, so a route naming one it
does not register fails only when a real invoice arrives — the check runs, the
verdict is persisted, the dispatch fails, and the Antwortfrist expires on a
process that looked healthy.

The table therefore names constants from `mako_markt::commands` instead of
writing the wire name twice. They are listed in `DISPATCHED_BY_SERVICES`, which
`makod`'s registry test asserts against; `cargo xtask check-answer-commands`
closes the loop from this side.

Strom Mehr-/Mindermengenpreise are **one nationwide monthly BDEW series**
(§ 13 Abs. 3 StromNZV, GPKE Teil 1 Kap. 8.4 from 01.01.2026), so the application
month is the whole key. Gas prices are per Marktgebiet — Trading Hub Europe is
the single German MGV.

A Rechnung flagged `ist_storno` takes the arithmetic-only check whatever its
PID: it carries the original's amounts negated, and a tariff comparison would
dispute every line.

## Outcomes

| Outcome | Meaning |
|---|---|
| `Ok` | accepted |
| `AcceptedPartial` | Storno accepted on the reduced check |
| `Warn` | warnings, auto-approved below `auto_dispute_threshold_eur` |
| `Dispute` | rejected; the answer carries the findings as `ablehnungsgrund` |
| `Resolved` | dispute closed by an operator |
| `Dispatched` / `Paid` | self-issued document sent / settled |

A `Warn` escalates to `Dispute` only when the invoice net total exceeds
`auto_dispute_threshold_eur`. `0.0` (the default) approves every warning.

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

Write actions are restricted to the `LF` role. `tests/cedar_actions.rs` pins the
actions the handlers check against `policies/invoicd.cedar` in both directions —
Cedar is deny-by-default, so an unlisted action is a permanent 403.

`dispatch-answer` re-sends the answer for a receipt whose automatic dispatch
failed, using the receipt's stored INVOIC message reference and the answering
PID's own command.

## Self-issued Mehrmengen-Rechnung (PID 31006)

`POST /api/v1/selbstausstellen` settles one Bilanzierungsmonat:

```json
{ "malo_id": "51238696012", "nb_mp_id": "9900357000004",
  "year": 2026, "month": 6, "bilanziert_kwh": "12500.000" }
```

The measured half comes from `edmd GET /api/v1/imbalance/{malo}/{y}/{m}`; the
balanced half is the caller's, because `edmd` measures and does not balance. The
prices come from `marktd`, and a month with none is refused rather than settled
against a neighbouring month's.

The document is built by `grid_billing::settle_mmm` with `selbstausgestellt`, so
the rendered BO4E states `netznutzungrechnungsart = Selbstausgestellt` and
`netznutzungrechnungstyp = Mehrmindermengenrechnung`. The receipt carries
`makod`'s process id, so the answering REMADV, a later Storno and the payment
confirmation all find the same row.

## ERP notification

`de.invoic.receipt.settled` / `.disputed` / `.dispatched` on every checked
invoice, and `de.invoic.payment.overdue` when a Zahlungsziel passes without
`confirm-payment`.

Delivery is durable at-least-once: the handler tries inline, and the outbox
worker retries with 30 s → 5 min → 30 min → 2 h backoff to a cap of 5 attempts.
A `4xx` is dead-lettered immediately. Batches are claimed with a lease, so
replicas do not double-deliver.

Without `[erp] webhook_url` the events are recorded and nothing delivers them;
the service warns at startup.

## Metrics — `GET /invoicd/metrics`

All tenant-scoped.

| Gauge | Alert when |
|---|---|
| `invoicd_receipts_total` | — |
| `invoicd_disputes_total` | rising against a single counterparty |
| `invoicd_overdue_remadv_total` | `> 0` — an unanswered invoice past its Zahlungsziel |
| `invoicd_erp_dead_lettered_total` | `> 0` — the ERP is not hearing about settled invoices |
| `invoicd_dlq_open_total` | `> 0` — an unprocessed Buchungsbeleg |
| `invoicd_receipts_by_pid_outcome` | — |

## Configuration

```toml
# invoicd.toml
[http]
addr = "0.0.0.0:8280"

[database]
url = "env:DATABASE_URL"

[identity]
tenant = "9900357000004"

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

## MCP server

`/mcp` (Streamable HTTP), read-only: `get_receipt`, `list_disputes`,
`get_check_result`, `list_overdue_remadv`, `get_zahlungsstatus`,
`summarize_billing_month`, `list_exceptions`. Prompts: `resolve-dispute`,
`check-overdue-remadv`, `monthly-billing-review`, `detect-systematic-errors`.

## Tests

`cargo test -p invoicd` runs the unit and policy tests. The real-PostgreSQL
suite (`tests/receipts_pg.rs`) needs a Docker daemon and skips without one:

```sh
DOCKER_HOST=unix://$HOME/.docker/run/docker.sock cargo test -p invoicd
```

# netzbilanzd

`netzbilanzd` (`:8680`) is the Netzbetreiber's outbound billing daemon. It
settles, checks and dispatches the invoices a network operator owes its
counterparties — Netznutzung and Konzessionsabgabe, Mehr-/Mindermengen,
Messstellenbetrieb and the GeLi Gas Sperrprozess fees — computes the Redispatch
2.0 cost sheets, closes the payment lifecycle when the REMADV comes back, and
files the Netzbetreiber's MaBiS Summenzeitreihen with the BIKO.

## Run it

```bash
just dev netzbilanzd   # dev/config/netzbilanzd.toml against the dev PostgreSQL
```

## At a glance

| Attribute | Value |
|---|---|
| **Database** | PostgreSQL — `invoice_drafts`, `invoice_number_seq`, `abschlag_verrechnungen`, `kostenblatt_records`, `fremdkosten_records`; MaBiS: `submission_runs`, `submission_series`, `submission_malo_log`, `pruefmitteilung` |
| **Calculation** | `grid-billing` — pure; every position carries a `CalculationTrace` and its `LegalReference`s. Money is `i64` × 10⁻⁵ EUR, never `f64` |
| **Pre-dispatch gate** | `invoic-checker` on the document that will be sent; a `Dispute` verdict blocks it |
| **Lifecycle** | `draft` → `dispatched` → `paid` \| `disputed`; `draft` → `rejected`. Only a dispatched invoice can be reversed (Storno) or corrected (Korrektur) |
| **Numbering** | consecutive per tenant/series/year, allocated by the database (§ 14 Abs. 4 Nr. 4 UStG) |
| **Events** | `de.netzbilanz.invoic.{drafted,dispatched,paid,disputed,dispatch-overdue}`, `de.netzbilanz.kostenblatt.{computed,deadline-approaching}`, `de.mabis.{submission.failed,korrekturbedarf.opened}` — through the transactional outbox |
| **MCP server** | 12 **read-only** tools · 7 prompts at `/mcp` (Streamable HTTP) |

## What it issues

| PID | Document | Direction | `billing_type` |
|---|---|---|---|
| 31001 | Abschlagsrechnung Netznutzung (payment on account) | NB → LF | `abschlag` |
| 31002 | NN-Rechnung (Netznutzungsentgelt + Konzessionsabgabe) | NB → LF | `nne` |
| 31005 | Mehr-/Mindermengensaldo | NB → LF | `mmm` |
| 31009 | MSB-Rechnung (Messstellenbetrieb) | **MSB → NB / LF / ESA** | `msb` |
| 31011 | Rechnung sonstige Leistung (GeLi Gas AWH Sperrprozesse) | GNB → LFG | `gas_awh` |

The Sparte is not in the PID: NN-Rechnung Strom and Gas both use 31002, so every
position states its `sparte`. 31009 is issued by the MSB, stored as
`sender_mp_id`.

## Settling an invoice

Each position names one MaLo, one period and one settlement — a tagged union
keyed on `billing_type`, so a field of another kind is a `422`:

```bash
curl -X POST http://localhost:8680/api/v1/billing/run \
  -H "Content-Type: application/json" \
  -d '{
    "invoice_date": "2026-02-01",
    "due_date": "2026-03-03",
    "rechnungskreis": "NNE",
    "positions": [{
      "malo_id": "51238696012",
      "period_from": "2026-01-01",
      "period_to": "2026-01-31",
      "settlement": {
        "billing_type": "nne",
        "nb_mp_id": "9900357000004",
        "lf_mp_id": "9900012345678",
        "sparte": "Strom",
        "arbeitspreis": {
          "Einheitlich": { "menge_kwh": "1500", "preis_ct_per_kwh": "3.5" }
        },
        "konzessionsabgabe": {
          "satz_ct_per_kwh": "0.11",
          "klasse": "Sondervertragskunde"
        },
        "netzebene": "Niederspannung"
      }
    }]
  }'
```

## Routes

| Area | Routes |
|---|---|
| Billing (`/api/v1/billing`) | `POST /run`, `GET /drafts` (+ `/{id}`), `PUT /drafts/{id}/dispatch` · `/reject` · `/mark-paid` · `/mark-disputed`, `POST /drafts/dispatch-batch`, `POST /drafts/{id}/storno` · `/korrektur`, `/fremdkosten/{draft_id}`, `POST /mmm-run/{malo_id}`, `POST /ggv-nne/{ggv_malo_id}`, `GET /malo/{malo_id}`, `/summary`, `/audit` |
| Redispatch (`/api/v1/redispatch`) | `/kostenblatt` (+ `/{activation_id}`, `/compute`, `/gaps/{year}/{month}`, `/submit/{year}/{month}`), `POST /verguetung/{activation_id}/compute`, `POST /ausfallarbeit/*` (stateless BilAReM engine) |
| MaBiS (`/api/v1/mabis`, with `[mabis]`) | `POST /sync`, `GET /runs` (+ `/{id}`), `PUT /runs/{id}/retry`, `POST /datenstatus` · `/pruefmitteilung`, `GET /korrekturbedarf` |
| Inbound | `POST /api/v1/webhooks/remadv` — HMAC-verified with `inbound_secret` |

## MaBiS Summenzeitreihen

With a `[mabis]` section, `netzbilanzd` aggregates per-MaLo Lastgänge from
`edmd` into one Bilanzierungsgebiets-Summenzeitreihe per territory on the
Berlin-local 15-minute grid and files each with the BIKO as MSCONS PID 13003
through `makod` (BK6-24-174 Anlage 3). Strom only — Gas balances through GaBi
Gas.

- **One filing per territory**, recorded in `submission_series` with its own
  `message_ref` (from `makod`'s `202`) or its failure reason; `submission_runs`
  keeps the aggregate.
- **Refuse rather than under-report.** A run fails when a discovered MaLo cannot
  be aggregated, a territory's grid still has empty slots, a territory has no
  MaBiS-Zählpunkt, or `edmd` reports its MaLo list `truncated`.
- **Acked is final.** A retry skips territories the BIKO already accepted for
  the Bilanzierungsmonat; a correction (`corrects_run_id`) re-files them under a
  higher version.
- **The BIKO assigns the Datenstatus** (inbound via IFTSTA); `netzbilanzd`
  records it and never derives one. The phase follows the Werktag calendar of
  § 3.10 Tabelle 2 (BG-SZR rows):

| Window | Werktage after period end | Datenstatus of a new version |
|---|---|---|
| Erstaufschlag (BKA) | 1.–10. WT | Abrechnungsdaten directly |
| Clearing (BKA) | 11.–30. WT | Prüfdaten, promoted by a positive Prüfmitteilung |
| KBKA | 31. WT – end of month 7 | Prüfdaten |

The scheduler fires at `run_hour_utc` on `erstaufschlag_werktag`, the last
Werktag of the Erstaufschlag window. `POST /api/v1/mabis/sync` needs
`trigger-mabis-run` (`NB`, `UENB`) and refuses a period with a live run; the two
BIKO-response routes need only `record-biko-response`.

## Configuration

```toml
# netzbilanzd.toml
tenant = "9900357000004"        # NB MP-ID / logical tenant name

marktd_url     = "http://marktd:8180"
marktd_api_key = "env:NETZBILANZD_MARKTD_API_KEY"
makod_url      = "http://makod:8080"
makod_api_key  = "env:NETZBILANZD_MAKOD_API_KEY"
edmd_url       = "http://edmd:8380"
edmd_api_key   = "env:NETZBILANZD_EDMD_API_KEY"

erp_webhook_url    = "http://erp:9000/webhooks/mako"
erp_webhook_secret = "env:NETZBILANZD_WEBHOOK_SECRET"

# Verifies inbound REMADV CloudEvents.
inbound_secret = "env:NETZBILANZD_INBOUND_SECRET"

# Alert workers. 0 disables either one.
dispatch_alert_interval_secs    = 3600
dispatch_stale_hours            = 48
kostenblatt_alert_interval_secs = 86400

[http]
addr = "0.0.0.0:8680"

[database]
url = "postgres://nb:secret@db:5432/netzbilanzd"

[mcp]
api_key = "env:NETZBILANZD_MCP_API_KEY"

# Verifies the bearer token on every REST route and on /mcp.
[oidc]
issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
audience = "api://mako-netzbilanzd"

# MaBiS Summenzeitreihen; absent, nothing is filed. Requires edmd_url.
[mabis]
sender_mp_id           = "9900357000004"     # MSCONS NAD+MS
receiver_mp_id         = "9900077000006"     # BIKO, MSCONS NAD+MR
bilanzierungsgebiet_id = "11YMAKO-TEST-01U"  # Y-type (Area) EIC

[mabis.schedule]
erstaufschlag_werktag = 10
run_hour_utc          = 5    # 06:00 CET / 07:00 CEST
```

Startup refuses to run without `[oidc]` and `inbound_secret` unless
`allow_insecure_no_auth = true`, refuses an unknown key anywhere, and refuses a
`[mabis]` whose `bilanzierungsgebiet_id` is not a Y-type EIC, whose
`submission_target` is `"mabis-hub"` (no published wire format), or that has no
`edmd_url`. DB suites: `just test-netzbilanzd-db`.

The [operator guide](https://hupe1980.github.io/mako/docs/services/netzbilanzd/)
covers Umsatzsteuer, § 14a, Abschläge, corrections, Mehr-/Mindermengen, GGV,
Redispatch, MaBiS and the full configuration reference.

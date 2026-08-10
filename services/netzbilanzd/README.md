# netzbilanzd — NNE/KA/MMM/MSB/AWH Billing Daemon (NB role)

`netzbilanzd` automates the complete outbound billing cycle for German network operators (NB, GNB):
generating, validating, and dispatching INVOIC messages for Netznutzungsentgelt (NNE),
Konzessionsabgabe (KA), Mehr-/Mindermengen (MMM), MSB-Rechnung, and GeLi Gas AWH Sperrprozesse.
Closes the payment lifecycle on REMADV receipt. Zero `f64` in the billing path.

| Attribute | Value |
|---|---|
| **Port** | `:8680` |
| **Database** | PostgreSQL (`invoice_drafts`, `kostenblatt_records`, `fremdkosten_records`) |
| **INVOIC PIDs** | 31002 (NN-Rechnung, NNE Strom + Gas) · 31005 (MMM Strom + Gas) · 31009 (MSB-Rechnung) · 31011 (AWH Sperrprozesse Gas) |
| **Calculation** | `grid_billing::settle_{nne,mmm,msb}()` — returns a `SettlementResult`. The service wraps it in an `InvoiceDocument` and renders via `into_rechnung()`. `EuroAmount` (`i64 × 10⁻⁵`), zero `f64` |
| **Pre-dispatch gate** | `invoic-checker` mandatory; `check_outcome = Dispute` blocks dispatch |
| **MMM prices** | Auto-fetches `mehr`/`minder` from `marktd` (Gas: THE monthly; Strom: ÜNB via `vnb_mp_id`) |
| **Draft lifecycle** | `draft` → `dispatched` → `paid` / `dispatched` ← `Dispute`; `rejected` unblocks re-billing |
| **REMADV handling** | 33001/33003/33004 → `paid`; 33002 → `Dispute` + `de.netzbilanz.invoic.disputed` CE |
| **§14a Modul 2** | HT/NT ToU split → 2 separate Arbeit positions (mandatory for controllable loads since 01.01.2024) |
| **§42b EnWG GGV** | Proportional or equal-split NNE for community solar tenant MaLos |
| **Redispatch 2.0** | `kostenblatt_records` + auto-compute from edmd; 15th-of-month alert worker |
| **BilAReM (BK6-23-241)** | Stateless Kap.-3 Ausfallarbeit compute endpoints (`POST /api/v1/redispatch/ausfallarbeit/compute` + `/ueberbauung`) over `mako_redispatch::ausfallarbeit` |
| **Background workers** | Hourly dispatch-overdue alert; daily Kostenblatt deadline alert |
| **CloudEvents emitted** | `de.netzbilanz.invoic.{drafted,dispatched,paid,disputed,dispatch_overdue}` · `de.netzbilanz.kostenblatt.deadline-approaching` |
| **MCP server** | 13 tools · 6 prompts at `/mcp` (Streamable HTTP 2025-11-25) |
| **Retention** | § 147 Abs. 3 AO / § 14b UStG — invoices are Buchungsbelege, 8 years; `GET /api/v1/billing/audit` for BNetzA export (up to 50k rows) |
| **Health** | `GET /health/live` · `GET /health/ready` |

## Billing types

| `billing_type` | PID | Direction | Description |
|---|---|---|---|
| `nne_strom` | 31002 | NB → LF | NN-Rechnung Strom, flat-rate or §14a Modul 2 ToU |
| `nne_gas` | 31002 | GNB → LFG | NN-Rechnung Gas |
| `mmm_strom` | 31005 | NB → LF | Mehr-/Mindermengensaldo Strom |
| `mmm_gas` | 31005 | GNB → LFG | Mehr-/Mindermengensaldo Gas (THE prices) |
| `msb_31009` | 31009 | **MSB → NB / LF / ESA** | MSB-Rechnung (Messstellenbetrieb). The MSB is the invoicer; set `msb_empfaenger_rolle` (`NB`/`LF`/`ESA`) and `msb_empfaenger_mp_id` to pick the recipient. |
| `nne_gas_awh_31011` | 31011 | GNB → LFG | AWH Sperrprozesse Gas (GeLi Gas 3.0 (BK7-24-01-009)) |

## Configuration

```toml
# netzbilanzd.toml
port            = 8680
tenant          = "9900357000004"   # NB MP-ID / logical tenant name

marktd_url      = "http://marktd:8180"
marktd_api_key  = "env:NETZBILANZD_MARKTD_API_KEY"

makod_url       = "http://makod:8080"
makod_api_key   = "env:NETZBILANZD_MAKOD_API_KEY"

edmd_url        = "http://edmd:8380"
edmd_api_key    = "env:NETZBILANZD_EDMD_API_KEY"

# ÜNB MP-ID for Strom MMM price auto-fetch (set to your Regelzone's ÜNB)
vnb_mp_id       = "9907324000007"   # 50Hertz / TenneT / Amprion / TransnetBW

# ERP webhook receives all de.netzbilanz.* CloudEvents. Delivery is durable:
# each event is written to `event_outbox` in the same transaction as the draft
# change (persist-before-dispatch) and drained by a background worker with
# retry + dead-letter — a crash never drops an event.
erp_webhook_url = "http://erp:9000/webhooks/mako"

[database]
url = "postgres://nb:secret@db:5432/netzbilanzd"

[mcp]
api_key = "env:NETZBILANZD_MCP_API_KEY"
```

## Quick start

```bash
# Generate and dispatch a standard NNE invoice
curl -X POST http://localhost:8680/api/v1/billing/run \
  -H "Content-Type: application/json" \
  -d '{
    "nb_mp_id": "9900357000004",
    "lf_mp_id": "9900012345678",
    "invoice_date": "2026-02-01",
    "due_date": "2026-03-03",
    "rechnungsnummer_prefix": "NNE-2026-01",
    "positions": [{
      "malo_id": "51238696780",
      "period_from": "2026-01-01",
      "period_to": "2026-01-31",
      "billing_type": "nne_strom",
      "arbeitsmenge_kwh": "1500.000",
      "arbeitspreis_ct_per_kwh": "3.500",
      "ka_satz_ct_per_kwh": "1.320"
    }]
  }'
# → {"draft_ids": ["550e8400-..."]}

# Review the draft
curl http://localhost:8680/api/v1/billing/drafts/550e8400-...

# Dispatch to makod (blocked if check_outcome == Dispute)
curl -X PUT http://localhost:8680/api/v1/billing/drafts/550e8400-.../dispatch
# → {"dispatch_ref": "..."}

# Monthly summary
curl "http://localhost:8680/api/v1/billing/summary?year=2026&month=1"

# BNetzA § 147 AO / GoBD audit export
curl "http://localhost:8680/api/v1/billing/audit?from=2026-01-01&to=2026-01-31"
```

## MCP server (AI tooling)

The MCP server at `/mcp` exposes 13 tools for billing automation and compliance monitoring.
Useful prompts: `nb-invoic-overview`, `mmm-monthly-run`, `investigate-dispute`,
`ggv-nne-billing`, `redispatch-monthly-submit`.

## See also

- [Operator guide](https://hupe1980.github.io/mako/docs/services/netzbilanzd/) — full API reference, configuration, diagrams
- [`grid-billing`](../../crates/grid-billing/README.md) — pure billing calculation library
- [`invoic-checker`](../../crates/invoic-checker/README.md) — pre-dispatch plausibility gate

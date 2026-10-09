# netzbilanzd

`netzbilanzd` (`:8680`) is the Netzbetreiber's outbound billing daemon. It
settles, checks and dispatches the invoices a network operator owes its
counterparties — Netznutzung and Konzessionsabgabe, Mehr-/Mindermengen,
Messstellenbetrieb and the GeLi Gas Sperrprozess fees — computes the Redispatch
2.0 cost sheets, and closes the payment lifecycle when the REMADV comes back.

## Run it

```bash
just dev netzbilanzd   # dev/config/netzbilanzd.toml against the dev PostgreSQL
```

## At a glance

| Attribute | Value |
|---|---|
| **Database** | PostgreSQL — `invoice_drafts`, `invoice_number_seq`, `abschlag_verrechnungen`, `kostenblatt_records`, `fremdkosten_records` |
| **Calculation** | `grid-billing` — pure; every position carries a `CalculationTrace` and its `LegalReference`s. Money is `i64` × 10⁻⁵ EUR, never `f64` |
| **Pre-dispatch gate** | `invoic-checker` on the document that will be sent; a `Dispute` verdict blocks it |
| **Lifecycle** | `draft` → `dispatched` → `paid` \| `disputed`; `draft` → `rejected`. Only a dispatched invoice can be reversed (Storno) or corrected (Korrektur) |
| **Numbering** | consecutive per tenant/series/year, allocated by the database (§ 14 Abs. 4 Nr. 4 UStG) |
| **Events** | `de.netzbilanz.invoic.{drafted,dispatched,paid,disputed,dispatch-overdue}`, `de.netzbilanz.kostenblatt.{computed,deadline-approaching}` — through the transactional outbox |
| **MCP server** | 8 **read-only** tools · 6 prompts at `/mcp` (Streamable HTTP) |

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
| Inbound | `POST /api/v1/webhooks/remadv` — HMAC-verified with `inbound_secret` |

## Configuration

```toml
# netzbilanzd.toml
port   = 8680
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

[database]
url = "postgres://nb:secret@db:5432/netzbilanzd"

[mcp]
api_key = "env:NETZBILANZD_MCP_API_KEY"

# Verifies the bearer token on every REST route and on /mcp.
[oidc]
issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
audience = "api://mako-netzbilanzd"
```

Startup refuses to run without `[oidc]` and `inbound_secret` unless
`allow_insecure_no_auth = true`.

The [operator guide](https://hupe1980.github.io/mako/docs/services/netzbilanzd/)
covers Umsatzsteuer, § 14a, Abschläge, corrections, Mehr-/Mindermengen, GGV,
Redispatch and the full configuration reference.

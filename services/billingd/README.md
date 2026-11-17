# billingd

`billingd` is the Lieferant's billing engine. It prices a Marktlokation's
period from the product in `productd`, the consumption in `edmd` and the grid
pass-through costs in `marktd`, and issues the result as a BO4E `Rechnung` with
an EN 16931 model (XRechnung CII, PEPPOL UBL, ZUGFeRD via `outputd`). It knows
no grid topology, no EDIFACT and no customer master data.

| | |
|---|---|
| **Port** | 9280 (`deploy/ports.toml`) |
| **Database** | PostgreSQL — `billing_records`, `steuerzeilen`, `invoice_number_series`, `event_outbox` |
| **MCP** | 13 tools, 6 prompts at `/mcp` |
| **Emits** | `de.billing.rechnung.erstellt`, `de.billing.xrechnung.b2g.ready`, `de.billing.abrechnungsinformation.monatlich`, `de.vpp.settlement.berechnet` |
| **Consumes** | `de.vpp.dispatch.confirmed` at `POST /api/v1/webhooks/vpp-dispatch` |
| **Depends on** | `productd`, `edmd`, `marktd`, `vertragd`, `outputd`, `accountingd` |

## Run

```bash
just dev    # the whole estate, billingd on deploy/demo/billingd.toml, seeded
BILLINGD_CONFIG=deploy/demo/billingd.toml cargo run -p billingd -- --check-config
```

## API

| Route | Purpose |
|---|---|
| `POST /api/v1/billing/{malo_id}/calculate` · `/preview` · `/tarifwechsel` | bill a period, dry-run it, split it at a tariff change |
| `GET /api/v1/billing` · `/{id}` · `/review-queue` | records and the review queue |
| `POST /api/v1/billing/{id}/correction` | Storno, releasing the period for a fresh original |
| `POST /api/v1/billing/{id}/release` · `/reviewed` · `/versenden` | the risk gate's release, the review, dispatch |
| `GET /api/v1/billing/{id}/xrechnung` · `/ubl` · `/pdf` | XRechnung CII, PEPPOL UBL, ZUGFeRD PDF/A-3 |
| `POST /api/v1/billing/{id}/submit-b2g` | XRechnung CIUS check for a public-sector recipient |
| `POST /api/v1/billing/sammelrechnung/{rahmenvertrag_id}` | B2B Sammelrechnung |
| `POST /api/v1/billing/ggv/{ggv_id}` | § 42b EnWG Gemeinschaftliche Gebäudeversorgung |
| `POST /api/v1/billing/vpp/{vpp_id}` | § 41e EnWG VPP settlement as a Gutschrift |

## Configuration

`tenant`; `productd_url`, `edmd_url`, `marktd_url` (required), `vertragd_url`,
`outputd_url`, `accountingd_url`; `seller_name` with `seller_vat_id` and/or
`seller_tax_number`; `erp_webhook_url` / `erp_hmac_secret`; `[database]`,
`[oidc]`. `[gebuehrenbescheid]` is required to bill a public-law Gebühr.
`[policy]` holds
[`behg_regime`](https://hupe1980.github.io/mako/docs/services/billingd/#policy-behg-regime)
and [`settlement_form`](https://hupe1980.github.io/mako/docs/services/billingd/#policy-settlement-form).

The [operator guide](https://hupe1980.github.io/mako/docs/services/billingd/)
covers the billing arithmetic, § 40–§ 40c EnWG compliance, the risk gate,
scheduled billing runs and the full configuration reference.

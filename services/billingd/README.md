# billingd

`billingd` (`:9280`) is mako's supplier-side billing engine. It prices a
Marktlokation's period from the product in `productd`, the consumption in
`edmd` and the grid pass-through costs in `marktd`, and issues the result as a
BO4E `Rechnung` with an EN 16931 model (XRechnung CII, PEPPOL UBL, ZUGFeRD via
`outputd`). It knows no grid topology, no EDIFACT and no customer master data.

## Run it

```bash
just dev billingd      # dev/config/billingd.toml against the dev PostgreSQL
```

## At a glance

| | |
|---|---|
| **Database** | PostgreSQL — `billing_records`, `invoice_number_series`, `event_outbox` |
| **Auth** | OIDC on every business route (fail-closed; `allow_insecure_no_auth` for dev) + Cedar `policies/billingd.cedar`: reads for any tenant caller, issuing/correcting/releasing for `LF`, `MSB`, `ESA` |
| **Products** | 13 categories, from STROM and GAS to WALLBOX, HEMS and SHARING |
| **Rechnungsnummer** | fortlaufend per tenant (§ 14 Abs. 4 Nr. 4 UStG): `RE-` invoice, `SR-` Sammelrechnung, `ST-` Storno, `VG-` § 41e Gutschrift |
| **Errors** | one envelope with a stable code — `{"error":{"code":"PERIOD_ALREADY_BILLED",…}}` |
| **Events** | CloudEvents to `erp_webhook_url` through the transactional outbox, HMAC-signed |
| **MCP** | 12 read-only tools at `/mcp` |

## Routes

| Route | Purpose |
|---|---|
| `POST /api/v1/billing/{malo_id}/calculate` · `/preview` · `/tarifwechsel` | bill a period, dry-run it, or split it at a tariff change |
| `GET /api/v1/billing` · `/{id}` · `/review-queue` | records, filtered in the query (`malo_id`, `lf_mp_id`, `outcome`, `category`, `is_correction`) |
| `POST /api/v1/billing/{id}/correction` | Storno + release of the period for a fresh original |
| `POST /api/v1/billing/{id}/release` · `/versenden` | release a record the risk gate held; dispatch it |
| `GET /api/v1/billing/{id}/xrechnung` · `/ubl` · `/pdf` | XRechnung 3.0 CII, PEPPOL BIS 3.0 UBL, ZUGFeRD PDF/A-3 (rendered by `outputd`) |
| `POST /api/v1/billing/{id}/submit-b2g` | XRechnung CIUS check, then `de.billing.xrechnung.b2g.ready` |
| `POST /api/v1/billing/sammelrechnung/{rahmenvertrag_id}` | B2B consolidated invoice |
| `POST /api/v1/billing/ggv/{ggv_id}` | § 42b EnWG Gemeinschaftliche Gebäudeversorgung |
| `POST /api/v1/billing/vpp/{vpp_id}`, `POST /api/v1/webhooks/vpp-dispatch` | § 41e EnWG VPP settlement as a Gutschrift |

A dynamic § 41a tariff is billed per market time unit or refused
(`SECT41A_NO_LASTGANG`, `SECT41A_MISSING_EPEX_PRICES`); a period whose tax or
BEHG rate is not in a dated series is refused, naming tax and year.

## Configuration

`dev/config/billingd.toml` is a working example.

- **Top level:** `tenant`; upstream URLs `productd_url`, `edmd_url`,
  `marktd_url`, `vertragd_url`, `outputd_url`; the § 40 Abs. 2 Nr. 1 EnWG
  supplier identity `seller_name` plus `seller_vat_id` and/or
  `seller_tax_number`; `erp_webhook_url` / `erp_hmac_secret`;
  `min_meter_coverage_pct`.
- **Tables:** `[http]` (`addr`), `[database]`, `[policy]` (`behg_regime` — `rvo_bezugspreis` or
  `korridor`; `settlement_form` — `endrechnung` or `restrechnung`), `[rates]`
  (`mwst_rate`, dated statutory rows), `[seller]` (BG-5/BG-6 address),
  `[e_rechnung]`, `[billing_runs]`, `[oidc]`, `[mcp]`. `--print-config` runs the
  same validation as startup and prints the policy report; the report is also
  served at `GET /api/v1/policy` and by the MCP tool `get_policy`.

The [operator guide](https://hupe1980.github.io/mako/docs/services/billingd/)
covers the billing arithmetic, § 40–§ 40c EnWG compliance, the risk gate,
scheduled billing runs and the full configuration reference.

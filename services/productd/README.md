# productd

`productd` is the LF's product and tariff catalogue and the source of the
market price series its invoices derive from: EPEX Spot day-ahead (§ 41a EnWG)
and nEHS certificate prices (BEHG). It freezes the all-in priced
Endkunden-Angebot an online order cites, runs B2B quotations and publishes the
§ 41c EnWG comparison feed. It knows what a product code costs on a date, not
who is on it — that assignment lives in `vertragd`.

| | |
|---|---|
| **Port** | 9080 (`deploy/ports.toml`) |
| **Database** | PostgreSQL, `public` schema — `products`, `epex_prices`, `nehs_prices`, `angebote`, `endkunden_angebote` (append-only) |
| **MCP** | 16 tools, 3 prompts at `/mcp` |
| **Emits** | `de.mako.tarif.product.updated` to every webhook subscriber; `de.mako.tarif.angebot.angenommen` to `erp_webhook_url` (vertragd) — both through the transactional outbox |
| **Consumes** | none |
| **Depends on** | billingd (`[policy] behg_regime`), marktd (Preisblätter for an Endkunden-Angebot) |

The catalogue holds 14 categories: 13 are billable `energy_billing::Product`
kinds, and `BUNDLE` expands into its component products before billing sees it.

## Run

```bash
just dev    # the whole estate, productd on deploy/demo/productd.toml, seeded
PRODUCTD_CONFIG=deploy/demo/productd.toml cargo run -p productd -- --check-config
just test-productd-db
```

## API

| Route | Purpose |
|---|---|
| `GET /api/v1/products/{lf_mp_id}`, `PUT\|DELETE /{product_code}` (+ `/history`, `/energiemix`) | Catalogue; one row per price version, `DELETE` withdraws |
| `POST /api/v1/products/{lf_mp_id}/resolve` | Many `(code, as_of)` prices in one call — `billingd`'s read |
| `PUT /api/v1/epex-prices/{date}`, `GET /{date}/quarter-hourly`, `GET /{year}/{month}/average` | EPEX day-ahead series |
| `PUT /api/v1/nehs-prices/{date}`, `GET /nehs-prices/latest` | nEHS certificate prices |
| `POST /api/v1/products/{lf_mp_id}/endkunden-angebote`, `GET /api/v1/endkunden-angebote/{id}` | Frozen Endkunden-Angebot; a missing input refuses `422 eingabe_fehlt` |
| `/api/v1/angebote` (+ `/{id}`, `/comparison`, `/versenden`, `/annehmen`, `/ablehnen`, `/expire`) | B2B quotations |
| `GET /api/v1/comparison-feed`, `/comparison-feed/bo4e` | § 41c EnWG feed, public |
| `/api/v1/webhooks/endpoints` (+ `/deliveries`, `/replay`, `/test`), `GET /api/v1/events` | Webhook subscriptions and the event feed (`ADMIN`) |
| `GET /api/v1/openapi.json` | The OpenAPI document, committed as `openapi.json` |

## Configuration

`tenant`, `lf_mp_id` (defaults to `tenant`), `billingd_url` (without it a Gas
position in a year § 10 BEHG fixes no price for is refused `BEHG_PREIS_FEHLT`),
`marktd_url` (without it the Endkunden-Angebot route answers `503`),
`erp_webhook_url` / `erp_hmac_secret` (the webhook endpoint `default`),
`[subscriptions]` (`retention_days`, `allow_loopback` for `just dev`),
`[database]`, `[oidc]`, `[mcp]`.
productd has no `[policy]` of its own; a `behg_regime` key is refused at
startup because billingd's
[`behg_regime`](https://hupe1980.github.io/mako/docs/services/billingd/#policy-behg-regime)
decides it.

The [operator guide](https://hupe1980.github.io/mako/docs/services/productd/)
covers the catalogue model, the price series, the Endkunden-Angebot, quotations,
the comparison feed and the full configuration.

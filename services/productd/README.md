# productd

`productd` (`:9080`) is the LF's product and tariff catalogue and the source of
the market price series its invoices derive from: EPEX Spot day-ahead (§ 41a
EnWG) and nEHS certificate prices (BEHG). It also runs B2B quotations (CPQ) and
publishes the § 41c EnWG comparison feed. `billingd` is its service reader;
which product a MaLo is on lives in `vertragd`.

## Run it

```bash
just dev productd      # dev/config/productd.toml against the dev PostgreSQL
```

## At a glance

| | |
|---|---|
| **Database** | PostgreSQL — `products`, `product_history`, `epex_prices`, `nehs_prices`, `angebote` |
| **Auth** | OIDC + Cedar `policies/productd.cedar`: reads for the tenant; setting prices and the Angebot lifecycle for LF, MSB, ESA, ADMIN. The two § 41c feed routes are public by statute |
| **Identities** | `tenant` is the data-isolation key, `lf_mp_id` the market identity products are sold under (defaults to `tenant`) |
| **Versioning** | one row per price version, `[valid_from, valid_to]`, no overlap (GiST); `PUT` of a future version end-dates the running one; `DELETE` withdraws, never deletes (§ 147 AO) |
| **EPEX** | keyed on the UTC start of the market time unit; 15-minute MTUs since delivery day 1 Oct 2025 |
| **MCP** | 13 tools + 3 prompts at `/mcp` |

## Routes

| Area | Routes |
|---|---|
| Catalogue | `GET /api/v1/products/{lf_mp_id}`, `PUT`/`DELETE /{product_code}`, `/history`, `/energiemix`, `POST /products/{lf_mp_id}/resolve` (many `(code, as_of)` in one call; `null` in place for an unpriceable leg) |
| Market prices | `PUT /api/v1/epex-prices/{date}`, `GET /epex-prices/{date}/quarter-hourly`, `/epex-prices/{year}/{month}/average`, `PUT /nehs-prices/{date}`, `GET /nehs-prices/latest` |
| § 41c feed | `GET /api/v1/comparison-feed`, `/comparison-feed/bo4e` — `PUBLISHED` products of the comparison categories only |
| Angebote | `/api/v1/angebote` (+ `/{id}`, `/comparison`, `/versenden`, `/annehmen`, `/ablehnen`, `/expire`) — `ANGELEGT → VERSANDT → ANGENOMMEN \| ABGELEHNT \| ABGELAUFEN`; an unpriceable position refuses the whole quotation (`422`, `unpreisbare_positionen`) |

Acceptance emits `de.tarif.angebot.angenommen`; `vertragd` creates the
Rahmenvertrag from the accepted variant.

## Configuration

```toml
# productd.toml
tenant   = "9900357000004"   # data-isolation key
lf_mp_id = "9900357000004"   # market identity; defaults to tenant

erp_webhook_url = "http://erp:8000/events"
erp_hmac_secret = "env:PRODUCTD_ERP_HMAC_SECRET"

# Which law prices nEHS certificates in a year § 10 BEHG fixes no price for
# (2027): "rvo_bezugspreis" (default) or "korridor" (55–65 EUR/t) — the same
# setting billingd bills under, so a Gas quotation and its invoice agree.
behg_regime = "rvo_bezugspreis"

[http]
addr = "0.0.0.0:9080"

[database]
url = "postgresql://productd:secret@db:5432/productd"

[oidc]
issuer   = "https://auth.example.de/realms/mako"
audience = "productd"

# billingd reads the catalogue and the price series with a service key:
# [[oidc.service_keys]]
# secret = "env:PRODUCTD_BILLINGD_KEY"
# sub    = "billingd"

[mcp]
api_key = "env:PRODUCTD_MCP_API_KEY"
```

DB suite: `just test-productd-db`.

The [operator guide](https://hupe1980.github.io/mako/docs/services/productd/)
covers the catalogue model, the price series, quotations and the full
configuration reference.

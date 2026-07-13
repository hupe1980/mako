# tarifbd — Product & Tariff Catalog

`tarifbd` is the single source of truth for **everything the LF sells** to end customers.
`billingd` and `portald` query it exclusively — `marktd` is never used for retail pricing.

| Feature | Detail |
|---|---|
| **HTTP port** | `:9080` |
| **Database** | PostgreSQL (products, customer_products, epex_prices) |
| **Auth** | OIDC/JWT + Cedar ABAC |
| **Product categories** | 12: STROM, GAS, WAERME, SOLAR, EEG, EINSPEISUNG, WAERMEPUMPE, WALLBOX, HEMS, EMOBILITY, ENERGIEDIENSTLEISTUNG, BUNDLE |
| **BO4E validation** | `Tarifpreisblatt` validated on PUT — `_typ`, `sparte`/`tariftyp`/`kundentypen`/`registeranzahl`/`berechnungsparameter` enums; 34-value `preistyp` whitelist |
| **Energiemix** | `PUT/GET/DELETE /api/v1/products/{lf}/{code}/energiemix` — §42 EnWG Herkunftsnachweis |
| **EPEX Spot** | `epex_prices` table (hourly ct/kWh); `PUT /api/v1/epex-prices/{date}` import; `GET /api/v1/epex-prices/{date}/hourly` |
| **MaLo→product** | `GET/PUT /api/v1/customer/{malo_id}/product` — current product assignment |
| **Version history** | `GET /api/v1/products/{lf}/{code}/history` — full audit log of price changes |
| **Angebote** | `POST/GET /api/v1/angebote` — B2B formal quotation workflow (ANGELEGT→VERSANDT→ANGENOMMEN); auto-expires stale quotes |
| **Health** | `GET /health/live`, `GET /health/ready` |

## Configuration

```toml
# tarifbd.toml
database_url = "postgresql://tarifbd:secret@db:5432/tarifbd"
port         = 9080
tenant       = "9900357000004"

[erp]
webhook_url = "http://erp:8000/events"
```

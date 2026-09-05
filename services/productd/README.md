# productd — Product & Tariff Catalog

`productd` is the single source of truth for **everything the LF sells** to end
customers, and for the market prices its invoices are derived from.
`billingd` is its only service-to-service reader — `marktd` is never used for
retail pricing.

| | |
|---|---|
| **HTTP port** | `:9080` |
| **Database** | PostgreSQL (`products`, `product_history`, `epex_prices`, `nehs_prices`, `angebote`) |
| **Auth** | OIDC/JWT + Cedar ABAC (`policies/productd.cedar`) on every route, plus a per-handler `lf_mp_id == claims.tenant()` check and a tenant-scoped SQL read. The two § 41c EnWG comparison feeds take no `Claims` at all and are public by statute |
| **MCP** | 13 tools + 3 prompts at `/mcp` |
| **Health** | `GET /health/live`, `GET /health/ready` |

## Authentication is not authorization

A verified token says *who* is calling. `policies/productd.cedar` says what they
may do, and the two are separate decisions here for a specific reason: the
Tarifpreisblatt these routes store is what the next `billingd` run bills, and
nothing downstream re-checks it. `PUT /api/v1/products/{lf_mp_id}/{code}`
rewrites a live tariff's Arbeitspreis; `PUT /api/v1/epex-prices/{date}` does the
same for every § 41a dynamic tariff at once. The per-handler tenant check
answers "is this my tenant's catalogue?", which is not the same question.

| Action | Routes | Who |
|---|---|---|
| `read-product` | `GET /products/{lf_mp_id}[/{code}[/history\|/energiemix]]`, `POST /products/{lf_mp_id}/resolve` | any token of the tenant |
| `read-marktpreise` | `GET /epex-prices/…`, `GET /nehs-prices/latest` | any token of the tenant |
| `write-product` | `PUT`/`DELETE` of a product and its `/energiemix` | LF, MSB, ESA, ADMIN |
| `write-marktpreise` | `PUT /epex-prices/{date}`, `PUT /nehs-prices/{date}` | LF, MSB, ESA, ADMIN |
| `read-angebot` | `GET /angebote[/{id}[/comparison]]` | LF, MSB, ESA, ADMIN |
| `write-angebot` | `POST /angebote`, `PUT /angebote/{id}` | LF, MSB, ESA, ADMIN |
| `versenden-angebot` | `POST /angebote/{id}/versenden` | LF, MSB, ESA, ADMIN |
| `entscheiden-angebot` | `POST /angebote/{id}/annehmen`, `/ablehnen` | LF, MSB, ESA, ADMIN |
| `expire-angebote` | `POST /angebote/expire` | LF, MSB, ESA, ADMIN |
| `use-mcp` | the whole `/mcp` surface | LF, MSB, ESA, ADMIN |

Reads of the catalogue and the price series carry no role requirement: the
published subset of exactly that data already leaves the house unauthenticated
under § 41c, `billingd` resolves products with a narrow service credential, and
an auditor reading a price sheet moves no money. Everything that *sets* a price,
and the whole Angebot lifecycle — which names a prospect and quotes them a
bespoke price — is held to a market role. MSB and ESA are in that set because
the `ENERGIEDIENSTLEISTUNG` category is their catalogue too; naming LF alone
would lock an MSB deployment out of its own price sheet.

`tests/authorization_guard.rs` pins the shape: every routed handler extracts
`Claims` and calls `authorize`, the code's action set and the policy's match in
both directions, and the two § 41c routes are the only unauthenticated ones.

## Two identities, not one

`tenant` is the **data-isolation key**; `lf_mp_id` is the **market identity**
products are sold under and assignments are filed against. They are the same
string in a single-mandant install and different in a shared one, so
`lf_mp_id` defaults to `tenant` and is configured separately when it differs.

Conflating them filed every MaLo→product assignment under a party that does not
trade, where `billingd`'s lookup by the real MP-ID found nothing.

## Products are versioned in valid time

A product code has one row per price version, `[valid_from, valid_to]`
inclusive.

- **Scheduling a price change is one act.** `PUT` a version with next year's
  `valid_from` and the running version is end-dated to the day before. Requiring
  the operator to end-date it first would turn one act into two, with an
  unpriced gap whenever they forget.
- **Versions may not overlap** (`products_no_overlap`, GiST). Two versions in
  force on one day made `fetch_product`'s `ORDER BY valid_from DESC LIMIT 1`
  pick one and bill it, with nothing to say which.
- **`fetch_product` applies both bounds.** A withdrawn product stops pricing new
  periods immediately and still prices the past, which is what a Schlussrechnung
  for a closed period needs.
- `DELETE` is a withdrawal (`valid_to = today`), never a delete: § 147 AO keeps
  the basis of every invoice already issued.

## What productd is *not*

**Which product a customer is on does not live here.** Agreeing it is a
Tarifwechsel — a contract act governed by § 41 Abs. 5 EnWG and the contract's
Preisgarantie — so the valid-time MaLo→product assignment lives in `vertragd`,
with the contract, and is asked for at
`GET /api/v1/malo/{malo_id}/produkte`.

`productd` answers the other half — what a product **costs** on a given day:

```bash
# One version, by code and date. Both validity bounds are applied.
POST /api/v1/products/{lf_mp_id}/resolve
{ "anfragen": [ { "product_code": "STROM-H0-2026", "as_of": "2026-03-01" },
                { "product_code": "STROM-H0-2027", "as_of": "2026-03-15" } ] }
```

`billingd` bills a period as one leg per assignment slice, so it resolves every
leg's product in **one** round trip; asking per leg would be an N+1 on every
invoice, and two calls could disagree if the catalogue changed between them. A
code with no version valid on its date comes back as `null` **in place**, so the
caller can name which leg is unpriceable.

## Market price series

### EPEX Spot day-ahead (§ 41a EnWG)

Keyed on the **UTC start of the market time unit** — DST-safe and
resolution-agnostic. SDAC moved the day-ahead auction to 15-minute MTUs for
delivery day **1 October 2025**, so a delivery day has 96 quarter-hours (92 or
100 across a DST change). Legacy 60-minute source data is stored as 60-minute
rows and expanded on fetch.

```bash
PUT /api/v1/epex-prices/2026-03-15        { "prices": [ …96 values… ], "mtu_minutes": 15 }
GET /api/v1/epex-prices/2026-03-15/quarter-hourly
GET /api/v1/epex-prices/2026/03/average   # einsd: Marktprämie
```

### nEHS certificates (BEHG)

The CO₂ component of every Gas and Wärme invoice is derived from what the
supplier paid for its certificates (CO2KostAufG § 3), so a decimal slip here
mis-bills every gas customer at once and shows on no single invoice. § 10 BEHG
fixes enough to catch it at import (`src/behg.rs`):

| Phase | Period | Price | § 10 BEHG |
|---|---|---|---|
| **Einführungsphase** (`verkaufsphase`) | 2021–2025 | 25 / 30 / 30 / 45 / **55** EUR/t, checked exactly | Abs. 2 |
| **Versteigerung** (`auktion`) | 2026 | clearing price inside **55–65** EUR/t | Abs. 1, Abs. 2 |
| **Nachkauf** (`nachkauf`) | after the 2026 auctions | **68** EUR/t (Mehrmengenpreis) | Versteigerungsbedingungen |
| `manual` | any | plausibility only (5–500 EUR/t) | — |

68 EUR/t is the **Nachkauf** price, not a "Verkaufsphase" price — the
Verkaufsphase ended at 55 EUR/t in 2025. From 2027 § 10 Abs. 2 fixes no figures
(it defers to § 24 Abs. 2 Nr. 2), so nothing is asserted about those years.

```bash
PUT /api/v1/nehs-prices/2026-07-08   { "eur_per_t": "63.50", "source": "auktion" }
# 6.35 → 422 "der Zuschlagspreis 6.35 EUR/t liegt außerhalb des Preiskorridors
#             55–65 EUR/t für 2026 (§ 10 Abs. 2 BEHG)"
GET /api/v1/nehs-prices/latest?date=2026-08-01
```

## § 41c EnWG comparison feed

§ 41c EnWG obliges suppliers to let third parties operating independent
comparison tools use offer-relevant information **free of charge, in open data
formats**, for Haushaltskunden and Kleinstunternehmen consuming under
100 000 kWh a year. An obligation to publish that is discharged only behind a
bearer token is not discharged, so these two routes — and only these two — carry
no token and no Cedar action:

```bash
GET /api/v1/comparison-feed        # canonical form, ETag + pagination
GET /api/v1/comparison-feed/bo4e   # full BO4E Tarifinfo array
```

Only `PUBLISHED` products appear, and only from the comparison categories
(`STROM`, `GAS`, `WAERME`, `SOLAR`, `WAERMEPUMPE`, `WALLBOX`); a `DRAFT` is
staged and invisible to billing, the portal and the feed alike. That bound is
what makes the open route safe — there is no second credential to check here, so
the exemption's justification is *what leaves the house*, and
`tests/authorization_guard.rs` asserts the query still carries it.

## B2B Angebote (CPQ)

`ANGELEGT → VERSANDT → ANGENOMMEN | ABGELEHNT | ABGELAUFEN`, auto-expired daily
per tenant. Acceptance emits `de.tarif.angebot.angenommen`, and `vertragd`
creates the Rahmenvertrag and one Versorgungsvertrag per site from the **accepted
variant of the BO4E `Angebot`** — so what was quoted and what is contracted
cannot drift.

A supply point therefore has to carry what its registration needs:
`malo_id`, **`melo_id`** (a gas Lieferbeginn is filed with the
Zählpunktbezeichnung; a MaLo-ID is not one) and **`nb_mp_id`** (the UTILMD's
recipient). Without them the accepted quotation produced a contract nothing
could ever register.

The term is any positive number of months — a B2B term is negotiated, not chosen
from a list.

### What a quotation may quote

An Angebot is a binding offer, so pricing is all-or-nothing: a position that
cannot be priced refuses the whole quotation with 422 and
`unpreisbare_positionen`, naming the position index and the reason. Quoting the
sum of the positions that *did* resolve prices a five-site Rahmenvertrag for
four sites.

A position is refused when its product code does not resolve, when no
Preisstaffel covers the quantity asked about, when a Zweitarif product is quoted
without `jahresverbrauch_ht_kwh`/`jahresverbrauch_nt_kwh`, when a product with a
Leistungspreis is quoted without `leistung_kw`, or when a Gas position has no
CO₂ certificate price.

| Component | Where the figure comes from |
|---|---|
| Arbeitspreis, Grundpreis, Leistungspreis | the Preisstaffel that covers the position's own quantity — HT and NT against their own volumes |
| Stromsteuer | 2,05 ct/kWh (§ 3 StromStG) unless overridden |
| Energiesteuer Gas | 0,55 ct/kWh (§ 2 Abs. 3 Satz 1 Nr. 4 EnergieStG) unless overridden |
| BEHG | the nEHS price for the quotation's Stichtag; certificates are auctioned since 2026 (§ 10 Abs. 1 BEHG), so there is no fixed fallback |
| Umsatzsteuer | 19 % (§ 12 Abs. 1 UStG) or the product's own `mwst_rate_override`, per position |

`wiederverkaeufer_13b: true` states the offer without Umsatzsteuer: the
recipient is a Wiederverkäufer i.S.d. § 3g UStG and owes the tax
(§ 13b Abs. 2 Nr. 5 Buchst. b i.V.m. Abs. 5 UStG). Whether that holds is the
caller's assessment.

Jahreskosten are quoted on a 365-day year.

## Configuration

```toml
# productd.toml
port     = 9080
tenant   = "9900357000004"   # data-isolation key
lf_mp_id = "9900357000004"   # market identity; defaults to tenant

erp_webhook_url = "http://erp:8000/events"
erp_hmac_secret = "env:PRODUCTD_ERP_HMAC_SECRET"

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

## Tests

```bash
cargo test -p productd          # BEHG rules, BO4E projection, Tarifpreisblatt validation
just test-productd-db           # real PostgreSQL (testcontainers)
```

The real-PostgreSQL suite proves what lives in SQL: product versioning across a
scheduled price change, the no-overlap constraint, tenant scoping, retroactive
lookups and product withdrawal. The assignment's own slice arithmetic is proven
in `vertragd`, which owns it.

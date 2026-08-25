# billingd — Multi-Product Energy Billing Engine

`billingd` is a **pure calculation service**: no grid topology knowledge, no EDIFACT,
no customer management. It pulls product definitions from `productd`, consumption from
`edmd`, and grid pass-through costs from `marktd`.

| Feature | Detail |
|---|---|
| **HTTP port** | `:9280` |
| **Database** | PostgreSQL (billing_records) |
| **Authn** | OIDC/JWT on every business route — fail-closed at startup (`allow_insecure_no_auth` for dev) |
| **Authz** | Cedar ABAC (`policies/billingd.cedar`) on every business route: tenant isolation plus a market-role gate on issuing, correcting and releasing |
| **Product API** | `Product` typed enum — `#[serde(tag="category")]` deserialization from productd JSONB |
| **Categories** | 13: STROM, GAS, WAERME, WASSER, SOLAR, EEG, EINSPEISUNG, WAERMEPUMPE, WALLBOX, HEMS, EMOBILITY, ENERGIEDIENSTLEISTUNG, SHARING |
| **§41a EPEX dynamic** | 15-min Lastgang × 15-min EPEX day-ahead (SDAC MTU) → `STROM` dynamic category |
| **§41a iMSys guard** | Hard error when `dynamic_epex=true` and `MeteringMode != Imsys` — reachable now that the dynamic path resolves the meter reading |
| **§14a discount** | `ControllableLoadProvider` — Modul 1 (pauschale Reduzierung), Modul 2 (prozentuale Arbeitspreisreduzierung), Modul 3 (zeitvariable Netzentgelte, three Tarifstufen). Modul 2 and 3 are mutually exclusive per BK6-22-300 |
| **§42b EnWG GGV** | `POST /api/v1/billing/ggv/{ggv_id}` — Gebäudestromnutzung: Aufteilungsschlüssel per Abs. 2 Nr. 1, residual grid supply per Abs. 3, whole run in one transaction |
| **§42c Sharing** | `Product::Sharing(SharingProduct)` — community energy allocation credit via `EnergyShareProvider` |
| **Gas H2-blend** | `gasqualitaet` field on `GasMeterInput` — annotates Rechnung as `ZusatzAttribut` (per DVGW G 260, measured Brennwert already reflects blend) |
| **Gas RLM Leistungspreis** | `gas_leistungspreis_ct_per_kw_month` in `GasProduct` — demand charge for large gas customers |
| **Rechnungsnummer** | § 14 Abs. 4 Nr. 4 UStG **fortlaufende** number from a per-tenant counter (`invoice_number_series`): `RE-2026-000123` invoice, `SR-` consolidated, `ST-` Storno, `VG-` § 41e Gutschrift |
| **Storno + Neuberechnung** | `POST /api/v1/billing/{id}/correction` negates the original and marks it `cancelled`, which **releases the period** so the corrected amounts can be billed as a fresh original — with the next number of the series |
| **Errors** | One envelope, one stable code: `{"error":{"code":"PERIOD_ALREADY_BILLED","message":…,"record_id":…}}` |
| **Sammelrechnung** | `POST /api/v1/billing/sammelrechnung/{rv_id}` — B2B consolidated invoice for a Rahmenvertrag; whole run in one transaction, bundle scored by the risk gate |
| **EN 16931 rendering** | `en16931` + `en16931-formats` render XRechnung/CII and PEPPOL UBL from the stored `en16931_json` semantic model — mapped at bill time (`energy_billing::Invoice::to_en16931`) with **per-line VAT** (BT-151/152), not re-parsed from BO4E |
| **XRechnung 3.0** | `GET /api/v1/billing/{id}/xrechnung` — CII XML via `en16931-formats` |
| **PEPPOL BIS 3.0** | `GET /api/v1/billing/{id}/ubl` — UBL 2.1 XML. UBL and CII are the two permitted **syntaxes** of EN 16931, not a hierarchy: § 14 UStG and Directive 2014/55/EU require the norm and accept either |
| **ZUGFeRD PDF/A-3** | `GET /api/v1/billing/{id}/pdf` — the page and the CII XML in one file. billingd proves the payload and projects the view; **outputd** renders the operator's template and stamps the carrier, and the template hash it answers with is pinned to the record |
| **XRechnung B2G** | `POST /api/v1/billing/{id}/submit-b2g` — proves the model against the XRechnung 3.0 CIUS before writing, then emits `de.billing.xrechnung.b2g.ready` (§4a EGovG i.V.m. ERechV, mandatory to the Bund since 27.11.2020) |
| **§41e VPP** | `POST /api/v1/billing/vpp/{vpp_id}` + `POST /api/v1/webhooks/vpp-dispatch` — dispatch settlement as a **Gutschrift** to the flexibility provider |
| **§41a no-fallback** | A dynamic tariff is billed per market time unit or not at all: a period without Lastgang is refused (`SECT41A_NO_LASTGANG`), unpriced intervals hard-block (`SECT41A_MISSING_EPEX_PRICES`) |
| **Record filters** | `GET /api/v1/billing?malo_id=&lf_mp_id=&outcome=&category=&is_correction=` — every predicate runs in the query, never over a page already fetched |
| **MCP** | 11 **read-only** tools at `/mcp`: list/get/preview, `validate_tariff_config`, `explain_invoice_position`, `check_billing_anomaly` |
| **Health** | `GET /health/live`, `GET /health/ready` |

## Authorization

Authentication establishes *who* is calling. `policies/billingd.cedar` decides
what they may do, and every business route checks it before touching the
database:

| Action | Who |
|---|---|
| `read-billing`, `preview-billing` | any authenticated caller in the tenant |
| `run-billing`, `settle-flexibility`, `correct-billing`, `release-billing`, `submit-b2g` | `LF`, `MSB` or `ESA` |

Authentication alone is not enough here: without a policy, any token the OIDC
verifier accepts could reverse an invoice the customer has already received, or
release one the risk gate is deliberately holding back — which is the gate's
entire purpose. `einsd`, the analogous service on the feed-in side, gates the
same way.

`tests/authorization.rs` pins the decisions, including that the dev-mode
synthetic principal reaches every endpoint. `mako_roles` carries **market**
roles (NB/LF/MSB/ESA/UENB), not job functions, so a policy may only name those —
one naming an invented `BUCHHALTUNG` denies every caller.

The MCP surface is authenticated separately (`[mcp]`) and is read-only by
construction; the VPP dispatch webhook is HMAC-authenticated. Neither carries a
Cedar action.

Running billing and releasing a held invoice are one permission, not two:
separating them needs a job function no IdP here issues. `released_by` /
`released_at` make a release attributable.

## Record store integrity

Every write goes through `pg::insert_billing_record` with a named
`NewBillingRecord`, and the store — not the code around it — holds the rules:

- **`invoice_number_series (tenant, series, year)`** is the § 14 Abs. 4 Nr. 4
  UStG counter behind the *fortlaufende* number. Numbers read `RE-2026-000123`;
  a caller may state its own. Gaps are legal and expected: a number is allocated
  before the engine runs because it is the document's BT-1, so a refused
  calculation burns one (UStAE 14.5 Abs. 11 — no gapless sequence is required,
  only that no number is issued twice). Deriving a number from the billed facts
  instead would be neither sequential nor re-issuable, and re-billing a
  cancelled period would regenerate the original's own number.
- **`br_unique_rechnungsnummer (tenant, rechnungsnummer)`** enforces the
  einmalig half at write time. Kept as a column rather than inside the JSONB,
  where nothing could enforce it and a collision would surface years later in
  an audit.
- **`br_unique_original`** keeps one live original per
  `(malo, lf, period, product, tenant)`. Its predicate excludes corrections,
  the per-MaLo children of a bundle, **cancelled** periods and **VPP**
  settlements — and the upsert repeats all four clauses, because PostgreSQL
  cannot infer a partial index from a column list.
- **A re-run may replace a withheld record, never an issued document.** Once
  `outcome` moves past `generated`, the stored Rechnung is what the counterparty
  received; a conflicting re-run is refused with **`409 PERIOD_ALREADY_BILLED`
  naming the record that holds the period**, so a client retrying a request
  whose response it lost reconciles against a record id instead of a database
  string.
- **Issuance does not depend on having an ERP.** `outcome` advances to
  `dispatched` whenever a document is released; the CloudEvent is enqueued
  *additionally* when `erp_webhook_url` is configured. Whether an invoice has
  been issued is a property of the document, not of the deployment — tying the
  stamp to the webhook would leave an operator without an ERP holding permanent
  drafts, with the overwrite guard unarmed and `pin_template` refusing to pin.
- **A Storno releases its period.** `insert_correction_record` writes the
  Korrekturrechnung and advances the original to `cancelled` in one
  transaction; the period then drops out of `br_unique_original` and can be
  billed again.
- **VPP settlements are per dispatch, not per period.** Several dispatches
  legitimately settle within one calendar day, so they are exempt from the
  period index; `vpp_dispatch_ledger (tx_id, tenant)` is their idempotency
  guard — and **both** writers consult it now. The manual endpoint takes an
  optional `tx_id` per dispatch event and skips anything the webhook already
  settled; without that the two paths were blind to each other and a hand
  back-filled period paid the provider twice for the same flexibility.

`tests/schema_code_guard.rs` pins these rules textually on every `cargo test` —
including that no handler derives a Rechnungsnummer from billed facts and that
no issuance stamp sits behind an `erp_webhook_url` check;
`just test-billingd-db` proves twenty of them against a real PostgreSQL.

## Errors are coded, not prose

Every route returns `BillingError` (`src/error.rs`) and every failure renders the
same envelope:

```json
{ "error": { "code": "ZEITRAUM_UEBERSCHREITET_SATZGRENZE",
             "message": "…",
             "stichtage": ["2024-04-01"],
             "legal_basis": "§28 Abs. 5/6 UStG (Gas/Fernwärme), §10 BEHG" } }
```

One shape for every failure, so a client matches on `error.code` instead of
sniffing the body. The codes are stable and part of the API:

| Code | Status | Meaning |
|---|---|---|
| `INVALID_PERIOD`, `INVALID_DATE` | 400 | a malformed or reversed date |
| `PERIOD_ALREADY_BILLED` | 409 | an issued document holds the period — the body names it |
| `NOT_YET_ISSUED` | 409 | a Storno of a record the risk gate never issued — nothing was booked, so re-bill or release it instead |
| `RECHNUNGSNUMMER_IN_USE` | 409 | § 14 Abs. 4 Nr. 4 UStG collision |
| `ZEITRAUM_UEBERSCHREITET_SATZGRENZE` | 422 | the period straddles a rate boundary — the body names the Stichtage |
| `VALIDATION_BLOCKED` | 422 | the engine refused — the body carries every blocking warning |
| `SECT41A_NO_LASTGANG` | 422 | a dynamic tariff with no interval data to price |
| `NO_METER_DATA`, `NO_ACTIVE_PRODUCT` | 422 | edmd/productd has nothing for this MaLo |
| `MODEL_MISSING`, `XRECHNUNG_NOT_CONFORMANT` | 422 | the stored EN 16931 model is absent or does not satisfy its own BT-24 |
| `UPSTREAM_UNAVAILABLE` | 502 | an upstream did not answer — the body names which |

## E-invoicing is EN 16931, not BO4E

XRechnung/CII and PEPPOL UBL *are* EN 16931 — so the render source is the
**EN 16931 semantic model**, not a re-parse of the BO4E `Rechnung`. At bill time
`energy_billing::Invoice::to_en16931` maps the invoice — at the layer that still
has each position's own amount, VAT category and rate — into an `en16931::Invoice`
and stores it in `billing_records.en16931_json`. `en16931-formats` renders that to
XRechnung/CII (`/xrechnung`, `/submit-b2g`) and PEPPOL UBL (`/ubl`); the B2G path
runs the `en16931` rule engine first and refuses to submit a rejectable document.
BG-23 and the BG-22 totals are derived from the rounded line amounts, so a
mixed-rate invoice (gas 19 % + Fernwärme 7 % + PV 0 %) carries a correct **per-line**
VAT that reconciles (BR-CO-10/13, BR-S-08). **Every** invoice-producing path
(calculate, correction credit-note, VPP, GGV, Sammelrechnung) stores the model,
and the renderers read only from it — a record without one answers 422 rather
than falling back to a second, hand-rolled mapping.
BO4E stays the accounting representation on the `de.billing.rechnung.erstellt` event.

The mapping builds the BG-25 lines and `en16931::reconcile` derives the BG-23
breakdown and BG-22 totals from them (crate-owned, so BR-CO/BR-S reconcile by
construction). The seller party is filled from `[seller]` config (split address,
contact, IBAN); BT-23 business process, the BG-16 SEPA payment instruction and
**BG-14 the billing period** are stamped on every document. BT-34 (the seller's
own MP-ID as a GLN) is emitted only when `Identifier::eas_checked` confirms the
GS1 check digit — a mistyped `tenant` omits the term and logs it, rather than
asserting a GLN the identifier is not. The **B2G** path (`/submit-b2g`) takes the recipient in
the request `buyer` (name/address/contact) plus the `reference` Leitweg-ID (BT-10),
completes the buyer, and renders through `en16931-formats::cii::to_string_for(&…, &XRECHNUNG)`
— which **validates against the full XRechnung 3.0 profile before writing**, so a
rejectable document is never emitted; on failure it returns the violated rules and
the precise `buyer_gaps` (via `Party::missing_for`). (`en16931`/`en16931-formats` are
**v0.5.0** — pinned exactly; cross-check against KoSIT/Mustang before production B2G.)

## The document a customer receives

`GET /api/v1/billing/{id}/pdf` is the ZUGFeRD file: a page a person reads with
the EN 16931 invoice embedded inside it, both from the same stored model.
Rendering lives in **outputd** (`services/outputd`), the customer-communications
daemon extracted from billingd 2026-08-10 — the operator's Typst templates, the
PDF/A-3 carrier, the publish gates and the append-only template store are all
documented there.

billingd's half of the boundary is everything about what the document *says*:

- **The model crosses the wire, not a view.** The render request carries the
  EN 16931 **model**; outputd projects it to `DocumentView` on the side that
  proves templates against that projection. Projecting here as well would be
  two implementations of one contract, and a field added to either yields
  templates that pass the publish gate and fail in production.
- **Payload, proven before it leaves.** The profile is derived from BT-24. A
  B2G document renders through `einvoice::render_xrechnung_cii` (validates the
  full CIUS before writing); every other profile is validated against what it
  declares, and an invalid stored model answers `422` here — outputd wraps an
  invalid payload exactly as faithfully as a valid one, so the sender is the
  only place this check can live.
- **Cacheable once pinned.** A pinned record's PDF is immutable by
  construction — same stored model, same pinned template, and a creation
  timestamp taken from BT-2 rather than the clock — so the endpoint answers a
  strong `ETag` of `"{record_id}-{template_hash}"` plus
  `Cache-Control: private, max-age=31536000, immutable`, and an `If-None-Match`
  hit returns `304` without waking the renderer. A **draft** carries neither
  header: it re-renders with whatever template is current, so it is not the same
  document twice. `private`, because an invoice is one customer's document.
- **Pin.** outputd answers every render with `X-Mako-Template-Hash`. The first
  render after dispatch pins it into `billing_records.template_hash` (a
  conditional `COALESCE` update that never overwrites), so re-rendering an
  issued invoice in 2034 reproduces the document that was sent. A draft pins
  nothing. The hash crosses a service boundary, so no foreign key guards it —
  outputd's append-only store policy is what keeps it resolvable (§ 147 AO).

## Every document through the engine

No handler assembles BO4E invoice JSON by hand:

- **VPP** (webhook auto-billing and `POST /billing/vpp/:id`): positions plus
  the engine's tax provider plus `to_rechnung_json`. The previous inline VAT
  block hardcoded `UST_19` even when the contract overrode the rate.
- **GGV and Sammelrechnung aggregates** (`build_aggregate_invoice`): the
  per-MaLo engine runs stay stored as calculation records; the consolidated
  document strips their tax positions and recomputes VAT **once** over the
  combined base per rate. At the BG-23 breakdown (cent-rounded per BT-117)
  this matters: three sub-invoices of 10.01 EUR each show 1.90 apiece, the
  combined base 30.03 correctly shows 5.71. Each rendered position carries
  the `marktlokationsId` it came from; rechnungsdatum is derived, not
  wall-clock.

## §41e VPP settlement is a Gutschrift

A dispatch settlement pays the flexibility provider: the provider delivered the
energy, the aggregator owes the remuneration, and the aggregator writes the
document — § 14 Abs. 2 Satz 2 UStG Gutschriftverfahren, the same self-billing
shape `eeg-billing` uses for feed-in. Positions are `BillingPosition::credit`
under `PositionCategory::Credit`, the document type is
`InvoiceType::CreditNote`, and the totals are negative from the aggregator's
side.

Both VPP paths (the manual endpoint and the `de.vpp.dispatch.confirmed`
§ 41e EnWG governs the *contract* (Textform, pre-contractual information, the
provider's right to their load-management data); the remuneration itself is
contractual, and the invoice states both.

## Typed engine errors on the wire

Engine failures answer with a structured body, not prose:

```json
{ "error": { "code": "VALIDATION_BLOCKED", "context": "51238696012",
             "message": "…", "warnings": [{ "code": "MODUL3_AND_FLAT_NNE", … }] } }
```

`code` is `EngineError::code()` — stable, machine-readable; `warnings` carries
the full set behind a blocked validation.

## §40 contract facts from vertragd — one lookup, one snapshot

`dispatch_invoice` resolves the active contract behind the MaLo via
`GET vertragd /api/v1/vertraege/by-malo/{malo_id}` and puts the §40 Abs. 1
EnWG facts on the invoice: Vertragsdauer, Kündigungsfrist, the next possible
Kündigungstermin (computed by vertragd, including the §309 Nr. 9 BGB one-month
cap after an automatic renewal) and the **next Abrechnungstermin**. The contract
dates also set `vertragsbeginn`/`vertragsende`, so first and last invoices
pro-rate to the actual contract days (§41 EnWG). The dependency is soft: an
unreachable vertragd degrades to an invoice without the facts, logged.

The same answer carries the **EN 16931 BG-7 buyer**, and `dispatch_invoice`
returns it alongside the priced invoice. The §40 contract facts and the BG-7
buyer are two views of one contract, so they come from **one** read: fetching
the buyer separately would be a second round trip per invoice, and two answers a
concurrent master-data change could make disagree about which customer the
document is for.

The next Abrechnungstermin is **calendar arithmetic**: a period spanning whole
months advances by that many months, so a January bill announces 28 February and
a Q1 bill announces 30 June. Adding the day count instead would announce
3 March for every January.

## Two services answer half the question each

Which product a MaLo is on is a **contract** fact — agreeing it is a
Tarifwechsel under § 41 Abs. 5 EnWG — so `vertragd` owns it, as valid-time
slices. What that product **costs** on a given day is a catalogue fact, so
`productd` owns it.

```text
vertragd  GET /api/v1/malos/{malo}/produkte?from=&to=   → the slices, in order
productd   POST /api/v1/products/{lf}/resolve            → one version per (code, date)
```

Both in one round trip each, however many legs the period has. Asking `productd`
per leg would be an N+1 on every invoice, and two calls could disagree if the
catalogue changed between them.

## A period is billed in legs

An invoice covers a period, and two things can split it. Both are answered the
same way: bill each leg under its own product and its own statutory rates, and
merge the legs into one document — which is also what § 41 Abs. 1 Nr. 4 EnWG
requires, the old and the new price itemised with the periods they applied to.

| Split at | Because |
|---|---|
| a **Tarifwechsel** | `vertragd` reports more than one product-assignment slice covering the period |
| a **statutory Stichtag** | a VAT or levy regime changes inside the period — gas at 31.03.2024 (§ 28 Abs. 5/6 UStG) is 7 % before and 19 % after |

The scheduled sweep, `POST /calculate` and `POST /tarifwechsel` all take this
path, and each leg's meter reading is fetched for **its own dates**.

A leg whose reading the caller supplied by hand is not split further — nothing
can apportion a given reading across a boundary — so those are refused with the
Stichtage named.

## §40c and §41a

**§ 40c Abs. 1** — the invoice is due two weeks after it is **issued**, since the
statute measures from when the payment request reaches the customer. `billingd`
supplies the issue date; the engine stays clock-free and falls back to the period
end only for a caller that has no clock.

**§ 40c Abs. 2** is the delivery deadline: six weeks after the period ends, six
after the supply relationship ends for a Schlussrechnung, and three where § 40b
Abs. 1 monthly billing applies. Missing it attaches
`SECT40C_DEADLINE_EXCEEDED`.

**§ 40c Abs. 3** — a credit balance is offset in full against the next Abschlag
or paid out within two weeks; from an Abschlussrechnung it is always paid out,
there being no next Abschlag. The document states amount and deadline as the
`guthabenerstattung` ZusatzAttribut, so the ledger and the payout run act on it
directly.

**§41a has no fallback, and both halves of that are enforced.** A dynamic tariff
is billed per market time unit against verifiable market prices or it is not
billed:

- A period whose Lastgang is empty is refused with `SECT41A_NO_LASTGANG`, and an
  unreachable `edmd` is a `502` rather than an empty interval list. Degrading
  the fetch to `Vec::new()` would make `DynamicElectricityProvider` price
  *nothing* — an invoice carrying the Grundpreis and no Arbeitspreis, no
  Stromsteuer and no NNE-Arbeitspreis, looking entirely ordinary. That is the
  §41a twin of the priceless-product defect below.
- Intervals that carry consumption but have no EPEX price hard-block the run
  (`SECT41A_MISSING_EPEX_PRICES`) inside the engine.
- The **§41a Abs. 1 iMSys guard** is
  `DynamicElectricityProvider::validate_warnings` reading
  `quantities.electricity.metering_mode`, so the dynamic path must populate
  `electricity` for it to fire at all. The meter reading is resolved for
  dynamic products too (it also carries the §40 Abs. 2 Nr. 6 register readings
  and the §40a estimation flag); the dynamic provider still prices from the
  Lastgang alone.

## Billing arithmetic

All monetary amounts use `billing::Amount<5>` (`EuroAmount` — `i64 × 10⁻⁵` EUR). Never `f64`.
The billing calculator is in the **pure `energy-billing` crate** — exhaustively tested with no I/O:

```bash
cargo test -p energy-billing --all-features
```

Tests cover all 13 product categories (incl. §42c SHARING and municipal WASSER), §41a iMSys guard, §9 StromStG typed exemptions,
`EnergieQuellen` CO₂ label, MwSt override, EEG Gutschrift, HT/NT ToU, gas Brennwert,
Mieterstrom, Tarifwechsel merge, proportional allocation, batch billing, and pre-flight validation.

## Configuration

```toml
# billingd.toml
port          = 9280
tenant        = "9900357000004"

productd_url     = "http://productd:9080"
edmd_url        = "http://edmd:8380"
edmd_api_key    = "env:BILLINGD_EDMD_SERVICE_KEY"  # opaque Bearer; register in edmd [[oidc.service_keys]]
marktd_url      = "http://marktd:8180"
vertragd_url    = "http://vertragd:9780"
outputd_url     = "http://outputd:9880"   # renders the ZUGFeRD PDF; without it /pdf answers 502

[database]
url = "postgresql://billingd:secret@db:5432/billingd"
# pool_size = 10   # optional pool tuning (min_connections, acquire/idle/max_lifetime)

[rates]
stromsteuer_ct_per_kwh        = 2.05   # §3 StromStG
energiesteuer_gas_ct_per_kwh  = 0.55   # § 2 Abs. 3 S. 1 Nr. 4 EnergieStG (constant since 2003)
behg_gas_ct_per_kwh           = 1.310  # BEHG §10, 65 EUR/t × 0.20160 kg/kWh (2026)
mwst_rate                     = 0.19   # § 12 Abs. 1 UStG
mwst_rate_reduced             = 0.07   # § 12 Abs. 2 UStG — Trinkwasser (Anlage 2 Nr. 34)

[billing]
# § 14 Abs. 5 Satz 2 UStG — how an invoice that deducts advances presents them.
# ENDRECHNUNG (default): the whole supply, then the advances and their tax.
# RESTRECHNUNG: only the remainder, each advance as a BG-20 document-level
# allowance carrying its own VAT rate — what the BMF recommends for e-invoices
# (Schreiben v. 15.10.2024, Rn. 48). A request may override it per invoice.
settlement_form = "ENDRECHNUNG"

# §40 Abs. 2 Nr. 1 EnWG — supplier identity as shown on invoices. The
# statutory consumer hints (Schlichtungsstelle Energie §111b EnWG, BNetzA
# Verbraucherservice, Energieberatung, §41c Wechselhinweis) are emitted into
# every Rechnung automatically as `verbraucherinformationen`.
seller_name    = "Stadtwerke Musterstadt GmbH"

# §14 Abs. 4 Nr. 2 UStG — the seller's tax identifier. The statute names two and
# requires **one**: set `seller_vat_id`, or `seller_tax_number`, or both.
# billingd refuses to start with neither, because every invoice it issued would
# omit a mandatory term. A §19 UStG Kleinunternehmer generally holds no
# USt-IdNr. and configures only the Steuernummer.
seller_vat_id    = "DE123456789"            # BT-31 USt-IdNr.
seller_tax_number = "123/456/78901"         # BT-32 Steuernummer
seller_iban    = "DE89370400440532013000"   # BT-84, XRechnung BG-16 SEPA credit transfer
seller_bic     = "COBADEFFXXX"              # BT-86 (optional)

# BG-5 / BG-6 — stated field by field, never parsed. A single free-text address
# line was split on the last comma and the first space, so an operator whose
# address does not read "Street 1, 12345 City" shipped documents silently
# missing BT-52/BT-53 — a BR-DE-8/9 failure at the B2G path.
# `seller_address` and `seller_contact` are **removed**. billingd refuses to
# start if either is still present rather than ignoring it — a silently dropped
# key would ship invoices without the §40 Abs. 2 Nr. 1 EnWG supplier address.
[seller]
street       = "Musterstraße 1"
post_code    = "12345"
city         = "Musterstadt"
contact_name = "Kundenservice"              # BT-41; defaults to seller_name
phone        = "0800 1234567"               # BT-42
email        = "service@stadtwerke-musterstadt.de"  # BT-43

# OIDC token verification for the HTTP API. billingd refuses to start
# without it unless `allow_insecure_no_auth = true` (dev only) — an open
# billing API accepts calculate/correction/mutation calls from anyone.
[oidc]
issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
audience = "api://mako-billingd"

# Outbound ERP CloudEvents. `erp_hmac_secret` signs them (webhook-signature,
# HMAC-SHA256) so the receiver can verify the origin. Delivery is durable:
# each event is written to the `event_outbox` table in the same transaction as
# the business change (persist-before-dispatch) and drained by a background
# worker with retry + dead-letter — a crash never drops an event.
erp_webhook_url = "http://erp:8000/events"
erp_hmac_secret = "env:BILLINGD_ERP_HMAC_SECRET"
```

## A product that cannot price its commodity is refused

`energy-billing`'s validation pass carries `KEIN_ARBEITSPREIS` at **Error**
severity, so `bill()` refuses rather than issuing the invoice.

The `Product` price fields are populated by mapping `productd`'s `preistyp`
strings onto struct fields (`clients::extract_tariff_from_product_data`). A
renamed position, a typo in the mapper, or a catalog row saved without its price
maps to `None` — in silence. The resulting invoice was not an error: a STROM
product with every price field absent billed 1000 kWh for **€20.50**, which is
the Stromsteuer and nothing for the electricity, and looked entirely ordinary.
The risk gate caught it only where a rolling baseline already existed, and then
only into the SAMPLE band, which still dispatches.

The guard asks whether the product can price its commodity *at all* — Eintarif,
HT/NT, dynamic, indexed, seasonal or tiered all satisfy it — and covers Strom,
Gas and Fernwärme. An operator who genuinely charges nothing per kWh states a
`0.0`, which is how a decision is distinguished from missing data. Water has the
same shape — a tariff pricing only the Abwasser side bills a plausible Gebühr and
nothing for the drinking water — and is refused as `KEIN_TRINKWASSERPREIS`.

The quantity side has the same shape. On the § 41a path the quarter-hour series
*is* the billed quantity, so a short Lastgang bills every levy on whatever
arrived. The register total is the witness:
`SECT41A_INTERVALLSUMME_WEICHT_AB` refuses a series missing it by more than
0,5 % (1 kWh floor), `SECT41A_KEINE_INTERVALLE` one absent altogether. This
service refuses an empty Lastgang earlier still, as `SECT41A_NO_LASTGANG`.

`crates/energy-billing/tests/priceless_product_is_refused.rs` pins all of it.

## Layered billing quality assurance

The platform implements the state-of-the-art layered model — deterministic
where regulation demands auditability, ML-ready where statistics end:

1. **Rule engine (blocking)** — `energy-billing`'s validation pass: an
   Error-severity violation (§41a iMSys, missing EPEX prices, §14a
   double-billing, Ersatzversorgung > 3 months) means the invoice **never
   exists** (`VALIDATION_BLOCKED`); `assert_valid` pins the arithmetic
   invariants; the DB uniqueness guard prevents double-billing a period; and
   metering's V01–V09/V11/V12 + Hampel grades (F blocks billing) guard the inputs.
2. **Deterministic risk gate (`[risk]`, default on)** — every calculated
   invoice is scored 0–100 from coded findings: content checks
   (Σ Steuerbeträge vs gesamtsteuer, valid German VAT rates, negative/zero
   consumption), engine warnings (estimated readings, Vorjahr deviation
   > 50 %, USt-Stichtag, §40c lateness, Preisgarantie), and history checks
   (rolling-baseline deviation, **cross-invoice period overlap/gap**,
   **≥ 3 consecutive estimate-based invoices**). Bands: 0–19 auto-release,
   20–49 sample, 50–79 review, **80–100 HELD — not dispatched** until
   `POST /api/v1/billing/{id}/release`. `GET /api/v1/billing/review-queue`
   is the analyst work list. Every point on the score is a coded,
   human-readable finding persisted in `billing_records.risk_findings` —
   explainability by construction, no post-hoc SHAP needed.

   A few findings are **verdicts, not evidence**: `MWST_STICHTAG_IM_ZEITRAUM`
   and `BEHG_JAHRESGRENZE_IM_ZEITRAUM` mean the period has no correct single
   rate, so they carry `blocking` and hold the invoice regardless of score and
   thresholds. A weight cannot express that — `hold_at` is operator-tunable, and
   raising it would silently release them.

   `ZERO_ENERGY` measures energy **moved**, not consumed: a feed-in settlement
   is made of `Credit` positions and would otherwise read as a dead meter.
   Magnitudes, not a signed sum, so a Mieterstrom invoice's supply and feed-in
   do not cancel.

   The risk assessment is written **inside** the business transaction: a HELD
   invoice whose band failed to write would be withheld from dispatch, invisible
   to the review queue (`risk_band IN ('REVIEW','HELD')`) and unreleasable by the
   endpoint (`WHERE risk_band = 'HELD'`) — a permanent draft no operator can see.
   "Held" and "known to be held" are one fact.

3. **Statistical/ML analytics (external by design)** — the industry pattern:
   edmd's Iceberg/S3 archive, Arrow IPC streams and DataFusion SQL are the
   feed for external ML platforms (Isolation Forests, autoencoders,
   time-series models); their verdicts can flow back as analyst reviews.
   No ML runtime lives in the billing core — determinism is the product.
4. **AI-assisted investigation** — agentd's `billing-anomaly-agent` triages
   every `de.billing.rechnung.erstellt` event from the persisted
   `risk_findings` first, then the rolling baseline
   (`check_billing_anomaly`), and escalates with root-cause taxonomy;
   `billing-regulatory-guard-agent` independently re-checks §40a/§41/§42.

Inbound invoices get the mirror image: `invoic-checker` recomputes NNE
invoices line-by-line against PRICAT price sheets (digital-twin
reconciliation) with Ok/Warn/Dispute outcomes driving
`gpke.abrechnung.annehmen|ablehnen`.

## §40b EnWG scheduled billing runs

The config-gated `[billing_runs]` worker sweeps daily (after `run_hour_utc`,
default 04 UTC): it pulls active contracts + their `abrechnungszyklus`
(MONATLICH/VIERTELJAEHRLICH/HALBJAEHRLICH/JAEHRLICH) from vertragd
(`GET /api/v1/vertraege/billing-candidates`), computes each contract's most
recently completed period (calendar-aligned; JAEHRLICH rolls on the
`vertragsbeginn` anniversary), clips it to the supply window, and bills every
period without an existing `billing_records` row through the same
dispatch→persist→emit pipeline as `POST …/calculate`. Each calendar month's
sweeps accumulate one `billing_run_log` audit row with three counters —
`records_count`, `skipped_count`, `errors_count`. A **skip** is a period the
sweep deliberately did not bill and is not a fault; only `errors_count` marks
the month `failed`.

A settling cadence (`JAEHRLICH`) additionally reads the advances it must deduct
from `accountingd` (`GET /accounts/{malo}/abschlaege`), already filtered to what
§ 14 Abs. 5 Satz 2 UStG allows a settlement to deduct: received, unabsorbed,
each with its rate. Without `accountingd_url` configured there is no source and
those contracts are skipped, because a document stating the year's gross with
zero Vorauszahlungen demands money the customer already paid;
`jahresrechnung = true` opts into emitting them anyway.

`versand` (default **true**) then issues each invoice as an `outputd` document
and queues it on the customer's channels — § 40c Abs. 2 EnWG puts the invoice in
their hands within three or six weeks of the period end. The send happens
outside the billing transaction: a delivery failure is logged and repeated by
`POST /api/v1/billing/{id}/versenden` rather than rolling back a billed period,
which would re-bill it under a second Rechnungsnummer.

For **iMSys** MaLos the worker additionally delivers the free monthly
**Abrechnungsinformation** (§ 40b Abs. 3 EnWG) as a
`de.billing.abrechnungsinformation.monatlich` CloudEvent (a preview
calculation, never a persisted invoice), claimed once per MaLo and month in
`abrechnungsinfo_log` and enqueued through the same transactional outbox as
every other event. The claim is taken before the work so two sweeps cannot both
deliver, and **released again** on every path that does not deliver — holding a
claim whose delivery failed would suppress that month's statutory information
for good.

```toml
[billing_runs]
enabled              = true
run_hour_utc         = 4
abrechnungsinformation = true
versand              = true    # default: issue and deliver each invoice
jahresrechnung       = false   # default: skip settlements with no advance source
```

All outbound CloudEvents (invoices, settlements and monthly infos) go through
the transactional outbox and are HMAC-signed with `erp_hmac_secret`
(`webhook-signature`).

## Rounding

All monetary rounding uses **kaufmännisches Runden** (DIN 1333, half away
from zero), and the mode has a single authority: the `billing` arithmetic
core's `RoundingStrategy::MidpointAwayFromZero` — the same strategy every
`billing::Amount` conversion, multiplication and division applies
internally. `energy_billing::round_money` / `.round_kfm(dp)` delegate to it
for runtime-precision rounding on raw `Decimal`s; statutory precisions go
through the typed core (`EuroAmount = Amount<5>` for unit prices,
`Amount<2>` for cents). `Decimal::round_dp` (banker's rounding) is not used
anywhere in the billing path; a grep for `round_dp(` finding only the
helper is the invariant.

Sum-exact money splitting also comes from the core: GGV tenant allocation
uses `billing::proportional_split` (largest remainder), and
`Abschlagsplan::monthly_uniform` distributes the annual estimate via
`Amount::distribute` — any 12 consecutive instalments sum to exactly the
annual amount, instead of drifting up to 6 ct/year from naïve
`round(annual/12)`.

## §40–§40c EnWG invoice compliance

- **Zahlungsziel (§40c):** every Rechnung carries `zahlungsziel` = issue + 14
  days; XRechnung (BT-9), UBL `DueDate` and the MCP tool all render it —
  payment is never implied due before the statutory two weeks after receipt.
- **Fristen (§40c):** invoice generation warns (`SECT40C_DEADLINE_EXCEEDED`)
  when issued later than six weeks after the end of the billed period (or, for a
  Schlussrechnung, of the Lieferverhältnis) — **three** weeks where §40b Abs. 1
  monthly billing applies. The short deadline follows the agreed *cadence*, not
  the length of the period at hand: inferring it from the day count warned about
  a ten-day move-out Schlussrechnung three weeks early.
- **Schlussrechnung (§40c):** `POST …/calculate` with `"schlussrechnung":
  true` renders `rechnungsart = SCHLUSSRECHNUNG`; paid advances passed as
  `"abschlaege": [{datum, betrag_eur, ust_satz}]` are itemised and settled
  against the Zahlbetrag (each at the VAT rate it was invoiced at,
  §14 Abs. 5 UStG).
- **Verbraucherinformationen (§40 Abs. 2):** supplier contact plus the
  statutory Schlichtungsstelle/BNetzA/Energieberatung/Wechsel hints are part
  of every `rechnung_json`.
- **Historic VAT is commodity-aware:** gas/Fernwärme carried 7 % from
  01.10.2022 to 31.03.2024 (§28 Abs. 5/6 UStG) and 16 % in H2/2020. A period
  straddling a boundary is split at the Stichtag and billed in legs.
- **Rechnungsnummern (§14 Abs. 4 Nr. 4 UStG):** a fortlaufende number from the
  tenant's counter (`RE-`/`SR-`/`ST-`/`VG-` + year + sequence), stored in its own
  column under a unique index per tenant, so a collision is a write-time database
  error. A second Storno of the same original is still refused with `409` — the
  first one set `outcome = cancelled`.
- **Zählerstände + Zählernummer (§40 Abs. 2 Nr. 6):** start/end register
  readings and the aggregate quality flag come from edmd's billing-period
  response (`zaehlerstand_anfang/ende`, `quality`, `messtyp`); estimated or
  substituted values render the §40a EnWG / §60 Abs. 2 MsbG (Ersatzwert)
  estimation notice and an
  `ESTIMATED_READING` warning. The Zählernummer is resolved from the marktd
  device registry (MaLo → Lokationszuordnung → MeLo → Zähler).
- **Vorjahresvergleich + Vergleichsgruppe (§40 Abs. 2 Nr. 7/8):** the
  prior-year consumption is fetched from edmd (same window one year
  earlier); the comparable-customer-group value comes from
  `vergleichsgruppe_kwh_pro_jahr` / `vergleichsgruppe_label` (Stromspiegel/
  BDEW reference data), pro-rated to the billing period. Rendered as
  machine-readable ZusatzAttribute so the invoice renderer can chart them
  (the law asks for graphical display).

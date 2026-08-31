# energy-billing

**Pure multi-product retail energy billing library for German markets.**

`energy-billing` is the calculation core of [`billingd`](../../services/billingd/) — the
Energy Billing Engine daemon for the Lieferant (LF) role. The library is **zero I/O,
zero async, zero hardcoded regulatory rates**. It answers one question:

> Given a product definition, meter readings, and statutory rates —
> what does the customer's invoice look like?

---

## §42 Stromkennzeichnung — structured, and on the invoice

`BillingContext.energiequellen` carries the typed `EnergieQuellen` (fuel-mix
percentages, the CO₂ g/kWh figure §42 Abs. 2 Nr. 2 EnWG makes mandatory, HKN
certification) and `to_rechnung_json` emits it as the `stromkennzeichnung`
ZusatzAttribut with the structure intact. billingd copies it from the productd
product via `Product::energiequellen()`.

The invoice emits the structured type — there is no free-text Energiemix
field.

## §14a — all three modules

BK6-22-300 defines exactly three, and their numbering matters — it is printed on
the invoice and shared with the NB-side `grid-billing` engine:

| Modul | What it is | Fields |
|---|---|---|
| **1** | *pauschale Reduzierung des Netzentgelts* — a flat reduction needing no extra metering, hence the default where no choice is made | `sect14a_modul1_pauschale_eur_per_kw_year` |
| **2** | *prozentuale Reduzierung des Arbeitspreises* — attaches to the device's **separately metered** energy | `sect14a_modul2_nne_reduktion_ct_per_kwh` |
| **3** | *zeitvariable Netzentgelte* (from 01.04.2025) — **three** Tarifstufen HT/ST/NT, requires an iMSys | `sect14a_modul3_nne_*` + `Sect14aModul3Verbrauch` |

**Modul 2 and Modul 3 are mutually exclusive.** Both re-price the Arbeitspreis, so
holding both reduces the same network usage twice; configuring both raises the
Error-severity `MODUL2_AND_MODUL3` and the run is refused. Modul 1 composes with
either.

The Modul 3 bands *replace* the flat NNE Arbeitspreis; setting both raises
`MODUL3_AND_FLAT_NNE` for the same reason.

A **Steuerungsentschädigung** (`sect14a_steuerungsentschaedigung_*`) is compensation
for a dispatch that actually happened. It is deliberately not numbered: all three
BK6-22-300 modules are rate reductions, none of them a payment for a Steuerungseingriff. The bands come from the
Netzbetreiber's time windows, which is why they are not derived from the
supplier's own HT/NT split.

## Warnings that actually fire

`invoice.warnings` carries machine-readable codes beyond the §41a guard:
`ESTIMATED_READING` (§ 60 Abs. 2 MsbG), `PREISGARANTIE_ENDET` (ends within 30
days of the period), `VERBRAUCH_ABWEICHUNG_50PCT` (deviation beyond half the
prior year's consumption). They are `Warning` severity: they inform dispatch,
they do not block it.

Every position built through the shared helpers (Arbeitspreis, Grundpreis, and
all levy positions — Stromsteuer, Energiesteuer, BEHG, KA, NNE) carries a
populated `PositionTrace`.

## Explainability reaches the stored invoice

Every `BillingPosition` carries a `PositionTrace` (formula, inputs, §-citations,
tariff source). `to_rechnung_json()` emits it per position as the
`mako:calculation_trace` ZusatzAttribut — BO4E has no field for a calculation
trace, and the attribute is the sanctioned place for what the schema does not
model. This is the only surviving record of *why* an amount is what it is once
the `Invoice` value is dropped after storage; billingd's
`explain_invoice_position` MCP tool reads it from there.

## Two boundary representations: BO4E and EN 16931

`to_rechnung()` (feature `bo4e`) produces the BO4E `Rechnung` for accounting.
`to_en16931(spec_id, seller, buyer)` (feature `en16931`) produces the
[`en16931::Invoice`](https://docs.rs/en16931) semantic model that
`en16931-formats` renders to XRechnung/CII and PEPPOL UBL (its `zugferd` PDF/A-3
feature exists but is not enabled here). The map runs
here — where every position still carries its own VAT category and rate — so each
BG-25 line keeps a correct BT-151/152, and the BG-23 breakdown plus BG-22 totals
are derived from the rounded line amounts so BR-CO-10/13 and BR-S-08 reconcile.
E-invoicing does not round-trip through BO4E.

**Both mappings emit net supply lines only.** `Tax` and `Abschlag` positions
live in this crate's flat position vector so one pass can compute everything,
but neither is an invoice *line* at either boundary: EN 16931 carries them as
BG-23 and BT-113, and BO4E as `steuerbetraege`/`gesamtsteuer` and
`vorauszahlungen`/`zuZahlen`. Emitting them as lines states each amount twice
and leaves the document's own totals irreconcilable —
`BillingPosition::is_rechnungsposition` is the single predicate both mappings
use, so they cannot drift apart again. `Info` positions do belong: they carry
`net_eur == 0`, so they change no sum, and § 40 EnWG wants the Zählerstand and
Brennwert lines on the document.

Every shape `to_rechnung()` can emit is asserted against mako's outbound BO4E
gate in `tests/golden_scenarios.rs` — out-of-schema enums *and* the rules BO4E
states about an invoice's totals. mako refuses a received document that breaks
those, so it must not emit one.

`to_en16931` is **fallible**, for one reason: EN 16931 category `O` (*not
subject to VAT*) is exclusive to its document under **BR-O-11 … BR-O-14**. A
hoheitliche Abwassergebühr is category `O`, so a combined
Trinkwasser-plus-Gebühr invoice — the shape over 90 % of German municipalities
bill in — has no valid rendering. The engine still produces the combined paper
document (with a `GEBUEHR_UND_ENTGELT_AUF_EINEM_BELEG` warning, because that
statement is lawful); it is the *e-invoice* that is refused, with the reason,
instead of handing the recipient a file their schematron rejects days later.

## Period-correct rates

The year tables (`stromsteuer_for_year`, `energiesteuer_gas_for_year`,
`behg_ct_per_kwh_for_year`) are joined by `mwst_rate_for_period`: 19 % since
2007 except the COVID window 01.07.2020–31.12.2020 at 16 %. A period straddling
the window yields `None` — no single rate is correct for it, so the caller
splits rather than misbilling half of it. billingd derives its default
`RegulatoryRates` from these tables per billing period; explicit configuration
still wins.

## Pro-rating conventions (stated, deliberately)

Everything is clipped to the **active contract window** — `vertragsbeginn` /
`vertragsende` intersected with the billing period — and then expressed in the
unit its price is quoted in:

| Price quoted in | Billed as | Helper |
|---|---|---|
| EUR/day, ct/day | active contract days | `ctx.prorate_days().0` |
| EUR/month | calendar-exact months | `ctx.billed_months()` |
| EUR/year, EUR/kW·a | calendar-exact years | `ctx.billed_years()` |

**Calendar-exact** means each calendar month contributes `billed days ÷ that
month's own length` and each year `billed days ÷ that year's length`. January
1–31 is exactly `1` month, a full year exactly `12`, a leap year exactly `1`
year, and 16–31 January exactly `16/31` — none of which an average-month
divisor produces.

## Typed errors

`BillingEngine::bill` returns `EngineError`, not a stringly error:

| Variant | Meaning | `code()` |
|---|---|---|
| `ValidationBlocked { warnings }` | `Error`-severity regulatory warnings blocked the run — carries **all** collected warnings | `VALIDATION_BLOCKED` |
| `PriceOutOfRange { field, value }` | A tariff price exceeds the monetary range (corrupt tariff) | `PRICE_OUT_OF_RANGE` |
| `InvalidPeriod { from, to }` | What `BillingPeriod::new` returns for `from > to` | `INVALID_PERIOD` |
| `AllocationMismatch { fractions, contexts }` | `allocate_proportionally` shape mismatch | `ALLOCATION_MISMATCH` |
| `Arithmetic(billing::BillingError)` | Passthrough from the arithmetic core | `ARITHMETIC` |

`code()` is stable and machine-readable; `blocking_warnings()` exposes the
warnings behind a blocked validation so services can answer with structured
error bodies instead of parsed prose.


### Refusals, not silent zeros

A pairing the engine cannot price refuses the run rather than issuing an invoice
that charges the levies and nothing for the energy:

| Finding | The pairing |
|---|---|
| `KEIN_ARBEITSPREIS` | a product with every price field `None` |
| `INDEXWERT_FEHLT` | an index-linked tariff whose index value has not arrived |
| `ZWEITARIF_UNVOLLSTAENDIG` | one HT/NT band priced and not the other |
| `ZWEITARIF_OHNE_HT_NT_AUFTEILUNG` | an HT/NT-only product against a meter reporting one total |
| `HT_NT_SUMME_WEICHT_AB` | an HT/NT split that does not reconcile with the stated total |
| `SECT41A_MISSING_EPEX_PRICES` | a §41a interval carrying consumption at no market price |
| `SECT41A_KEINE_INTERVALLE` | a §41a tariff with no interval series at all, against a meter reporting consumption |
| `SECT41A_INTERVALLSUMME_WEICHT_AB` | a §41a interval series that does not reconcile with the meter total (0,5 % tolerance, 1 kWh floor) |
| `KEIN_TRINKWASSERPREIS` | water delivered under a tariff pricing only the Abwasser side |
| `KEIN_LADEPREIS` | charging energy measured at the charge point under a tariff with no per-kWh price |

The two §41a series findings are the dynamic twin of the HT/NT pair above: on
that path the quarter-hour series *is* the billed quantity — Arbeitspreis,
Netzentgelt, Konzessionsabgabe and Stromsteuer all ride the sum of the priced
intervals — so an absent or short series bills every levy on whatever arrived.
The meter total is the independent witness.

Two neighbouring shapes are *billed* instead, because the data is complete and
only its reading is at issue: HT/NT registers with no stated total bill on their
sum (`MeterInput::billable_kwh`), and a two-register meter on an Eintarif product
bills on its total.

One finding does not refuse. `KEIN_EREIGNISPREIS` reports counted
Energiedienstleistung events that nothing prices, at `Warning` severity: an
event count is also a legitimate informational figure — how many Einsätze a
maintenance flat rate covered — so it alone does not settle which was meant.

The general statement is a pair of property tests in `proptest_invoice`:
whenever `bill()` succeeds and a quantity was delivered, an Arbeitspreis position
exists and is non-zero. `any_billable_consumption_produces_a_work_price` covers
the electricity pricing shapes (Eintarif, HT/NT, indexed, tiered, split and
unsplit meters); `any_delivered_commodity_produces_a_work_price` covers gas,
Fernwärme and charging energy. The second exists because the first was
electricity-only, which is precisely why the defect kept recurring outside it.

### Every line multiplies out

**PEPPOL-EN16931-R120** allows ±0.02 between a line's `price × quantity` and its
amount. The trap is a *rounded* price: rounding what the page prints is right,
but the same figure is BT-146, and the further it is rounded and the more units
it multiplies, the further the product drifts from the amount beside it.

§41a states a weighted average — rarely representable — so the machine field
carries it at full precision and only the description rounds
(`∅ 7,8430 ct/kWh`). `every_position_price_multiplies_out_to_its_amount` holds
the rule for every position of every product.

## Validated period, stated regime

`BillingContext.period` is a `BillingPeriod` — the constructor (and the serde
path) refuse `from > to`, so an inverted period is unrepresentable in every
provider and helper downstream.

`BillingContext.vertragsart` states the contractual regime and is emitted as
the `vertragsart` ZusatzAttribut on every invoice:

- **`Sondervertrag`** (default) — freely negotiated, §41 EnWG.
- **`Grundversorgung`** — the published Allgemeine Preise apply (§36 EnWG,
  StromGVV/GasGVV).
- **`Ersatzversorgung`** — §38 EnWG fallback supply. It ends by law three
  months after it began (§ 38 Abs. 4 EnWG), so the engine **refuses** a
  longer Ersatzversorgung period with `ERSATZVERSORGUNG_UEBER_3_MONATE`:
  such a supply cannot exist, and billing it would invent one.

## Architecture

```
billingd (HTTP service)
    │   productd/edmd/marktd clients · HTTP endpoints
    │   XRechnung 3.0 CII / PEPPOL UBL · PostgreSQL · CloudEvents
    │
    └── energy-billing (pure crate)
            │
            ├── Product                — typed enum with 13 per-category variants
            │     ├── Strom(ElectricityProduct)
            │     ├── Waermepumpe/Wallbox(ControllableLoadProduct)   §14a
            │     ├── Gas(GasProduct)
            │     ├── Waerme(HeatProduct)
            │     ├── Wasser(WaterProduct)                        Trinkwasser + Abwasser
            │     ├── Solar(SolarProduct)
            │     ├── Eeg(EegProduct)
            │     ├── Einspeisung(EinspeisungProduct)
            │     ├── Hems/Emobility/Energiedienstleistung(…)
            │     └── Sharing(SharingProduct)                        §42c
            │
            ├── Quantities             — all meter inputs for one billing period
            ├── BillingContext         — period, IDs, invoice type, regulatory rates
            │     └── period: BillingPeriod   — validated; from > to unrepresentable
            ├── BillingEngine          — composes BillingProvider instances
            │     ├── validate()       — pre-flight regulatory check (no positions)
            │     ├── bill(&self, …)   — pure function → Result<Invoice, EngineError>
            │     └── bill_batch(…)    — portfolio billing
            ├── BillingProvider        — one implementation per product/tax type
            └── Invoice                — result with positions + totals + warnings + BO4E JSON
                  ├── warnings: Vec<BillingWarning>    — regulatory compliance notices
                  ├── has_errors()                     — any Error-severity warning?
                  └── to_rechnung_json()               — BO4E JSONB for accountingd
```

The engine runs in passes:

```
Pass 0  validate_warnings()      §38/§41a guards · regulatory pre-checks
Pass 1  commodity / levy providers   (ElectricityProvider, GasProvider, …)
Pass 2  tax provider                 (MwStProvider — sees all net positions)
Pass 3  Abschlag deductions          (Final invoice reconciliation)
Pass 4  Minimum invoice top-up       (B2B Mindestabnahmeverpflichtung)
Pass 5  Cancellation sign reversal   (Stornorechnung — all signs negated)
```

---

## Quick start

```rust
use energy_billing::{BillingContext, BillingPeriod, GridInput, InvoiceType, MeterInput,
                     Product, Quantities, RegulatoryRates};
use rust_decimal::dec;
use time::macros::date;

// Deserialize directly from productd JSONB using the "category" discriminator
let product: Product = serde_json::from_str(r#"{
    "category": "STROM",
    "arbeitspreis_ct_per_kwh": "32.0",
    "grundpreis_ct_per_day": 12.0
}"#)?;

let ctx = BillingContext {
    malo_id:          "51238696012".to_owned(),
    lf_mp_id:         "9910000000002".to_owned(),
    rechnungsnummer:  "R2026-06-001".to_owned(),
    period:           BillingPeriod::new(date!(2026-06-01), date!(2026-06-30))?,
    invoice_type:     InvoiceType::Initial,
    regulatory_rates: RegulatoryRates::default(),
    ..Default::default()
};

let quantities = Quantities {
    electricity: Some(MeterInput {
        arbeitsmenge_kwh: dec!(312.5),
        ..Default::default()
    }),
    ..Default::default()
};

// Build and run — Product::build_engine() handles all category routing
let invoice = product
    .build_engine(&GridInput::default(), &ctx.regulatory_rates)
    .bill(ctx, &quantities)?;

invoice.assert_valid();
println!("Brutto: {} EUR", invoice.brutto_eur);

let rechnung_json: serde_json::Value = invoice.to_rechnung_json();
```

---

## Product enum

`Product` is the typed dispatch enum that replaces the old flat `TariffInput` god-struct.
Each category has its own struct with only the relevant fields — no silent field confusion.

```rust
// Deserializes via #[serde(tag = "category")] from flat productd JSONB:
// {"category":"STROM","arbeitspreis_ct_per_kwh":"28.5"} → Product::Strom(ElectricityProduct{...})
// {"category":"WAERMEPUMPE","sect14a_modul2_nne_reduktion_ct_per_kwh":"1.5",...} → Product::Waermepumpe(...)
// {"category":"GAS","gas_arbeitspreis_ct_per_kwh_hs":"7.5",...} → Product::Gas(GasProduct{...})
```

| `Product` variant | Category string | Provider | Key features |
|---|---|---|---|
| `Strom(ElectricityProduct)` | `STROM` | `ElectricityProvider` or `DynamicElectricityProvider` | SLP/RLM; HT/NT; block tariffs; §41a EPEX |
| `Waermepumpe(ControllableLoadProduct)` | `WAERMEPUMPE` | `ControllableLoadProvider` | §14a Modul 1/2/3 |
| `Wallbox(ControllableLoadProduct)` | `WALLBOX` | `ControllableLoadProvider` | §14a Modul 1/2/3 |
| `Gas(GasProduct)` | `GAS` | `GasProvider` | Brennwertkorrektur; Energiesteuer; BEHG CO₂ |
| `Waerme(HeatProduct)` | `WAERME` | `HeatProvider` | Fernwärme; standard-rated (19 %); AVBFernwärmeV §24 Preisgleitklausel; CO2KostAufG § 3 CO₂-Kosten + § 14 WPG Anteil |
| `Wasser(WaterProduct)` | `WASSER` | `WaterProvider` | Trinkwasser 7 % USt; gesplittete Abwassergebühr (Schmutzwasser − Absetzungen, Niederschlagswasser m²); public-law fee is EN 16931 `O`, not `Z` |
| `Solar(SolarProduct)` | `SOLAR` | `SolarProvider` | §42b EnWG GGV; Mieterstrom mit § 42a Abs. 4 EnWG 90 %-Deckel; Stromsteuer per § 9 Abs. 1 Nr. 3 StromStG, **stated** not omitted; 0 % USt if Kleinunternehmer (§19 UStG) |
| `Eeg(EegProduct)` | `EEG` | `EegProvider` | LF-side Gutschrift; `eeg` feature for §51/§52 |
| `Einspeisung(EinspeisungProduct)` | `EINSPEISUNG` | `EinspeisungProvider` | Direktvermarktung Marktwert − Gebühr |
| `Hems(HemsProduct)` | `HEMS` | `HemsProvider` | Platform subscription + events |
| `Emobility(EmobilityProduct)` | `EMOBILITY` | `EmobilityProvider` | CPO/EMSP: service + kWh + session/roaming |
| `Energiedienstleistung(ServiceProduct)` | `ENERGIEDIENSTLEISTUNG` | `ServiceProvider` | Flat fee + per-event |
| `Sharing(SharingProduct)` | `SHARING` | `ElectricityProvider` + `EnergyShareProvider` | §42c Energiegemeinschaft credit |

`ControllableLoadProduct` composes `ElectricityProduct` (via `#[serde(flatten)]`) plus §14a fields — the standard electricity billing is delegated to `ElectricityProvider` then §14a credits are appended.

---

## Pricing capabilities

| Feature | How |
|---|---|
| HT/NT Zweitarif | `billing::TimeOfUsePricing` (validated, penny-correct) |
| Block / graduated tariffs | `billing::TariffSchedule::graduated()` |
| Indexed prices (TTF, Phelix, NCG) | `IndexedPriceConfig { base_ct, spread_ct, index_value, factor }` |
| Gas indexed price | `gas_indexed_price: Option<IndexedPriceConfig>` in `GasProduct` |
| Fernwärme Preisgleitklausel | `waerme_indexed_price: Option<IndexedPriceConfig>` (AVBFernwärmeV §24 Abs. 4) |
| Seasonal prices | `SeasonalPriceOverride` by month range (wraps year boundary) |
| §41a EPEX dynamic | `billing::DynamicPricing`, per 15-min MTU: kWh × (spot + Aufschlag); spot clamped into `[dynamic_epex_floor_ct_kwh, dynamic_epex_cap_ct_kwh]` |
| §41a iMSys guard | Hard error when `dynamic_epex=true` and `MeteringMode != Imsys` |
| Pro-rata Grundpreis | `ctx.prorate_days()` clips to `vertragsbeginn`/`vertragsende` |
| Minimum invoice (B2B) | Pass 4 auto-top-up to `minimum_invoice_eur_brutto` |
| Discounts | `auf_abschlag_ct_per_kwh`, `auf_abschlag_eur_per_month` (`Discount` category) |
| Boni (§17 UStG Entgeltminderung) | `sofortbonus_eur` (one-time), `treuebonus_eur_per_year` (pro-rated) → `Bonus` category |
| MSB pass-through | `msb_gebuehr_ct_per_day` (MsbG) |
| Multi-rate MwSt | Per-position `applicable_tax_rate` → grouped `MwStProvider` |
| 0% USt feed-in Gutschrift | `kleinunternehmer_19_ustg` (§19 UStG Kleinunternehmer) |
| Verbrauchsteuer-Begünstigungen | `stromsteuer_tarif` / `energiesteuer_tarif` (Befreiung, Ermäßigung) and `steuerentlastungen` (notes only) — see below |
| Gas RLM Leistungspreis | `gas_leistungspreis_ct_per_kw_month` in `GasProduct` |
| §42 Energiemix | `EnergieQuellen` struct with `co2_g_per_kwh` (mandatory §42 Abs. 2 Nr. 2 EnWG) |

---

## Regulatory compliance

### §41a Abs. 1 EnWG — iMSys guard for dynamic tariffs

Dynamic tariffs (`Product::Strom(p)` where `p.dynamic_epex = true`) require an intelligent
metering system. `BillingEngine::bill()` rejects with
`EngineError::ValidationBlocked` — carrying every collected warning — when
`quantities.electricity.metering_mode != MeteringMode::Imsys`:

```rust
// Pre-flight check: validate without generating positions
let warnings = engine.validate(&ctx, &quantities);
for w in &warnings {
    if w.severity == WarningSeverity::Error {
        eprintln!("[{}] {}", w.code, w.message);
    }
}
// §41a violations produce BillingWarning { code: "SECT41A_IMSYS_REQUIRED", severity: Error }
```

### Verbrauchsteuern — Befreiung, Ermäßigung, Entlastung (`steuer` module)

German excise law knows three instruments and only two of them change what a
supplier may invoice:

| Instrument | Who acts | Effect on the invoice |
|---|---|---|
| **Steuerbefreiung** — § 9 Abs. 1 StromStG, §§ 25–28 EnergieStG | supplier, against the customer's Erlaubnis | the levy is **not** invoiced |
| **Steuerermäßigung** — § 9 Abs. 2/3 StromStG | supplier | invoiced at the **reduced** statutory rate |
| **Steuerentlastung** — § 9a/§ 9b/§ 9c StromStG, §§ 53a, 54 EnergieStG | the *customer*, afterwards, at the Hauptzollamt | **none** — invoiced in full |

```rust
pub enum StromsteuerTarif {
    Regel,                                        // § 3 StromStG — 2,05 ct/kWh
    Befreiung   { grund: StromsteuerBefreiung },  // § 9 Abs. 1 Nr. 1–8
    Ermaessigung{ grund: StromsteuerErmaessigung },// § 9 Abs. 2 (11,42 €/MWh) / Abs. 3 (0,50 €/MWh)
}

pub enum EnergiesteuerTarif {
    Regel,                                        // § 2 Abs. 3 S. 1 Nr. 4 — 0,55 ct/kWh_Hs
    Befreiung { grund: EnergiesteuerBefreiung },  // §§ 25–28 gegen Erlaubnis (§ 24 Abs. 2)
}

/// Notes only. Never an amount.
pub enum Steuerentlastung {
    Stromsteuer9a, Stromsteuer9b, Stromsteuer9c,
    Energiesteuer53a, Energiesteuer54,
}
```

**Why the split is load-bearing.** Zero-rating an Entlastung at supply
under-declares the supplier's own Stromsteueranmeldung, and the customer's later
Entlastungsantrag duplicates it rather than repairing it — a Unternehmen des
Produzierenden Gewerbes is invoiced the full 2,05 ct/kWh and reclaims 2,00 from
the Hauptzollamt (§ 9b, permanent at the EU minimum rate since 01.01.2026, from
12 500 kWh a year). Treating an Ermäßigung as an exemption fails the other way:
§ 9 Abs. 2 Fahrstrom is a *rate* of 11,42 EUR/MWh, and dropping the line loses
1,142 ct/kWh on every rail-traction invoice.

A `Steuerentlastung` renders one 0-EUR informational position stating the levy
it may be claimed against — the customer cannot file without that figure.

---

## Invoice types

```rust
pub enum InvoiceType {
    Initial,             // RECHNUNG — normal periodic billing
    AdvancePayment,      // ABSCHLAGSRECHNUNG — estimated advance request
    Final,               // SCHLUSSRECHNUNG — Jahresabrechnung, deducts ctx.abschlage
    CreditNote,          // GUTSCHRIFT — LF pays generator (EEG, EINSPEISUNG)
    PartialInvoice,      // TEILRECHNUNG — §41 EnWG move-in/move-out / Tarifwechsel
    Correction { original_invoice_id, reason },  // KORREKTURRECHNUNG (§ 147 AO / GoBD)
    Cancellation { original_invoice_id },         // STORNORECHNUNG — all signs negated
}
```

---

## Advance payments (Abschläge)

A Jahresabrechnung (`InvoiceType::Final`) reconciles the advances the customer
already paid. Each one is an `AbschlagDeduction`:

```rust
AbschlagDeduction {
    datum: date!(2026 - 01 - 15),
    betrag_eur: dec!(120.00),   // gross, as paid
    ust_satz: dec!(0.19),       // rate this advance was invoiced at
    beschreibung: Some("Abschlag Januar 2026".to_owned()),
}
```

`ust_satz` is mandatory because **§14 Abs. 5 Satz 2 UStG** requires an Endrechnung
to deduct the advances *and the tax attributable to them* — "die vereinnahmten
Teilentgelte und die auf sie entfallenden Steuerbeträge". A gross total alone
cannot express that: EUR 120 collected at 19 % and EUR 120 collected at 7 %
deduct different amounts of tax. The rate is per advance rather than per invoice,
so a rate change mid-year leaves earlier advances at the rate they were billed at.

| Field | Meaning |
|---|---|
| `betrag_eur` | gross paid |
| `netto_eur()` | `betrag_eur / (1 + ust_satz)`, to cents |
| `ust_eur()` | `betrag_eur - netto_eur()` — derived, so net + tax always re-sums to the gross paid |

On the resulting invoice:

```text
brutto_eur            gross for the period
- abschlag_total_eur  gross already paid
= zahlbetrag_eur      balance due (negative → refund)

abschlag_ust_eur      tax contained in abschlag_total_eur (§14 Abs. 5 Satz 2 UStG)
```

Abschlag positions never affect `netto_eur` / `mwst_eur` / `brutto_eur` — they
reconcile what was paid, they are not turnover.

### Two lawful settlement forms

`BillingContext::settlement_form` picks how a settling invoice presents them.
Both are lawful; they differ in what the document shows, not in what the customer
pays.

| `SettlementForm` | Shows | Basis |
|---|---|---|
| `Endrechnung` (default) | the whole supply, then deducts the advances **and their tax** | §14 Abs. 5 Satz 2 UStG |
| `Restrechnung` | only the remainder; advances are not listed | BMF 15.10.2024, Rn. 48 |

The Endrechnung form has one failure mode worth naming: deducting the advances
but not the tax contained in them. Under UStAE 14.8 Abs. 10 the issuer then owes
the tax shown **plus** the advance-related portion again under §14c Abs. 1 — the
same tax billed twice. `abschlag_ust_eur` exists so that figure is always
available to state.

The Restrechnung form is what the BMF recommends for e-invoices, because
EN 16931's core profiles have nowhere to carry per-advance tax. Compute the
residual directly with:

```rust
let residual = invoice.residual_breakdown(default_rate)?;  // supply − advances, per rate
```

Over-deduction is refused rather than silently accepted: advances exceeding the
supply in any `(category, rate)` group would understate the output tax owed.

**`to_en16931` implements the difference.** An Endrechnung states the full
supply and the advances as BT-113 *paid*. A Restrechnung deducts each advance as
a **BG-20 document-level allowance** carrying its own BT-95/96 VAT category and
rate — one per `(category, rate)` group, not one per advance, so a monthly
Abschlagsplan does not put eleven identical rows on the page. The reconciler then
derives BG-23 as `lines − allowances` per rate, which *is* the residual, and
nothing is stated as paid. Both documents ask the customer for the same BT-115.

That is what the flat BT-113 cannot do: an advance invoiced at 19 % stays a 19 %
deduction on a settlement billed at another rate. The field selected nothing at
all until it was wired — declared, documented here, and read by no code path, so
every settling invoice went out as an Endrechnung regardless.

### Crossing into `billing`

```rust
invoice.advance_payments()?   // Vec<billing::AdvancePayment> — each with its own tax
invoice.prepayment()?         // billing::Prepayment::Itemised, or ::None
```

Advances are always itemised, never collapsed to a flat total: the per-advance tax
is what makes the deduction lawful. `AdvancePayment` mirrors the ZUGFeRD /
Factur-X EXTENDED group `SpecifiedAdvancePayment` (BG-X-45), the standardised
place where per-advance tax data has a home.

---

## VAT breakdown (EN 16931 BG-23 / BO4E `steuerbetraege`)

`Invoice::tax_subtotals(default_rate)` groups the positions into one entry per
distinct rate, each with its own taxable base (BT-116) and tax amount (BT-117).
A single aggregate `mwst_eur` cannot describe an invoice that mixes 19 %
commodity with 7 % Fernwärme or 0 % PV feed-in.

Zero-rated bases are included. A supply taxed at 0 % is still a taxable supply,
and omitting it would make the sum of the bases differ from the invoice net —
exactly what the EN 16931 total-reconciliation rules check.

The breakdown is emitted as BO4E `steuerbetraege`, whose entries must sum to
`gesamtsteuer`, and is carried into XRechnung as BG-23.

---

## Meter inputs

```rust
pub struct MeterInput {
    pub arbeitsmenge_kwh:    Decimal,
    pub arbeitsmenge_ht_kwh: Option<Decimal>,  // HT register
    pub arbeitsmenge_nt_kwh: Option<Decimal>,  // NT register
    pub spitzenleistung_kw:  Option<Decimal>,  // peak demand (RLM)
    pub steuerung_stunden:   Option<Decimal>,  // §14a load-shedding hours
    pub zaehlernummer:       Option<String>,   // §41 EnWG — shown on invoice
    pub zaehlerstand_von:    Option<Decimal>,  // start reading
    pub zaehlerstand_bis:    Option<Decimal>,  // end reading
    pub metering_mode:       MeteringMode,     // Slp | Rlm | Imsys
    pub is_estimated:        bool,             // § 60 Abs. 2 MsbG notice on invoice
    pub zaehler_replaced:    bool,             // Zählerwechsel notice on invoice
}
```

---

## Key regulatory fields per product

### `ElectricityProduct` / `ControllableLoadProduct`

| Field | Law | Effect |
|---|---|---|
| `kleinunternehmer_19_ustg` | §19 UStG | 0 % USt on the feed-in Gutschrift (operator has elected Kleinunternehmer) |
| `stromsteuer_tarif` | § 9 StromStG | `REGEL` \| `BEFREIUNG{grund}` \| `ERMAESSIGUNG{grund}` |
| `steuerentlastungen` | § 9a/9b/9c StromStG | Notes only — the levy is billed in full |
| `leistungspreis_strom_ct_per_kw_month` | §41 EnWG | RLM demand charge (ct/kW/month) |
| `preisgarantie_bis` | §41 Abs. 1 Nr. 4 EnWG | Price guarantee expiry on invoice |
| `mwst_rate_override` | §12 UStG | Override 19% per product |
| `dynamic_epex` | §41a EnWG | 15-min EPEX MTU spot billing (requires `MeteringMode::Imsys`) |
| `dynamic_epex_floor_ct_kwh` | §41a EnWG | Price floor for the spot component (Aufschlag added on top) |
| `energiequellen` | §42 Abs. 2 Nr. 2 EnWG | Typed fuel mix with CO₂ label |

### `ControllableLoadProduct` (§14a extras)

| Field | Law | Effect |
|---|---|---|
| `sect14a_modul2_nne_reduktion_ct_per_kwh` | §14a EnWG Modul 2 | Per-kWh Arbeitspreis reduction |
| `sect14a_modul1_pauschale_eur_per_kw_year` | §14a EnWG Modul 1 | Pauschale Reduzierung (EUR/kW/year) |
| `sect14a_steuerungsentschaedigung_ct_per_kwh` | §14a EnWG | Per-kWh Steuerungsentschädigung (not a module) |
| `sect14a_steuerungsentschaedigung_eur_per_kw_year` | §14a EnWG | Capacity Steuerungsentschädigung (not a module) |

### `GasProduct`

| Field | Law | Effect |
|---|---|---|
| `energiesteuer_tarif` | §§ 24 Abs. 2, 25–28 EnergieStG | `REGEL` \| `BEFREIUNG{grund}` (Erlaubnisschein) |
| `gas_leistungspreis_ct_per_kw_month` | §41 EnWG | RLM demand charge for large gas customers |
| `gas_indexed_price` | §41 EnWG (Sonderkundenvertrag) | B2B TTF/NCG indexed price |

---

## Advanced operations

### Tarifwechsel — mid-period price change

```rust
// Old tariff: Jan 1–14
let inv_old = old_product.build_engine(&grid, &rates).bill(ctx_jan1_14, &meter_old)?;
// New tariff: Jan 15–31
let inv_new = new_product.build_engine(&grid, &rates).bill(ctx_jan15_31, &meter_new)?;
// Combined January invoice
let merged = inv_old.merge(inv_new);
```

### Portfolio billing

```rust
let engine = product.build_engine(&grid, &rates);
let results: Vec<Result<Invoice, EngineError>> = engine.bill_batch(
    customers.into_iter().map(|(ctx, quantities)| (ctx, quantities)).collect()
);
```

### Regulatory pre-flight

```rust
let engine = product.build_engine(&grid, &rates);
let warnings = engine.validate(&ctx, &quantities);
if invoice.has_errors() {
    // Block dispatch — Error-severity regulatory violation
}
```

### Proportional cost allocation (B2B shared buildings)

```rust
let parts = building_invoice.allocate_proportionally(
    &[dec!(0.40), dec!(0.35), dec!(0.25)],
    vec![ctx_tenant_a, ctx_tenant_b, ctx_tenant_c],
)?;
// Guaranteed: parts[0].brutto + parts[1].brutto + parts[2].brutto == original.brutto
```

### §41a Abs. 6 annual savings comparison

```rust
let comparison = Sect41aAnnualComparison::compute(
    dec!(2400),   // actual kWh under dynamic tariff
    dec!(650.00), // actual EUR brutto
    dec!(40.0),   // reference fixed tariff ct/kWh
);
// Rendered as Info position on the annual invoice
```

---

## Optional features

```toml
energy-billing = { path = "…", features = ["eeg"] }   # full eeg-billing accuracy
energy-billing = { path = "…", features = ["full"] }  # all optional features
```

| Feature | Enables |
|---|---|
| `eeg` | `EegProvider` delegates to `eeg_billing::calculate_settlement()` for §51/§52/§36h |

> **Note:** `energy-billing` carries no `bo4e` / `rubo4e` dependency.
> `Invoice::to_rechnung_json()` produces BO4E-compatible JSON without one.
> For typed `rubo4e::current::Rechnung` output, convert the JSON in `billingd`'s service layer.

---

## Audit trail and explainability

Every `BillingPosition` carries a `PositionTrace` with the full calculation audit:

```rust
pub struct PositionTrace {
    pub formula: String,              // "500.000 kWh × 0.30000 EUR/kWh = 150.00000 EUR"
    pub input_quantity: Decimal,
    pub input_unit_price_eur: Decimal,
    pub gross_eur: Decimal,
    pub regulatory_basis: Vec<String>, // ["§3 StromStG", "§41 EnWG"]
    pub tariff_source: Option<String>, // product sheet ID from productd
    pub pro_rata_fraction: Option<Decimal>,
}
```

The `BillingWarning` field on `Invoice` carries regulatory compliance notices:

```rust
// Check for dispatch-blocking violations
if invoice.has_errors() {
    for w in invoice.warnings.iter().filter(|w| w.severity == WarningSeverity::Error) {
        // e.g. { code: "SECT41A_IMSYS_REQUIRED", message: "§41a Abs. 1 EnWG: …" }
    }
}
```

---

## Regulatory basis

| Law | Coverage |
|---|---|
| §3 StromStG | Stromsteuer 2.05 ct/kWh; `stromsteuer_for_year(year)` for retroactive corrections |
| § 9 StromStG | All eight Abs. 1 Befreiungen plus the Abs. 2/3 ermäßigte Sätze, typed; § 9a/9b/9c Entlastungen kept out of the amounts |
| §2 EnergieStG | Erdgassteuer 0.55 ct/kWh; `energiesteuer_gas_for_year(year)` (incl. 2022 0-rate) |
| §54 EnergieStG | KWK / industrial gas Energiesteuer exemption |
| BEHG §10 | CO₂-Preis H-Gas (65 EUR/t 2026) + L-Gas factor; `behg_ct_per_kwh_for_year(year)` |
| §25 Nr. 4 MessEV | Brennwertkorrektur m³ → kWh_Hs |
| §12 Abs. 2 Nr. 1 UStG | Reduced 7% MwSt for Anlage-2 goods (Trinkwasser) — NOT district heating |
| §19 UStG | 0% USt on the feed-in Gutschrift (Kleinunternehmer election) |
| §14a EnWG | Controllable loads, Modul 1/2/3 (BNetzA BK6-22-300) via `ControllableLoadProvider` |
| § 60 Abs. 2 MsbG | Estimated reading notice on invoice |
| §40 / §40b EnWG | Mandatory ct/kWh; structured price-comparison data in JSON |
| §40 EnWG | Invoice content (Netzbetreiber, Energiemix §42) |
| § 40 Abs. 2 Nr. 6 EnWG | Anfangs-/Endzählerstand, the consumption **and** the `Ablesungsart` — the third is its own duty and was on the page only for estimates |
| §40 Abs. 2 Nr. 7/8 EnWG | Verbrauchshistorie (prior-year + national average) |
| §41a / §41a Abs. 1 EnWG | §41a EPEX per-interval; iMSys guard and missing-price guard both hard errors; the § 40 Abs. 2 display duties ride the dynamic path too |
| §42 Abs. 2 Nr. 2 EnWG | CO₂ emissions label via typed `EnergieQuellen.co2_g_per_kwh` |
| § 42a Abs. 4 EnWG | Mieterstrom price capped at 90 % of the Grundversorgung — refused above it, and the ceiling stated on the page |
| § 9 Abs. 1 Nr. 3 StromStG | A rooftop Mieterstrom/GGV supply is exempt (≤ 2 MW, räumlicher Zusammenhang) — the **default**, and the ground reaches the invoice |
| §42b EnWG | Gemeinschaftliche Gebäudeversorgung (PV/grid hybrid split) |
| CO2KostAufG § 3 / § 14 WPG | Fernwärme CO₂-Kosten pass-through, specific emissions and renewable share |
| §42c EnWG | Energiegemeinschaft sharing credit via `SharingProduct` |
| §51 EEG 2023 | Negativpreisregel (contractual LF feature via `eeg` feature) |

---

## Testing

```bash
cargo test -p energy-billing --all-features
```

Coverage spans six suites:

| Suite | Coverage |
|---|---|
| Unit tests (lib) | `RegulatoryRates`, levy lookups, `prorate_days`, `InvoiceType`, `Product` enum roundtrip, `StromsteuerTarif`/`Steuerentlastung`, `billed_months`/`billed_years`, tariff deserialization |
| `calculator_tests` | All 13 categories (incl. WASSER), §14a/§41a/§41a Abs. 1, GGV, seasonal, indexed, prosumer, block tariffs, RLM demand charge, multi-rate MwSt, cancellation, BO4E JSON, pro-rata, Tarifwechsel, `bill_batch`, `validate` |
| `golden_scenarios` | Golden master: SLP electricity; gas + levies; EEG Gutschrift; RLM demand charge; §54 KWK exemption; historic rates 2022 (heating gas constant 0.55, 7 % gas-USt window); §41a Abs. 1 rejection; §40 ct/kWh; §40 mandatory fields; §42c sharing; §9 exemption |
| `proptest_invoice` | Property-based: `brutto == netto + mwst`, cancellation sign, 0% MwSt, gas arithmetic, demand charge non-negative, StromStG year table |
| `en16931_conformance` | `Invoice::to_en16931` passes the real EN 16931 rule engine (per-line VAT + BG-23 reconcile) |
| Doc tests | Inline usage examples |

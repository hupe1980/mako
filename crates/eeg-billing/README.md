# eeg-billing

**Pure EEG/KWKG feed-in settlement calculation for German energy markets.**

`eeg-billing` is the settlement arithmetic core used by [`einsd`](../../services/einsd/) —
the Einspeiser Registry daemon. It covers the full EEG legal framework from EEG 2000 through
EEG 2023 (Solarpaket I) and KWKG 2023, with all version-specific rule variants enforced
automatically based on the plant's `EegGesetz` year.

zero I/O · zero async · zero `unsafe` · no float money (`rust_decimal`) ·
MSRV 1.94

---

## Design constraints

| Constraint | Detail |
|---|---|
| **No I/O** | All inputs are passed as arguments. No database calls, no HTTP. |
| **No async** | Synchronous — wraps cheaply in `tokio::task::spawn_blocking`. |
| **No float money** | Amounts are computed in `rust_decimal::Decimal`; every EUR result is rounded and range-checked through `EuroAmount` (i64 × 10⁻⁵ EUR). |
| **Deterministic** | Same inputs always produce the same output. Pure functions. |
| **EEG-version-aware** | `EegGesetz` enum drives all version-specific rule dispatch. |
| **Domain-rich** | Multiple domain modules covering settlement, degression, sanctions, repowering. |

---

## Architecture

```
SettleInput {
    scheme: SettlementScheme,        ← HOW remuneration is computed
    tariff_source: TariffSource,     ← WHERE the AW/rate comes from
    settlement_type: SettlementType, ← Initial / Correction / Reversal
    einspeisemenge_kwh, ...          ← measurement data
}
    │
    ▼
calculate_settlement(&SettleInput) → SettleOutput {
    settlement_eur,             ← total net payment in EUR
    eligible_kwh,               ← kWh used (may be reduced by §51)
    positions[],                ← itemized billing positions (Σ = settlement_eur)
    pflichtzahlung_eur,         ← §52 penalty (separate from Vergütung)
    verlaengerungsanspruch_qh,  ← §51a Förderdauer extension entitlement
    faelligkeitsdatum,          ← §26 Abs. 1 EEG 2023 — 15th of following month (when billing_date set)
    status,                     ← Calculated / NoData / PriceMissing / ...
}
```

The `SettlementScheme + TariffSource` split reflects the EEG structure directly:

- **`SettlementScheme`** = which formula applies (`FeedInTariff`, `MarketPremium`, …)
- **`TariffSource`** = where the AW comes from (`Statutory`, `Auction(meta)`, `Transitional(rule)`)

`Ausschreibung` is **not** a separate scheme. It is:
`scheme: MarketPremium + tariff_source: Auction(AusschreibungMetadata)`.
It uses the `MarketPremium` calculation with an **auction-determined anzulegender Wert**.
Award validity, award reductions, and revocation are the caller's responsibility — the library
receives the already-resolved AW from the caller.

---

## Settlement schemes

| `SettlementScheme` | EEG basis | Formula |
|---|---|---|
| `FeedInTariff` | §21 EEG | `kwh × verguetungssatz_ct / 100` |
| `MarketPremium` | §20 EEG | `max(0, (AW + Mgmt) − EPEX) × kwh / 100` (see §20 Abs. 3) |
| `TenantElectricity` | §21 Abs. 3 EEG 2023 | `kwh × (verguetung + zuschlag) / 100` |
| `PostEeg` | post-20yr | `kwh × EPEX / 100` (configurable `price_floor` on the variant) |
| `KwkSurcharge` | §7 KWKG 2023 | `eligible_kwh × rate / 100` (hour-limit cap) |
| `TemporaryFeedInTariff` | §21 Abs. 1 Satz 1 Nr. 3 | Ausfallvergütung (temporary feed-in when the Direktvermarkter drops out) |
| `Eigenverbrauch` | §21 Abs. 3 EEG | No EEG feed-in remuneration is calculated. |
| `FlexibilityPremium` | §50b EEG 2023 | `kwh × (verguetung + flex_praemie) / 100` |
| `FlexibilitySurcharge` | §50a EEG 2023 | `kw × rate / 12` (monthly capacity payment) |
| `SonstigeDirektvermarktung` | §21a EEG | EUR 0 — direct third-party sale, no NB payment; period recorded in settlement history |

### §23a EEG 2023 i.V.m. Anlage 1 — Gleitende Marktprämie

Anlage 1 Nr. 3.1.2 (Monatsmarktwert) and Nr. 4.1.2 (Jahresmarktwert) give the
whole formula:

```
MP = AW − MW          (Nr. 3.1.2)
```

with `AW` defined in Anlage 1 Nr. 1 as "der anzulegende Wert **unter
Berücksichtigung der §§ 19 bis 54**" — so §36h, §51 and §§53b–54 all reach it —
and floored at zero by Nr. 3.1.2 Satz 2.

**There is no additive Managementprämie.** §20 EEG 2023 has no Absätze at all; it
lists the three conditions under which the Marktprämie is payable. Since EEG 2014
the marketing cost sits *inside* the anzulegender Wert, and its mirror image is
the §53 Abs. 1 deduction of 0,4 ct (Solar, Wind) / 0,2 ct (everything else) that
the **Einspeisevergütung** route takes off the same AW.

---

## Domain modules

```
eeg-billing/src/
├── formula.rs           Core settlement dispatcher — pure, all §§ rules applied
├── model.rs             SettleInput / SettleOutput / SettlePosition
├── scheme.rs            SettlementScheme, TariffSource, Paragraph100Rule
├── technology.rs        ErzeugungsArt (18 variants), InbetriebnahmeTyp, RepoweringScope
├── version.rs           EegGesetz (8 variants), §51 thresholds and kW-exemption tables
├── rates.rs             §48 AW tables: solar PV per §49 window, wind, biomasse, KWKG
├── foerderdauer.rs      foerderendedatum_eeg(), §52 Pflichtzahlung, §51a extension
├── foerderungsende.rs   FoerderendeGrund enum, SanktionStatus lifecycle
│
├── degression.rs        §49 semi-annual solar AW degression — 1 % every 1 Feb / 1 Aug
├── direktverm.rs        §§20–22 — Direktvermarktungspflicht, Ausschreibungspflicht, §21b/§21c Wechsel
├── negativpreis.rs      §51 per-interval negative-price derivation (version-aware runs)
├── reductions.rs        §52 Pflichtzahlungen — §52 Abs. 6 netting (a euro-level offset)
├── aw_reductions.rs     §§53b–54 — cuts to the anzulegender Wert, before the formula
├── zusammenfassung.rs   §24 Abs. 1 — the full Zusammenfassung decision (Sätze 1–5)
├── settlement_state.rs  Monthly lifecycle state machine — Active/Reduced/Suspended/PostEeg
│
├── wind.rs              §36h Korrekturfaktor, WindStandort, Gütegrad/Standortklasse
├── biomasse.rs          §43/§44 fuel classes, Güllekleinanlage (≤75 kW, ≥80% Gülle)
│
├── tariff.rs            billing::PricingModel adapter — EegSettleTariff, VAT variants
├── bridge.rs            settlement_to_line_items() → billing::LineItem
├── gutschrift.rs        §14 UStG Gutschrift → rubo4e::current::Rechnung (feature `bo4e`)
└── ust.rs               §19 UStG Kleinunternehmer (E) / Regelbesteuerung (S)
```

### §14 UStG Gutschrift (feature `bo4e`)

EEG feed-in is settled under the **Gutschriftverfahren** (§14 Abs. 2 Satz 2 UStG):
the Netzbetreiber *issues* the settlement document to the Anlagenbetreiber. The
settlement *amount* alone is not that document — VAT law requires a Gutschrift with
the per-rate breakdown (EN 16931 BG-23).

`gutschrift::settlement_to_gutschrift(output, vat, meta)` produces it as a BO4E
`rubo4e::current::Rechnung`: it assembles a `billing::BillingDocument` (positions +
the VAT layers for the operator's declared tax status — Regelbesteuerung 19 %
category `S` / §19 Kleinunternehmer 0 % category `E`) and renders it. The `billing`
crate does the money and VAT
(shared with `energy-billing`/`grid-billing`); the BO4E rendering lives here — the
same per-crate `bo4e` pattern those crates follow, with **no shared bridge crate**.

---

## Quick start

```rust
use eeg_billing::{SettleInput, SettlementScheme, SettlementStatus, calculate_settlement};
use rust_decimal::dec;

// §21 EEG 2023 — 500 kWh × 8.11 ct/kWh = 40.55 EUR
let out = calculate_settlement(&SettleInput {
    scheme: SettlementScheme::FeedInTariff,
    einspeisemenge_kwh: Some(dec!(500)),
    verguetungssatz_ct: dec!(8.11),
    ..SettleInput::default()
});
assert_eq!(out.status, SettlementStatus::Calculated);
assert_eq!(out.settlement_eur, Some(dec!(40.55)));
```

```rust
// §23a EEG 2023 — Direktvermarktung
// AW = 6.28 ct; Monatsmarktwert = 4.50 ct
// Marktprämie = (6.28 − 4.50) × 100,000 / 100 = 1,780 EUR
use eeg_billing::{SettleInput, SettlementScheme, calculate_settlement};
use rust_decimal::dec;

let out = calculate_settlement(&SettleInput {
    scheme: SettlementScheme::MarketPremium,
    einspeisemenge_kwh: Some(dec!(100_000)),
    direktverm_aw_ct: Some(dec!(6.28)),    // statutory or tendered AW
    epex_avg_ct_kwh: Some(dec!(4.50)),
    ..SettleInput::default()
});
// 1,780 EUR
```

---

## §51 EEG — Negativpreisregel (keyed on the Inbetriebnahmedatum)

§51 is **not** a function of the law year. The Solarspitzengesetz rewrote it with effect from
**25.02.2025**, mid-year and inside the EEG 2023 range, so two "EEG 2023" plants are governed
by different rules depending on the day they were commissioned. Derive the regime with
`NegativpreisRegime::fuer_inbetriebnahme` — `EegGesetz` deliberately exposes no §51 rule.

| Inbetriebnahme | Trigger | Exemption | §51a extension |
|---|---|---|---|
| ≤ 2015-12-31 | never (§100 Abs. 1 Satz 4 EEG 2017) | — | — |
| 2016-01-01 – 2020-12-31 | ≥ 6 consecutive hours | Wind < 3 MW · others < 500 kW | none |
| 2021-01-01 – 2022-12-31 | ≥ 4 consecutive hours | < 500 kW | ausschreibungspflichtige only |
| 2023-01-01 – 2025-02-24 | staged 4-3-2-1 h | < 400 kW | ausschreibungspflichtige only |
| ≥ 2025-02-25 | first negative ¼h | < 100 kW until iMSys · < 2 kW pending §85 Abs. 2 Nr. 12 | all plants |

Pilotwindenergieanlagen (§3 Nr. 37) are exempt under every version at any size
(`ist_pilotwindanlage`).

`derive_negativpreis` applies the run-length threshold to a quarter-hour series;
`calculate_settlement` then applies the size / iMSys / Pilotwind exemptions to the derived kWh.

### §51 Abs. 3 — the Ausfallvergütung reporting duty
An operator on the Ausfallvergütung must report the feed-in during continuously negative
prices with its §71 Abs. 1 Nr. 1 data. Unreported, the month's claim falls 5 % per calendar
day such a period touched (`sect51_abs3_unreported_days`), floored at zero.

### §100 — the Bestandsanlagen opt-in
A plant on an older vintage may declare in Textform that §§51/51a shall apply. The declaration
runs from the end of the calendar year in which its iMSys goes in (`optin_wirksam_ab`), and its
anzulegender Wert rises by `SECT51_OPTIN_ZUSCHLAG_CT_KWH` (0,6 ct/kWh) from then.

### §51a — Verlängerungsanspruch
Non-solar plants extend by whole calendar days (96 QH/day, rounded up **once** over the total).
Solar plants convert at factor 0,5 into Volllastviertelstunden and draw them down against the
§51a Abs. 2 monthly table (73 in December, 508 in June). Returned in
`verlaengerungsanspruch_qh`.

---

## §51b EEG 2023 — Biogas Ausschreibung

§51b applies exclusively to **biogas plants (fermentation, not biomethane)** whose AW was
set by BNetzA tender. Per §51b Satz 1 EEG 2023:

> *„Für Anlagen, die Biogas mit Ausnahme von Biomethan einsetzen und deren anzulegender Wert
> in einem Zuschlagsverfahren ermittelt worden ist, **verringert sich der anzulegende Wert
> auf null** für Zeiträume, in denen der Spotmarktpreis 2 Cent pro Kilowattstunde oder
> weniger beträgt.“*

The statute explicitly **reduces the AW to zero** (not merely the Marktprämie).
Since Marktprämie = max(0, AW − EPEX) × kwh/100, zeroing the AW makes the payment zero.
The outcome is identical, but the legal mechanism matters for audit positions.

Two key differences from §51 (source: §51b Satz 2 EEG 2023):
- §51 and §51a do **not** apply to §51b plants
- No Verlängerungsanspruch accrues for §51b periods

```rust
use eeg_billing::{SettleInput, SettlementScheme, TariffSource, AusschreibungMetadata,
                  calculate_settlement};
use rust_decimal::dec;

// Biogas auction plant: EPEX 1.5 ct ≤ 2 ct → AW = 0, EUR 0
let out = calculate_settlement(&SettleInput {
    scheme: SettlementScheme::MarketPremium,
    tariff_source: TariffSource::Auction(AusschreibungMetadata {
        is_biogas_sect51b: true, // explicit biogas §51b flag
        ..AusschreibungMetadata::default()
    }),
    einspeisemenge_kwh: Some(dec!(10_000)),
    direktverm_aw_ct: Some(dec!(8.5)),
    epex_avg_ct_kwh: Some(dec!(1.5)), // ≤ 2 ct/kWh → §51b triggers
    ..SettleInput::default()
});
assert_eq!(out.settlement_eur, Some(dec!(0)));
assert!(out.positions[0].legal_basis.contains("51b"));
```

---

## §100 EEG — Übergangsregelung auto-override

For old plants that fall under a specific `§100` transition provision, supply
`tariff_source = Transitional(rule)`. The library automatically derives the correct
`EegGesetz` for §51/§52 dispatch, preventing silent miscalculations:

| `Paragraph100Rule` | Effective `EegGesetz` | §51 behaviour | Source |
|---|---|---|---|
| `Pre2016Bestandsschutz` | `Eeg2012` | **Never applies** | §100 Abs. 1 Satz 4 EEG 2017 |
| `Eeg2017Negativpreis6h` | `Eeg2017` | ≥6h; Wind <3 MW / other <500 kW | §100 Abs. 2 Nr. 13 EEG 2021 |
| `BiomassOldFuelClassContinuation` | `Eeg2017` | ≥6h; old §42–44 fuel rules | §100 Abs. 6 EEG 2023 |
| `SmallBiomassBelow150kw` | `Eeg2017` | ≥6h; small biomass FiT | §100 Abs. 11 EEG 2023 |
| `OldPlantBeforeEeg2023` | `Eeg2021` | ≥4h; all <500 kW | §100 Abs. 1 EEG 2023 |
| all other variants | caller's `eeg_gesetz` | as per `eeg_gesetz` | — |

```rust
use eeg_billing::{SettleInput, SettlementScheme, EegGesetz, calculate_settlement};
use eeg_billing::scheme::{TariffSource, Paragraph100Rule};
use rust_decimal::dec;

// Pre-2016 plant — §51 must NEVER apply, regardless of eeg_gesetz setting.
// TariffSource::Transitional auto-overrides to Eeg2012 → §51 exempt.
let out = calculate_settlement(&SettleInput {
    scheme: SettlementScheme::FeedInTariff,
    tariff_source: TariffSource::Transitional(Paragraph100Rule::Pre2016Bestandsschutz),
    eeg_gesetz: EegGesetz::Eeg2017,      // ← might be set wrong in DB; rule corrects it
    einspeisemenge_kwh: Some(dec!(1000)),
    kwh_during_negative_epex: Some(dec!(500)), // would trigger §51 under Eeg2017
    leistung_kwp: Some(dec!(1000)),             // 1 MW >> 500 kW threshold
    verguetungssatz_ct: dec!(8.11),
    ..SettleInput::default()
});
// Pre2016Bestandsschutz → no §51 deduction → full 1000 kWh × 8.11 ct = 81.10 EUR
assert_eq!(out.settlement_eur, Some(dec!(81.10)));
```

Use `SettleInput::effective_eeg_gesetz()` directly when building settle logic outside the library.

---

## §26 Abs. 1 EEG — Fälligkeitsdatum

`SettleOutput.faelligkeitsdatum` contains the **15th calendar day of the month following
the billing month**, computed automatically from `billing_date`:

> §26 Abs. 1 EEG 2023: *„monatlich jeweils zum 15. Kalendertag für den Vormonat"*

| Billing month | `faelligkeitsdatum` |
|---|---|
| June 2024 | **2024-07-15** |
| December 2024 | **2025-01-15** (year rolls over) |
| February 2025 | **2025-03-15** |

`None` when `billing_date` is not set. The final Endabrechnung deadline (§26 Abs. 2, conditioned
on §71 data submission) is outside the scope of this library.

---

## §52 EEG — Sanctions

**EEG 2023 (commissioned ≥2023)**: `pflichtverstoss: Vec<Pflichtverstoss>` → `pflichtzahlung_eur`.
Vergütung continues. Multiple violations summed, capped at §52 Abs. 5.

**EEG ≤2021 (§100 Übergangsregelung)**: `sanktion: Some(SanktionAlt::…)`.
Three tiers: `VerguetungAufNull` / `VerguetungAufMarktwert` / `VerguetungReduziert20Prozent`.

### §52 Abs. 6 Netting
```rust
use eeg_billing::reductions::apply_sect52_netting;
let r = apply_sect52_netting(settlement_eur, pflichtzahlung_eur);
// r.net_vergütung_eur              — disbursed to operator
// r.residual_pflichtzahlung_eur    — still owed to NB
```

---

## §49 EEG 2023 — Semi-annual solar degression

The anzulegende Werte of §48 Abs. 1, 2 and 2a fall by a fixed **1 % every six
months**, on 1 February and 1 August, from 01.02.2024. Each step compounds on the
**unrounded** predecessor (§49 Satz 2); the 2-dp rounding is presentation only.
The GW-keyed "atmender Deckel" of §49 EEG 2021 is gone.

```rust
use eeg_billing::rates::solar_pv_ueberschuss_aw_ct;
use rust_decimal::dec;
use time::macros::date;

// 9 kWp roof, commissioned in the 1 Aug 2024 window:
// §48 Abs. 2 Nr. 1 base 8.60 ct × 0.99² = 8.42886 → 8.43 ct gross AW.
assert_eq!(solar_pv_ueberschuss_aw_ct(dec!(9), date!(2024-09-01)), Some(dec!(8.43)));
```

---

## §§20–22 EEG 2023 — Veräußerungsformen, Direktvermarktungspflicht, Ausschreibung

The EEG has no section imposing a duty to market directly. The duty is the
shadow of **§21 Abs. 1 Satz 1 Nr. 1**: the Einspeisevergütung mit gesetzlich
bestimmtem anzulegenden Wert exists only „für Strom aus Anlagen mit einer
installierten Leistung von bis zu 100 Kilowatt", so anything larger has to take
the Marktprämie — which §20 pays only for months the Strom is direkt vermarktet.

```rust
use eeg_billing::direktverm::{SolarSegment, direktvermarktungspflicht, requires_ausschreibung};
use eeg_billing::ErzeugungsArt;
use rust_decimal::dec;
use time::macros::date;

// §21 Abs. 1 Satz 1 Nr. 1 — the ceiling, keyed on the Inbetriebnahmedatum.
assert_eq!(direktvermarktungspflicht(dec!(150), date!(2024-05-01)), Some(true));
// A pre-2016 plant is governed by a text outside mako's corpus: unanswered, not "no".
assert_eq!(direktvermarktungspflicht(dec!(600), date!(2013-05-01)), None);

// §22 Abs. 3 Satz 2 — Solar has two thresholds, keyed on the Segment.
assert!( requires_ausschreibung(dec!(900), ErzeugungsArt::SolarAufdach,      SolarSegment::Zweites));
assert!(!requires_ausschreibung(dec!(900), ErzeugungsArt::SolarFreiflaeche, SolarSegment::Erstes));
// §22 Abs. 5 Satz 2 — Wasserkraft and Geothermie are never tendered, at any size.
assert!(!requires_ausschreibung(dec!(5000), ErzeugungsArt::Wasserkraft, SolarSegment::Erstes));
```

`§21b Abs. 1 Satz 2` (a change takes effect only on the first of a month) and
`§21c Abs. 1 Satz 1` (the Mitteilung is due before the *preceding* month begins)
are `validate_wechsel`. There is no „one switch per calendar month" rule — Satz 2
already makes a second change within one month impossible.

---

## ErzeugungsArt

| Variant | Technology | Notes |
|---|---|---|
| `SolarAufdach` | Rooftop PV (Gebäude/Lärmschutzwand) | §48 Abs. 2 rates; **zweites Segment** (§3 Nr. 41b) → tender > 750 kW |
| `SolarFreiflaeche` | Ground-mounted PV | §48 Abs. 1/1a; **erstes Segment** (§3 Nr. 41a) → tender > 1 MW |
| `SolarAgriPv` | Agri-PV | §48 Abs. 1 S. 1 Nr. 5 lit. a, uplift Abs. 1b; erstes Segment — there is **no** 6-MW Agri-PV exemption |
| `SolarMieterstrom` | §21 Abs. 3 building solar | — |
| `SolarStecker` | Steckersolargerät, ≤2 kW **and** ≤800 VA inverter | §3 Nr. 43, §8 Abs. 5a, §9, §10a Abs. 2 |
| `WindOnshore` | Wind onshore | **No fixed AW** — §46 Abs. 1 computes it from §36h Abs. 1 with the Vorvorjahr auction average; a Zuschlag is required only above 1 MW (§22 Abs. 2 S. 2 Nr. 1) |
| `WindOffshore` | Wind offshore | Zuschlag and AW come from the **WindSeeG**, to which §22 Abs. 1 refers |
| `Biomasse` | Biomasse | §42 — 12,67 ct ≤150 kW Bemessungsleistung; above that, tender |
| `BiomassHolz` | Feste Biomasse | No separate AW; tendered plants meet the §39i Abs. 2 Höchstbemessungsleistung |
| `Biogas` | Fermentation biogas | §43 Bioabfälle / §44 Gülle where they qualify |
| `Biomethan` | Upgraded biomethane | Excluded from the §42 statutory value by Satz 2 |
| `Klaegas` / `Grubengas` / `Deponiegas` | Special gases | §41 — **one ladder each**, Abs. 1/2/3 |
| `Wasserkraft` | Hydro | §40 — seven tiers by Bemessungsleistung |
| `Geothermie` | Geothermal | §45 — flat 25,20 ct |
| `Gezeiten` | Tidal, wave, salinity gradient, current | §40 — these *are* Wasserkraft (§3 Nr. 21 lit. a); there is no §41a EEG |
| `Kwk` | CHP/BHKW | KWKG 2023, not EEG |

## The statutory rates

`rates` carries the **anzulegende Werte** — the ct/kWh a Netzbetreiber owes an
Anlagenbetreiber, each a figure fixed by statute:

| Erzeugungsart | § EEG 2023 | Shape |
|---|---|---|
| Wasserkraft (incl. Gezeiten, § 3 Nr. 21 lit. a) | § 40 Abs. 1 | seven tiers, `12,03` → `3,37` |
| Deponiegas | § 41 Abs. 1 | `7,46` ≤ 500 kW, `5,17` ≤ 5 MW |
| Klärgas | § 41 Abs. 2 | `5,93` ≤ 500 kW, `5,17` ≤ 5 MW |
| Grubengas | § 41 Abs. 3 | `5,98` ≤ 1 MW, `3,81` ≤ 5 MW, `3,37` above |
| Biomasse | § 42 Satz 1 | one tier, `12,67` ≤ 150 kW; above that, tender |
| Bioabfallvergärung | § 43 Abs. 1 | `14,16` ≤ 500 kW, `12,41` ≤ 20 MW |
| Güllevergärung | § 44 Abs. 1 | `22,00` ≤ 75 kW, `19,00` ≤ 150 kW |
| Geothermie | § 45 Abs. 1 | flat `25,20` |
| Solar | § 48 | see below (§ 101 Abs. 1 Satz 2) |
| Wind an Land | § 46 Abs. 1 | **no fixed figure**: the AW is § 36h Abs. 1 with the Zuschlagswert replaced by the Vorvorjahr auction average (§ 46 Abs. 2). A Zuschlag is needed only above 1 MW (§ 22 Abs. 2 Satz 2 Nr. 1) |

Every figure is asserted against the statute by `rates::statutory_rate_tests`,
which walks each ladder tier by tier and pins the two non-tables — § 42 answering
`Err` above 150 kW rather than inventing a rate, and `wind_onshore_lookup`
answering `None`.

### § 101 — a provision under a Genehmigungsvorbehalt has a start date

§ 101 EEG 2023 lists provisions that "erst nach der beihilferechtlichen
Genehmigung durch die Europäische Kommission … angewandt werden" dürfen, and for
some names the version that applies meanwhile. Two of them are load-bearing here:

- **§ 48 Abs. 2** (Satz 2 fallback) — why the in-force solar base values are
  `8,60 / 7,50 / 6,20` and not the consolidated `8,51 / 7,43 / 7,64`.
- **§ 51b** — the biogas AW → 0 at a spot price ≤ 2 ct/kWh. It names no fallback
  version, so it applies only from the Commission's approval on **18 September
  2025** (`version::SECT51B_GENEHMIGT_AB`), keyed on the settled **supply
  period**. An undated settlement does not apply it: a provision under a
  Genehmigungsvorbehalt is not applied on the strength of not knowing when the
  supply happened.

Every table is the **Startwert** as enacted. The statutory annual Absenkung is
separate, and each Erzeugungsart carries its own rate and cadence — biomass steps
on **1 July**, the rest on 1 January — via
`degression::JaehrlicheAbsenkung`.

---

## Repowering

> ⚠ **Repowering law is nuanced.** `foerderendedatum_repowering()` computes a **hypothetical**
> funding end (Dec 31 of year+20) **if and only if** the applicable repowering provisions
> actually result in a new Förderzeitraum. Whether that is the case depends on the type and
> extent of the repowering (`RepoweringScope::Full` vs. partial), the applicable §22 EEG
> provisions for the specific plant, and current BNetzA guidance.
> `RepoweringScope::resets_foerderdauer_definitely()` returns `true` only for full replacement;
> partial replacements are legally contested. Always obtain qualified legal or regulatory
> advice for the specific plant situation before relying on this function.

```rust
use eeg_billing::{foerderendedatum_repowering, RepoweringScope};
use time::macros::date;

// Full repowering — uses statutory Dec 31 rule (§25 Abs. 1 Satz 2 EEG 2023)
// Whether Förderdauer actually resets depends on the type of repowering.
let new_end = foerderendedatum_repowering(date!(2025-06-01)).unwrap();
assert_eq!(new_end, date!(2045-12-31));

// Partial replacement: Förderdauer does NOT reset
assert!(!RepoweringScope::RotorOnly.resets_foerderdauer_definitely());
```

---

## Settlement calculation pipeline

The typical pipeline for a single billing period:

```
1. Einspeisemenge input (from metering / edmd)
         │
         ▼
2. Eligibility check (FoerderungBeendet? foerderendedatum > billing_date?)
         │     ↳ also: §35a award_expired check for Ausschreibungsanlagen
         ▼
3. Scheme dispatch (FeedInTariff / MarketPremium / KwkSurcharge / …)
         │     ↳ §51b: AW = 0 when EPEX ≤ 2 ct/kWh (biogas Ausschreibung)
         ▼
4. §51 Negativpreisregel (version-aware kWh reduction)
         │     ↳ §51a: Verlängerungsanspruch accrued (solar: 0.5×, others: 1×)
         ▼
5. §25 Abs. 1 Satz 3 billing_days_fraction (partial-month commissioning/decommissioning)
         │
         ▼
6. §52 Pflichtzahlungen (separate penalty, Vergütung unchanged)
         │
         ▼
7. §52 Abs. 6 netting (optional: NB deducts penalty from disbursement)
         │
         ▼
8. SettleOutput { settlement_eur, eligible_kwh, positions, pflichtzahlung_eur, faelligkeitsdatum }

(§§53b–54 do not appear as a step: they cut the anzulegender Wert at step 3,
 before the scheme formula, because the Marktprämie floors at zero.)
```

VAT is applied by the caller via `EegSettleTariff` + `ust::ust_tax_layers()` — not
inside `calculate_settlement`. A feed-in Gutschrift has exactly two treatments, and
each yields exactly one tax layer (the exempt one included):

| `VatStatus` | Rate | EN 16931 category | Basis |
|---|---|---|---|
| `Regelbesteuerung` | 19 % | `S` — Standard | §12 Abs. 1 UStG |
| `Kleinunternehmer` | 0 % | `E` — Exempt | §19 UStG (tax not levied) |

The status is a **declared property of the operator** (masterdata), not something
plant size decides — `VatStatus::default_for_plant` only *suggests* the value a
new plant would usually carry. **§12 Abs. 3 UStG is deliberately absent**: its 0 %
Nullsteuersatz taxes the *supply of the PV hardware*, not the feed-in of
electricity. Its only bearing here is indirect — because a ≤30 kWp operator buys
the plant at 0 %, they have no input tax to reclaim and stay §19 Kleinunternehmer.

An exempt supply is still a taxable supply, so it belongs in the EN 16931 BG-23 VAT
breakdown under its own UNTDID 5305 category with a zero tax amount. Omitting the
layer would drop that turnover from the breakdown entirely and understate the
taxable base. §19 UStG does not levy the tax at all and maps to `E`, which carries
the exemption reason EN 16931 requires (BT-120).

A document mixing treatments — a 0 % PV feed-in credit beside 19 % NNE grid
charges — cannot use a single status. Build the layers directly and scope each to
its own positions with `FixedRateTax::with_tag`, so each contributes its own
breakdown entry.

---

## Scope

**Explicitly in scope** — tested and **production-oriented**:
- §21 EEG Einspeisevergütung (all EEG versions 2000–2023)
- §20 EEG Marktprämie + §§22/22a/28 Ausschreibung
- §21 Abs. 3 Mieterstrom, §50a/b Flexibilitätsprämie, §7 KWKG
- §51/§51a/§51b Negativpreisregel, §52 sanctions and Abs. 6 netting
- §53 Einspeisevergütungsabzug, and the AW-level cuts of §53b (Regionalnachweise,
  0,1 ct/kWh, statutory-AW plants only), §53c (Stromsteuerbefreiung, capped at the
  §3 StromStG rate) and §54 (solar first-segment auctions, four Absätze)
- §19 EInsMan curtailment compensation (separate position, §51 exempt)
- §49 semi-annual solar degression, §36h wind Korrekturfaktor
- §24 **Zusammenfassung**: `sind_eine_anlage` decides the whole of Abs. 1 — the four
  cumulative conditions of Satz 1 and the Sätze 2–5 carve-outs (same
  Biogaserzeugungsanlage, Freifläche vs. building solar, differing
  Netzverknüpfungspunkte, disregarded Steckersolargeräte) — and returns the rule
  that decided. Ownership is deliberately not an input: Satz 1 says "unabhängig
  von den Eigentumsverhältnissen".
- §24 multi-block allocation: `CapacityBlock` allocates the metered
  Einspeisemenge across the blocks of an already-fused plant group by installed
  capacity, by **largest remainder** (`billing::proportional_split`) so the
  blocks add back up to the metered quantity exactly. A per-block share rounded
  on its own does not: three equal blocks of a 1000 kWh month take 333.333 kWh
  each and settle 999.999. Expired blocks are allocated and then dropped — their
  capacity still counts towards the plant, and the energy falling to them is
  simply no longer eligible.
- §42b EnWG GGV / §21 Abs. 3 multi-meter split is **not** modelled here — the metering topology,
  Eigenverbrauch/Überschuss split and GGV tenant allocation live in the external `metering`
  crate (`AggregationRule`, `compute_virtual_meter`) + edmd; this crate settles the resulting
  Einspeisemenge
- SettlementType: Initial, Correction (with `original_id`), Reversal
- §25 billing_days_fraction (partial billing periods per §25 Abs. 1 Satz 3)
- §26 Abs. 1 Fälligkeitsdatum (15th of following month, auto-computed)
- `TariffSource::Transitional(Paragraph100Rule)` → `effective_eeg_gesetz()` auto-override

**Intentionally out of scope** (caller's responsibility):
- The §21b/§21c Wechsel *act* — `direktverm::validate_wechsel` decides it here, but
  `einsd` owns the plant record it decides against and issues the §21c notification
- §§53b–54 fact lookups — `einsd` reads the triggering facts and passes an
  `AwReductionContext`; the amounts themselves are statutory and live here
- §55 Pönalen — outside this domain entirely: a bidder↔regelverantwortlicher-ÜNB
  obligation from the tender process, not operator↔NB settlement. `einsd` tracks
  commissioning deadlines for the **Erlöschen** of a Zuschlag (§36e/§37e/§39e)
- §52 cumulative months tracking — `einsd` computes from `violation_start` dates
- § 147 AO / GoBD receipt archival — `einsd` manages `settlement_receipt_history`
- Redispatch 2.0 compensation (§13a/§14 EnWG) — see `crates/mako-redispatch`
- SEPA CT payment dispatch — handled by `accountingd`
- EPEX Spot price import — handled by `einsd`

---

## Regulatory basis

| Topic | Source |
|---|---|
| EEG 2023 | BGBl. I Nr. 28, 10.01.2023 |
| §48 Abs. 2 / 2a anzulegende Werte | Fassung vom 15.05.2024, kept in force by §101 Abs. 1 Satz 2 pending EU state-aid approval; cross-checked against the BNetzA "Anzulegende Werte für Solaranlagen" tables |
| §49 Solardegression | fixed 1 % every six months from 01.02.2024, compounded unrounded (§49 Satz 2) |
| §36h Korrekturfaktor | §36h Abs. 1 Satz 2 Stützwerte, linear interpolation (Satz 3), Satz 4 out-of-range rules |
| KWKG 2023 | BGBl. I Nr. 59, 28.12.2023 |
| §23a Marktprämie | `MP = AW − MW`, floored at zero (Anlage 1 Nr. 3.1.2) — no additive Managementprämie |
| §51 Negativpreisregel | reduces the AW, so it reaches the Marktprämie; size test aggregated per §24 (§51 Abs. 2 Satz 2) |
| §51 Bestandsschutz | §100 Abs. 1 Satz 4 EEG 2017 — boundary 2016-01-01 |
| §51b mechanism | `verringert sich der anzulegende Wert auf null` — AW = 0 (§51b Satz 1 EEG 2023) |
| §52 Pflichtzahlungen | €10/kW/month; §52 Abs. 3 retroactive €2/kW |
| §53 Vergütungsabzug | Solar/Wind: −0.4 ct; Biomasse/Wasser/Gas: −0.2 ct |
| §100 Übergangsregelung | Settlement rules resolved per applicable §100 transition provisions |

---

## Legal disclaimer

This library implements a deterministic computation of EEG/KWKG settlement rules
based on the cited statutory provisions.

Certain provisions — particularly those subject to evolving case law, BNetzA guidance,
or Clearingstelle EEG|KWKG interpretations — may admit multiple legally defensible
readings. Where applicable, this library documents its chosen interpretation and notes
where alternatives exist.

**Users remain responsible for:**
- Validating the chosen interpretation against the EEG/KWKG version applicable to their specific settlement scenario
- Confirming correctness against current BNetzA guidance and publications
- Consulting Clearingstelle EEG|KWKG rulings where relevant
- Obtaining qualified legal advice before using this library in contested settlements

The library has not been validated against official DSO settlement examples, BNetzA benchmark calculations, or Clearingstelle decisions. It is production-oriented but not independently legally certified.

Source: EEG 2023 Clearingstelle EEG|KWKG working text (23.12.2025). Cite as: *Clearingstelle EEG|KWKG, Arbeitsausgabe EEG 2023.*

---

## Testing

```bash
cargo test -p eeg-billing --all-features
# lib tests + proptest integration + regulatory showcase + doctests
```

The regulatory showcase (`tests/regulatory_showcase.rs`) is executable documentation
for every §§ rule, including the Anlage 1 Marktprämie formula,
§51 version-specific thresholds, §52 Abs. 6 netting, §100 Übergangsregelung,
and all settlement scheme edge cases.

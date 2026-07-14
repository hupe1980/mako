# eeg-billing

**Pure EEG/KWKG feed-in settlement calculation for German energy markets.**

`eeg-billing` is the settlement arithmetic core used by [`einsd`](../../services/einsd/) —
the Einspeiser Registry daemon. It covers the full EEG legal framework from EEG 2000 through
EEG 2023 (Solarpaket I) and KWKG 2023, with all version-specific rule variants enforced
automatically based on the plant's `EegGesetz` year.

**284 tests** · zero I/O · zero async · zero `unsafe` · no float money (`rust_decimal`) ·
MSRV 1.94

---

## Design constraints

| Constraint | Detail |
|---|---|
| **No I/O** | All inputs are passed as arguments. No database calls, no HTTP. |
| **No async** | Synchronous — wraps cheaply in `tokio::task::spawn_blocking`. |
| **No float money** | All amounts use `billing::EuroAmount` (i64 × 10⁻⁵ EUR) internally. |
| **Deterministic** | Same inputs always produce the same output. Pure functions. |
| **EEG-version-aware** | `EegGesetz` enum drives all version-specific rule dispatch. |
| **Domain-rich** | 21 source modules covering settlement, metering, degression, sanctions. |

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
    status,                     ← Calculated / NoData / PriceMissing / ...
}
```

The `SettlementScheme + TariffSource` split reflects the EEG structure directly:

- **`SettlementScheme`** = which formula applies (`FeedInTariff`, `MarketPremium`, …)
- **`TariffSource`** = where the AW comes from (`Statutory`, `Auction(meta)`, `Transitional(rule)`)

`Ausschreibung` is **not** a separate scheme. It is:
`scheme: MarketPremium + tariff_source: Auction(AusschreibungMetadata)`.
The formula is identical to `MarketPremium`; only the AW source and billing label differ.

---

## Settlement schemes

| `SettlementScheme` | EEG basis | Formula |
|---|---|---|
| `FeedInTariff` | §21 EEG | `kwh × verguetungssatz_ct / 100` |
| `MarketPremium` | §20 EEG | `max(0, (AW + Mgmt) − EPEX) × kwh / 100` (see §20 Abs. 3) |
| `TenantElectricity` | §38a EEG 2023 | `kwh × (verguetung + zuschlag) / 100` |
| `PostEeg` | post-20yr | `kwh × EPEX / 100` (configurable `post_eeg_price_floor`) |
| `KwkSurcharge` | §7 KWKG 2023 | `eligible_kwh × rate / 100` (hour-limit cap) |
| `FailsafeTariff` | §21 Abs. 1 Nr. 2 | Ausfallvergütung for mandatory DV plants |
| `Eigenverbrauch` | §38a EEG | EUR 0 — no feed-in remuneration |
| `FlexibilityPremium` | §50b EEG 2023 | `kwh × (verguetung + flex_praemie) / 100` |
| `FlexibilitySurcharge` | §50a EEG 2023 | `kw × rate / 12` (monthly capacity payment) |

### §20 Abs. 3 EEG 2023 — Managementprämie (critical)

The Managementprämie is **incorporated into the AW** before computing the spread.
§20 Abs. 3 EEG 2023: *"der anzulegende Wert um 0,4 ct/kWh zu erhöhen."*

```
eff_AW = direktverm_aw_ct + managementpraemie_ct
Marktprämie = max(0, eff_AW − EPEX) × kwh / 100
```

When `EPEX > eff_AW`, the total payment is **zero** — no guaranteed floor.
This differs from the old EEG ≤2012 model (separate always-paid management fee).

---

## Domain modules (21)

```
eeg-billing/src/
├── formula.rs           Core settlement dispatcher — pure, all §§ rules applied
├── model.rs             SettleInput / SettleOutput / SettlePosition
├── scheme.rs            SettlementScheme, TariffSource, Paragraph100Rule
├── technology.rs        ErzeugungsArt (19 variants), InbetriebnahmeTyp, RepoweringScope
├── version.rs           EegGesetz (8 variants), §51 thresholds and kW-exemption tables
├── rates.rs             Static AW tables: solar PV (Solarpaket I), wind, biomasse, KWKG
├── foerderdauer.rs      foerderendedatum_eeg(), §52 Pflichtzahlung, §51a extension
├── foerderungsende.rs   FoerderendeGrund enum, SanktionStatus lifecycle
│
├── degression.rs        §23a quarterly solar PV degression — Quarter, DegressionTier
├── direktverm.rs        §§20–22 — mandatory threshold, Ausschreibungspflicht, period model
├── metering.rs          Multi-meter Messkonzept — §42b GGV, §14a HT/NT
├── reductions.rs        §§52–54 reduction pipeline — §52 Abs. 6 netting, §53c, §54
├── settlement_state.rs  Monthly lifecycle state machine — Active/Reduced/Suspended/PostEeg
│
├── solar.rs             §48 PV subtypes, §12 Abs. 3 UStG, Agri-PV bonus
├── wind.rs              §36k Korrekturfaktor, WindStandort, Gütegrad/Standortklasse
├── biomasse.rs          §43/§44 fuel classes, Güllekleinanlage (≤75 kW, ≥80% Gülle)
│
├── tariff.rs            billing::Tariff adapter — EegSettleTariff, VAT variants
├── bridge.rs            settlement_to_line_items() → billing::LineItem
└── ust.rs               §12 Abs. 3 UStG, §19 UStG Kleinunternehmer
```

---

## Quick start

```rust
use eeg_billing::{SettleInput, SettlementScheme, SettlementStatus, calculate_settlement};
use rust_decimal_macros::dec;

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
// §20 EEG 2023 — Direktvermarktung
// eff_AW = 6.28 + 0.4 = 6.68 ct; EPEX = 4.50 ct
// Marktprämie = (6.68 − 4.50) × 100,000 / 100 = 2,180 EUR
use eeg_billing::{SettleInput, SettlementScheme, calculate_settlement};
use rust_decimal_macros::dec;

let out = calculate_settlement(&SettleInput {
    scheme: SettlementScheme::MarketPremium,
    einspeisemenge_kwh: Some(dec!(100_000)),
    direktverm_aw_ct: Some(dec!(6.28)),    // statutory AW (before Managementprämie)
    epex_avg_ct_kwh: Some(dec!(4.50)),
    managementpraemie_ct: Some(dec!(0.4)), // §20 Abs. 3: incorporated into AW
    ..SettleInput::default()
});
// 2,180 EUR in two positions: "Gleitende Marktprämie" + "Managementprämie"
```

---

## §51 EEG — Negativpreisregel (version-aware)

| EEG version | Threshold | kW exemption |
|---|---|---|
| EEG ≤2012 | none (Bestandsschutz §66 EEG 2017 Satz 4) | — |
| EEG 2017 | ≥ 6 consecutive hours | Wind < 3 MW; others < 500 kW |
| EEG 2021 | ≥ 4 consecutive hours | all plants < 500 kW |
| EEG 2023 | any negative period | < 100 kW (until iMSys rollout) |

Pass `kwh_during_negative_epex` — the engine applies the correct threshold automatically.

### §51a — Verlängerungsanspruch
Solar PV: `ceil(lost_qh / 2)` · Others: `lost_qh` (1:1). Returned in `verlaengerungsanspruch_qh`.

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

## §23a EEG 2023 — Quarterly solar PV degression

```rust
use eeg_billing::degression::{solar_ueberschuss_rate_for_quarter, DegressionTier, Quarter};
use rust_decimal_macros::dec;

// 9 kWp, Q4 2024 (2 quarters after Solarpaket I), 1% tier → 8.51 × 0.99² = 8.34 ct
let rate = solar_ueberschuss_rate_for_quarter(
    Quarter { year: 2024, quarter: 4 },
    dec!(9),
    DegressionTier::Standard,
);
assert_eq!(rate, Some(dec!(8.34)));
```

---

## §§20–22 EEG 2023 — Direktvermarktung rules

```rust
use eeg_billing::direktverm::{is_direktvermarktung_mandatory, requires_ausschreibung};
use eeg_billing::{EegGesetz, ErzeugungsArt};
use rust_decimal_macros::dec;

assert!(is_direktvermarktung_mandatory(dec!(150), EegGesetz::Eeg2023)); // >100 kW: mandatory
assert!(requires_ausschreibung(dec!(1500), ErzeugungsArt::SolarAufdach)); // >1 MWp: tender
```

---

## ErzeugungsArt (19 variants)

| Variant | Technology | Notes |
|---|---|---|
| `SolarAufdach` | Rooftop PV | Higher §48 rates |
| `SolarFreiflaeche` | Ground-mounted PV | Tender >1 MWp |
| `SolarAgriPv` | Agri-PV | §51a factor 0.5 |
| `SolarMieterstrom` | §38a building solar | — |
| `SolarStecker` | Balkonkraftwerk ≤800 W | Simplified registration |
| `WindOnshore` | Wind onshore | §36k Korrekturfaktor required |
| `WindOffshore` | Wind offshore | Always Ausschreibungspflicht |
| `Biomasse` | Solid biomass | §43 |
| `BiomassHolz` | Wood biomass | §42a restricted |
| `Biogas` | Fermentation biogas | — |
| `Biomethan` | Upgraded biomethane | — |
| `Klaegas` / `Grubengas` / `Deponiegas` | Special gases | §41 EEG |
| `Wasserkraft` | Hydro | — |
| `Geothermie` | Geothermal | — |
| `Gezeiten` | Tidal | — |
| `Kwk` | CHP/BHKW | KWKG 2023, not EEG |

---

## Repowering

```rust
use eeg_billing::{foerderendedatum_repowering, RepoweringScope};
use time::macros::date;

// Full repowering (§22 EEG 2023): Förderdauer resets → 2045-12-31
let new_end = foerderendedatum_repowering(date!(2025-06-01)).unwrap();
assert_eq!(new_end, date!(2045-12-31));

// Rotor-only replacement: Förderdauer does NOT reset
assert!(!RepoweringScope::RotorOnly.resets_foerderdauer_definitely());
// Use foerderendedatum_eeg(original_commissioning_date) for partial repowering
```

---

## Regulatory basis

| Topic | Source |
|---|---|
| EEG 2023 | BGBl. I Nr. 28, 10.01.2023 |
| Solarpaket I | BGBl. I Nr. 107, 16.05.2024 (§48 rates, §51a) |
| KWKG 2023 | BGBl. I Nr. 59, 28.12.2023 |
| §20 Abs. 3 Managementprämie | Incorporated into AW before spread (not a floor) |
| §51 Bestandsschutz | §66 EEG 2017 Satz 4 — boundary 2016-01-01 |
| §52 Pflichtzahlungen | €10/kW/month; §52 Abs. 3 retroactive €2/kW |
| §53 Vergütungsabzug | Solar/Wind: −0.4 ct; Biomasse/Wasser/Gas: −0.2 ct |
| §100 Übergangsregelung | Old plants keep their EEG version's rules permanently |

---

## Testing

```bash
cargo test -p eeg-billing --all-features
# 82 lib tests + 143 regulatory showcase + 59 doctests = 284 total
```

The regulatory showcase (`tests/regulatory_showcase.rs`) is executable documentation
for every §§ rule, including the correct EEG 2023 Managementprämie formula,
§51 version-specific thresholds, §52 Abs. 6 netting, §100 Übergangsregelung,
and all settlement scheme edge cases.

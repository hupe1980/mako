# eeg-billing

**Pure EEG/KWKG feed-in settlement calculation for German energy markets.**

`eeg-billing` is the settlement arithmetic core used by [`einsd`](../../services/einsd/) —
the Einspeiser Registry daemon. It covers the full EEG legal framework from EEG 2000 through
EEG 2023 (Solarpaket I) and KWKG 2023, with all version-specific rule variants enforced
automatically based on the plant's `EegGesetz` year.

**339 tests** · zero I/O · zero async · zero `unsafe` · no float money (`rust_decimal`) ·
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
| **Domain-rich** | Multiple domain modules covering settlement, metering, degression, sanctions, repowering. |

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
| `PostEeg` | post-20yr | `kwh × EPEX / 100` (configurable `post_eeg_price_floor`) |
| `KwkSurcharge` | §7 KWKG 2023 | `eligible_kwh × rate / 100` (hour-limit cap) |
| `TemporaryFeedInTariff` | §21 Abs. 1 Nr. 2 | Ausfallvergütung (temporary feed-in when Direktvermarkter fails) |
| `Eigenverbrauch` | §21 Abs. 3 EEG | No EEG feed-in remuneration is calculated. |
| `FlexibilityPremium` | §50b EEG 2023 | `kwh × (verguetung + flex_praemie) / 100` |
| `FlexibilitySurcharge` | §50a EEG 2023 | `kw × rate / 12` (monthly capacity payment) |

### §20 Abs. 3 EEG 2023 — Managementprämie

**This library's implementation** computes the Managementprämie by **incorporating it into the AW** before the spread calculation. The implementation is based on this reading of §20 Abs. 3 EEG 2023:

> *„Bei der Berechnung der Marktprämie ist der anzulegende Wert um 0,4 Cent pro Kilowattstunde zu erhöhen.“*

Under this reading:

```
eff_AW = direktverm_aw_ct + managementpraemie_ct
Marktprämie = max(0, eff_AW − EPEX) × kwh / 100
```

When `EPEX > eff_AW`, the total payment is **zero** — no guaranteed floor.

> ⚠ **This is one defensible interpretation. Verify before production use.**
> The Managementprämie treatment has evolved across EEG versions and is subject to
> evolving BNetzA guidance and bilateral contract terms. This implementation **must be
> independently verified** against:
> - the EEG version applicable to the specific plant (EEG 2017/2021/2023 differ),
> - current BNetzA published guidance on §20 Abs. 3 EEG 2023, and
> - the contractual framework between Netzbetreiber, Direktvermarkter, and operator.
> Do not rely on this formula for settlement disputes without such verification.

---

## Domain modules

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
// §20 EEG 2023 — Direktvermarktung
// eff_AW = 6.28 + 0.4 = 6.68 ct; EPEX = 4.50 ct
// Marktprämie = (6.68 − 4.50) × 100,000 / 100 = 2,180 EUR
use eeg_billing::{SettleInput, SettlementScheme, calculate_settlement};
use rust_decimal::dec;

let out = calculate_settlement(&SettleInput {
    scheme: SettlementScheme::MarketPremium,
    einspeisemenge_kwh: Some(dec!(100_000)),
    direktverm_aw_ct: Some(dec!(6.28)),    // statutory AW (before Managementprämie)
    epex_avg_ct_kwh: Some(dec!(4.50)),
    managementpraemie_ct: Some(dec!(0.4)), // §20 Abs. 3: incorporated into AW
    ..SettleInput::default()
});
// 2,180 EUR; see Managementprämie section for treatment caveats
```

---

## §51 EEG — Negativpreisregel (version-aware)

| EEG version | Threshold | kW exemption |
|---|---|---|
| EEG ≤2012 | none (Bestandsschutz §100 Abs. 1 Satz 4 EEG 2017) | — |
| EEG 2017 | ≥ 6 consecutive hours | Wind < 3 MW; others < 500 kW |
| EEG 2021 | ≥ 4 consecutive hours | all plants < 500 kW |
| EEG 2023 | any negative period | < 100 kW (until iMSys rollout) |

Pass `kwh_during_negative_epex` — the engine applies the correct threshold automatically.

### §51a — Verlängerungsanspruch
Solar PV: `ceil(lost_qh / 2)` · Others: `lost_qh` (1:1). Returned in `verlaengerungsanspruch_qh`.

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
    managementpraemie_ct: Some(dec!(0.4)),
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

## §23a EEG 2023 — Quarterly solar PV degression

```rust
use eeg_billing::degression::{solar_ueberschuss_rate_for_quarter, DegressionTier, Quarter};
use rust_decimal::dec;

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
use rust_decimal::dec;

assert!(is_direktvermarktung_mandatory(dec!(150), EegGesetz::Eeg2023)); // >100 kW: mandatory
assert!(requires_ausschreibung(dec!(1500), ErzeugungsArt::SolarAufdach)); // >1 MWp: tender
```

---

## ErzeugungsArt

| Variant | Technology | Notes |
|---|---|---|
| `SolarAufdach` | Rooftop PV | Higher §48 rates |
| `SolarFreiflaeche` | Ground-mounted PV | Tender >1 MWp |
| `SolarAgriPv` | Agri-PV | §51a factor 0.5 |
| `SolarMieterstrom` | §21 Abs. 3 building solar | — |
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
7. §53b regional reduction / §54 Ausschreibungsreduzierung
         │
         ▼
8. §52 Abs. 6 netting (optional: NB deducts penalty from disbursement)
         │
         ▼
9. SettleOutput { settlement_eur, eligible_kwh, positions, pflichtzahlung_eur, faelligkeitsdatum }
```

VAT is applied by the caller via `EegSettleTariff` + `ust::ust_tax_layers()` — not
inside `calculate_settlement`. Every status yields exactly one tax layer, including
the two that charge nothing:

| `VatStatus` | Rate | EN 16931 category | Basis |
|---|---|---|---|
| `Regelbesteuerung` | 19 % | `S` — Standard | §12 Abs. 1 UStG |
| `BefreitNach12Abs3` | 0 % | `Z` — Zero rated | §12 Abs. 3 UStG (Nullsteuersatz) |
| `Kleinunternehmer` | 0 % | `E` — Exempt | §19 UStG (tax not levied) |

A supply taxed at 0 % is still a taxable supply, so it belongs in the EN 16931
BG-23 VAT breakdown under its own UNTDID 5305 category with a zero tax amount.
Omitting the layer would drop that turnover from the breakdown entirely and
understate the taxable base. §12 Abs. 3 UStG sets a zero *rate* and maps to `Z`;
§19 UStG does not levy the tax at all and maps to `E`, which carries the
exemption reason EN 16931 requires (BT-120).

A document mixing treatments — a 0 % PV feed-in credit beside 19 % NNE grid
charges — cannot use a single status. Build the layers directly and scope each to
its own positions with `FixedRateTax::with_tag`, so each contributes its own
breakdown entry.

---

## Scope

**Explicitly in scope** — tested and **production-oriented**:
- §21 EEG Einspeisevergütung (all EEG versions 2000–2023)
- §20 EEG Gleitende Marktprämie + §§22a/28 Ausschreibung
- §21 Abs. 3 Mieterstrom, §50a/b Flexibilitätsprämie, §7 KWKG
- §51/§51a/§51b Negativpreisregel, §52 sanctions, §53/§53b/§54 reductions
- §19 EInsMan curtailment compensation (separate position, §51 exempt)
- §23a quarterly degression, §36k wind Korrekturfaktor
- §24 multi-block **Anlagenzusammenfassung**: proportional allocation for pre-aggregated plant groups
  > The library computes settlement **after** §24 aggregation has been determined by the caller.
  > The legal aggregation analysis itself (operator identity, location, commissioning window,
  > technology criteria) is **not** performed here — that is the caller's responsibility.
- §42b GGV / §21 Abs. 3 multi-meter Messkonzept
- SettlementType: Initial, Correction (with `original_id`), Reversal
- §25 billing_days_fraction (partial billing periods per §25 Abs. 1 Satz 3)
- §26 Abs. 1 Fälligkeitsdatum (15th of following month, auto-computed)
- `TariffSource::Transitional(Paragraph100Rule)` → `effective_eeg_gesetz()` auto-override

**Intentionally out of scope** (caller's responsibility):
- §21b monthly switch enforcement — enforced by `einsd` (`validate_switch_to_vergütung`)
- §53b/§54 DB lookups — resolved by `einsd` before calling `calculate_settlement`
- §55 Pönalen computation — `einsd` tracks commissioning deadlines
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
| Solarpaket I | BGBl. I Nr. 107, 16.05.2024 (§48 rates, §51a) |
| KWKG 2023 | BGBl. I Nr. 59, 28.12.2023 |
| §20 Abs. 3 Managementprämie | Incorporated into AW before spread (not a floor) — **verify against BNetzA guidance** |
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
# 89 lib + 12 integration + 173 regulatory showcase + 65 doctests = 339 total
```

The regulatory showcase (`tests/regulatory_showcase.rs`) is executable documentation
for every §§ rule, including the correct EEG 2023 Managementprämie formula,
§51 version-specific thresholds, §52 Abs. 6 netting, §100 Übergangsregelung,
and all settlement scheme edge cases.

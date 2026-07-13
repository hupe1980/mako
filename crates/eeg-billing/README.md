# eeg-billing

**Pure EEG/KWKG feed-in settlement library for German energy markets.**

`eeg-billing` implements all nine settlement models mandated by EEG 2000–2024
(Solarpaket I) and §7 KWKG 2023. It is the calculation core used by
[`einsd`](../../services/einsd/) — the Einspeiser Registry daemon.

---

## Design constraints

| Constraint | Detail |
|---|---|
| **No I/O** | All inputs are passed as arguments. No database calls, no HTTP. |
| **No async** | Synchronous throughout — wraps cheaply in `tokio::task::spawn_blocking` if needed. |
| **No float money** | All monetary amounts use `rust_decimal` with fixed precision. |
| **Deterministic** | Same inputs always produce the same output. |
| **EEG-version-aware** | Rules and rates dispatch on `EegGesetz` enum (Eeg2000–Eeg2023 + Kwkg). |

---

## Settlement models

| # | `SettlementModel` | Regulatory basis |
|---|---|---|
| 1 | `Verguetung` | §21 EEG 2023 (§16 EEG 2012, §18 EEG 2017) |
| 2 | `Mieterstrom` | §38a EEG 2023 |
| 3 | `DirektvermarktungGleitend` | §20 EEG 2023 + §20 Abs. 3 Managementprämie |
| 4 | `Ausschreibung` | §§22a, 28 EEG 2023 |
| 5 | `PostEegSpot` | §21b EEG 2023 (expired Förderung) |
| 6 | `Eigenverbrauch` | §42 EEG 2023 |
| 7 | `KwkgZuschlag` | §7 KWKG 2023 |
| 8 | `Flexibilitaetspraemie` | §50 EEG 2023 |
| 9 | `Flexibilitaetszuschlag` | §50b EEG 2023 |

---

## Regulatory features

| Feature | Basis |
|---|---|
| §51 Negativpreisregel — suspend settlement during 6/4/any consecutive negative-price hours | §51 EEG 2023 / §24 EEG 2017 / §6 EEG 2012; version-aware thresholds via `EegGesetz` |
| §52 Pflichtzahlungen — `SanktionsTyp` (€10/kW or €2/kW per §52 Abs. 3 Nr. 2) + `SanktionAlt` (3-tier old-regime) | §52 EEG 2023 |
| §53 Vergütungsabzug — 0.4 ct solar/wind, 0.2 ct biomasse/gas | §53 EEG 2023 |
| §25 MaStR Bestandsschutz — blocks settlement for unregistered Anlagen after deadline | §25 EEG 2023 |
| §27 Mehr-/Mindererzeugung | §27 EEG 2023 |
| Repowering §22 / Zusammenlegung §24 | §§22, 24 EEG 2023 |
| `foerderendedatum_eeg()` = Dec 31 of year+20 (statutory) | §25 Abs. 1 Satz 2 EEG 2023 |
| `foerderendedatum_eeg_ausschreibung()` = exact 20 years (Ausschreibung) | §22 EEG 2023 |
| §12 Abs. 3 UStG (PV Nullsteuersatz) via `mwst_rate_override` | §12 Abs. 3 UStG (seit 01.01.2023) |

---

## Usage

```rust
use eeg_billing::{
    calculate_settlement, SettlementInput, SettlementModel,
    EegGesetz, ErzeugungsArt,
};
use rust_decimal_macros::dec;
use time::macros::date;

let input = SettlementInput {
    gesetz: EegGesetz::Eeg2023,
    model: SettlementModel::DirektvermarktungGleitend,
    erzeugungsart: ErzeugungsArt::Photovoltaik,
    installed_kw: dec!(100),
    // kWh produced in the settlement period
    kwh_produced: dec!(850),
    // Marktwert Solar from BDEW monthly table
    marktwert_ct_kwh: dec!(8.34),
    // Anzulegender Wert (from Ausschreibungsergebnis or statutory table)
    anzulegender_wert_ct_kwh: dec!(9.20),
    period_start: date!(2026-06-01),
    period_end: date!(2026-06-30),
    managementpraemie_ct_kwh: Some(dec!(0.40)), // §20 Abs. 3
    ..Default::default()
};

let result = calculate_settlement(&input)?;
println!("Marktprämie: {} EUR", result.total_eur());
for pos in &result.positions {
    println!("  {}: {} ct/kWh × {} kWh", pos.name, pos.rate_ct_kwh, pos.kwh);
}
```

---

## `EegGesetz` variants

| Variant | Applies to Anlagen commissioned |
|---|---|
| `Eeg2000` | before 2012-01-01 |
| `Eeg2012` | 2012-01-01 to 2016-12-31 |
| `Eeg2017` | 2017-01-01 to 2020-12-31 |
| `Eeg2021` | 2021-01-01 to 2022-12-31 |
| `Eeg2023` | 2023-01-01+ (including Solarpaket I) |
| `Kwkg` | BHKW/KWK Anlagen under KWKG |

---

## `ErzeugungsArt` variants (20 types)

`Photovoltaik`, `PhotovoltaikFreiflaeche`, `Wind`, `WindOffshore`,
`Biomasse`, `BiomasseFluessig`, `Biogas`, `Geothermie`,
`Wasserkraft`, `Deponie`, `Klaergas`, `Grubengas`, `Klaeranlagengas`,
`SonstigeGase`, `SonstigeBiomasse`, `SonstigeErneuerbare`,
`KwkgBhkw`, `KwkgDampf`, `KwkgGasmotor`, `KwkgBrennstoffzelle`

---

## Regulatory basis

- **EEG 2023** (BGBl. I 2023 Nr. 138, in force 29.07.2023) — Solarpaket I amendments
- **KWKG 2023** (BGBl. I 2023 Nr. 396, in force 29.12.2023)
- **BNetzA rate tables** — published annually at bundesnetzagentur.de
- **§51 thresholds** — version-resolved: EEG 2012 (6h Wind<3MW / other<500kW),
  EEG 2017 (6h/<500kW), EEG 2021 (4h/<500kW), EEG 2023 (any/<100kW)
- **§66 EEG 2017 Bestandsschutz boundary** — 2016-01-01

---

## Testing

```bash
cargo test -p eeg-billing --all-features
```

190+ unit tests covering all 9 models, all EEG versions, §51 edge cases,
§52 Pflichtzahlungen, DST-safe date arithmetic, and §25 MaStR sanctions.

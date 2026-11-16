# eeg-billing

[![Crates.io](https://img.shields.io/crates/v/eeg-billing?label=eeg-billing&color=f59e0b&logo=rust)](https://crates.io/crates/eeg-billing)

EEG and KWKG feed-in settlement for German renewable and CHP plants, EEG 2000
through EEG 2023, for the Netzbetreiber paying them. `calculate_settlement` is
a pure function from one `SettleInput` to a `SettleOutput`, every
version-specific rule selected by the plant's `EegGesetz`; money is `Decimal`,
range-checked as fixed-point EUR. Award validity, § 55 Pönalen, receipt
archival and payment dispatch are the caller's.

## Example

```rust
use eeg_billing::{SettleInput, SettlementScheme, SettlementStatus, calculate_settlement};
use rust_decimal::dec;

// § 21 EEG 2023 — 500 kWh × 8.11 ct/kWh = 40.55 EUR
let out = calculate_settlement(&SettleInput {
    scheme: SettlementScheme::FeedInTariff { verguetungssatz_ct: dec!(8.11) },
    einspeisemenge_kwh: Some(dec!(500)),
    ..SettleInput::default()
});
assert_eq!(out.status, SettlementStatus::Calculated);
assert_eq!(out.settlement_eur, Some(dec!(40.55)));
```

## What it covers

| `SettlementScheme` | Basis | Formula |
|---|---|---|
| `FeedInTariff` | § 21 EEG | `kwh × verguetungssatz_ct / 100` |
| `TemporaryFeedInTariff` | § 21 Abs. 1 Satz 1 Nr. 3 EEG | Ausfallvergütung, AW cut by 20 % (§ 53 Abs. 3) |
| `MarketPremium` | § 23a EEG i.V.m. Anlage 1 | `max(0, AW − MW) × kwh / 100`, no Managementprämie |
| `TenantElectricity` | § 21 Abs. 3 EEG 2023 | Vergütung plus Mieterstromzuschlag |
| `PostEeg` | § 21 EEG, after the Förderung | `kwh × EPEX / 100` with a configurable floor |
| `KwkSurcharge` | §§ 7, 8 KWKG | Mischsatz, capped by Vollbenutzungsstunden and the § 8 Abs. 4 annual cap |
| `FlexibilityPremium` | § 50b EEG 2023 | Vergütung plus Flexibilitätsprämie |
| `FlexibilitySurcharge` | § 50a EEG 2023 | `kw × rate / 12` |
| `Eigenverbrauch` | — | no feed-in remuneration |
| `SonstigeDirektvermarktung` | § 21a EEG 2023 | EUR 0, no NB payment; period recorded |

A tender is `MarketPremium` with `TariffSource::Auction`, not a scheme of its
own. Around the formula:

| Module | Rule |
|---|---|
| `aw_reductions` | §§ 53b, 53c, 54 EEG 2023 reduce the anzulegender Wert before the formula, never the euro result |
| `negativpreis` | § 51 Negativpreisregel, keyed on the Inbetriebnahmedatum |
| `degression`, `wind` | § 49 solar degression, § 36h Korrekturfaktor |
| `kwkg` | §§ 7–8 KWKG Zuschlag and Jahreskontingent |
| `zusammenfassung` | § 24 EEG 2023 Anlagenzusammenfassung |
| `ust` | § 19 UStG Kleinunternehmer (no USt) or Regelbesteuerung, a declared operator status |

`SettleOutput` carries `positions`, the § 52 Pflichtzahlung, the § 51a
Verlängerungsanspruch and a `status` (`Calculated`, `NoData`, `PriceMissing`,
`FoerderungBeendet`, …).

## Features

| Feature | Enables |
|---|---|
| `serde` | `Serialize` / `Deserialize` on the model |
| `bo4e` | `gutschrift::settlement_to_gutschrift`, the § 14 Abs. 2 UStG Gutschrift as a BO4E `Rechnung` |

## Sources

- EEG 2000–2023, KWKG
- UStG §§ 14, 19

The crate documents the interpretation it chose where a provision admits
several; it is not independently legally certified. The executable rule
showcase is `tests/it/regulatory_showcase.rs`.

Operator reference: [einsd](https://hupe1980.github.io/mako/docs/services/einsd/) ·
API: <https://docs.rs/eeg-billing>

License: MIT OR Apache-2.0

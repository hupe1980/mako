# eeg-billing

[![Crates.io](https://img.shields.io/crates/v/eeg-billing?label=eeg-billing&color=f59e0b&logo=rust)](https://crates.io/crates/eeg-billing)

EEG and KWKG feed-in settlement for German renewable and CHP plants, EEG 2000
through EEG 2023. `calculate_settlement` is a pure function from one
`SettleInput` to a `SettleOutput` — itemised positions, § 52 Pflichtzahlung,
§ 51a extension and status — with every version-specific rule selected by the
plant's `EegGesetz`. No I/O, no async, money in `Decimal`. It is the settlement
core of [`einsd`](https://hupe1980.github.io/mako/docs/services/einsd/).

```bash
cargo add eeg-billing rust_decimal
```

## Example

```rust
use eeg_billing::{SettleInput, SettlementScheme, SettlementStatus, calculate_settlement};
use rust_decimal::dec;

// §21 EEG 2023 — 500 kWh × 8.11 ct/kWh = 40.55 EUR
let out = calculate_settlement(&SettleInput {
    scheme: SettlementScheme::FeedInTariff { verguetungssatz_ct: dec!(8.11) },
    einspeisemenge_kwh: Some(dec!(500)),
    ..SettleInput::default()
});
assert_eq!(out.status, SettlementStatus::Calculated);
assert_eq!(out.settlement_eur, Some(dec!(40.55)));
```

## Key types

| Item | Purpose |
|---|---|
| `SettlementScheme` | which formula applies — each variant carries the figures its formula reads |
| `TariffSource` | where the anzulegender Wert comes from: `Statutory`, `Auction(…)`, `Transitional(Paragraph100Rule)`. A tender is `MarketPremium` + `Auction`, not a scheme of its own |
| `SettleOutput` | `settlement_eur`, `eligible_kwh`, `positions`, `pflichtzahlung_eur`, `verlaengerungsanspruch_qh`, `faelligkeitsdatum`, `status` (`Calculated`, `NoData`, `PriceMissing`, …) |
| `aw_reductions` | §§ 53b, 53c, 54 — cuts to the anzulegender Wert, applied before the formula |
| `negativpreis`, `degression`, `wind`, `kwkg`, `zusammenfassung` | § 51, § 49, § 36h, §§ 7–9 KWKG, § 24 Abs. 1 |
| feature `bo4e` | `gutschrift::settlement_to_gutschrift` — the § 14 Abs. 2 UStG Gutschrift as a BO4E `Rechnung` |

| `SettlementScheme` | EEG basis | Formula |
|---|---|---|
| `FeedInTariff` | §21 EEG | `kwh × verguetungssatz_ct / 100` |
| `MarketPremium` | §23a EEG i.V.m. Anlage 1 | `max(0, AW − MW) × kwh / 100` — no additive Managementprämie (see below) |
| `TenantElectricity` | §21 Abs. 3 EEG 2023 | `kwh × (verguetung + zuschlag) / 100` |
| `PostEeg` | post-20yr | `kwh × EPEX / 100` (configurable `price_floor` on the variant) |
| `KwkSurcharge` | §§7, 8 KWKG | `eligible_kwh × Mischsatz / 100`, capped by the §8 Abs. 1–3 Vollbenutzungsstunden (`FoerderungBeendet`) and, separately, by the §8 Abs. 4 annual cap (`JahreskontingentErschoepft` — the year is used up, the Förderung is not) |
| `TemporaryFeedInTariff` | §21 Abs. 1 Satz 1 Nr. 3 | Ausfallvergütung (temporary feed-in when the Direktvermarkter drops out) |
| `Eigenverbrauch` | §21 Abs. 3 EEG | No EEG feed-in remuneration is calculated. |
| `FlexibilityPremium` | §50b EEG 2023 | `kwh × (verguetung + flex_praemie) / 100` |
| `FlexibilitySurcharge` | §50a EEG 2023 | `kw × rate / 12` (monthly capacity payment) |
| `SonstigeDirektvermarktung` | §21a EEG | EUR 0 — direct third-party sale, no NB payment; period recorded in settlement history |

The Marktprämie is `max(0, AW − MW)` (Anlage 1 Nr. 3.1.2) with no additive
Managementprämie. Award validity, § 55 Pönalen, receipt archival and payment
dispatch are the caller's (`einsd`, `accountingd`).

The library documents the interpretation it chose where a provision admits
several; it is not independently legally certified. API documentation:
<https://docs.rs/eeg-billing>; the executable rule showcase is
`tests/it/regulatory_showcase.rs`.

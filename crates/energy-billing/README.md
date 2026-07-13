# energy-billing

**Pure multi-product retail energy billing library for German markets.**

`energy-billing` is the calculation core used by
[`billingd`](../../services/billingd/) — the Energy Billing Engine daemon (LF role).
All commercial prices are user-defined in `tarifbd`; this crate handles only the
arithmetic, regulatory rate application, and BO4E `Rechnung` JSON generation.

---

## Design constraints

| Constraint | Detail |
|---|---|
| **No I/O** | All inputs are passed as arguments. No database calls, no HTTP. |
| **No async** | Synchronous throughout. |
| **No float money** | All monetary amounts use `rust_decimal` with fixed precision. |
| **Deterministic** | Same inputs always produce the same output. |
| **`#[must_use]`** | All `calculate_*` functions are annotated `#[must_use]`. |

---

## Product categories

| `calculate_*` function | Category | Key features |
|---|---|---|
| `calculate_strom` | `STROM` | Eintarif / Zweitarif HT/NT / RLM Leistungspreis; §14a Modul 1/3 Steuerungsrabatt; EEG-Umlage |
| `calculate_dynamic_strom` | `STROM` (§41a) | 15-min Lastgang × hourly EPEX Spot; optional price floor (`dynamic_epex_floor_ct_kwh`); §14a compatible |
| `calculate_gas` | `GAS` | Brennwertkorrektur §10 GasGVV; Energiesteuer §2 EnergieStG; BEHG CO₂; H2-blend `gasqualitaet` audit annotation |
| `calculate_waerme` | `WAERME` | Fernwärme (Arbeitspreis + Grundpreis + Leistungspreis) |
| `calculate_solar` | `SOLAR` | Mieterstrom §42b EEG; Gemeinschaftliche Gebäudeversorgung §42a EEG; §12 Abs. 3 UStG Nullsteuersatz |
| `calculate_eeg` | `EEG` | EEG Gutschrift; §51 Negativpreisregel contractual feature via `kwh_during_negative_epex` |
| `calculate_einspeisung` | `EINSPEISUNG` | Feed-in credit note (Einspeisevergütung) |
| `calculate_waermepumpe` | `WAERMEPUMPE` | §14a Modul 1/3 Wärmepumpe reduced-rate |
| `calculate_wallbox` | `WALLBOX` | §14a Modul 1/3 Ladeeinrichtung reduced-rate |
| `calculate_hems` | `HEMS` | Home Energy Management System subscription + optional event-based charges |
| `calculate_emobility` | `EMOBILITY` | E-mobility charging (AC/DC) with optional kWh measurement |
| `calculate_energiedienstleistung` | `ENERGIEDIENSTLEISTUNG` | Custom energy service fee (flat or per-kWh) |

---

## §41a EPEX dynamic tariffs

`calculate_dynamic_strom` multiplies 15-minute meter intervals against hourly EPEX
Spot prices fetched from `tarifbd`. An optional floor prevents credits during
deeply negative hours:

```rust
let tariff = DynamicStromTariff {
    epex_markup_ct_kwh: dec!(2.50),          // markup above spot price
    network_costs_ct_kwh: dec!(8.20),        // NNE pass-through
    dynamic_epex_floor_ct_kwh: Some(dec!(5.00)), // floor: spot never below 5 ct/kWh
    ..Default::default()
};
```

Set `dynamic_epex_floor_ct_kwh: Some(Decimal::ZERO)` to allow zero but block negative
credits. Set `None` to pass through all negative spot prices as credits.

---

## §51 EEG Negativpreisregel (contractual LF feature)

For LF retail contracts that pass through EEG Gutschrift, the optional
`kwh_during_negative_epex` field reduces the credited kWh. Unlike `eeg-billing`
(which implements the mandatory §51 rule for NB/Einspeiser), this is a
**contractual feature** only — the LF is not legally required to apply §51.

```rust
let input = EegMeterInput {
    kwh_total: dec!(320.0),
    kwh_during_negative_epex: Some(dec!(40.0)), // suspended hours per contract
    ..Default::default()
};
```

---

## `BillingResult`

All `calculate_*` functions return `Result<BillingResult, BillingError>`.
`BillingResult` is `#[non_exhaustive]` and provides:

```rust
let result = calculate_strom(&tariff, &meter)?;
result.assert_valid();                                     // panics on internal invariant breach
let nne_total = result.levy_total_eur();                   // sum of NNE/KA/Abgaben positions
let positions = result.positions_by_tag("14a").collect::<Vec<_>>(); // filter by tag
let base_total = result.position_total_by_tag("base");
let rechnung_json: serde_json::Value = result.rechnung_json(meta)?; // BO4E Rechnung
```

---

## BO4E output

`rechnung_json()` returns a `serde_json::Value` shaped as `rubo4e::current::Rechnung`
with `rechnungsempfaenger`, `zahlungsziel`, and typed `Rechnungsposition` entries.
Pass this directly to `billingd`'s persistence layer or the XRechnung/ZUGFeRD renderer.

---

## Usage

```toml
[dependencies]
energy-billing = { path = "../crates/energy-billing" }
```

```rust
use energy_billing::{calculate_strom, StromTariff, StromMeterInput};
use rust_decimal_macros::dec;

let tariff = StromTariff {
    arbeitspreis_ht_ct_kwh: dec!(28.50),
    arbeitspreis_nt_ct_kwh: dec!(22.00),
    grundpreis_eur_month: dec!(9.95),
    ..Default::default()
};

let meter = StromMeterInput {
    kwh_ht: dec!(220.0),
    kwh_nt: dec!(80.0),
    billing_months: dec!(1),
    ..Default::default()
};

let result = calculate_strom(&tariff, &meter)?;
println!("Total: {} EUR", result.total_eur_gross());
```

---

## Regulatory basis

- **§14a EnWG** — Steuerung steuerbarer Verbrauchseinrichtungen (Modul 1/3 discounts, BK6-24-174)
- **§41a EnWG** — Dynamic EPEX tariffs (mandatory offer from 2025)
- **§3 StromStG** — Stromsteuer 2.050 ct/kWh (Regelsteuersatz)
- **§2 EnergieStG** — Erdgassteuer 0.55 ct/kWh
- **BEHG** — Brennstoffemissionshandel CO₂-Preis (Gas, update annually)
- **§10 GasGVV** — Brennwertkorrektur (Zustandszahl, Brennwert)
- **§42b / §42a EEG 2023** — Mieterstrom / Gemeinschaftliche Gebäudeversorgung
- **§12 Abs. 3 UStG** — PV Nullsteuersatz (since 01.01.2023)
- **§51 EEG 2023** — Negativpreisregel (contractual LF feature only)

---

## Testing

```bash
cargo test -p energy-billing --all-features
```

44 tests covering all 12 product categories, §14a Modul 1/3, §41a floor,
§51 suspension, `BillingResult` helper methods, rechnung_json, and XRechnung output.

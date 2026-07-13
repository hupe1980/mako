//! Pure EEG/KWKG feed-in settlement calculation for German energy markets.
//!
//! Implements all settlement models defined in **EEG** (multiple versions 2000–2024)
//! and **KWKG 2023**. Zero I/O, zero async — every function is deterministic
//! and synchronous. No floating-point money: all monetary arithmetic uses
//! [`billing::EuroAmount`] (`i64 × 10⁻⁵ EUR`) internally.
//!
//! # Settlement models (9)
//!
//! | Variant | Formula | Legal basis |
//! |---|---|---|
//! | [`SettlementModel::Verguetung`] | `kwh × rate_ct / 100` | §21 EEG |
//! | [`SettlementModel::Mieterstrom`] | Vergütung + `kwh × zuschlag_ct / 100` | §38a EEG 2023 |
//! | [`SettlementModel::Direktvermarktung`] | `max(0, AW−EPEX) × kwh / 100 + Mgmt.prämie` | §20 EEG |
//! | [`SettlementModel::Ausschreibung`] | same as Direktvermarktung (BNetzA tender AW) | §§22a,28 EEG 2023 |
//! | [`SettlementModel::PostEegSpot`] | `kwh × EPEX / 100` (§23b cap: 10 ct) | §21 EEG (post-Förderung) |
//! | [`SettlementModel::Eigenverbrauch`] | EUR 0 | §38a EEG (self-consumption) |
//! | [`SettlementModel::KwkgZuschlag`] | `eligible_kwh × kwk_ct / 100` (hour-limit capped) | §7 KWKG 2023 |
//! | [`SettlementModel::Flexibilitaet`] | Vergütung + `kwh × flex_ct / 100` | §50b EEG 2023 (bestehende Anlagen) |
//! | [`SettlementModel::FlexibilitaetZuschlag`] | `kw × rate / 12` (monthly capacity payment) | §50a EEG 2023 (neue Anlagen) |
//!
//! # One formula — all EEG versions (2000–2024)
//!
//! **No separate tariff per EEG version is needed.** The settlement formula is
//! identical across all EEG versions. What differs between versions:
//!
//! 1. **Vergütungssatz (rate)** — fixed at commissioning for 20 years; caller provides it.
//!    Use [`rates::solar_pv_ueberschuss_lookup`] or `einsd`'s `lookup_verguetungssatz`.
//!
//! 2. **§51 Negativpreisregel** — guard applied automatically from `inbetriebnahme`.
//!    EEG 2023: any negative hour. Pre-2023 EEG 2017–2021: ≥6 consecutive hours.
//!
//! 3. **§100 EEG 2023 Übergangsregelung** — old plants continue under their EEG version's
//!    rules (§100 Abs. 1). The rate is the only thing that changes per plant.
//!
//! # Umsatzsteuer (not Mehrwertsteuer)
//!
//! "Umsatzsteuer" (USt) is the legal term; "Mehrwertsteuer" (MwSt) is colloquial.
//! Three distinct situations — see [`ust`]:
//! - **§12 Abs. 3 UStG**: Solar PV ≤30 kWp, after 01.01.2023 → **no USt**
//! - **§19 UStG Kleinunternehmer**: turnover ≤€25 000/yr → **no USt**
//! - **Regelbesteuerung**: all others → **19 % USt**
//!
//! Use [`ust::ust_tax_layers`] to get the right `billing::TaxLayer` for a document.
//!
//! # Multi-EEG-version support
//!
//! Supply `inbetriebnahme` and `leistung_kwp` in [`SettleInput`] for automatic
//! version-specific rule enforcement:
//! - §51 Negativpreisregel guard (≥100 kWp, after 2016-01-01)
//! - Automatic `FoerderungBeendet` when `billing_date > foerderendedatum`
//!
//! # §24 EEG Anlagenerweiterung
//!
//! Plants extended with additional capacity blocks use [`CapacityBlock`].
//! Settlement is proportionally allocated across all blocks by installed kWp.
//!
//! # Quick start
//!
//! ```rust
//! use eeg_billing::{SettleInput, SettlementModel, calculate_settlement, SettlementStatus};
//! use rust_decimal::Decimal;
//! use std::str::FromStr;
//!
//! fn d(s: &str) -> Decimal { Decimal::from_str(s).unwrap() }
//!
//! // §21 EEG 2023 — 100 kWh × 8.51 ct/kWh (Solarpaket I, ≤10 kWp Überschuss) = 8.51 EUR
//! let out = calculate_settlement(&SettleInput {
//!     model: SettlementModel::Verguetung,
//!     einspeisemenge_kwh: Some(d("100")),
//!     verguetungssatz_ct: d("8.51"),
//!     ..SettleInput::default()
//! });
//! assert_eq!(out.status, SettlementStatus::Calculated);
//! assert_eq!(out.settlement_eur, Some(d("8.51")));
//! ```
#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod bridge;
mod error;
pub mod foerderdauer;
mod formula;
mod model;
pub mod rates;
pub mod tariff;
pub mod technology;
pub mod ust;
pub mod version;

pub use error::SettlementError;
pub use foerderdauer::{
    calculate_pflichtzahlung, foerderendedatum_eeg, foerderendedatum_eeg_ausschreibung,
    foerderendedatum_kwkg_years, foerderendedatum_repowering, kwk_eligible_kwh,
    kwk_foerderend_calendar, kwk_max_kwh, managementpraemie_ct, negativpreis_kw_exemption,
    negativpreis_rule_applies, negativpreis_rule_applies_for_version,
    verguetungszeitraum_verlaengerung_qh, zusammenlegung_within_12_months,
};
pub use formula::calculate_settlement;
pub use model::{
    CapacityBlock, Messkonzept, Pflichtverstoss, SanktionAlt, SanktionsTyp, SettleInput,
    SettleOutput, SettlePosition, SettlementModel, SettlementStatus,
};
pub use technology::{ErzeugungsArt, InvalidErzeugungsArt};
pub use version::{EegGesetz, InvalidEegGesetz};

//! Pure EEG/KWKG feed-in settlement calculation for German energy markets.
//!
//! Implements all settlement models defined in **EEG** (multiple versions 2000–2024)
//! and **KWKG 2023**. Zero I/O, zero async — every function is deterministic
//! and synchronous. No floating-point money: all monetary arithmetic uses
//! [`billing::EuroAmount`] (`i64 × 10⁻⁵ EUR`) internally.
//!
//! # Settlement schemes (9)
//!
//! | `SettlementScheme` | Formula | Legal basis |
//! |---|---|---|
//! | `FeedInTariff` | `kwh × verguetungssatz_ct / 100` | §21 EEG |
//! | `TenantElectricity` | Vergütung + `kwh × mieter_zuschlag_ct / 100` | §21 Abs. 3 EEG 2023 |
//! | `MarketPremium` | `max(0, (AW+Mgmt) − EPEX) × kwh / 100` (§20 Abs. 3) | §20 EEG |
//! | `MarketPremium` + `TariffSource::Auction` | same formula, AW from BNetzA tender | §§22a,28 EEG 2023 |
//! | `PostEeg` | `kwh × EPEX / 100` (§23b cap: 10 ct; configurable floor) | §21 EEG (post-Förderung) |
//! | `Eigenverbrauch` | EUR 0 (no feed-in remuneration) | §21 Abs. 3 EEG |
//! | `KwkSurcharge` | `eligible_kwh × rate / 100` (hour-limit cap) | §7 KWKG 2023 |
//! | `FlexibilityPremium` | Vergütung + `kwh × flex_praemie_ct / 100` | §50b EEG 2023 (bestehende Anlagen) |
//! | `FlexibilitySurcharge` | `kw × rate / 12` (monthly capacity payment) | §50a EEG 2023 (neue Anlagen) |
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
//! use eeg_billing::{SettleInput, SettlementScheme, calculate_settlement, SettlementStatus};
//! use rust_decimal::Decimal;
//! use std::str::FromStr;
//!
//! fn d(s: &str) -> Decimal { Decimal::from_str(s).unwrap() }
//!
//! // §21 EEG 2023 — 100 kWh × 8.51 ct/kWh (Solarpaket I, ≤10 kWp Überschuss) = 8.51 EUR
//! let out = calculate_settlement(&SettleInput {
//!     scheme: eeg_billing::SettlementScheme::FeedInTariff { verguetungssatz_ct: d("8.51") },
//!     einspeisemenge_kwh: Some(d("100")),
//!     ..SettleInput::default()
//! });
//! assert_eq!(out.status, SettlementStatus::Calculated);
//! assert_eq!(out.settlement_eur, Some(d("8.51")));
//! ```
#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod biomasse;
pub mod bridge;
pub mod degression;
pub mod direktverm;
mod error;
pub mod foerderdauer;
pub mod foerderungsende;
mod formula;
pub mod metering;
mod model;
pub mod rates;
pub mod reductions;
pub mod scheme;
pub mod settlement_state;
pub mod solar;
pub mod tariff;
pub mod technology;
pub mod ust;
pub mod version;
pub mod wind;

pub use error::SettlementError;
pub use foerderdauer::{
    calculate_pflichtzahlung, compute_billing_days_fraction, foerderendedatum_eeg,
    foerderendedatum_eeg_ausschreibung, foerderendedatum_kwkg_years, foerderendedatum_repowering,
    kwk_eligible_kwh, kwk_foerderend_calendar, kwk_max_kwh, managementpraemie_ct,
    negativpreis_kw_exemption, negativpreis_rule_applies, negativpreis_rule_applies_for_version,
    pflichtzahlung_verjaehrt_am, sect52a_netztrennung_erforderlich,
    verguetungszeitraum_verlaengerung_qh, wind_onshore_korrekturfaktor_corrected_aw,
    zusammenlegung_within_12_months,
};
pub use formula::calculate_settlement;
pub use model::{
    CapacityBlock, Messkonzept, Pflichtverstoss, SanktionAlt, SanktionsTyp, SettleInput,
    SettleOutput, SettlePosition, SettlementStatus,
};
pub use scheme::{
    AusschreibungMetadata, CorrectionReason, MarktpreisKategorie, Paragraph100Rule,
    SettlementScheme, SettlementType, TariffSource,
};
pub use technology::{
    ErzeugungsArt, InbetriebnahmeTyp, InvalidErzeugungsArt, InvalidInbetriebnahmeTyp,
    RepoweringScope,
};
pub use version::{EegGesetz, InvalidEegGesetz};

// Domain module guide:
// degression: §23a quarterly solar PV tariff degression — Quarter, DegressionTier, apply_degression
// direktverm: §§20–22 Direktvermarktung — mandatory threshold, Ausschreibungspflicht, period model
// metering:   Multi-meter Messkonzept — MeterConfiguration, compute_einspeisemenge, §42b GGV, §14a
// reductions: §§52–54 reduction pipeline — Sect52Netting, Sect53c, Sect54, ReductionPipeline
// settlement_state: Monthly lifecycle state machine — SettlementPeriodState, derive_settlement_state
// solar: §48 EEG PV subtypes, Volleinspeisung/Überschuss, §12 Abs. 3 UStG, Agri-PV
// wind:  §36k Korrekturfaktor, Standortklasse, reference yield model
// biomasse: §43/§44 fuel classes, Güllekleinanlage (≤75 kW, ≥80% Gülle)
// foerderungsende: FoerderendeGrund enum, SanktionStatus lifecycle
// scheme: SettlementScheme, TariffSource, Paragraph100Rule, SettlementType

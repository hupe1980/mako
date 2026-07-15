//! German VAT (Umsatzsteuer) rules for EEG/KWKG feed-in settlements.
//!
//! The EEG itself does **not** regulate VAT — those rules come from the
//! Umsatzsteuergesetz (UStG) and BMF circulars. This module provides
//! helpers for the three distinct situations an EEG plant operator can be in.
//!
//! ## Terminology
//!
//! "Umsatzsteuer" (USt) is the legal term; "Mehrwertsteuer" (MwSt) is the
//! colloquial name. This library uses "Umsatzsteuer" throughout.
//!
//! ## Three VAT situations
//!
//! | Status | Legal basis | USt on Vergütung | Vorsteuerabzug |
//! |---|---|---|---|
//! | `BefreitNach12Abs3` | §12 Abs. 3 UStG (JStG 2022) | **None** | Not applicable |
//! | `Kleinunternehmer` | §19 UStG | **None** | Not applicable |
//! | `Regelbesteuerung` | Standard | **19 %** | Applicable (e.g. installation costs) |
//!
//! ## §12 Abs. 3 UStG — photovoltaic exemption (since 01.01.2023)
//!
//! Plants ≤ **30 kWp** commissioned on or after **01.01.2023** are exempt from
//! all VAT obligations related to the operation of the PV system (Liebhaberei-Erlass
//! replaced by statutory exemption through JStG 2022).
//!
//! - The Netzbetreiber pays the Vergütungssatz WITHOUT adding USt.
//! - The operator does NOT issue a VAT invoice and does NOT register for USt solely
//!   because of the PV plant.
//! - No input-tax deduction on installation costs.
//!
//! ## §19 UStG Kleinunternehmer
//!
//! Operators whose total annual turnover does not exceed **€ 25 000** (from 01.01.2025;
//! previously € 22 000) are treated as Kleinunternehmer and charge no USt on any
//! business income, including EEG feed-in.
//!
//! ## Regelbesteuerung
//!
//! All other operators (large plants, commercial operators, opted-in operators)
//! apply standard USt at **19 %** on the Einspeisevergütung / Marktprämie.
//! The Netzbetreiber pays the gross amount (Netto + USt) and deducts the input tax.
//!
//! ## Usage in billing documents
//!
//! ```rust
//! use eeg_billing::ust::{VatStatus, ust_tax_layers};
//! use billing::{DocumentMeta, Tariff};
//! use eeg_billing::{SettleInput, SettlementScheme, calculate_settlement};
//! use eeg_billing::tariff::EegSettleTariff;
//! use rust_decimal_macros::dec;
//! use time::macros::date;
//!
//! let output = calculate_settlement(&SettleInput {
//!     scheme: eeg_billing::SettlementScheme::FeedInTariff { verguetungssatz_ct: dec!(8.51) },
//!     einspeisemenge_kwh: Some(dec!(500)),
//!     leistung_kwp: Some(dec!(9.5)),
//!     inbetriebnahme: Some(date!(2024-06-01)),
//!     ..SettleInput::default()
//! });
//!
//! // Determine VAT status automatically
//! let vat = VatStatus::from_plant(true, dec!(9.5), Some(date!(2024-06-01)));
//! assert_eq!(vat, VatStatus::BefreitNach12Abs3);
//! assert!(vat.is_exempt());
//!
//! // Build document with no VAT (§12 Abs. 3 exempt)
//! let tariff = EegSettleTariff::new(&output);
//! // tax_layers = [] since EegSettleTariff leaves VAT to the caller:
//! // let layers = ust_tax_layers(vat);  // → empty for BefreitNach12Abs3
//! let _ = tariff; // suppress unused warning in doctest
//! ```
//!
//! ## §100 EEG Übergangsregelung
//!
//! Plants commissioned **before 01.01.2023** are governed by the EEG version that
//! was in force at their commissioning date (§100 Abs. 1 EEG 2023):
//! "sind die Bestimmungen des EEG in der am 31. Dezember 2022 geltenden Fassung
//! anzuwenden."
//!
//! The Vergütungssatz is fixed at commissioning for the full 20-year Förderdauer.
//! VAT rules depend on the *current* UStG (not the EEG version) and the operator's
//! current tax status — these can change independently of the EEG Vergütungssatz.

use billing::TaxLayer;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use time::Date;
use time::macros::date;

// ── VatStatus ─────────────────────────────────────────────────────────────────

/// The operator's German VAT (Umsatzsteuer) status for EEG settlement purposes.
///
/// Determines whether USt appears on the feed-in billing document
/// and at what rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum VatStatus {
    /// **§12 Abs. 3 UStG** — photovoltaic Liebhaberei exemption.
    ///
    /// Applies to solar PV plants with ≤ 30 kWp installed capacity that were
    /// commissioned on or after **01.01.2023** (JStG 2022, BGBl 2022 I S. 2294).
    ///
    /// No USt on Einspeisevergütung; no Vorsteuerabzug on installation costs.
    BefreitNach12Abs3,

    /// **§19 UStG** — Kleinunternehmer (small business exemption).
    ///
    /// Applies when total annual turnover does not exceed:
    /// - **€ 22 000** in 2023/2024
    /// - **€ 25 000** from 01.01.2025 (raised by Jahressteuergesetz 2024)
    ///
    /// No USt charged on EEG income; no Vorsteuerabzug on installation costs.
    Kleinunternehmer,

    /// **Regelbesteuerung** — standard German VAT at 19 %.
    ///
    /// Applies to:
    /// - Plants > 30 kWp
    /// - Plants commissioned before 01.01.2023
    /// - Operators who opted into Regelbesteuerung
    /// - Commercial operators (automatically)
    ///
    /// The Netzbetreiber pays the gross amount (Vergütung + 19 % USt) and
    /// deducts the input tax. The operator issues a VAT invoice.
    Regelbesteuerung,
}

impl VatStatus {
    /// Determine the most likely VAT status from plant characteristics.
    ///
    /// This is a **heuristic** — the operator's actual tax status may differ.
    /// Always confirm with the Anlagenbetreiber or their Steuerberater.
    ///
    /// ## Logic
    ///
    /// 1. Solar PV ≤ 30 kWp AND commissioned after 01.01.2023 → `BefreitNach12Abs3`
    /// 2. Otherwise → `Regelbesteuerung` (conservative; operator may be Kleinunternehmer)
    ///
    /// If the operator is a Kleinunternehmer (§19 UStG), set `VatStatus::Kleinunternehmer`
    /// explicitly — this cannot be determined from plant characteristics alone.
    ///
    /// # Example
    ///
    /// ```rust
    /// use eeg_billing::ust::VatStatus;
    /// use rust_decimal_macros::dec;
    /// use time::macros::date;
    ///
    /// // 9.5 kWp solar, commissioned 2024 → §12 Abs. 3 exempt
    /// assert_eq!(
    ///     VatStatus::from_plant(true, dec!(9.5), Some(date!(2024-06-01))),
    ///     VatStatus::BefreitNach12Abs3
    /// );
    ///
    /// // 50 kWp solar → too large, Regelbesteuerung
    /// assert_eq!(
    ///     VatStatus::from_plant(true, dec!(50), Some(date!(2024-01-01))),
    ///     VatStatus::Regelbesteuerung
    /// );
    ///
    /// // Wind plant → always Regelbesteuerung (§12 Abs. 3 is solar-only)
    /// assert_eq!(
    ///     VatStatus::from_plant(false, dec!(5), Some(date!(2024-01-01))),
    ///     VatStatus::Regelbesteuerung
    /// );
    /// ```
    #[must_use]
    pub fn from_plant(
        is_solar_pv: bool,
        leistung_kwp: Decimal,
        inbetriebnahme: Option<Date>,
    ) -> Self {
        if qualifies_for_12_abs3(is_solar_pv, leistung_kwp, inbetriebnahme) {
            Self::BefreitNach12Abs3
        } else {
            Self::Regelbesteuerung
        }
    }

    /// Return `true` when no USt is charged on EEG feed-in income.
    #[must_use]
    pub fn is_exempt(self) -> bool {
        matches!(self, Self::BefreitNach12Abs3 | Self::Kleinunternehmer)
    }

    /// Return the applicable USt rate (0.00 or 0.19).
    ///
    /// ```rust
    /// use eeg_billing::ust::VatStatus;
    /// use rust_decimal_macros::dec;
    ///
    /// assert_eq!(VatStatus::Regelbesteuerung.ust_rate(), dec!(0.19));
    /// assert_eq!(VatStatus::Kleinunternehmer.ust_rate(), dec!(0.00));
    /// ```
    #[must_use]
    pub fn ust_rate(self) -> Decimal {
        match self {
            Self::Regelbesteuerung => dec!(0.19),
            _ => Decimal::ZERO,
        }
    }

    /// Human-readable label for invoice footers or document notes.
    #[must_use]
    pub fn invoice_note(self) -> &'static str {
        match self {
            Self::BefreitNach12Abs3 => {
                "Steuerbefreiung gem\u{00e4}\u{00df} \u{00a7}\u{202f}12 Abs.\u{202f}3 UStG \
                 (Photovoltaik \u{2264}30\u{202f}kWp, ab 01.01.2023)"
            }
            Self::Kleinunternehmer => {
                "Kein Umsatzsteuerausweis gem\u{00e4}\u{00df} \u{00a7}\u{202f}19 UStG \
                 (Kleinunternehmerregelung)"
            }
            Self::Regelbesteuerung => "Umsatzsteuer 19\u{202f}%",
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Return `true` when a solar PV plant qualifies for the **§12 Abs. 3 UStG** exemption.
///
/// All three conditions must hold:
/// 1. Solar PV (not wind, biomass, KWKG, or other technology)
/// 2. Installed capacity **≤ 30 kWp** at this location
/// 3. Commissioned **on or after 01.01.2023**
///
/// When `inbetriebnahme` is absent, returns `false` (conservative/safe).
///
/// # Legal basis
///
/// §12 Abs. 3 UStG, introduced by JStG 2022 (BGBl I 2022 S. 2294), effective
/// 01.01.2023.
///
/// # Note
///
/// The 30 kWp threshold refers to the **total installed capacity** at one location
/// (§12 Abs. 3 Satz 3 UStG). For plants with multiple locations/properties,
/// each location is assessed separately.
///
/// # Example
///
/// ```rust
/// use eeg_billing::ust::qualifies_for_12_abs3;
/// use rust_decimal_macros::dec;
/// use time::macros::date;
///
/// assert!( qualifies_for_12_abs3(true,  dec!(9),  Some(date!(2024-01-01)))); // ≤30 kWp, post-2023
/// assert!(!qualifies_for_12_abs3(true,  dec!(31), Some(date!(2024-01-01)))); // >30 kWp
/// assert!(!qualifies_for_12_abs3(true,  dec!(9),  Some(date!(2022-06-01)))); // pre-2023
/// assert!(!qualifies_for_12_abs3(false, dec!(9),  Some(date!(2024-01-01)))); // not solar
/// assert!(!qualifies_for_12_abs3(true,  dec!(9),  None));                    // unknown date
/// ```
#[must_use]
pub fn qualifies_for_12_abs3(
    is_solar_pv: bool,
    leistung_kwp: Decimal,
    inbetriebnahme: Option<Date>,
) -> bool {
    if !is_solar_pv {
        return false;
    }
    if leistung_kwp > dec!(30) {
        return false;
    }
    inbetriebnahme.is_some_and(|d| d >= date!(2023 - 01 - 01))
}

/// Return the `billing::TaxLayer` list for a given VAT status.
///
/// - `BefreitNach12Abs3` / `Kleinunternehmer` → empty `Vec` (no USt position)
/// - `Regelbesteuerung` → single `FixedRateTax` at 19 %
///
/// Add the returned layers to a `BillingDocument` via `from_positions(…, tax_layers, …)`.
///
/// # Example
///
/// ```rust
/// use eeg_billing::ust::{VatStatus, ust_tax_layers};
///
/// let layers = ust_tax_layers(VatStatus::BefreitNach12Abs3);
/// assert!(layers.is_empty(), "§12 Abs. 3 exempt: no USt layer");
///
/// let layers = ust_tax_layers(VatStatus::Regelbesteuerung);
/// assert_eq!(layers.len(), 1, "Regelbesteuerung: 19% USt layer");
/// ```
#[must_use]
pub fn ust_tax_layers(status: VatStatus) -> Vec<Box<dyn TaxLayer>> {
    use billing::tax::FixedRateTax;
    // Returns a tax layer that applies to ALL positions in the document.
    //
    // ## Mixed-rate documents (e.g. EEG feed-in credit + NNE grid charge)
    //
    // When a single `BillingDocument` mixes positions with different VAT treatment
    // (e.g. 0% on PV feed-in under §12 Abs. 3 UStG, 19% on NNE grid charges),
    // do NOT use `ust_tax_layers` — build the tax layer directly with `.with_tag()`:
    //
    // ```rust,ignore
    // use billing::tax::FixedRateTax;
    // // Only NNE positions (tagged "nne") get 19% VAT:
    // let vat_nne = FixedRateTax::new("USt 19\u{202f}%", dec!(0.19)).with_tag("nne");
    // // EEG positions (tagged "eeg") remain tax-exempt.
    // ```
    //
    // `FixedRateTax::with_tag` is available in `billing 0.5.1`.
    match status {
        VatStatus::Regelbesteuerung => vec![Box::new(FixedRateTax::new(
            "Umsatzsteuer 19\u{202f}%",
            dec!(0.19),
        ))],
        _ => vec![],
    }
}

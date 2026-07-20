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
//! use rust_decimal::dec;
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
//! // EegSettleTariff itself adds no tax layer — VAT is the caller's to apply.
//! let tariff = EegSettleTariff::new(&output);
//! assert!(tariff.tax_layers().is_empty());
//!
//! // §12 Abs. 3 charges nothing, but still contributes a zero-rated entry to the
//! // EN 16931 BG-23 breakdown, so the layer is present rather than omitted.
//! let layers = ust_tax_layers(vat);
//! assert_eq!(layers.len(), 1);
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

use billing::{TaxCategory, TaxLayer, tax::FixedRateTax};
use rust_decimal::Decimal;
use rust_decimal::dec;
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
    /// use rust_decimal::dec;
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
    /// use rust_decimal::dec;
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
/// use rust_decimal::dec;
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
/// Every status yields exactly one layer, including the two that charge nothing.
/// A supply taxed at 0 % is still a taxable supply: EN 16931 BG-23 requires it to
/// appear in the VAT breakdown under its UNTDID 5305 category with a zero tax
/// amount. Omitting the layer would drop the turnover from the breakdown
/// altogether, which understates the taxable base on the invoice.
///
/// | Status | Rate | Category | Basis |
/// |---|---|---|---|
/// | `Regelbesteuerung` | 19 % | `S` (Standard) | §12 Abs. 1 UStG |
/// | `BefreitNach12Abs3` | 0 % | `Z` (`ZeroRated`) | §12 Abs. 3 UStG — Nullsteuersatz |
/// | `Kleinunternehmer` | 0 % | `E` (Exempt) | §19 UStG — tax not levied |
///
/// §12 Abs. 3 UStG is a zero *rate*, not an exemption, so it maps to `Z`; §19 UStG
/// does not levy the tax at all and maps to `E`, which EN 16931 requires to carry
/// an exemption reason (BT-120).
///
/// Add the returned layers to a `BillingDocument` via `from_positions(…, tax_layers, …)`.
///
/// # Mixed-rate documents
///
/// A document combining supplies with different treatment — a PV feed-in credit at
/// 0 % beside NNE grid charges at 19 % — cannot use a single status. Build the
/// layers directly and restrict each to its own positions with
/// [`FixedRateTax::with_tag`], so each contributes its own breakdown entry.
///
/// # Example
///
/// ```rust
/// use eeg_billing::ust::{VatStatus, ust_tax_layers};
///
/// // Every status yields one layer — the zero-rated ones included.
/// assert_eq!(ust_tax_layers(VatStatus::BefreitNach12Abs3).len(), 1);
/// assert_eq!(ust_tax_layers(VatStatus::Regelbesteuerung).len(), 1);
/// ```
#[must_use]
pub fn ust_tax_layers(status: VatStatus) -> Vec<Box<dyn TaxLayer>> {
    let layer = match status {
        VatStatus::Regelbesteuerung => FixedRateTax::new("Umsatzsteuer 19\u{202f}%", dec!(0.19))
            .expect("19 % is a valid rate")
            .with_category(TaxCategory::Standard),
        VatStatus::BefreitNach12Abs3 => {
            FixedRateTax::new("Umsatzsteuer 0\u{202f}% (§12 Abs. 3 UStG)", Decimal::ZERO)
                .expect("0 % is a valid rate")
                .with_category(TaxCategory::ZeroRated)
        }
        VatStatus::Kleinunternehmer => FixedRateTax::new("Umsatzsteuer (§19 UStG)", Decimal::ZERO)
            .expect("0 % is a valid rate")
            .with_category(TaxCategory::Exempt)
            .with_exemption_reason(
                "Kein Ausweis von Umsatzsteuer, da Kleinunternehmer gemäß §19 UStG",
            ),
    };
    vec![Box::new(layer)]
}

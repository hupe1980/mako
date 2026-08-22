//! German VAT (Umsatzsteuer) rules for EEG/KWKG feed-in settlements.
//!
//! The EEG itself does **not** regulate VAT — those rules come from the
//! Umsatzsteuergesetz (UStG) and BMF circulars. This module models the VAT
//! treatment of the **feed-in Gutschrift** (the payout the Netzbetreiber issues
//! to the Anlagenbetreiber).
//!
//! ## Terminology
//!
//! "Umsatzsteuer" (USt) is the legal term; "Mehrwertsteuer" (MwSt) is the
//! colloquial name. This library uses "Umsatzsteuer" throughout.
//!
//! ## The feed-in has exactly two VAT treatments
//!
//! | Status | Legal basis | EN 16931 category | USt on Vergütung |
//! |---|---|---|---|
//! | `Kleinunternehmer` | §19 UStG | `E` (Exempt) | **None** — tax not levied |
//! | `Regelbesteuerung` | §12 Abs. 1 UStG | `S` (Standard) | **19 %** |
//!
//! This is a **declared property of the operator**, not something the plant's
//! size decides — carry it in masterdata (see `einsd`'s `einspeiser.ust_status`).
//! [`VatStatus::default_for_plant`] only *suggests* the value an operator would
//! usually declare when seeding a new plant record; the stored value wins.
//!
//! ## Why not §12 Abs. 3 UStG?
//!
//! §12 Abs. 3 UStG (the 0 % Nullsteuersatz since 01.01.2023) taxes the **supply
//! and installation of the PV system itself** — the hardware transaction between
//! the installer and the operator. It does **not** apply to the operator's
//! ongoing feed-in of electricity, which is what an EEG settlement bills. Its
//! practical effect on this module is indirect: because a ≤30 kWp operator buys
//! the plant at 0 %, there is no input tax to reclaim, so almost all of them stay
//! **Kleinunternehmer (§19)** rather than opting into Regelbesteuerung. That is
//! why [`VatStatus::default_for_plant`] suggests `Kleinunternehmer` for a small
//! post-2023 solar plant — the 0 % on the feed-in comes from §19, never §12 Abs. 3.
//!
//! ## §19 UStG Kleinunternehmer
//!
//! Operators whose total annual turnover does not exceed **€ 25 000** (from
//! 01.01.2025; previously € 22 000) are treated as Kleinunternehmer and charge no
//! USt on any business income, including EEG feed-in. The Gutschrift shows the
//! §19 exemption reason (EN 16931 BT-120) and no USt.
//!
//! ## Regelbesteuerung
//!
//! All other operators (large plants exceeding the §19 turnover limit, commercial
//! operators, operators who opted into regular taxation) apply standard USt at
//! **19 %** on the Einspeisevergütung / Marktprämie. The Netzbetreiber pays the
//! gross amount (Netto + USt) and deducts the input tax.
//!
//! ## Usage in billing documents
//!
//! ```rust
//! use eeg_billing::ust::{VatStatus, ust_tax_layers};
//! use billing::{DocumentMeta, PricingModel};
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
//! // A small post-2023 solar operator is, by default, a Kleinunternehmer (§19).
//! let vat = VatStatus::default_for_plant(true, dec!(9.5), Some(date!(2024-06-01)));
//! assert_eq!(vat, VatStatus::Kleinunternehmer);
//! assert!(vat.is_exempt());
//!
//! // EegSettleTariff itself adds no tax layer — VAT is the caller's to apply.
//! let tariff = EegSettleTariff::new(&output);
//! assert!(tariff.tax_layers().is_empty());
//!
//! // §19 charges nothing, but still contributes an exempt entry to the
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

/// The operator's German VAT (Umsatzsteuer) status for EEG feed-in settlement.
///
/// Determines whether USt appears on the feed-in Gutschrift and at what rate. A
/// feed-in has exactly two treatments — [`Kleinunternehmer`](Self::Kleinunternehmer)
/// (§19, 0 %) and [`Regelbesteuerung`](Self::Regelbesteuerung) (19 %).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum VatStatus {
    /// **§19 UStG** — Kleinunternehmer (small business exemption).
    ///
    /// Applies when total annual turnover does not exceed:
    /// - **€ 22 000** in 2023/2024
    /// - **€ 25 000** from 01.01.2025 (raised by Jahressteuergesetz 2024)
    ///
    /// The overwhelming default for a ≤30 kWp post-2023 solar operator: the plant
    /// was supplied at 0 % under §12 Abs. 3 UStG, so there is no input tax to
    /// reclaim and no reason to opt into Regelbesteuerung. No USt on EEG income;
    /// the Gutschrift carries the §19 exemption reason (BT-120).
    Kleinunternehmer,

    /// **Regelbesteuerung** — standard German VAT at 19 % (§12 Abs. 1 UStG).
    ///
    /// Applies to:
    /// - Operators whose turnover exceeds the §19 Kleinunternehmer limit
    /// - Operators who opted into Regelbesteuerung
    /// - Commercial operators (automatically)
    ///
    /// The Netzbetreiber pays the gross amount (Vergütung + 19 % USt) and
    /// deducts the input tax. The operator issues (receives, in the
    /// Gutschriftverfahren) a VAT invoice.
    Regelbesteuerung,
}

impl VatStatus {
    /// Suggest the VAT status an operator would **usually declare** for a plant.
    ///
    /// This is a **seeding heuristic**, not the authoritative value — the operator's
    /// actual tax status is a declared property that belongs in masterdata
    /// (`einsd`'s `einspeiser.ust_status`). Use this only to pre-fill a new plant
    /// record when no status was supplied; a stored value always wins.
    ///
    /// ## Logic
    ///
    /// 1. Solar PV ≤ 30 kWp commissioned on/after 01.01.2023 → `Kleinunternehmer`
    ///    (the post-§12-Abs.-3 norm — see the module docs).
    /// 2. Otherwise → `Regelbesteuerung` (larger/commercial plants exceed the §19
    ///    turnover limit; the operator can still declare Kleinunternehmer).
    ///
    /// # Example
    ///
    /// ```rust
    /// use eeg_billing::ust::VatStatus;
    /// use rust_decimal::dec;
    /// use time::macros::date;
    ///
    /// // 9.5 kWp solar, commissioned 2024 → Kleinunternehmer (§19)
    /// assert_eq!(
    ///     VatStatus::default_for_plant(true, dec!(9.5), Some(date!(2024-06-01))),
    ///     VatStatus::Kleinunternehmer
    /// );
    ///
    /// // 50 kWp solar → exceeds the §19 limit, Regelbesteuerung
    /// assert_eq!(
    ///     VatStatus::default_for_plant(true, dec!(50), Some(date!(2024-01-01))),
    ///     VatStatus::Regelbesteuerung
    /// );
    ///
    /// // Wind plant → Regelbesteuerung (the small-PV default is solar-only)
    /// assert_eq!(
    ///     VatStatus::default_for_plant(false, dec!(5), Some(date!(2024-01-01))),
    ///     VatStatus::Regelbesteuerung
    /// );
    /// ```
    #[must_use]
    pub fn default_for_plant(
        is_solar_pv: bool,
        leistung_kwp: Decimal,
        inbetriebnahme: Option<Date>,
    ) -> Self {
        let small_post_2023_solar = is_solar_pv
            && leistung_kwp <= dec!(30)
            && inbetriebnahme.is_some_and(|d| d >= date!(2023 - 01 - 01));
        if small_post_2023_solar {
            Self::Kleinunternehmer
        } else {
            Self::Regelbesteuerung
        }
    }

    /// The canonical database / API token for this status.
    #[must_use]
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Kleinunternehmer => "KLEINUNTERNEHMER",
            Self::Regelbesteuerung => "REGELBESTEUERUNG",
        }
    }

    /// Parse the database / API token; `None` for an unrecognised value.
    #[must_use]
    pub fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "KLEINUNTERNEHMER" => Some(Self::Kleinunternehmer),
            "REGELBESTEUERUNG" => Some(Self::Regelbesteuerung),
            _ => None,
        }
    }

    /// Return `true` when no USt is charged on EEG feed-in income.
    #[must_use]
    pub fn is_exempt(self) -> bool {
        matches!(self, Self::Kleinunternehmer)
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
            Self::Kleinunternehmer => Decimal::ZERO,
        }
    }

    /// Human-readable label for invoice footers or document notes.
    #[must_use]
    pub fn invoice_note(self) -> &'static str {
        match self {
            Self::Kleinunternehmer => {
                "Kein Umsatzsteuerausweis gem\u{00e4}\u{00df} \u{00a7}\u{202f}19 UStG \
                 (Kleinunternehmerregelung)"
            }
            Self::Regelbesteuerung => "Umsatzsteuer 19\u{202f}%",
        }
    }
}

// ── Tax layers ────────────────────────────────────────────────────────────────

/// Return the `billing::TaxLayer` list for a given VAT status.
///
/// Every status yields exactly one layer, including the exempt one. A supply
/// taxed at 0 % is still a taxable supply: EN 16931 BG-23 requires it to appear
/// in the VAT breakdown under its UNTDID 5305 category. Omitting the layer would
/// drop the turnover from the breakdown altogether, which understates the taxable
/// base on the invoice.
///
/// | Status | Rate | Category | Basis |
/// |---|---|---|---|
/// | `Regelbesteuerung` | 19 % | `S` (Standard) | §12 Abs. 1 UStG |
/// | `Kleinunternehmer` | 0 % | `E` (Exempt) | §19 UStG — tax not levied |
///
/// §19 UStG does not levy the tax at all and maps to `E`, which EN 16931 requires
/// to carry an exemption reason (BT-120).
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
/// // Every status yields one layer — the exempt one included.
/// assert_eq!(ust_tax_layers(VatStatus::Kleinunternehmer).len(), 1);
/// assert_eq!(ust_tax_layers(VatStatus::Regelbesteuerung).len(), 1);
/// ```
#[must_use]
pub fn ust_tax_layers(status: VatStatus) -> Vec<Box<dyn TaxLayer>> {
    // `exempt` validates the category/reason pairing up front (EN 16931 zero-tax
    // families) instead of at breakdown time; `.boxed()` is the trait-object
    // shorthand for `Box::new(_) as Box<dyn TaxLayer>`.
    let layer = match status {
        VatStatus::Regelbesteuerung => FixedRateTax::new("Umsatzsteuer 19\u{202f}%", dec!(0.19))
            .expect("19 % is a valid rate")
            .with_category(TaxCategory::Standard),
        // §19 UStG is a genuine exemption (category E) requiring a reason (BT-120).
        VatStatus::Kleinunternehmer => FixedRateTax::exempt(
            "Umsatzsteuer (§19 UStG)",
            TaxCategory::Exempt,
            "Kein Ausweis von Umsatzsteuer, da Kleinunternehmer gemäß §19 UStG",
        )
        .expect("§19 exemption is a valid (category, reason) pairing"),
    };
    vec![layer.boxed()]
}

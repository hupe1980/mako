//! Direktvermarktung domain model — §§20–22 EEG 2023.
//!
//! Covers the **Direktvermarktung** regulatory framework:
//! - §20 **Pflichtgemäße Direktvermarktung** — mandatory for plants > 100 kW
//! - §21 **Freiwillige Direktvermarktung** — optional for smaller plants
//! - §22 **Ausschreibungspflicht** — tendering mandatory above capacity thresholds
//! - Monthly switching rules between Einspeisevergütung and Marktprämie (§21 Abs. 3)
//!
//! ## Key capacity thresholds (EEG 2023)
//!
//! | Rule | Threshold | Legal basis |
//! |---|---|---|
//! | Mandatory Direktvermarktung | > 100 kW | §20 Abs. 1 EEG 2023 |
//! | Ausschreibung — Solar PV | > 1,000 kWp | §22 Abs. 1 EEG 2023 |
//! | Ausschreibung — Wind Onshore | > 750 kW | §22 Abs. 2 EEG 2023 |
//! | Ausschreibung — Biomasse | > 150 kW | §22 Abs. 3 EEG 2023 |
//! | Ausschreibung — Wasserkraft | > 500 kW | §22 Abs. 4 EEG 2023 |
//! | Ausschreibung — Geothermie | > 150 kW | §22 Abs. 4 EEG 2023 |
//!
//! ## Switching between FeedInTariff and MarketPremium (§21 Abs. 3)
//!
//! A plant may switch from Direktvermarktung back to Einspeisevergütung:
//! - Only **once per calendar month** (not multiple times within a month)
//! - Requires written notice to the NB before the start of the billing period
//! - Not permitted for plants subject to **mandatory** Direktvermarktung (§20)
//!
//! **Important**: Plants in mandatory Direktvermarktung that are temporarily unable
//! to market (e.g. Direktvermarkter insolvency) use **Ausfallvergütung** (§21 Abs. 1 Nr. 2
//! EEG 2023, `TemporaryFeedInTariff` scheme) — this is NOT the same as switching back to
//! regular Einspeisevergütung.
//!
//! ## Managementprämie (§20 Abs. 3 EEG 2023)
//!
//! Paid monthly by NB to the plant operator (or Direktvermarkter) as a flat fee
//! for the administrative effort of participating in direct marketing.
//! Rate: 0.4 ct/kWh for plants ≤ 100 MW; 0.2 ct/kWh for plants > 100 MW.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use time::Date;

use crate::technology::ErzeugungsArt;
use crate::version::EegGesetz;

// ── Threshold helpers ─────────────────────────────────────────────────────────

/// Capacity threshold (kW) above which Direktvermarktung is **mandatory** per §20 EEG.
///
/// This threshold has been 100 kW since EEG 2012 and was not changed in EEG 2023.
pub const DIREKTVERMARKTUNG_PFLICHT_KW: Decimal = dec!(100);

/// Returns `true` when the plant is subject to **pflichtgemäße Direktvermarktung**
/// under §20 EEG 2023 (mandatory Direktvermarktung, > 100 kW installed).
///
/// Plants ≤ 100 kW may still participate voluntarily (§21) but are not required to.
///
/// ## EEG version sensitivity
///
/// The threshold has been 100 kW since EEG 2012. For EEG ≤2009 plants there was
/// no mandatory Direktvermarktung — they may stay on Einspeisevergütung forever
/// under §100 Übergangsregelung.
///
/// # Example
///
/// ```rust
/// use eeg_billing::direktverm::is_direktvermarktung_mandatory;
/// use eeg_billing::EegGesetz;
/// use rust_decimal_macros::dec;
///
/// // 150 kW plant under EEG 2023 — mandatory
/// assert!(is_direktvermarktung_mandatory(dec!(150), EegGesetz::Eeg2023));
///
/// // 80 kW plant — not mandatory (may still use voluntarily)
/// assert!(!is_direktvermarktung_mandatory(dec!(80), EegGesetz::Eeg2023));
///
/// // Any size under EEG 2009 — §20 did not exist yet
/// assert!(!is_direktvermarktung_mandatory(dec!(500), EegGesetz::Eeg2009));
/// ```
#[must_use]
pub fn is_direktvermarktung_mandatory(leistung_kw: Decimal, gesetz: EegGesetz) -> bool {
    match gesetz {
        EegGesetz::Eeg2012 | EegGesetz::Eeg2017 | EegGesetz::Eeg2021 | EegGesetz::Eeg2023 => {
            leistung_kw > DIREKTVERMARKTUNG_PFLICHT_KW
        }
        // EEG ≤2009 and KWKG: no mandatory Direktvermarktung
        _ => false,
    }
}

/// Returns `true` when the plant **must** participate in a BNetzA Ausschreibungsverfahren
/// (competitive tender) under §22 EEG 2023.
///
/// Plants above these thresholds may only receive EEG support via tender-awarded
/// `anzulegender Wert` (i.e. `TariffSource::Auction`). Statutory rates do not apply.
///
/// # Example
///
/// ```rust
/// use eeg_billing::direktverm::requires_ausschreibung;
/// use eeg_billing::ErzeugungsArt;
/// use rust_decimal_macros::dec;
///
/// // 1.5 MWp solar → tender mandatory
/// assert!(requires_ausschreibung(dec!(1500), ErzeugungsArt::SolarAufdach));
///
/// // 800 kW wind onshore → tender mandatory
/// assert!(requires_ausschreibung(dec!(800), ErzeugungsArt::WindOnshore));
///
/// // 500 kWp solar → no tender required
/// assert!(!requires_ausschreibung(dec!(500), ErzeugungsArt::SolarAufdach));
/// ```
#[must_use]
pub fn requires_ausschreibung(leistung_kw: Decimal, art: ErzeugungsArt) -> bool {
    match art {
        ErzeugungsArt::Solar
        | ErzeugungsArt::SolarAufdach
        | ErzeugungsArt::SolarFreiflaeche
        | ErzeugungsArt::SolarAgriPv
        | ErzeugungsArt::SolarMieterstrom
        | ErzeugungsArt::SolarStecker => leistung_kw > dec!(1000),

        ErzeugungsArt::WindOnshore => leistung_kw > dec!(750),
        ErzeugungsArt::WindOffshore => true, // all offshore is tendered (§23 EEG 2023)

        ErzeugungsArt::Biomasse
        | ErzeugungsArt::BiomassHolz
        | ErzeugungsArt::Biogas
        | ErzeugungsArt::Biomethan => leistung_kw > dec!(150),

        ErzeugungsArt::Wasserkraft => leistung_kw > dec!(500),
        ErzeugungsArt::Geothermie => leistung_kw > dec!(150),

        // Gas variants, Gezeiten, KWKG: no Ausschreibung under EEG 2023
        ErzeugungsArt::Klaegas
        | ErzeugungsArt::Grubengas
        | ErzeugungsArt::Deponiegas
        | ErzeugungsArt::Gezeiten
        | ErzeugungsArt::Kwk => false,
    }
}

// ── DirektvermarktungsPeriode ─────────────────────────────────────────────────

/// One period during which a plant participates in Direktvermarktung.
///
/// A plant may switch between Einspeisevergütung (FeedInTariff scheme) and
/// Marktprämie (MarketPremium scheme). Each contiguous block of Direktvermarktung
/// is one `DirektvermarktungsPeriode`.
///
/// ## Storage in `einsd`
///
/// The `direktvermarktung_perioden` column (`JSONB`) in `eeg_anlagen` stores a
/// `Vec<DirektvermarktungsPeriode>` sorted by `beginn_datum`. Use
/// `current_period()` to find the active period for a billing month.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DirektvermarktungsPeriode {
    /// First day of this Direktvermarktung period (inclusive).
    ///
    /// Must be the first day of a calendar month (§21 Abs. 3 EEG 2023).
    pub beginn_datum: Date,

    /// Last day of this period (inclusive), or `None` for the ongoing current period.
    ///
    /// When `Some(end)`, `end` must be the last day of a calendar month.
    pub ende_datum: Option<Date>,

    /// MP-ID of the Direktvermarkter (energy trader executing market access).
    ///
    /// The BDEW-Codenummer of the company acting as Direktvermarkter under §11 EEG.
    /// `None` for voluntary Direktvermarktung by the plant operator themselves.
    pub direktvermarkter_mp_id: Option<String>,

    /// Whether this period is **freiwillige Direktvermarktung** (voluntary).
    ///
    /// - `false` = pflichtgemäße Direktvermarktung (plants > 100 kW, §20)
    /// - `true` = freiwillige Direktvermarktung (plants ≤ 100 kW, §21)
    pub ist_freiwillig: bool,

    /// Anzulegender Wert agreed or awarded in this period (ct/kWh).
    ///
    /// For tender-based plants (`TariffSource::Auction`): the BNetzA-awarded AW.
    /// For non-tender plants: the statutory AW from `rates::wind_onshore_lookup` etc.
    pub anzulegender_wert_ct: Decimal,
}

impl DirektvermarktungsPeriode {
    /// Returns `true` when this period covers the given billing date.
    ///
    /// A period is active when `beginn_datum <= billing_date` and
    /// either `ende_datum.is_none()` or `billing_date <= ende_datum`.
    #[must_use]
    pub fn is_active_on(&self, billing_date: Date) -> bool {
        if billing_date < self.beginn_datum {
            return false;
        }
        match self.ende_datum {
            Some(end) => billing_date <= end,
            None => true,
        }
    }
}

/// Find the active `DirektvermarktungsPeriode` for a given billing date.
///
/// Returns `None` when no period covers `billing_date` (plant is on Einspeisevergütung).
///
/// # Example
///
/// ```rust
/// use eeg_billing::direktverm::{DirektvermarktungsPeriode, current_period};
/// use rust_decimal_macros::dec;
/// use time::macros::date;
///
/// let periods = vec![
///     DirektvermarktungsPeriode {
///         beginn_datum: date!(2024-01-01),
///         ende_datum:   Some(date!(2024-06-30)),
///         direktvermarkter_mp_id: Some("9904234560001".into()),
///         ist_freiwillig: false,
///         anzulegender_wert_ct: dec!(6.28),
///     },
/// ];
///
/// // March 2024 — within the period
/// assert!(current_period(&periods, date!(2024-03-15)).is_some());
/// // July 2024 — after the period ended
/// assert!(current_period(&periods, date!(2024-07-01)).is_none());
/// ```
#[must_use]
pub fn current_period(
    periods: &[DirektvermarktungsPeriode],
    billing_date: Date,
) -> Option<&DirektvermarktungsPeriode> {
    periods.iter().find(|p| p.is_active_on(billing_date))
}

// ── Switching validation ──────────────────────────────────────────────────────

/// Reason why a switch from Direktvermarktung to Einspeisevergütung is blocked.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SwitchBlockedReason {
    /// Plant > 100 kW → mandatory Direktvermarktung (§20 EEG 2023).
    ///
    /// Such plants can only temporarily revert to **Ausfallvergütung** (§21 Abs. 1 Nr. 2).
    PflichtgemasseDirektvermarktung,
    /// Switch attempted within the same calendar month as the last switch.
    ///
    /// §21 Abs. 3 EEG 2023 allows at most one switch per calendar month.
    AlreadySwitchedThisMonth {
        /// Date of the most recent switch that blocks this request.
        last_switch: Date,
    },
}

/// Validate whether a plant may switch from Direktvermarktung to Einspeisevergütung
/// on the given effective date.
///
/// Returns `Ok(())` when the switch is permitted, or `Err(reason)` when blocked.
///
/// ## Note
///
/// This is a **compile-time validation helper** — it does not interact with any
/// persistent state. The caller must pass the most recent switch date from
/// the plant's history.
///
/// # Example
///
/// ```rust
/// use eeg_billing::direktverm::{validate_switch_to_vergütung, SwitchBlockedReason};
/// use eeg_billing::EegGesetz;
/// use rust_decimal_macros::dec;
/// use time::macros::date;
///
/// // 80 kW voluntary plant switching once per month — OK
/// let result = validate_switch_to_vergütung(
///     dec!(80),
///     EegGesetz::Eeg2023,
///     date!(2025-07-01), // effective from July 1
///     None,              // no previous switch
/// );
/// assert!(result.is_ok());
///
/// // 150 kW mandatory plant — blocked
/// let result = validate_switch_to_vergütung(
///     dec!(150),
///     EegGesetz::Eeg2023,
///     date!(2025-07-01),
///     None,
/// );
/// assert_eq!(result, Err(SwitchBlockedReason::PflichtgemasseDirektvermarktung));
/// ```
pub fn validate_switch_to_vergütung(
    leistung_kw: Decimal,
    gesetz: EegGesetz,
    effective_date: Date,
    last_switch_date: Option<Date>,
) -> Result<(), SwitchBlockedReason> {
    if is_direktvermarktung_mandatory(leistung_kw, gesetz) {
        return Err(SwitchBlockedReason::PflichtgemasseDirektvermarktung);
    }
    if let Some(last) = last_switch_date
        && last.year() == effective_date.year()
        && last.month() == effective_date.month()
    {
        return Err(SwitchBlockedReason::AlreadySwitchedThisMonth { last_switch: last });
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn mandatory_above_100kw() {
        assert!(is_direktvermarktung_mandatory(
            dec!(100.1),
            EegGesetz::Eeg2023
        ));
        assert!(!is_direktvermarktung_mandatory(
            dec!(100),
            EegGesetz::Eeg2023
        ));
        assert!(!is_direktvermarktung_mandatory(
            dec!(50),
            EegGesetz::Eeg2023
        ));
    }

    #[test]
    fn no_mandatory_for_old_eeg() {
        // EEG 2009 plants may stay on Einspeisevergütung forever (§100 Übergangsregelung)
        assert!(!is_direktvermarktung_mandatory(
            dec!(500),
            EegGesetz::Eeg2009
        ));
        assert!(!is_direktvermarktung_mandatory(
            dec!(500),
            EegGesetz::Eeg2000
        ));
    }

    #[test]
    fn ausschreibung_solar_above_1mwp() {
        assert!(requires_ausschreibung(
            dec!(1001),
            ErzeugungsArt::SolarAufdach
        ));
        assert!(!requires_ausschreibung(
            dec!(999),
            ErzeugungsArt::SolarAufdach
        ));
    }

    #[test]
    fn ausschreibung_wind_onshore_above_750kw() {
        assert!(requires_ausschreibung(
            dec!(751),
            ErzeugungsArt::WindOnshore
        ));
        assert!(!requires_ausschreibung(
            dec!(750),
            ErzeugungsArt::WindOnshore
        ));
    }

    #[test]
    fn ausschreibung_wind_offshore_always() {
        assert!(requires_ausschreibung(dec!(1), ErzeugungsArt::WindOffshore));
    }

    #[test]
    fn ausschreibung_biomasse_above_150kw() {
        assert!(requires_ausschreibung(dec!(151), ErzeugungsArt::Biomasse));
        assert!(!requires_ausschreibung(dec!(150), ErzeugungsArt::Biomasse));
    }

    #[test]
    fn period_active_on() {
        let p = DirektvermarktungsPeriode {
            beginn_datum: date!(2024 - 01 - 01),
            ende_datum: Some(date!(2024 - 06 - 30)),
            direktvermarkter_mp_id: None,
            ist_freiwillig: true,
            anzulegender_wert_ct: dec!(6.28),
        };
        assert!(p.is_active_on(date!(2024 - 03 - 15)));
        assert!(p.is_active_on(date!(2024 - 01 - 01)));
        assert!(p.is_active_on(date!(2024 - 06 - 30)));
        assert!(!p.is_active_on(date!(2024 - 07 - 01)));
        assert!(!p.is_active_on(date!(2023 - 12 - 31)));
    }

    #[test]
    fn open_period_always_active() {
        let p = DirektvermarktungsPeriode {
            beginn_datum: date!(2024 - 01 - 01),
            ende_datum: None,
            direktvermarkter_mp_id: None,
            ist_freiwillig: false,
            anzulegender_wert_ct: dec!(6.28),
        };
        assert!(p.is_active_on(date!(2030 - 12 - 31)));
    }

    #[test]
    fn current_period_finds_active() {
        let periods = vec![DirektvermarktungsPeriode {
            beginn_datum: date!(2024 - 01 - 01),
            ende_datum: Some(date!(2024 - 06 - 30)),
            direktvermarkter_mp_id: Some("9904234560001".into()),
            ist_freiwillig: false,
            anzulegender_wert_ct: dec!(6.28),
        }];
        assert!(current_period(&periods, date!(2024 - 03 - 15)).is_some());
        assert!(current_period(&periods, date!(2024 - 07 - 01)).is_none());
    }

    #[test]
    fn switch_blocked_mandatory_plant() {
        let result = validate_switch_to_vergütung(
            dec!(150),
            EegGesetz::Eeg2023,
            date!(2025 - 07 - 01),
            None,
        );
        assert_eq!(
            result,
            Err(SwitchBlockedReason::PflichtgemasseDirektvermarktung)
        );
    }

    #[test]
    fn switch_blocked_same_month() {
        let result = validate_switch_to_vergütung(
            dec!(80),
            EegGesetz::Eeg2023,
            date!(2025 - 07 - 15),
            Some(date!(2025 - 07 - 01)),
        );
        assert!(matches!(
            result,
            Err(SwitchBlockedReason::AlreadySwitchedThisMonth { .. })
        ));
    }

    #[test]
    fn switch_allowed_different_month() {
        let result = validate_switch_to_vergütung(
            dec!(80),
            EegGesetz::Eeg2023,
            date!(2025 - 08 - 01),
            Some(date!(2025 - 07 - 01)),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn switch_allowed_voluntary_no_history() {
        let result = validate_switch_to_vergütung(
            dec!(50),
            EegGesetz::Eeg2023,
            date!(2025 - 07 - 01),
            None,
        );
        assert!(result.is_ok());
    }
}

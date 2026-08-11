//! Solar PV technology-specific EEG rules — §48 EEG 2023.
//!
//! Solar PV has more technology-specific sub-rules than any other EEG technology:
//!
//! - **§48 Abs. 1** — four legal *Bauformen* with different tariff thresholds
//! - **§48 Abs. 2 vs. Abs. 2a** — Überschusseinspeisung vs. Volleinspeisung rates
//! - **§48a EEG 2023** — Mieterstromzuschlag bei solarer Strahlungsenergie (the §21 Abs. 3 rate)
//! - **§48 Abs. 5** — Freiflächenanlage restrictions (location, ecological rules)
//! - **§22 EEG 2023** — auction obligation for large plants (> 1 MWp)
//! - **§51a EEG 2023** — Verlängerungsanspruch uses a 0.5 factor for solar
//!   (§51a Abs. 2: only 50% of lost kWh extend the period, not 100%)
//! - **Solarpaket I (BGBl I 2024 Nr. 107)** — increased rates from 01.05.2024,
//!   new `Stecker-PV` category (≤ 2 kWp)

use rust_decimal::Decimal;
use rust_decimal::dec;

// ── SolarBauform ──────────────────────────────────────────────────────────────

/// Legal installation form under §48 EEG 2023 — determines tariff thresholds
/// and eligibility for Volleinspeisung bonus.
///
/// §48 Abs. 1 EEG 2023 distinguishes four Bauformen. The classification at
/// commissioning is binding for the full 20-year Förderdauer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum SolarBauform {
    /// **Gebäudeanlage** — installed on or at a building.
    ///
    /// §48 Abs. 1 Nr. 1 EEG 2023: "Solaranlage auf, an oder in einem Gebäude".
    /// Standard rooftop PV, facade-integrated, building-attached carports.
    /// Eligible for Volleinspeisung bonus (§48 Abs. 2a).
    Gebaeude,

    /// **Lärmschutzwand** — installed on a noise barrier.
    ///
    /// §48 Abs. 1 Nr. 2 EEG 2023: "Solaranlage auf, an oder in einer Lärmschutzwand".
    /// Eligible for Volleinspeisung bonus.
    Laermschutzwand,

    /// **Freiflächenanlage** — ground-mounted on open land.
    ///
    /// §48 Abs. 1 Nr. 3 EEG 2023: all installations not covered by Nr. 1 or Nr. 2.
    /// Lower tariff than Gebäudeanlage. Subject to §48 Abs. 5 location restrictions.
    /// Auction obligation above 1 MWp (§22 Abs. 1 EEG 2023).
    /// NOT eligible for Volleinspeisung bonus above statutory threshold.
    Freiflaeche,

    /// **Agri-PV** — dual-use agricultural land + solar power (§48 Abs. 3 EEG 2023).
    ///
    /// Introduced with Solarpaket I (BGBl I 2024 Nr. 107). Higher bonus rate
    /// due to dual land-use benefit. Certified by a DLG- or LfL-accredited body.
    AgriPv,

    /// **Floating PV** — installed on water surfaces.
    ///
    /// §48 Abs. 1 Nr. 4 EEG 2023 (Solarpaket I): floating panels on reservoirs,
    /// mining lakes, etc. Special ecological approval required.
    Floating,

    /// **Parkplatz-Überdachung** — solar canopy over parking areas.
    ///
    /// §48 Abs. 1 Nr. 5 EEG 2023 (Solarpaket I): combines parking function
    /// with energy generation. Special tender allocation.
    Parkplatz,

    /// **Stecker-PV** (Balkonkraftwerk) — plug-in balcony solar ≤ 2 kWp.
    ///
    /// §48b EEG 2023 (Solarpaket I 2024): simplified registration, no smart meter
    /// obligation, no Einspeisevergütung above simplified MaStR threshold.
    /// Feed-in kWh typically registered via standardised `SLP S0` profile.
    SteckerPv,
}

impl SolarBauform {
    /// Returns `true` when this Bauform qualifies for the §48 Abs. 2a
    /// **Volleinspeisung** bonus (higher rate for 100% grid feed-in).
    ///
    /// Freiflächenanlage plants above the statutory threshold do NOT qualify
    /// (§48 Abs. 2a applies only to Gebäude, Lärmschutzwand, Agri-PV).
    #[must_use]
    pub fn volleinspeisung_bonus_eligible(self) -> bool {
        matches!(
            self,
            Self::Gebaeude
                | Self::Laermschutzwand
                | Self::AgriPv
                | Self::Floating
                | Self::Parkplatz
        )
    }

    /// Returns `true` for Freiflächenanlagen subject to §48 Abs. 5 location restrictions.
    ///
    /// §48 Abs. 5 EEG 2023: Freiflächenanlagen must comply with ecological
    /// criteria (no Class I agricultural land, protected areas, etc.) to receive EEG support.
    #[must_use]
    pub fn has_freiflaechen_restriction(self) -> bool {
        self == Self::Freiflaeche
    }

    /// Returns `true` when this Bauform may be subject to a §22 auction obligation.
    ///
    /// Freiflächenanlagen > 1 MWp and Gebäudeanlagen > 1 MWp require BNetzA tender.
    /// All other Bauformen have the same threshold logic.
    #[must_use]
    pub fn auction_threshold_kwp(self) -> Decimal {
        match self {
            Self::SteckerPv => dec!(2), // tiny plants, no auction
            Self::AgriPv | Self::Floating | Self::Parkplatz => dec!(6000), // higher threshold for special categories
            _ => dec!(1000), // standard: > 1 MWp → auction
        }
    }
}

// ── EinspeisungsModus ─────────────────────────────────────────────────────────

/// Whether the plant feeds in 100% of generation or only the surplus after self-consumption.
///
/// This has a DIRECT impact on the EEG tariff rate (§48 Abs. 2 vs. Abs. 2a EEG 2023).
/// For a ≤ 10 kW Gebäudeanlage, in gross anzulegende Werte before §49 degression:
///
/// | Modus | Anzulegender Wert |
/// |---|---|
/// | `Ueberschusseinspeisung` | 8,60 ct/kWh (§48 Abs. 2 Nr. 1) |
/// | `Volleinspeisung` | **13,40 ct/kWh** (§48 Abs. 2 Nr. 1 + Abs. 2a Nr. 1: +4,8 ct) |
///
/// The uplift is large, not marginal — over half the rate again — which is why
/// §48 Abs. 2a Satz 1 conditions it on a textual declaration to the Netzbetreiber
/// before commissioning (or before 1 December of the preceding year), and why
/// §52 sanctions self-consumption on a plant registered for Volleinspeisung.
///
/// Resolve the rate for a given commissioning date with
/// [`crate::rates::solar_pv_ueberschuss_aw_ct`] /
/// [`crate::rates::solar_pv_volleinspeisung_aw_ct`], which apply §49.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum EinspeisungsModus {
    /// 100% of generation is fed into the grid (Volleinspeisung).
    ///
    /// Higher EEG rate (§48 Abs. 2a). Operator may not self-consume any kWh.
    /// Violation triggers §52 `VolleinspeisungspflichtVerletzt` (see `SanktionsTyp`).
    Volleinspeisung,

    /// Only surplus after self-consumption is fed in (Überschusseinspeisung).
    ///
    /// Standard rate (§48 Abs. 2). Self-consumption is allowed and encouraged.
    #[default]
    Ueberschusseinspeisung,
}

// ── SolarAnlageData ───────────────────────────────────────────────────────────

/// Solar PV plant data needed for correct §48 EEG 2023 settlement.
///
/// Combine with `SettleInput` to ensure the settlement engine applies the
/// correct tariff rate and §51a factor.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SolarAnlageData {
    /// Physical installation form — determines tariff table and auction obligation.
    pub bauform: SolarBauform,

    /// Feed-in mode — determines whether §48 Abs. 2 or Abs. 2a rate applies.
    pub einspeisungs_modus: EinspeisungsModus,

    /// Whether the plant has a certified **MaStR registration** (required since §25 EEG).
    ///
    /// `false` → §52 penalty applies until registration confirmed.
    /// For EEG 2023 plants: €10/kW/month (§52 Abs. 1 Nr. 11 EEG 2023).
    pub mastr_registriert: bool,

    /// Agri-PV certification issued by accredited body (DLG, LfL).
    ///
    /// Required when `bauform = AgriPv` to receive the Agri-PV bonus rate.
    /// Without certification, the plant is settled at standard Freiflächenanlage rates.
    pub agripv_zertifiziert: bool,
}

impl SolarAnlageData {
    /// Determine whether the operator must register for Volleinspeisung sanctions.
    ///
    /// §52 `VolleinspeisungspflichtVerletzt` applies when the plant is registered
    /// for Volleinspeisung (§48 Abs. 2a) but the measured feed-in is less than
    /// the total generation (self-consumption detected).
    #[must_use]
    pub fn volleinspeisung_sanktionspflichtig(&self) -> bool {
        self.einspeisungs_modus == EinspeisungsModus::Volleinspeisung
    }
}

// ── Auction obligation check ──────────────────────────────────────────────────

/// Returns `true` when a solar PV plant requires a BNetzA tender award
/// to receive EEG market premium (§22 Abs. 1 EEG 2023).
///
/// ## Thresholds (§22 Abs. 1 EEG 2023)
///
/// | Bauform | Auction threshold |
/// |---|---|
/// | Gebäudeanlage, Freiflächenanlage | **> 1 MWp** (1 000 kWp) |
/// | Agri-PV, Floating, Parkplatz | **> 6 MWp** (6 000 kWp) |
/// | Stecker-PV | no auction (≤ 2 kWp) |
///
/// ## Bestandsschutz
/// Plants commissioned before the relevant EEG introduced the tender system
/// do not need a retrospective auction award.
///
/// # Example
/// ```rust
/// use eeg_billing::solar::{SolarBauform, requires_ausschreibung};
/// use rust_decimal::dec;
///
/// assert!( requires_ausschreibung(dec!(1001), SolarBauform::Gebaeude));  // > 1 MWp
/// assert!(!requires_ausschreibung(dec!(999),  SolarBauform::Gebaeude));  // ≤ 1 MWp
/// assert!(!requires_ausschreibung(dec!(5000), SolarBauform::AgriPv));    // ≤ 6 MWp
/// ```
#[must_use]
pub fn requires_ausschreibung(leistung_kwp: Decimal, bauform: SolarBauform) -> bool {
    leistung_kwp > bauform.auction_threshold_kwp()
}

// ── §51a Abs. 2 — solar PV factor ────────────────────────────────────────────

/// §51a Abs. 2 EEG 2023 — solar-specific Verlängerungsanspruch factor.
///
/// For solar PV plants, the payment period is extended by only **50%** of the
/// lost quarter-hours (rounded up), unlike wind/biomass which get a 1:1 extension.
///
/// **Legal basis**: §51a Abs. 2 EEG 2023:
/// > "Für Solaranlagen gilt eine Verringerung um die Hälfte ..."
///
/// Use [`crate::foerderdauer::verguetungszeitraum_verlaengerung_qh`] with
/// `is_solar = true` to apply this factor automatically.
///
/// # Example
///
/// ```rust
/// use eeg_billing::foerderdauer::verguetungszeitraum_verlaengerung_qh;
///
/// // 100 lost quarter-hours for a solar plant:
/// assert_eq!(verguetungszeitraum_verlaengerung_qh(100, true), 50); // ceil(100/2)
/// // 101 lost quarter-hours:
/// assert_eq!(verguetungszeitraum_verlaengerung_qh(101, true), 51); // ceil(101/2)
/// // Same plant if it were wind — §51a Abs. 1 Satz 2 rounds up to a full day (96 QH):
/// assert_eq!(verguetungszeitraum_verlaengerung_qh(100, false), 192);
/// ```
pub const SECT51A_SOLAR_FACTOR_DENOMINATOR: u64 = 2;

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── SolarBauform ──────────────────────────────────────────────────────────

    #[test]
    fn gebaeude_qualifies_for_volleinspeisung_bonus() {
        assert!(SolarBauform::Gebaeude.volleinspeisung_bonus_eligible());
        assert!(SolarBauform::AgriPv.volleinspeisung_bonus_eligible());
        assert!(!SolarBauform::Freiflaeche.volleinspeisung_bonus_eligible());
    }

    #[test]
    fn freiflaeche_has_location_restriction() {
        assert!(SolarBauform::Freiflaeche.has_freiflaechen_restriction());
        assert!(!SolarBauform::Gebaeude.has_freiflaechen_restriction());
    }

    #[test]
    fn agripv_has_higher_auction_threshold() {
        // Standard: > 1 MWp
        assert_eq!(SolarBauform::Gebaeude.auction_threshold_kwp(), dec!(1000));
        // Agri-PV: > 6 MWp (Solarpaket I)
        assert_eq!(SolarBauform::AgriPv.auction_threshold_kwp(), dec!(6000));
    }

    // ── requires_ausschreibung ────────────────────────────────────────────────

    #[test]
    fn auction_required_above_1mwp_gebaeude() {
        assert!(!requires_ausschreibung(dec!(1000), SolarBauform::Gebaeude)); // exactly 1 MWp: no
        assert!(requires_ausschreibung(dec!(1001), SolarBauform::Gebaeude)); // > 1 MWp: yes
    }

    #[test]
    fn no_auction_for_agripv_below_6mwp() {
        assert!(!requires_ausschreibung(dec!(5999), SolarBauform::AgriPv));
        assert!(requires_ausschreibung(dec!(6001), SolarBauform::AgriPv));
    }

    #[test]
    fn stecker_pv_never_requires_auction() {
        assert!(!requires_ausschreibung(dec!(2), SolarBauform::SteckerPv));
    }

    // ── EinspeisungsModus ─────────────────────────────────────────────────────

    #[test]
    fn volleinspeisung_triggers_sanktionspflicht() {
        let anlage = SolarAnlageData {
            bauform: SolarBauform::Gebaeude,
            einspeisungs_modus: EinspeisungsModus::Volleinspeisung,
            mastr_registriert: true,
            agripv_zertifiziert: false,
        };
        assert!(anlage.volleinspeisung_sanktionspflichtig());
    }

    #[test]
    fn ueberschuss_no_sanktionspflicht() {
        let anlage = SolarAnlageData {
            bauform: SolarBauform::Gebaeude,
            einspeisungs_modus: EinspeisungsModus::Ueberschusseinspeisung,
            mastr_registriert: true,
            agripv_zertifiziert: false,
        };
        assert!(!anlage.volleinspeisung_sanktionspflichtig());
    }

    // ── §51a solar factor ────────────────────────────────────────────────────

    #[test]
    fn sect51a_solar_uses_half_factor() {
        // Verify that foerderdauer helper applies 50% factor for solar
        assert_eq!(
            crate::foerderdauer::verguetungszeitraum_verlaengerung_qh(200, true),
            100 // 50% of 200
        );
        assert_eq!(
            crate::foerderdauer::verguetungszeitraum_verlaengerung_qh(201, true),
            101 // ceil(201/2)
        );
    }

    #[test]
    fn sect51a_wind_rounds_up_to_full_calendar_day() {
        // §51a Abs. 1 Satz 2: 200 QH = 2.08 days → 3 full days = 288 QH.
        assert_eq!(
            crate::foerderdauer::verguetungszeitraum_verlaengerung_qh(200, false),
            288
        );
    }
}

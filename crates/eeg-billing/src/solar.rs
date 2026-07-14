//! Solar PV technology-specific EEG rules — §48 EEG 2023.
//!
//! Solar PV has more technology-specific sub-rules than any other EEG technology:
//!
//! - **§48 Abs. 1** — four legal *Bauformen* with different tariff thresholds
//! - **§48 Abs. 2 vs. Abs. 2a** — Überschusseinspeisung vs. Volleinspeisung rates
//! - **§48a EEG 2023** — Direktlieferung (community solar, Gemeinschaftliche Gebäudeversorgung)
//! - **§48 Abs. 5** — Freiflächenanlage restrictions (location, ecological rules)
//! - **§22 EEG 2023** — auction obligation for large plants (> 1 MWp)
//! - **§12 Abs. 3 UStG** — zero VAT for PV ≤ 30 kWp since 01.01.2023
//! - **§51a EEG 2023** — Verlängerungsanspruch uses a 0.5 factor for solar
//!   (§51a Abs. 2: only 50% of lost kWh extend the period, not 100%)
//! - **Solarpaket I (BGBl I 2024 Nr. 107)** — increased rates from 01.05.2024,
//!   new `Stecker-PV` category (≤ 2 kWp)

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

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
/// This has a DIRECT impact on the EEG tariff rate (§48 Abs. 2 vs. Abs. 2a EEG 2023):
///
/// | Modus | EEG rate (≤10 kWp, Solarpaket I 2024) |
/// |---|---|
/// | `Ueberschuss` | 8.11 ct/kWh (§48 Abs. 2) |
/// | `Volleinspeisung` | **8.51 ct/kWh** (§48 Abs. 2a, +0.40 ct bonus) |
///
/// **Billing consequence**: Using the wrong tariff causes systematic billing errors.
/// Always check whether the plant registered for Volleinspeisung at commissioning.
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

    /// Whether the plant qualifies for the §12 Abs. 3 UStG zero-VAT regime.
    ///
    /// Applies automatically for PV ≤ 30 kWp installed on or at a building,
    /// commissioned on or after 01.01.2023 (§12 Abs. 3 Nr. 1 UStG, as amended
    /// by JStG 2022).
    ///
    /// When `true`, no VAT (Umsatzsteuer) is charged on EEG feed-in receipts.
    /// Use [`ustg_12_3_applies`] to compute this automatically from capacity and date.
    pub ustg_12_3_zero_vat: bool,

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
/// use rust_decimal_macros::dec;
///
/// assert!( requires_ausschreibung(dec!(1001), SolarBauform::Gebaeude));  // > 1 MWp
/// assert!(!requires_ausschreibung(dec!(999),  SolarBauform::Gebaeude));  // ≤ 1 MWp
/// assert!(!requires_ausschreibung(dec!(5000), SolarBauform::AgriPv));    // ≤ 6 MWp
/// ```
#[must_use]
pub fn requires_ausschreibung(leistung_kwp: Decimal, bauform: SolarBauform) -> bool {
    leistung_kwp > bauform.auction_threshold_kwp()
}

// ── §12 Abs. 3 UStG zero-VAT ──────────────────────────────────────────────────

/// Returns `true` when §12 Abs. 3 Nr. 1 UStG applies (zero VAT on PV supply).
///
/// ## Legal basis — §12 Abs. 3 UStG (as amended by JStG 2022, BGBl I 2022 Nr. 58)
///
/// Since **01.01.2023**, supply and installation of solar PV systems, including
/// storage systems, are subject to **zero percent VAT** when ALL of the
/// following conditions are met:
///
/// 1. Installation on or at a **residential building** or a building used for
///    activities serving the public interest (Wohngebäude / gemeinnützige Gebäude)
/// 2. Installed capacity ≤ **30 kWp** per plant / property
/// 3. Commissioned on or after **01.01.2023** (earlier plants: normal VAT regime)
///
/// ## Impact on EEG billing
///
/// When this applies:
/// - EEG Einspeisevergütung receipts are issued **without Umsatzsteuer** (0%)
/// - The operator does NOT need to register for Umsatzsteuer under §14 UStG
/// - Use `billing::TaxLayer` with rate 0 in `EegSettleTariff`
///
/// ## Bestandsschutz
/// Plants commissioned before 01.01.2023 fall under the previous regime
/// (19% USt for Regelbesteuerung, or Kleinunternehmer §19 UStG exemption).
/// This function returns `false` for pre-2023 plants.
///
/// # Example
/// ```rust
/// use eeg_billing::solar::ustg_12_3_applies;
/// use time::macros::date;
/// use rust_decimal_macros::dec;
///
/// // 10 kWp on residential house, commissioned Jan 2024 → zero VAT
/// assert!(ustg_12_3_applies(dec!(10), true, date!(2024-03-01)));
/// // Same plant but commissioned Dec 2022 → NO zero VAT (Bestandsschutz)
/// assert!(!ustg_12_3_applies(dec!(10), true, date!(2022-12-31)));
/// // 35 kWp: exceeds 30 kWp limit → NO zero VAT
/// assert!(!ustg_12_3_applies(dec!(35), true, date!(2024-03-01)));
/// ```
#[must_use]
pub fn ustg_12_3_applies(
    leistung_kwp: Decimal,
    on_residential_or_public_building: bool,
    inbetriebnahme: time::Date,
) -> bool {
    use time::macros::date;
    inbetriebnahme >= date!(2023 - 01 - 01)
        && on_residential_or_public_building
        && leistung_kwp <= dec!(30)
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
/// // Same plant if it were wind (1:1):
/// assert_eq!(verguetungszeitraum_verlaengerung_qh(100, false), 100);
/// ```
pub const SECT51A_SOLAR_FACTOR_DENOMINATOR: u64 = 2;

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

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

    // ── ustg_12_3_applies ─────────────────────────────────────────────────────

    #[test]
    fn zero_vat_applies_post_2023_residential_leq30kwp() {
        assert!(ustg_12_3_applies(dec!(10), true, date!(2024 - 03 - 15)));
        assert!(ustg_12_3_applies(dec!(30), true, date!(2023 - 01 - 01))); // boundary: exact
    }

    #[test]
    fn zero_vat_does_not_apply_pre_2023() {
        assert!(!ustg_12_3_applies(dec!(10), true, date!(2022 - 12 - 31)));
    }

    #[test]
    fn zero_vat_does_not_apply_above_30kwp() {
        assert!(!ustg_12_3_applies(dec!(31), true, date!(2024 - 01 - 01)));
    }

    #[test]
    fn zero_vat_does_not_apply_non_residential() {
        assert!(!ustg_12_3_applies(dec!(10), false, date!(2024 - 01 - 01)));
    }

    // ── EinspeisungsModus ─────────────────────────────────────────────────────

    #[test]
    fn volleinspeisung_triggers_sanktionspflicht() {
        let anlage = SolarAnlageData {
            bauform: SolarBauform::Gebaeude,
            einspeisungs_modus: EinspeisungsModus::Volleinspeisung,
            ustg_12_3_zero_vat: true,
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
            ustg_12_3_zero_vat: false,
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
    fn sect51a_wind_uses_full_factor() {
        assert_eq!(
            crate::foerderdauer::verguetungszeitraum_verlaengerung_qh(200, false),
            200 // 1:1 for non-solar
        );
    }
}

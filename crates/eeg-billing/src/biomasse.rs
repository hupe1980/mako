//! Biomass-specific EEG settlement models — §§ 42–44b EEG 2023.
//!
//! Biomass plants have fuel-type-dependent remuneration rules:
//!
//! - **§ 42** — the statutory anzulegender Wert for Biomasse: 12,67 ct/kWh up to
//!   a Bemessungsleistung of 150 kW, and Satz 2 excludes Biomethan. Above that,
//!   the value comes from a tender (§ 22 Abs. 4).
//! - **§ 43** — Vergärung von Bioabfällen: a separate, higher claim for plants
//!   running on ≥ 90 Masseprozent separately collected Bioabfälle.
//! - **§ 44** — Vergärung von Gülle: the Güllekleinanlage claim, ≤ 150 kW
//!   installed at the Biogaserzeugungsanlage's site with a minimum manure share.
//! - **§ 44b Abs. 1** — the 45 %-Bemessungsleistung annual quota for Biogas
//!   plants over 100 kW installed; § 44 Gülle plants and § 39 tendered plants
//!   are excluded by Satz 3.
//! - **§ 39i Abs. 1** — the Getreidekorn-und-Mais cap, which applies **only to
//!   plants holding a tender award** and steps down by award year: 40 % (2023),
//!   35 % (2024 – 24.02.2025), 30 % (25.02.2025 – 2025), 25 % (2026–2028).
//!
//! There is no § 42a EEG, and the EEG 2023 imposes no Holzbiomasse restriction —
//! the sustainability rules for solid biomass sit outside it.

use crate::rounding::RoundMoney;
use rust_decimal::Decimal;

// ── BiomassBrennstoff ─────────────────────────────────────────────────────────

/// Biomass fuel type — determines which §43/§44 EEG 2023 rules apply.
///
/// The fuel type affects:
/// - Whether the § 44 Güllekleinanlage claim is applicable.
/// - Whether the § 39i Abs. 1 Getreidekorn/Mais cap applies (tendered plants).
/// - Which of §§ 42/43/44 sets the anzulegender Wert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum BiomassBrennstoff {
    /// Standard plant-based biomass (agricultural residues, dedicated crops).
    ///
    /// Where the plant holds a tender award, subject to the § 39i Abs. 1
    /// Getreidekorn-und-Mais cap, which steps down 40 → 35 → 30 → 25 % by the
    /// year of the award.
    PflanzlicheBiomasse,

    /// Biomethane from biomass feedstocks — upgraded and fed into the gas grid.
    BiomethanAusBiomasse,

    /// Liquid manure (Gülle) and slurry — qualifies for Güllekleinanlage bonus.
    ///
    /// §44 EEG 2023 Güllekleinanlage rules:
    ///
    /// - Plant capacity ≤ 75 kW_el
    /// - ≥80% of energy input from liquid manure/slurry
    ///
    /// When both criteria are met, the Güllekleinanlage bonus rate applies.
    Guelle,

    /// Solid manure (Festmist) — also eligible for Güllekleinanlage if ≥80%.
    Festmist,

    /// Wood biomass (feste Biomasse).
    ///
    /// The EEG 2023 sets no separate anzulegender Wert for it and imposes no
    /// fresh-wood restriction — the sustainability requirements for solid
    /// biomass sit outside the EEG. What the EEG does add for tendered plants
    /// running feste Biomasse is the § 39i Abs. 2 Höchstbemessungsleistung: 25 %
    /// below the awarded Gebotsmenge, above which the claim falls to zero (or to
    /// the Marktwert under an Einspeisevergütung).
    Holzbiomasse,

    /// Sewage gas (Klärgas) from wastewater treatment.
    Klaegas,

    /// Landfill gas (Deponiegas).
    Deponiegas,

    /// Mine gas (Grubengas) from coal mines.
    Grubengas,

    /// Biogenic waste fractions (not covered by §43 substrate caps).
    BiogenicWaste,
}

impl BiomassBrennstoff {
    /// Returns `true` when this fuel type is eligible for the §44 Güllekleinanlage bonus.
    ///
    /// The bonus requires BOTH:
    /// - Plant capacity ≤ 75 kW_el (checked separately in [`BiomassSettlementData`])
    /// - ≥ 80% energy from Gülle/Festmist (this method confirms fuel type)
    #[must_use]
    pub fn guellebonusanlage_eligible(self) -> bool {
        matches!(self, Self::Guelle | Self::Festmist)
    }
}

// ── BiomassSettlementData ─────────────────────────────────────────────────────

/// Biomass-specific data required for correct §42–§44 EEG 2023 settlement.
///
/// Add this to `SettleInput` via the `biomasse` field when settling biomass
/// or biogas plants.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BiomassSettlementData {
    /// Primary fuel type fed into this plant.
    pub hauptbrennstoff: BiomassBrennstoff,

    /// Fraction of energy input from liquid/solid manure (0.0–1.0).
    ///
    /// Used to determine §44 Güllekleinanlage eligibility:
    /// - ≥ 0.80 (80% manure) + plant ≤ 75 kW → eligible for bonus
    pub guelle_anteil: Decimal,

    /// Whether the plant qualifies as a §44 Güllekleinanlage (≤75 kW + ≥80% Gülle).
    ///
    /// Set by the billing system based on `guelle_anteil >= 0.80` AND
    /// `leistung_kwp <= 75`. When `true`, use the Güllekleinanlage tariff rate.
    pub ist_guellebonusanlage: bool,

    /// Fraction of energy input from Energiepflanzen vom Acker (0.0–1.0).
    ///
    /// §43 Abs. 1 Nr. 2 EEG 2023 substrate cap: must be ≤ 0.40 (40%).
    /// Exceeding this cap can result in loss of EEG support for the excess.
    pub energiepflanzen_anteil: Decimal,

    /// Whether the §43 substrate cap is met (`energiepflanzen_anteil <= 0.40`).
    pub substrate_cap_ok: bool,
}

impl BiomassSettlementData {
    /// Construct from fuel type and composition data.
    ///
    /// Automatically computes `ist_guellebonusanlage` and `substrate_cap_ok`.
    #[must_use]
    pub fn new(
        hauptbrennstoff: BiomassBrennstoff,
        guelle_anteil: Decimal,
        energiepflanzen_anteil: Decimal,
        leistung_kw: Decimal,
    ) -> Self {
        use rust_decimal::dec;
        let ist_guellebonusanlage = hauptbrennstoff.guellebonusanlage_eligible()
            && guelle_anteil >= dec!(0.80)
            && leistung_kw <= dec!(75);
        let substrate_cap_ok = energiepflanzen_anteil <= dec!(0.40);
        Self {
            hauptbrennstoff,
            guelle_anteil,
            ist_guellebonusanlage,
            energiepflanzen_anteil,
            substrate_cap_ok,
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;

    #[test]
    fn guellebonusanlage_qualifies_when_criteria_met() {
        let data = BiomassSettlementData::new(
            BiomassBrennstoff::Guelle,
            dec!(0.85), // 85% Gülle — above 80% threshold
            dec!(0.05), // minimal energy crops
            dec!(50),   // 50 kW — below 75 kW limit
        );
        assert!(data.ist_guellebonusanlage);
        assert!(data.substrate_cap_ok);
    }

    #[test]
    fn guellebonusanlage_disqualified_by_capacity() {
        let data = BiomassSettlementData::new(
            BiomassBrennstoff::Guelle,
            dec!(0.90), // 90% Gülle — above threshold
            dec!(0.05),
            dec!(100), // 100 kW — ABOVE 75 kW limit
        );
        assert!(!data.ist_guellebonusanlage, "capacity > 75 kW → no bonus");
    }

    #[test]
    fn substrate_cap_exceeded() {
        let data = BiomassSettlementData::new(
            BiomassBrennstoff::PflanzlicheBiomasse,
            dec!(0.0),
            dec!(0.50), // 50% energy crops — EXCEEDS 40% cap
            dec!(100),
        );
        assert!(!data.substrate_cap_ok);
    }
}

// ── §3 Nr. 6 EEG — Bemessungsleistung ─────────────────────────────────────────

/// The hours a Bemessungsleistung is measured against for one calendar year.
///
/// §3 Nr. 6 EEG defines the Bemessungsleistung as the annual kWh divided by
/// *„die Summe der vollen Zeitstunden des jeweiligen Kalenderjahres abzüglich der
/// vollen Stunden vor der erstmaligen Erzeugung"*. Two things follow that a flat
/// 8 760 gets wrong:
///
/// - a **leap year has 8 784 hours**, so the flat figure understates the quota by
///   24 h × the capacity share — real money for a plant on the §44b cap;
/// - a plant that **first generated during the year** is measured against the
///   hours since, not the whole year, so a flat figure hands it a full-year quota
///   the statute does not give it.
///
/// Returns `None` only for a year the calendar cannot represent.
#[must_use]
pub fn bemessungsleistung_stunden(
    kalenderjahr: i32,
    erstmalige_erzeugung: Option<time::Date>,
) -> Option<rust_decimal::Decimal> {
    use rust_decimal::Decimal;
    let jahresbeginn =
        time::Date::from_calendar_date(kalenderjahr, time::Month::January, 1).ok()?;
    let naechstes_jahr =
        time::Date::from_calendar_date(kalenderjahr + 1, time::Month::January, 1).ok()?;

    // Hours before the first generation are deducted; a plant that started in an
    // earlier year has none to deduct, one that starts later has no claim at all.
    let start = match erstmalige_erzeugung {
        Some(d) if d >= naechstes_jahr => return Some(Decimal::ZERO),
        Some(d) if d > jahresbeginn => d,
        _ => jahresbeginn,
    };
    let tage = (naechstes_jahr - start).whole_days();
    Some(Decimal::from(tage) * Decimal::from(24))
}

/// §44b Abs. 1 EEG 2023 — the annual kWh a Biogas plant may be paid the full rate for.
///
/// The share of a calendar year's generation whose Bemessungsleistung equals 45 %
/// of the installed capacity: `0,45 × P_inst ×` the §3 Nr. 6 hours.
#[must_use]
pub fn sect44b_jahreskontingent_kwh(
    leistung_kw: rust_decimal::Decimal,
    kalenderjahr: i32,
    erstmalige_erzeugung: Option<time::Date>,
) -> rust_decimal::Decimal {
    use rust_decimal::dec;
    let stunden =
        bemessungsleistung_stunden(kalenderjahr, erstmalige_erzeugung).unwrap_or(dec!(8760));
    (leistung_kw * dec!(0.45) * stunden).round_kfm(3)
}

#[cfg(test)]
mod bemessungsleistung_tests {
    use super::*;
    use rust_decimal::dec;
    use time::macros::date;

    /// A leap year has 8 784 hours. The flat 8 760 cost a 500 kW plant
    /// 24 × 0,45 × 500 = 5 400 kWh of full-rate quota every leap year.
    #[test]
    fn a_leap_year_has_more_hours() {
        assert_eq!(bemessungsleistung_stunden(2027, None), Some(dec!(8760)));
        assert_eq!(bemessungsleistung_stunden(2028, None), Some(dec!(8784)));
        assert_eq!(
            sect44b_jahreskontingent_kwh(dec!(500), 2028, None)
                - sect44b_jahreskontingent_kwh(dec!(500), 2027, None),
            dec!(5400)
        );
    }

    /// §3 Nr. 6 deducts the hours before the first generation, so a plant that
    /// started mid-year is measured against the rest of it — a flat figure would
    /// hand it a full-year quota.
    #[test]
    fn hours_before_the_first_generation_are_deducted() {
        // 1 July 2027 → 184 days remain (Jul 31 + Aug 31 + Sep 30 + Oct 31 + Nov 30 + Dec 31).
        assert_eq!(
            bemessungsleistung_stunden(2027, Some(date!(2027 - 07 - 01))),
            Some(dec!(4416))
        );
        // A plant commissioned in an earlier year is measured against the whole year.
        assert_eq!(
            bemessungsleistung_stunden(2027, Some(date!(2020 - 03 - 01))),
            Some(dec!(8760))
        );
        // One that starts after the year has no quota in it at all.
        assert_eq!(
            bemessungsleistung_stunden(2027, Some(date!(2028 - 01 - 01))),
            Some(dec!(0))
        );
    }
}

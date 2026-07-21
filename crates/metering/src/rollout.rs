//! iMSys rollout obligations per §29 MsbG and the §45 MsbG Rollout-Fahrplan.
//!
//! The grundzuständige Messstellenbetreiber must equip certain Messstellen
//! with an intelligentes Messsystem (Pflichteinbaufälle); everywhere else the
//! installation is permitted but optional (Optionsfälle, §29 Abs. 2 MsbG).
//! Classification drives WiM device-change processes and the §45 quota
//! reporting.
//!
//! ## §29 Abs. 1 MsbG (GNDEW version) — Pflichteinbaufälle
//!
//! - **Nr. 1**: Letztverbraucher with annual consumption **> 6 000 kWh**.
//! - **Nr. 2a**: Letztverbraucher with a **§14a EnWG agreement** (steuerbare
//!   Verbrauchseinrichtung) — no consumption threshold applies; these also
//!   need a Steuerungseinrichtung at the Netzanschlusspunkt.
//! - **Nr. 2b**: Anlagenbetreiber (EEG/KWK) with installed capacity
//!   **> 7 kW**, to the extent required to meet the §45 Abs. 1 quotas.
//!   The pre-GNDEW 7–100 kW band no longer appears in the current text —
//!   there is no verified upper capacity limit.
//!
//! Source: gesetze-im-internet.de/messbg/__29.html (retrieved 2026-07).

use rust_decimal::Decimal;
use time::Date;
use time::macros::date;

/// The consumption threshold of §29 Abs. 1 Nr. 1 MsbG (kWh per year).
pub const PFLICHT_CONSUMPTION_KWH_PER_YEAR: u32 = 6_000;

/// The generation threshold of §29 Abs. 1 Nr. 2b MsbG (kW installed).
pub const PFLICHT_GENERATION_KW: u32 = 7;

// ── Classification ────────────────────────────────────────────────────────────

/// Why (or whether) a Messstelle must be equipped with an iMSys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum RolloutObligation {
    /// §29 Abs. 1 Nr. 1: annual consumption above 6 000 kWh.
    PflichtConsumption,
    /// §29 Abs. 1 Nr. 2a: a §14a EnWG agreement exists (steuerbare
    /// Verbrauchseinrichtung) — additionally requires a Steuerungseinrichtung.
    PflichtSteuerbare14a,
    /// §29 Abs. 1 Nr. 2b: EEG/KWK plant above 7 kW installed capacity.
    PflichtGeneration,
    /// §29 Abs. 2: optionaler Einbaufall — permitted, not required.
    Optionsfall,
}

impl RolloutObligation {
    /// `true` for every mandatory case of §29 Abs. 1 MsbG.
    #[must_use]
    pub fn is_pflichteinbaufall(self) -> bool {
        !matches!(self, Self::Optionsfall)
    }

    /// `true` when the case also requires a Steuerungseinrichtung at the
    /// Netzanschlusspunkt (§29 Abs. 1 Nr. 2 MsbG).
    #[must_use]
    pub fn requires_steuerungseinrichtung(self) -> bool {
        matches!(self, Self::PflichtSteuerbare14a)
    }

    /// Statutory basis, for audit output.
    #[must_use]
    pub fn legal_basis(self) -> &'static str {
        match self {
            Self::PflichtConsumption => "§29 Abs. 1 Nr. 1 MsbG",
            Self::PflichtSteuerbare14a => "§29 Abs. 1 Nr. 2a MsbG",
            Self::PflichtGeneration => "§29 Abs. 1 Nr. 2b MsbG",
            Self::Optionsfall => "§29 Abs. 2 MsbG",
        }
    }
}

/// Classify a Messstelle against §29 Abs. 1 MsbG.
///
/// Precedence follows the statute's numbering: a §14a agreement is reported
/// as Nr. 2a even when consumption alone would already mandate the iMSys,
/// because Nr. 2 additionally requires the Steuerungseinrichtung.
#[must_use]
pub fn classify_rollout_obligation(
    annual_consumption_kwh: Decimal,
    installed_generation_kw: Option<Decimal>,
    has_14a_agreement: bool,
) -> RolloutObligation {
    if has_14a_agreement {
        return RolloutObligation::PflichtSteuerbare14a;
    }
    if let Some(kw) = installed_generation_kw
        && kw > Decimal::from(PFLICHT_GENERATION_KW)
    {
        return RolloutObligation::PflichtGeneration;
    }
    if annual_consumption_kwh > Decimal::from(PFLICHT_CONSUMPTION_KWH_PER_YEAR) {
        return RolloutObligation::PflichtConsumption;
    }
    RolloutObligation::Optionsfall
}

// ── §45 Rollout-Fahrplan ──────────────────────────────────────────────────────

/// What a §45 Abs. 1 MsbG quota is measured against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum QuotaScope {
    /// Share of the total stock of agendagebundene Messstellen.
    TotalStock,
    /// Share of the Messstellen newly falling due within the window.
    NewInWindow,
}

/// One milestone of the §45 Abs. 1 MsbG Rollout-Fahrplan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RolloutMilestone {
    /// Window start (None for the stock quotas, which have no flow window).
    pub window_from: Option<Date>,
    /// Deadline by which the quota must be met.
    pub deadline: Date,
    /// Required share in percent.
    pub quota_pct: u8,
    /// Stock or flow quota.
    pub scope: QuotaScope,
}

/// The §45 Abs. 1 MsbG milestones for the agendagebundene Pflichteinbaufälle.
///
/// 20 % of the stock by end-2025; 90 % of the newly-due Messstellen in each
/// of the 2025/26, 2027/28 and 2029/30 windows; 90 % of the total stock by
/// end-2032. Source: gesetze-im-internet.de/messbg/__45.html.
pub const ROLLOUT_MILESTONES: [RolloutMilestone; 5] = [
    RolloutMilestone {
        window_from: None,
        deadline: date!(2025 - 12 - 31),
        quota_pct: 20,
        scope: QuotaScope::TotalStock,
    },
    RolloutMilestone {
        window_from: Some(date!(2025 - 02 - 25)),
        deadline: date!(2026 - 12 - 31),
        quota_pct: 90,
        scope: QuotaScope::NewInWindow,
    },
    RolloutMilestone {
        window_from: Some(date!(2027 - 01 - 01)),
        deadline: date!(2028 - 12 - 31),
        quota_pct: 90,
        scope: QuotaScope::NewInWindow,
    },
    RolloutMilestone {
        window_from: Some(date!(2029 - 01 - 01)),
        deadline: date!(2030 - 12 - 31),
        quota_pct: 90,
        scope: QuotaScope::NewInWindow,
    },
    RolloutMilestone {
        window_from: None,
        deadline: date!(2032 - 12 - 31),
        quota_pct: 90,
        scope: QuotaScope::TotalStock,
    },
];

/// The milestone whose deadline is next due on `today` (or `None` after 2032).
#[must_use]
pub fn next_milestone(today: Date) -> Option<&'static RolloutMilestone> {
    ROLLOUT_MILESTONES.iter().find(|m| m.deadline >= today)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;

    #[test]
    fn consumption_over_6000_kwh_is_pflicht_nr1() {
        let o = classify_rollout_obligation(dec!(6001), None, false);
        assert_eq!(o, RolloutObligation::PflichtConsumption);
        assert!(o.is_pflichteinbaufall());
        assert!(!o.requires_steuerungseinrichtung());
    }

    #[test]
    fn exactly_6000_kwh_is_not_mandatory() {
        // §29 Abs. 1 Nr. 1: "mehr als 6 000 Kilowattstunden" — strict.
        assert_eq!(
            classify_rollout_obligation(dec!(6000), None, false),
            RolloutObligation::Optionsfall
        );
    }

    #[test]
    fn a_14a_agreement_dominates_and_needs_steuerung() {
        let o = classify_rollout_obligation(dec!(12000), Some(dec!(20)), true);
        assert_eq!(o, RolloutObligation::PflichtSteuerbare14a);
        assert!(o.requires_steuerungseinrichtung());
        assert_eq!(o.legal_basis(), "§29 Abs. 1 Nr. 2a MsbG");
    }

    #[test]
    fn generation_over_7_kw_is_pflicht_nr2b_with_no_upper_cap() {
        assert_eq!(
            classify_rollout_obligation(dec!(0), Some(dec!(7.1)), false),
            RolloutObligation::PflichtGeneration
        );
        // No 100-kW upper bracket exists in the current §29.
        assert_eq!(
            classify_rollout_obligation(dec!(0), Some(dec!(950)), false),
            RolloutObligation::PflichtGeneration
        );
        assert_eq!(
            classify_rollout_obligation(dec!(0), Some(dec!(7)), false),
            RolloutObligation::Optionsfall
        );
    }

    #[test]
    fn milestones_are_ordered_and_end_2032() {
        assert!(
            ROLLOUT_MILESTONES
                .windows(2)
                .all(|w| w[0].deadline <= w[1].deadline)
        );
        assert_eq!(
            next_milestone(date!(2026 - 07 - 01)).unwrap().deadline,
            date!(2026 - 12 - 31)
        );
        assert!(next_milestone(date!(2033 - 01 - 01)).is_none());
    }
}

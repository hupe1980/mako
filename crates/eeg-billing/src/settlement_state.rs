//! Monthly settlement lifecycle state machine for EEG plants.
//!
//! Every EEG plant has a **per-period settlement state** that describes whether
//! the full Vergütung, a reduced amount, or no payment at all can be disbursed.
//!
//! ## State machine
//!
//! ```text
//!                ┌─────────────────────────────────────────┐
//!                │               NORMAL FLOW               │
//!                └─────────────────────────────────────────┘
//!
//!   PlantCommissioned ──→ Active (Vergütung flows normally)
//!                            │
//!                            ├──→ Reduced (§52 sanction, §53b, technical defect)
//!                            │        └──→ Active (when violation resolved)
//!                            │
//!                            ├──→ Suspended (no payment, §52 EEG ≤2021 MaStR)
//!                            │        └──→ Active (when MaStR registered)
//!                            │
//!                            ├──→ Interrupted (temporary: negative prices, force majeure)
//!                            │        └──→ Active (next period)
//!                            │
//!                            ├──→ PostEeg (Förderdauer expired, EPEX basis)
//!                            │
//!                            └──→ Ended (plant decommissioned or Förderdauer expired + no PostEEG)
//! ```
//!
//! ## Relationship to `SettlementStatus`
//!
//! `SettlementStatus` in `SettleOutput` reflects the **calculation result** for
//! a single period. `SettlementPeriodState` is the **persistent plant-level state**
//! stored in `einsd`'s DB and used as context for the next month's settlement.
//!
//! | SettlementStatus | Typical SettlementPeriodState |
//! |---|---|
//! | `Calculated` | `Active` or `Reduced` |
//! | `NoData` | `Active` (data pending) |
//! | `PriceMissing` | `Active` (EPEX data pending) |
//! | `Sanctioned` | `Suspended` or `Reduced` |
//! | `FoerderungBeendet` | `Ended` or `PostEeg` |

use crate::technology::ErzeugungsArt;
use rust_decimal::Decimal;
use time::Date;

// ── SettlementPeriodState ─────────────────────────────────────────────────────

/// Persistent per-plant monthly settlement lifecycle state.
///
/// Stored in `einsd`'s `eeg_anlagen.settlement_state` column.
/// Determines how the next billing period is processed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum SettlementPeriodState {
    /// Normal: full Vergütung / Marktprämie flows as per the applicable scheme.
    ///
    /// `SettleInput::sanktion` should be `None` and `pflichtverstoss` empty.
    Active,

    /// Vergütung reduced to a fraction or different basis due to ongoing sanction.
    ///
    /// Examples:
    /// - §52 Abs. 3 EEG ≤2021: 20% reduction (SanktionAlt::VerguetungReduziert20Prozent)
    /// - §52 Abs. 2 EEG ≤2021: reduced to EPEX Marktwert (SanktionAlt::VerguetungAufMarktwert)
    /// - §52 EEG 2023 Pflichtzahlungen active but Vergütung still flows
    /// - §53b regional reduction in effect
    Reduced,

    /// No EEG payment disbursed.
    ///
    /// Examples:
    /// - §52 Abs. 1 EEG ≤2021: MaStR not registered (VerguetungAufNull)
    /// - §52 Abs. 1 EEG ≤2021: Direktvermarktungspflicht not met (VerguetungAufNull)
    Suspended,

    /// Temporarily no payment this period (data, price, or force-majeure related).
    ///
    /// Unlike `Suspended`, this is not a regulatory sanction — the plant is healthy
    /// and will resume normally next period. No operator action required.
    ///
    /// Examples:
    /// - Meter data not yet available (`SettlementStatus::NoData`)
    /// - EPEX monthly price not yet imported (`SettlementStatus::PriceMissing`)
    Interrupted,

    /// 20-year Förderdauer expired; plant now eligible for post-EEG remuneration.
    ///
    /// Settlement continues but at EPEX spot price (`SettlementScheme::PostEeg`).
    /// The plant's `foerderendedatum` has passed.
    PostEeg,

    /// Plant has no further EEG billing (decommissioned or no post-EEG continuation).
    ///
    /// Terminal state. No more settlement periods expected.
    Ended,
}

impl SettlementPeriodState {
    /// Returns `true` when the plant can potentially receive a payment this period.
    #[must_use]
    pub fn is_payable(self) -> bool {
        matches!(
            self,
            Self::Active | Self::Reduced | Self::PostEeg | Self::Interrupted
        )
    }

    /// Returns `true` when this state represents a regulatory sanction that requires
    /// operator action to resolve.
    #[must_use]
    pub fn requires_operator_action(self) -> bool {
        matches!(self, Self::Suspended | Self::Reduced)
    }

    /// Returns `true` when this is a terminal state (no future settlements).
    #[must_use]
    pub fn is_terminal(self) -> bool {
        self == Self::Ended
    }

    /// Convert to the DB string representation.
    ///
    /// Used for `eeg_anlagen.settlement_state` column.
    #[must_use]
    pub fn to_db_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Reduced => "reduced",
            Self::Suspended => "suspended",
            Self::Interrupted => "interrupted",
            Self::PostEeg => "post_eeg",
            Self::Ended => "ended",
        }
    }

    /// Parse from DB string.
    ///
    /// # Errors
    ///
    /// Returns `Err` for unknown values.
    pub fn from_db_str(s: &str) -> Result<Self, InvalidSettlementPeriodState> {
        match s {
            "active" => Ok(Self::Active),
            "reduced" => Ok(Self::Reduced),
            "suspended" => Ok(Self::Suspended),
            "interrupted" => Ok(Self::Interrupted),
            "post_eeg" => Ok(Self::PostEeg),
            "ended" => Ok(Self::Ended),
            other => Err(InvalidSettlementPeriodState(other.to_owned())),
        }
    }
}

/// Error returned when a DB string cannot be parsed as [`SettlementPeriodState`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid settlement_period_state: '{0}'")]
pub struct InvalidSettlementPeriodState(pub String);

// ── StateTransition ───────────────────────────────────────────────────────────

/// A recorded transition of a plant's settlement state.
///
/// Stored in `einsd`'s `settlement_state_transitions` audit table.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StateTransition {
    /// State before the transition.
    pub from: SettlementPeriodState,
    /// State after the transition.
    pub to: SettlementPeriodState,
    /// First billing period in the new state (year-month).
    pub effective_from: Date,
    /// Human-readable reason for the transition.
    pub reason: StateTransitionReason,
}

/// Reason for a settlement state change.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum StateTransitionReason {
    /// Plant first commissioned and registered in einsd.
    InitialCommissioning,
    /// MaStR registration confirmed → suspending sanction lifted.
    MastrRegistered,
    /// §9 EEG Fernsteuerbarkeit installed.
    FernsteuerbarkeitInstalled,
    /// Direktvermarktung started (§20 / §21 EEG).
    DirektvermarktungStarted,
    /// Direktvermarktung ended, switched back to Einspeisevergütung.
    DirektvermarktungEnded,
    /// §52 violation detected.
    Sect52ViolationDetected,
    /// §52 violation resolved retroactively.
    Sect52ViolationResolved,
    /// Förderdauer expired.
    FoerderungExpired,
    /// Post-EEG operation started (EPEX spot basis).
    PostEegStarted,
    /// Plant decommissioned.
    Decommissioned,
    /// Repowering — new Förderdauer begins.
    Repowering,
}

// ── State derivation helpers ──────────────────────────────────────────────────

/// The compliance facts a settlement state is derived from.
///
/// A struct rather than six positional arguments: the old signature took two
/// `Option<Date>`s, a `Decimal`, a `bool` and an `i16` in a row, and every call
/// site had to be read against the definition to know which was which.
#[derive(Debug, Clone, Copy)]
pub struct SettlementStateFacts {
    /// Whether the plant has a confirmed MaStR registration.
    pub mastr_registriert: bool,
    /// How the plant satisfies §9 (Fernsteuerbarkeit, 60 % cap, or nothing).
    pub sect9_erfuellung: crate::settlement_state::Sect9Erfuellung,
    /// Installed capacity — §9 is staged by it.
    pub leistung_kwp: rust_decimal::Decimal,
    /// Technology, for the §9 Abs. 1 Satz 2 Steckersolar carve-out.
    pub erzeugungsart: Option<ErzeugungsArt>,
    /// Subsidy end date; `None` = never expires.
    pub foerderendedatum: Option<Date>,
    /// First day of the billing period being evaluated.
    pub billing_date: Date,
    /// EEG law year (0 = KWKG).
    pub eeg_gesetz_year: i16,
}

/// Derive the expected [`SettlementPeriodState`] from plant compliance facts.
///
/// This is a **deterministic helper** — it does not access the DB. The state
/// stored in `einsd` may lag by one billing period, since it is written after
/// each month's settlement run.
///
/// # Example
///
/// ```rust
/// use eeg_billing::settlement_state::{
///     derive_settlement_state, Sect9Erfuellung, SettlementPeriodState, SettlementStateFacts,
/// };
/// use rust_decimal::dec;
/// use time::macros::date;
///
/// let facts = SettlementStateFacts {
///     mastr_registriert: true,
///     sect9_erfuellung: Sect9Erfuellung::Fernsteuerbarkeit,
///     leistung_kwp: dec!(50),
///     erzeugungsart: None,
///     foerderendedatum: Some(date!(2040-12-31)),
///     billing_date: date!(2026-07-01),
///     eeg_gesetz_year: 2023,
/// };
/// assert_eq!(derive_settlement_state(&facts), SettlementPeriodState::Active);
///
/// // A 50 kW plant on the 60 % Leistungsbegrenzung is compliant too — §9 Abs. 2
/// // Nr. 2 offers it as an equal alternative below 100 kW.
/// let cap = SettlementStateFacts {
///     sect9_erfuellung: Sect9Erfuellung::Leistungsbegrenzung60,
///     ..facts
/// };
/// assert_eq!(derive_settlement_state(&cap), SettlementPeriodState::Active);
///
/// // MaStR not registered, EEG 2023 → Reduced (Pflichtzahlung, not suspension)
/// let no_mastr = SettlementStateFacts { mastr_registriert: false, ..facts };
/// assert_eq!(derive_settlement_state(&no_mastr), SettlementPeriodState::Reduced);
///
/// // The same under EEG ≤2021 → Suspended (Vergütung auf null)
/// let old = SettlementStateFacts { eeg_gesetz_year: 2017, ..no_mastr };
/// assert_eq!(derive_settlement_state(&old), SettlementPeriodState::Suspended);
///
/// // Förderdauer expired → PostEeg
/// let expired = SettlementStateFacts { foerderendedatum: Some(date!(2020-12-31)), ..facts };
/// assert_eq!(derive_settlement_state(&expired), SettlementPeriodState::PostEeg);
/// ```
#[must_use]
pub fn derive_settlement_state(facts: &SettlementStateFacts) -> SettlementPeriodState {
    let &SettlementStateFacts {
        mastr_registriert,
        sect9_erfuellung,
        leistung_kwp,
        erzeugungsart: art,
        foerderendedatum,
        billing_date,
        eeg_gesetz_year,
    } = facts;

    // ── Förderdauer expired ───────────────────────────────────────────────────
    if let Some(fed) = foerderendedatum
        && billing_date > fed
    {
        return SettlementPeriodState::PostEeg;
    }

    // ── MaStR not registered ──────────────────────────────────────────────────
    if !mastr_registriert {
        return if eeg_gesetz_year >= 2023 {
            // EEG 2023: Pflichtzahlung, Vergütung still flows (§52 Abs. 1 Nr. 11)
            SettlementPeriodState::Reduced
        } else {
            // EEG ≤2021 via §100: VerguetungAufNull (§47 EEG 2021 old regime)
            SettlementPeriodState::Suspended
        };
    }

    // ── §9 EEG not satisfied ──────────────────────────────────────────────────
    // The obligation is staged: from 100 kW only Fernsteuerbarkeit will do, in the
    // 25–100 kW band the 60 % Leistungsbegrenzung is an equal alternative, and a
    // Steckersolargerät below 2 kW is out of scope entirely.
    if sect9_verletzt(leistung_kwp, art, sect9_erfuellung) {
        return if eeg_gesetz_year >= 2023 {
            // EEG 2023: Pflichtzahlung €10/kW/month (§52 Abs. 1 Nr. 1)
            SettlementPeriodState::Reduced
        } else {
            // EEG ≤2021: VerguetungAufMarktwert (§52 Abs. 2 old regime)
            SettlementPeriodState::Reduced // reduced to EPEX Marktwert
        };
    }

    // ── All checks pass → Active ──────────────────────────────────────────────
    SettlementPeriodState::Active
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;
    use time::macros::date;

    #[test]
    fn db_roundtrip_all_states() {
        let states = [
            SettlementPeriodState::Active,
            SettlementPeriodState::Reduced,
            SettlementPeriodState::Suspended,
            SettlementPeriodState::Interrupted,
            SettlementPeriodState::PostEeg,
            SettlementPeriodState::Ended,
        ];
        for s in states {
            let db = s.to_db_str();
            let parsed = SettlementPeriodState::from_db_str(db).unwrap();
            assert_eq!(s, parsed, "roundtrip failed for {s:?}");
        }
    }

    #[test]
    fn unknown_db_str_returns_error() {
        assert!(SettlementPeriodState::from_db_str("unknown").is_err());
    }

    #[test]
    fn is_payable_states() {
        assert!(SettlementPeriodState::Active.is_payable());
        assert!(SettlementPeriodState::Reduced.is_payable());
        assert!(SettlementPeriodState::PostEeg.is_payable());
        assert!(SettlementPeriodState::Interrupted.is_payable());
        assert!(!SettlementPeriodState::Suspended.is_payable());
        assert!(!SettlementPeriodState::Ended.is_payable());
    }

    /// A healthy plant, and the four facts that move it off `Active`.
    fn gesund() -> SettlementStateFacts {
        SettlementStateFacts {
            mastr_registriert: true,
            sect9_erfuellung: Sect9Erfuellung::Fernsteuerbarkeit,
            leistung_kwp: dec!(50),
            erzeugungsart: None,
            foerderendedatum: Some(date!(2040 - 12 - 31)),
            billing_date: date!(2026 - 07 - 01),
            eeg_gesetz_year: 2023,
        }
    }

    #[test]
    fn derive_active_healthy_plant() {
        assert_eq!(
            derive_settlement_state(&gesund()),
            SettlementPeriodState::Active
        );
    }

    #[test]
    fn derive_post_eeg_expired() {
        let facts = SettlementStateFacts {
            foerderendedatum: Some(date!(2020 - 12 - 31)),
            ..gesund()
        };
        assert_eq!(
            derive_settlement_state(&facts),
            SettlementPeriodState::PostEeg
        );
    }

    #[test]
    fn derive_reduced_eeg2023_mastr_missing() {
        let facts = SettlementStateFacts {
            mastr_registriert: false,
            ..gesund()
        };
        assert_eq!(
            derive_settlement_state(&facts),
            SettlementPeriodState::Reduced
        );
    }

    #[test]
    fn derive_suspended_eeg2017_mastr_missing() {
        let facts = SettlementStateFacts {
            mastr_registriert: false,
            eeg_gesetz_year: 2017,
            ..gesund()
        };
        assert_eq!(
            derive_settlement_state(&facts),
            SettlementPeriodState::Suspended
        );
    }

    #[test]
    fn derive_reduced_when_sect9_is_not_satisfied_at_all() {
        let facts = SettlementStateFacts {
            sect9_erfuellung: Sect9Erfuellung::Keine,
            ..gesund()
        };
        assert_eq!(
            derive_settlement_state(&facts),
            SettlementPeriodState::Reduced
        );
    }

    /// §9 Abs. 2 Nr. 2 — the 25–100 kW band may satisfy §9 with the 60 %
    /// Leistungsbegrenzung, so a flat "≥ 25 kW needs Fernsteuerbarkeit" rule
    /// would put such a plant into `Reduced` and charge it 10 €/kW/month.
    #[test]
    fn the_sixty_percent_cap_keeps_a_50kw_plant_active() {
        let facts = SettlementStateFacts {
            sect9_erfuellung: Sect9Erfuellung::Leistungsbegrenzung60,
            ..gesund()
        };
        assert_eq!(
            derive_settlement_state(&facts),
            SettlementPeriodState::Active
        );
    }

    /// From 100 kW the 60 % route is gone (§9 Abs. 2 Nr. 1).
    #[test]
    fn the_sixty_percent_cap_does_not_carry_a_100kw_plant() {
        let facts = SettlementStateFacts {
            leistung_kwp: dec!(100),
            sect9_erfuellung: Sect9Erfuellung::Leistungsbegrenzung60,
            ..gesund()
        };
        assert_eq!(
            derive_settlement_state(&facts),
            SettlementPeriodState::Reduced
        );
    }

    #[test]
    fn derive_active_small_plant_on_the_cap() {
        let facts = SettlementStateFacts {
            leistung_kwp: dec!(5),
            sect9_erfuellung: Sect9Erfuellung::Leistungsbegrenzung60,
            ..gesund()
        };
        assert_eq!(
            derive_settlement_state(&facts),
            SettlementPeriodState::Active
        );
    }
}

// ── §9 EEG — Steuerbarkeit ────────────────────────────────────────────────────

/// How a plant satisfies the §9 EEG technical requirements.
///
/// §9 Abs. 2 is **staged by installed capacity**, not a single threshold, and the
/// middle band is a genuine choice: a 25–100 kW plant on the
/// 60-%-Leistungsbegrenzung route the statute offers it is compliant, so no
/// §52 Abs. 1 Nr. 1 Pflichtzahlung is owed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum Sect9Erfuellung {
    /// Nothing installed. A violation wherever §9 requires something.
    #[default]
    Keine,
    /// Technische Einrichtungen per §9 Abs. 1: the Netzbetreiber can read the
    /// Ist-Einspeisung and remotely reduce the Einspeiseleistung.
    Fernsteuerbarkeit,
    /// The 60 % Leistungsbegrenzung at the Netzverknüpfungspunkt — the
    /// alternative §9 Abs. 2 Nr. 2 grants plants below 100 kW.
    Leistungsbegrenzung60,
}

/// The §9 obligation a plant of this size and type carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum Sect9Pflicht {
    /// Steckersolargerät up to 2 kW — §9 Abs. 2 Satz 2 lifts Nr. 3 for it.
    ///
    /// Abs. 2 Satz 2 conditions the exemption on the installed capacity alone.
    /// The 800-VA Wechselrichter limit belongs to the separate Abs. 1 Satz 3
    /// exemption and to §24 Abs. 1 Satz 5 Nr. 2, not here.
    Keine,
    /// Below 25 kW: the 60 % Leistungsbegrenzung only.
    Leistungsbegrenzung60,
    /// 25 kW up to 100 kW: Fernsteuerbarkeit **or** the 60 % Leistungsbegrenzung.
    FernsteuerbarkeitOderBegrenzung,
    /// From 100 kW: full Fernsteuerbarkeit; the 60 % route is not available.
    Fernsteuerbarkeit,
}

/// Which §9 obligation applies to a plant.
///
/// | Installed capacity | Obligation | Basis |
/// |---|---|---|
/// | Steckersolargerät ≤ 2 kW | none | §9 Abs. 2 Satz 2 |
/// | < 25 kW | 60 % Leistungsbegrenzung | §9 Abs. 2 Nr. 3 |
/// | 25 kW – < 100 kW | Fernsteuerbarkeit **or** 60 % | §9 Abs. 2 Nr. 2 |
/// | ≥ 100 kW | Fernsteuerbarkeit | §9 Abs. 2 Nr. 1 |
#[must_use]
pub fn sect9_pflicht(leistung_kwp: Decimal, art: Option<ErzeugungsArt>) -> Sect9Pflicht {
    use rust_decimal::dec;
    // „bis zu 2 Kilowatt" — inclusive. A 2 kWp module string behind an 800-VA
    // inverter is the standard Steckersolar build, so the boundary is the case.
    if art == Some(ErzeugungsArt::SolarStecker) && leistung_kwp <= dec!(2) {
        return Sect9Pflicht::Keine;
    }
    if leistung_kwp >= dec!(100) {
        Sect9Pflicht::Fernsteuerbarkeit
    } else if leistung_kwp >= dec!(25) {
        Sect9Pflicht::FernsteuerbarkeitOderBegrenzung
    } else {
        Sect9Pflicht::Leistungsbegrenzung60
    }
}

/// Whether the plant is in breach of §9 Abs. 1/2 — the §52 Abs. 1 Nr. 1 trigger.
#[must_use]
pub fn sect9_verletzt(
    leistung_kwp: Decimal,
    art: Option<ErzeugungsArt>,
    erfuellung: Sect9Erfuellung,
) -> bool {
    match sect9_pflicht(leistung_kwp, art) {
        Sect9Pflicht::Keine => false,
        Sect9Pflicht::Fernsteuerbarkeit => erfuellung != Sect9Erfuellung::Fernsteuerbarkeit,
        Sect9Pflicht::FernsteuerbarkeitOderBegrenzung | Sect9Pflicht::Leistungsbegrenzung60 => {
            erfuellung == Sect9Erfuellung::Keine
        }
    }
}

#[cfg(test)]
mod sect9_tests {
    use super::*;
    use rust_decimal::dec;

    /// §9 Abs. 2 Nr. 2 states the middle band as a choice — „ab 25 Kilowatt und
    /// von weniger als 100 Kilowatt" is satisfied by either route.
    #[test]
    fn the_middle_band_may_take_either_route() {
        for e in [
            Sect9Erfuellung::Fernsteuerbarkeit,
            Sect9Erfuellung::Leistungsbegrenzung60,
        ] {
            assert!(!sect9_verletzt(dec!(50), None, e), "{e:?}");
        }
        assert!(sect9_verletzt(dec!(50), None, Sect9Erfuellung::Keine));
    }

    /// From 100 kW the 60 % route is no longer available.
    #[test]
    fn from_100_kw_only_fernsteuerbarkeit_satisfies_sect9() {
        assert!(sect9_verletzt(
            dec!(100),
            None,
            Sect9Erfuellung::Leistungsbegrenzung60
        ));
        assert!(!sect9_verletzt(
            dec!(100),
            None,
            Sect9Erfuellung::Fernsteuerbarkeit
        ));
    }

    /// Below 25 kW the 60 % Leistungsbegrenzung is the whole obligation
    /// (§9 Abs. 2 Nr. 3) — nothing else is owed, and having nothing is a breach.
    #[test]
    fn below_25_kw_the_sixty_percent_cap_is_enough() {
        assert!(!sect9_verletzt(
            dec!(10),
            None,
            Sect9Erfuellung::Leistungsbegrenzung60
        ));
        assert!(sect9_verletzt(dec!(10), None, Sect9Erfuellung::Keine));
    }

    /// §9 Abs. 2 Satz 2 — a Steckersolargerät „bis zu 2 Kilowatt" is out of scope.
    ///
    /// The boundary is the ordinary case, not an edge: 2 kWp of modules behind an
    /// 800-VA inverter is the standard build, and it is exempt.
    #[test]
    fn a_steckersolargeraet_is_exempt_up_to_and_including_two_kilowatt() {
        for kwp in [dec!(0.8), dec!(2)] {
            assert!(
                !sect9_verletzt(
                    kwp,
                    Some(ErzeugungsArt::SolarStecker),
                    Sect9Erfuellung::Keine
                ),
                "{kwp} kWp"
            );
            assert_eq!(
                sect9_pflicht(kwp, Some(ErzeugungsArt::SolarStecker)),
                Sect9Pflicht::Keine
            );
        }
        // Above 2 kW the exemption stops and Nr. 3 applies again.
        assert!(sect9_verletzt(
            dec!(2.01),
            Some(ErzeugungsArt::SolarStecker),
            Sect9Erfuellung::Keine
        ));
        assert_eq!(
            sect9_pflicht(dec!(2.01), Some(ErzeugungsArt::SolarStecker)),
            Sect9Pflicht::Leistungsbegrenzung60
        );
    }

    /// The exemption is for Steckersolargeräte only — a 2 kWp roof array is not one.
    #[test]
    fn the_exemption_does_not_reach_an_ordinary_small_plant() {
        assert_eq!(
            sect9_pflicht(dec!(2), None),
            Sect9Pflicht::Leistungsbegrenzung60
        );
    }
}

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
    FernsteuerbarkeitmInstalled,
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

/// Derive the expected [`SettlementPeriodState`] from plant compliance facts.
///
/// This is a **deterministic helper** — it does not access the DB.
/// The actual state stored in `einsd` may lag behind by one billing period
/// (state is updated after each month's settlement run).
///
/// ## Parameters
///
/// - `mastr_registriert`: whether the plant has confirmed MaStR registration
/// - `fernsteuerbarkeit_datum`: when §9 Fernsteuerbarkeit was installed (None = not installed)
/// - `leistung_kwp`: installed capacity
/// - `foerderendedatum`: subsidy end date (None = not expired)
/// - `billing_date`: first day of the billing period to evaluate
/// - `eeg_gesetz_year`: EEG law year (0 = KWKG, 2000/2004/…/2023 = EEG version)
///
/// # Example
///
/// ```rust
/// use eeg_billing::settlement_state::{derive_settlement_state, SettlementPeriodState};
/// use rust_decimal::dec;
/// use time::macros::date;
///
/// // Healthy plant — active (50 kW, Fernsteuerbarkeit installed 2024)
/// let state = derive_settlement_state(true, Some(date!(2024-01-01)), dec!(50), Some(date!(2040-12-31)), date!(2026-07-01), 2023);
/// assert_eq!(state, SettlementPeriodState::Active);
///
/// // MaStR not registered, EEG 2023 → Reduced (penalty, not suspension)
/// let state2 = derive_settlement_state(false, None, dec!(50), Some(date!(2040-12-31)), date!(2026-07-01), 2023);
/// assert_eq!(state2, SettlementPeriodState::Reduced);
///
/// // MaStR not registered, EEG 2017 → Suspended (VergütungAufNull)
/// let state3 = derive_settlement_state(false, None, dec!(50), Some(date!(2040-12-31)), date!(2026-07-01), 2017);
/// assert_eq!(state3, SettlementPeriodState::Suspended);
///
/// // Förderdauer expired → PostEeg
/// let state4 = derive_settlement_state(true, None, dec!(50), Some(date!(2020-12-31)), date!(2026-07-01), 2023);
/// assert_eq!(state4, SettlementPeriodState::PostEeg);
/// ```
#[must_use]
pub fn derive_settlement_state(
    mastr_registriert: bool,
    fernsteuerbarkeit_datum: Option<Date>,
    leistung_kwp: rust_decimal::Decimal,
    foerderendedatum: Option<Date>,
    billing_date: Date,
    eeg_gesetz_year: i16,
) -> SettlementPeriodState {
    use rust_decimal::dec;

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

    // ── Fernsteuerbarkeit not installed (§9 EEG) ──────────────────────────────
    let fernsteuerbarkeit_required = leistung_kwp >= dec!(25);
    if fernsteuerbarkeit_required && fernsteuerbarkeit_datum.is_none() {
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

    #[test]
    fn derive_active_healthy_plant() {
        let state = derive_settlement_state(
            true,
            Some(date!(2024 - 01 - 01)),
            dec!(50),
            Some(date!(2040 - 12 - 31)),
            date!(2026 - 07 - 01),
            2023,
        );
        assert_eq!(state, SettlementPeriodState::Active);
    }

    #[test]
    fn derive_post_eeg_expired() {
        let state = derive_settlement_state(
            true,
            None,
            dec!(50),
            Some(date!(2020 - 12 - 31)),
            date!(2026 - 07 - 01),
            2023,
        );
        assert_eq!(state, SettlementPeriodState::PostEeg);
    }

    #[test]
    fn derive_reduced_eeg2023_mastr_missing() {
        let state = derive_settlement_state(
            false,
            None,
            dec!(50),
            Some(date!(2040 - 12 - 31)),
            date!(2026 - 07 - 01),
            2023,
        );
        assert_eq!(state, SettlementPeriodState::Reduced);
    }

    #[test]
    fn derive_suspended_eeg2017_mastr_missing() {
        let state = derive_settlement_state(
            false,
            None,
            dec!(50),
            Some(date!(2040 - 12 - 31)),
            date!(2026 - 07 - 01),
            2017,
        );
        assert_eq!(state, SettlementPeriodState::Suspended);
    }

    #[test]
    fn derive_reduced_fernsteuerbarkeit_missing_eeg2023() {
        // 50 kW plant (≥25 kW requires Fernsteuerbarkeit)
        let state = derive_settlement_state(
            true,
            None,
            dec!(50), // fernsteuerbarkeit_datum = None
            Some(date!(2040 - 12 - 31)),
            date!(2026 - 07 - 01),
            2023,
        );
        assert_eq!(state, SettlementPeriodState::Reduced);
    }

    #[test]
    fn derive_active_small_plant_no_fernsteuerbarkeit_needed() {
        // 5 kW plant < 25 kW → Fernsteuerbarkeit not required
        let state = derive_settlement_state(
            true,
            None,
            dec!(5),
            Some(date!(2040 - 12 - 31)),
            date!(2026 - 07 - 01),
            2023,
        );
        assert_eq!(state, SettlementPeriodState::Active);
    }
}

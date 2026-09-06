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
use rust_decimal::dec;
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
/// A struct rather than a row of positional arguments, so a call site says which
/// fact it is passing.
#[derive(Debug, Clone, Copy)]
pub struct SettlementStateFacts {
    /// Whether the plant has a confirmed MaStR registration.
    pub mastr_registriert: bool,
    /// The plant facts § 9 Abs. 2 stages its obligation by — capacity,
    /// technology, Vergütungsform.
    pub sect9_anlage: Sect9Anlage,
    /// Which of the two § 9 Abs. 2 routes the plant actually carries.
    pub sect9_erfuellung: Sect9Erfuellung,
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
///     derive_settlement_state, Sect9Anlage, Sect9Erfuellung, SettlementPeriodState,
///     SettlementStateFacts,
/// };
/// use rust_decimal::dec;
/// use time::macros::date;
///
/// let facts = SettlementStateFacts {
///     mastr_registriert: true,
///     sect9_anlage: Sect9Anlage {
///         leistung_kwp: dec!(50),
///         erzeugungsart: None,
///         einspeiseverguetung_oder_mieterstrom: true,
///         ist_kwk_anlage: false,
///         wechselrichterleistung_va: None,
///     },
///     sect9_erfuellung: Sect9Erfuellung::BEIDES,
///     foerderendedatum: Some(date!(2040-12-31)),
///     billing_date: date!(2026-07-01),
///     eeg_gesetz_year: 2023,
/// };
/// assert_eq!(derive_settlement_state(&facts), SettlementPeriodState::Active);
///
/// // § 9 Abs. 2 Satz 1 Nr. 2 asks a geförderte Anlage of this size for lit. a
/// // **and** lit. b, so the 60 % cap alone leaves the Fernsteuerbarkeit open.
/// let cap = SettlementStateFacts {
///     sect9_erfuellung: Sect9Erfuellung::BEGRENZUNG_60,
///     ..facts
/// };
/// assert_eq!(derive_settlement_state(&cap), SettlementPeriodState::Reduced);
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
        sect9_anlage,
        sect9_erfuellung,
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
    // The obligation is staged by capacity and gated on the Vergütungsform;
    // `sect9_pflicht` carries the reading.
    if sect9_verletzt(sect9_anlage, sect9_erfuellung) {
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

    /// A healthy geförderte 50-kW plant, and the facts that move it off `Active`.
    fn gesund() -> SettlementStateFacts {
        SettlementStateFacts {
            mastr_registriert: true,
            sect9_anlage: Sect9Anlage {
                leistung_kwp: dec!(50),
                erzeugungsart: None,
                einspeiseverguetung_oder_mieterstrom: true,
                ist_kwk_anlage: false,
                wechselrichterleistung_va: None,
            },
            sect9_erfuellung: Sect9Erfuellung::BEIDES,
            foerderendedatum: Some(date!(2040 - 12 - 31)),
            billing_date: date!(2026 - 07 - 01),
            eeg_gesetz_year: 2023,
        }
    }

    fn mit_leistung(kwp: rust_decimal::Decimal) -> Sect9Anlage {
        Sect9Anlage {
            leistung_kwp: kwp,
            ..gesund().sect9_anlage
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
            sect9_erfuellung: Sect9Erfuellung::KEINE,
            ..gesund()
        };
        assert_eq!(
            derive_settlement_state(&facts),
            SettlementPeriodState::Reduced
        );
    }

    /// § 9 Abs. 2 Satz 1 Nr. 2 binds lit. a and lit. b together, so a geförderte
    /// Anlage in the 25-bis-100-kW band that carries only the 60 % cap is in
    /// breach — the § 52 Abs. 1 Nr. 1 Zahlung is 10 €/kW/Kalendermonat.
    #[test]
    fn the_sixty_percent_cap_alone_does_not_carry_a_50kw_plant() {
        let facts = SettlementStateFacts {
            sect9_erfuellung: Sect9Erfuellung::BEGRENZUNG_60,
            ..gesund()
        };
        assert_eq!(
            derive_settlement_state(&facts),
            SettlementPeriodState::Reduced
        );
    }

    /// The same plant in der Direktvermarktung owes lit. a alone, so the
    /// Fernsteuerbarkeit on its own keeps it `Active`.
    #[test]
    fn a_directly_marketed_50kw_plant_needs_only_the_remote_control() {
        let facts = SettlementStateFacts {
            sect9_anlage: Sect9Anlage {
                einspeiseverguetung_oder_mieterstrom: false,
                ..gesund().sect9_anlage
            },
            sect9_erfuellung: Sect9Erfuellung::FERNSTEUERBARKEIT,
            ..gesund()
        };
        assert_eq!(
            derive_settlement_state(&facts),
            SettlementPeriodState::Active
        );
    }

    /// From 100 kW the 60 % route is gone (§ 9 Abs. 2 Satz 1 Nr. 1).
    #[test]
    fn the_sixty_percent_cap_does_not_carry_a_100kw_plant() {
        let facts = SettlementStateFacts {
            sect9_anlage: mit_leistung(dec!(100)),
            sect9_erfuellung: Sect9Erfuellung::BEGRENZUNG_60,
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
            sect9_anlage: mit_leistung(dec!(5)),
            sect9_erfuellung: Sect9Erfuellung::BEGRENZUNG_60,
            ..gesund()
        };
        assert_eq!(
            derive_settlement_state(&facts),
            SettlementPeriodState::Active
        );
    }
}

// ── §9 EEG — Steuerbarkeit ────────────────────────────────────────────────────

/// Which of the two § 9 Abs. 2 technical routes a plant has in place.
///
/// The two are **not** alternatives. § 9 Abs. 2 Satz 1 Nr. 2 joins them with
/// „und" — lit. a the ferngesteuerte Reduzierung, lit. b the 60-%-Begrenzung —
/// and only Nr. 3 stands alone. A plant can therefore carry both, one or
/// neither, which is a pair of facts and not a choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Sect9Erfuellung {
    /// Technische Einrichtung nach § 9 Abs. 2 Satz 1 Nr. 1 bzw. Nr. 2 lit. a:
    /// the Netzbetreiber can reduce the Einspeiseleistung remotely — and, in the
    /// Nr. 1 band, read the Ist-Einspeisung as well.
    pub fernsteuerbarkeit: bool,
    /// Maximale Wirkleistungseinspeisung am Verknüpfungspunkt auf 60 % der
    /// installierten Leistung begrenzt — § 9 Abs. 2 Satz 1 Nr. 2 lit. b bzw. Nr. 3.
    pub begrenzung_60: bool,
}

impl Sect9Erfuellung {
    /// Nothing installed. A breach wherever § 9 Abs. 2 requires anything.
    pub const KEINE: Self = Self {
        fernsteuerbarkeit: false,
        begrenzung_60: false,
    };
    /// The ferngesteuerte Reduzierung alone.
    pub const FERNSTEUERBARKEIT: Self = Self {
        fernsteuerbarkeit: true,
        begrenzung_60: false,
    };
    /// The 60 % cap alone.
    pub const BEGRENZUNG_60: Self = Self {
        fernsteuerbarkeit: false,
        begrenzung_60: true,
    };
    /// Both routes — what a geförderte Anlage in the 25-bis-100-kW band owes.
    pub const BEIDES: Self = Self {
        fernsteuerbarkeit: true,
        begrenzung_60: true,
    };
}

/// The plant facts § 9 Abs. 2 stages its obligation by.
///
/// Capacity alone does not decide it: lit. b of Nr. 2 and the whole of Nr. 3
/// bind only „Anlagen, die der Einspeisevergütung oder dem Mieterstromzuschlag
/// nach § 19 Absatz 1 Nummer 2 oder Nummer 3 zugeordnet sind" — and Nr. 3
/// additionally every KWK-Anlage of that size. A directly-marketed Solaranlage
/// below 25 kW owes nothing at all under § 9 Abs. 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sect9Anlage {
    /// Installed capacity in kW — the Nr. 1 / Nr. 2 / Nr. 3 staging.
    pub leistung_kwp: Decimal,
    /// Technology, for the Steckersolar carve-out.
    pub erzeugungsart: Option<ErzeugungsArt>,
    /// The plant is settled under § 19 Abs. 1 Nr. 2 (Einspeisevergütung, in any
    /// of its Varianten) or Nr. 3 (Mieterstromzuschlag) — the gate on the 60 %
    /// cap in Nr. 2 lit. b and Nr. 3.
    pub einspeiseverguetung_oder_mieterstrom: bool,
    /// A KWK-Anlage. Nr. 3 names it beside the geförderten Anlagen, so a
    /// KWK-Anlage below 25 kW carries the cap whatever it is paid under.
    pub ist_kwk_anlage: bool,
    /// Wechselrichterleistung in Voltampere. The Steckersolar carve-out is a
    /// two-part test — „bis zu 2 Kilowatt **und** … bis zu 800 Voltampere" — so
    /// `None` leaves it closed rather than assuming the standard build.
    pub wechselrichterleistung_va: Option<Decimal>,
}

/// What § 9 Abs. 2 Satz 1 requires of this plant.
///
/// The same shape as [`Sect9Erfuellung`], so a breach is the pointwise
/// comparison of the two and no band has to be re-derived to report one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Sect9Pflicht {
    /// Nr. 1 (ab 100 kW) resp. Nr. 2 lit. a (ab 25 kW) — the ferngesteuerte
    /// Reduzierung.
    pub fernsteuerbarkeit: bool,
    /// Nr. 2 lit. b resp. Nr. 3 — the 60 % Wirkleistungsbegrenzung.
    pub begrenzung_60: bool,
}

impl Sect9Pflicht {
    /// Whether § 9 Abs. 2 asks anything of this plant at all.
    #[must_use]
    pub const fn ist_leer(self) -> bool {
        !self.fernsteuerbarkeit && !self.begrenzung_60
    }
}

/// The installed capacity up to which a Steckersolargerät escapes Nr. 3.
pub const STECKERSOLAR_LEISTUNG_KW: Decimal = dec!(2);

/// The Wechselrichterleistung up to which a Steckersolargerät escapes Nr. 3.
pub const STECKERSOLAR_WECHSELRICHTER_VA: Decimal = dec!(800);

/// Which § 9 Abs. 2 Satz 1 obligations apply to a plant.
///
/// | Installed capacity | Fernsteuerbarkeit | 60 % cap | Basis |
/// |---|---|---|---|
/// | Steckersolargerät ≤ 2 kW und ≤ 800 VA | – | – | Abs. 2 Satz 4 |
/// | < 25 kW | – | nur geförderte Anlagen und KWK-Anlagen | Nr. 3 |
/// | 25 kW bis < 100 kW | ja | nur geförderte Anlagen | Nr. 2 lit. a + b |
/// | ab 100 kW | ja | – | Nr. 1 |
///
/// „ab 25 Kilowatt" and „mindestens 100 Kilowatt" are inclusive, „weniger als"
/// exclusive, so both boundaries fall into the higher band.
///
/// # Example
///
/// ```rust
/// use eeg_billing::settlement_state::{Sect9Anlage, Sect9Pflicht, sect9_pflicht};
/// use rust_decimal::dec;
///
/// let gefoerdert = |kwp| Sect9Anlage {
///     leistung_kwp: kwp,
///     erzeugungsart: None,
///     einspeiseverguetung_oder_mieterstrom: true,
///     ist_kwk_anlage: false,
///     wechselrichterleistung_va: None,
/// };
///
/// // 50 kW in der Einspeisevergütung: Nr. 2 lit. a **und** lit. b.
/// assert_eq!(
///     sect9_pflicht(gefoerdert(dec!(50))),
///     Sect9Pflicht { fernsteuerbarkeit: true, begrenzung_60: true },
/// );
///
/// // Dieselbe Anlage in der Direktvermarktung: lit. b entfällt.
/// let direkt = Sect9Anlage { einspeiseverguetung_oder_mieterstrom: false, ..gefoerdert(dec!(50)) };
/// assert_eq!(
///     sect9_pflicht(direkt),
///     Sect9Pflicht { fernsteuerbarkeit: true, begrenzung_60: false },
/// );
///
/// // 10 kW in der Direktvermarktung: § 9 Abs. 2 verlangt nichts.
/// let klein = Sect9Anlage { einspeiseverguetung_oder_mieterstrom: false, ..gefoerdert(dec!(10)) };
/// assert!(sect9_pflicht(klein).ist_leer());
/// ```
#[must_use]
pub fn sect9_pflicht(anlage: Sect9Anlage) -> Sect9Pflicht {
    let Sect9Anlage {
        leistung_kwp,
        erzeugungsart,
        einspeiseverguetung_oder_mieterstrom,
        ist_kwk_anlage,
        wechselrichterleistung_va,
    } = anlage;

    // Abs. 2 Satz 4 lifts **Satz 1 Nr. 3** — and only Nr. 3 — for a
    // Steckersolargerät „bis zu 2 Kilowatt und mit einer Wechselrichterleistung
    // von insgesamt bis zu 800 Voltampere". Both halves are conditions: an
    // unknown Wechselrichterleistung leaves the carve-out shut, because
    // claiming it would suppress a real § 52 Abs. 1 Nr. 1 charge.
    let steckersolar_ausgenommen = erzeugungsart == Some(ErzeugungsArt::SolarStecker)
        && leistung_kwp <= STECKERSOLAR_LEISTUNG_KW
        && wechselrichterleistung_va.is_some_and(|va| va <= STECKERSOLAR_WECHSELRICHTER_VA);

    // The 60 % cap is owed by geförderte Anlagen throughout, and below 25 kW by
    // every KWK-Anlage as well.
    let cap_adressat = einspeiseverguetung_oder_mieterstrom;

    if leistung_kwp >= dec!(100) {
        // Nr. 1 — „mindestens 100 Kilowatt": Ist-Einspeisung abrufen und
        // ferngesteuert reduzieren. No 60 % route, and no Vergütungsform gate.
        Sect9Pflicht {
            fernsteuerbarkeit: true,
            begrenzung_60: false,
        }
    } else if leistung_kwp >= dec!(25) {
        // Nr. 2 — „ab 25 Kilowatt und von weniger als 100 Kilowatt": lit. a for
        // every plant, lit. b additionally for a geförderte one.
        Sect9Pflicht {
            fernsteuerbarkeit: true,
            begrenzung_60: cap_adressat,
        }
    } else {
        // Nr. 3 — „weniger als 25 Kilowatt": the cap alone, and only for a
        // geförderte Anlage or a KWK-Anlage.
        Sect9Pflicht {
            fernsteuerbarkeit: false,
            begrenzung_60: (cap_adressat || ist_kwk_anlage) && !steckersolar_ausgenommen,
        }
    }
}

/// Whether the plant is in breach of § 9 Abs. 2 — the § 52 Abs. 1 Nr. 1 trigger.
///
/// A breach is any obligation [`sect9_pflicht`] states and
/// [`Sect9Erfuellung`] does not carry. Having *more* than is owed is never a
/// breach: a directly-marketed plant that limits itself to 60 % anyway is
/// compliant, and so is a small plant with a Fernsteuerbarkeit it did not have
/// to install.
///
/// # Example
///
/// ```rust
/// use eeg_billing::settlement_state::{Sect9Anlage, Sect9Erfuellung, sect9_verletzt};
/// use rust_decimal::dec;
///
/// let anlage = Sect9Anlage {
///     leistung_kwp: dec!(50),
///     erzeugungsart: None,
///     einspeiseverguetung_oder_mieterstrom: true,
///     ist_kwk_anlage: false,
///     wechselrichterleistung_va: None,
/// };
///
/// // Nr. 2 asks for both, so the 60 % cap on its own is a breach of lit. a.
/// assert!(sect9_verletzt(anlage, Sect9Erfuellung::BEGRENZUNG_60));
/// assert!(sect9_verletzt(anlage, Sect9Erfuellung::FERNSTEUERBARKEIT));
/// assert!(!sect9_verletzt(anlage, Sect9Erfuellung::BEIDES));
/// ```
#[must_use]
pub fn sect9_verletzt(anlage: Sect9Anlage, erfuellung: Sect9Erfuellung) -> bool {
    let pflicht = sect9_pflicht(anlage);
    (pflicht.fernsteuerbarkeit && !erfuellung.fernsteuerbarkeit)
        || (pflicht.begrenzung_60 && !erfuellung.begrenzung_60)
}

#[cfg(test)]
mod sect9_tests {
    use super::*;
    use rust_decimal::dec;

    fn anlage(kwp: Decimal) -> Sect9Anlage {
        Sect9Anlage {
            leistung_kwp: kwp,
            erzeugungsart: None,
            einspeiseverguetung_oder_mieterstrom: true,
            ist_kwk_anlage: false,
            wechselrichterleistung_va: None,
        }
    }

    /// § 9 Abs. 2 Satz 1 Nr. 2 is „a) … **und** b) …" — the trailing „oder"
    /// separates Nr. 2 from Nr. 3, it does not offer the two lit. as a choice.
    /// Reading it as an alternative lets a geförderte Anlage in the middle band
    /// escape lit. a, which is the whole point of the Nummer.
    #[test]
    fn the_middle_band_owes_both_routes() {
        let a = anlage(dec!(50));
        assert_eq!(
            sect9_pflicht(a),
            Sect9Pflicht {
                fernsteuerbarkeit: true,
                begrenzung_60: true
            }
        );
        assert!(sect9_verletzt(a, Sect9Erfuellung::FERNSTEUERBARKEIT));
        assert!(sect9_verletzt(a, Sect9Erfuellung::BEGRENZUNG_60));
        assert!(sect9_verletzt(a, Sect9Erfuellung::KEINE));
        assert!(!sect9_verletzt(a, Sect9Erfuellung::BEIDES));
    }

    /// lit. b binds „Anlagen, die der Einspeisevergütung oder dem
    /// Mieterstromzuschlag … zugeordnet sind" — a Marktprämien-Anlage owes
    /// lit. a alone.
    #[test]
    fn the_middle_band_in_direktvermarktung_owes_only_the_remote_control() {
        let a = Sect9Anlage {
            einspeiseverguetung_oder_mieterstrom: false,
            ..anlage(dec!(50))
        };
        assert_eq!(
            sect9_pflicht(a),
            Sect9Pflicht {
                fernsteuerbarkeit: true,
                begrenzung_60: false
            }
        );
        assert!(!sect9_verletzt(a, Sect9Erfuellung::FERNSTEUERBARKEIT));
        assert!(sect9_verletzt(a, Sect9Erfuellung::BEGRENZUNG_60));
    }

    /// Nr. 1 has no Vergütungsform gate and no 60 % route.
    #[test]
    fn from_100_kw_only_the_remote_control_satisfies_sect9() {
        for gefoerdert in [true, false] {
            let a = Sect9Anlage {
                einspeiseverguetung_oder_mieterstrom: gefoerdert,
                ..anlage(dec!(100))
            };
            assert_eq!(
                sect9_pflicht(a),
                Sect9Pflicht {
                    fernsteuerbarkeit: true,
                    begrenzung_60: false
                }
            );
            assert!(sect9_verletzt(a, Sect9Erfuellung::BEGRENZUNG_60));
            assert!(!sect9_verletzt(a, Sect9Erfuellung::FERNSTEUERBARKEIT));
        }
    }

    /// Nr. 3 names „Anlagen, die der Einspeisevergütung oder dem
    /// Mieterstromzuschlag … zugeordnet sind" and „KWK-Anlagen"; a
    /// direktvermarktende Solaranlage below 25 kW is in neither list.
    #[test]
    fn below_25_kw_the_cap_is_owed_only_by_the_addressees_of_nr_3() {
        assert_eq!(
            sect9_pflicht(anlage(dec!(10))),
            Sect9Pflicht {
                fernsteuerbarkeit: false,
                begrenzung_60: true
            }
        );
        assert!(sect9_verletzt(anlage(dec!(10)), Sect9Erfuellung::KEINE));

        let direkt = Sect9Anlage {
            einspeiseverguetung_oder_mieterstrom: false,
            ..anlage(dec!(10))
        };
        assert!(sect9_pflicht(direkt).ist_leer());
        assert!(!sect9_verletzt(direkt, Sect9Erfuellung::KEINE));

        let kwk = Sect9Anlage {
            ist_kwk_anlage: true,
            ..direkt
        };
        assert!(sect9_verletzt(kwk, Sect9Erfuellung::KEINE));
        assert!(!sect9_verletzt(kwk, Sect9Erfuellung::BEGRENZUNG_60));
    }

    /// Abs. 2 Satz 4 — „bis zu 2 Kilowatt **und** … bis zu 800 Voltampere". Both
    /// halves are conditions, and an unstated Wechselrichterleistung keeps the
    /// carve-out shut.
    #[test]
    fn the_steckersolar_carve_out_is_a_two_part_test() {
        let stecker = |kwp, va| Sect9Anlage {
            erzeugungsart: Some(ErzeugungsArt::SolarStecker),
            wechselrichterleistung_va: va,
            ..anlage(kwp)
        };
        for kwp in [dec!(0.8), dec!(2)] {
            assert!(
                sect9_pflicht(stecker(kwp, Some(dec!(800)))).ist_leer(),
                "{kwp} kWp"
            );
        }
        // Above either limit the exemption stops and Nr. 3 applies again.
        assert!(!sect9_pflicht(stecker(dec!(2.01), Some(dec!(800)))).ist_leer());
        assert!(!sect9_pflicht(stecker(dec!(2), Some(dec!(1200)))).ist_leer());
        // Unknown inverter rating: no carve-out.
        assert!(!sect9_pflicht(stecker(dec!(2), None)).ist_leer());
    }

    /// The carve-out is for Steckersolargeräte only — a 2 kWp roof array is not one.
    #[test]
    fn the_carve_out_does_not_reach_an_ordinary_small_plant() {
        assert!(!sect9_pflicht(anlage(dec!(2))).ist_leer());
    }

    /// More than is owed is never a breach.
    #[test]
    fn exceeding_the_obligation_is_compliant() {
        assert!(!sect9_verletzt(anlage(dec!(10)), Sect9Erfuellung::BEIDES));
        assert!(!sect9_verletzt(anlage(dec!(120)), Sect9Erfuellung::BEIDES));
    }
}

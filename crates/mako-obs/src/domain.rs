//! The read-model `obsd` projects `de.mako.*` events into, and the reports it
//! answers from.
//!
//! ## Two clocks, never one number
//!
//! Every MaKo process carries **two independent deadlines**, and conflating them
//! is the defect this module is shaped to prevent:
//!
//! | Clock | What it is | Where it comes from |
//! |---|---|---|
//! | **APERAK Frist** | the *technical* acknowledgement — 45 min Strom weekday, next Werktag 12:00 Gas Folgeprozess, 3 Werktage Gas Initialprozess | `mako_fristen`, reported to obsd as `de.mako.aperak.timeout` |
//! | **Antwortfrist** | the *business* answer — 11:00 of the 1. Werktag for a GPKE Anmeldung, 4 Werktage for a Gas Anmeldung, 3/5/7/1 WT for WiM Strom | `mako_fristen::antwort`, stored as [`ProcessProjection::deadline_at`] |
//!
//! They differ by orders of magnitude and they fail for different reasons: a
//! missed APERAK is a transport or validation fault, a missed Antwortfrist is a
//! business one. A KPI that reports the second under the first's name tells an
//! operator to look in the wrong place, and tells a regulator something untrue.
//! [`KpiReport`] therefore carries both, named for the clock each measures.

use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

/// Lifecycle state of a MaKo process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessState {
    /// `de.mako.process.initiated` received — process started.
    Initiated,
    /// `de.mako.aperak.accepted` received — APERAK acknowledged.
    Running,
    /// `de.mako.aperak.timeout` received — the **APERAK** Frist lapsed with no
    /// acknowledgement. Not the business answer window: see the module note.
    AperakTimeout,
    /// `de.mako.process.completed` received — happy path finished.
    Completed,
    /// `de.mako.aperak.rejected` received — ERC code rejection.
    Rejected,
    /// `de.mako.process.failed` received — unrecoverable failure.
    ///
    /// Not `Cancelled`: nothing in mako emits a cancellation, and that name puts
    /// unrecoverable failures in a bucket the STP rate reads as a normal ending.
    Failed,
}

impl ProcessState {
    /// Parse from the CE type string.
    #[must_use]
    pub fn from_ce_type(ce_type: &str) -> Option<Self> {
        match ce_type {
            mako_events::mako::PROCESS_INITIATED => Some(Self::Initiated),
            mako_events::mako::APERAK_ACCEPTED => Some(Self::Running),
            mako_events::mako::APERAK_REJECTED => Some(Self::Rejected),
            mako_events::mako::APERAK_TIMEOUT => Some(Self::AperakTimeout),
            mako_events::mako::PROCESS_COMPLETED => Some(Self::Completed),
            mako_events::mako::PROCESS_FAILED => Some(Self::Failed),
            _ => None,
        }
    }

    /// The wire spelling, and the SQL literal.
    ///
    /// One function so a query, a filter and a JSON body cannot spell a state
    /// three ways. Casing drift is silent: a filter on a literal nothing stores
    /// counts zero, and zero rejections reads as perfect parity.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Initiated => "initiated",
            Self::Running => "running",
            Self::AperakTimeout => "aperak_timeout",
            Self::Completed => "completed",
            Self::Rejected => "rejected",
            Self::Failed => "failed",
        }
    }

    /// Parse the wire spelling. `None` for anything else — a caller filtering on
    /// an unknown state must get no rows rather than every row.
    #[must_use]
    pub fn from_str_exact(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|st| st.as_str() == s)
    }

    /// Every state, for exhaustive guards.
    pub const ALL: [Self; 6] = [
        Self::Initiated,
        Self::Running,
        Self::AperakTimeout,
        Self::Completed,
        Self::Rejected,
        Self::Failed,
    ];

    /// `true` for terminal states that will receive no further events.
    ///
    /// `AperakTimeout` is deliberately **not** terminal: a counterparty that
    /// missed the acknowledgement window can still answer the business message,
    /// and the process completes normally afterwards.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Rejected | Self::Failed)
    }

    /// `true` when the process ended without doing what it set out to do.
    ///
    /// The STP denominator distinguishes this from [`Self::Completed`]; the
    /// numerator is completions alone.
    #[must_use]
    pub fn is_unsuccessful_ending(self) -> bool {
        matches!(self, Self::Rejected | Self::Failed)
    }
}

/// Deadline risk classification for a live process.
///
/// Computed from `deadline_at` relative to `now()`. A process with no published
/// Antwortfrist has no `deadline_at` and is [`DeadlineRisk::Unknown`] — never
/// green, because "we have not read that Festlegung" and "there is time" are
/// different statements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeadlineRisk {
    /// No Antwortfrist is published for this PID, so no risk can be stated.
    Unknown,
    /// More than [`AMBER_HOURS`] before the deadline.
    Green,
    /// Less than [`AMBER_HOURS`] before the deadline.
    Amber,
    /// The deadline has passed and the process is still open.
    Red,
}

/// How close to its Antwortfrist a process must be to read Amber.
///
/// A day of warning is the operating convention obsd's sweep window shares; it
/// is not a regulatory figure and nothing cites it as one.
pub const AMBER_HOURS: i64 = 24;

impl DeadlineRisk {
    /// Classify risk given the deadline and current UTC time.
    #[must_use]
    pub fn classify(deadline: OffsetDateTime, now: OffsetDateTime) -> Self {
        if now > deadline {
            Self::Red
        } else if (deadline - now).whole_hours() < AMBER_HOURS {
            Self::Amber
        } else {
            Self::Green
        }
    }

    /// Classify an optional deadline: absent means [`Self::Unknown`].
    #[must_use]
    pub fn classify_opt(deadline: Option<OffsetDateTime>, now: OffsetDateTime) -> Self {
        deadline.map_or(Self::Unknown, |d| Self::classify(d, now))
    }

    /// The wire spelling, and the SQL literal.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Green => "green",
            Self::Amber => "amber",
            Self::Red => "red",
        }
    }

    /// Parse the wire spelling, defaulting to [`Self::Unknown`].
    #[must_use]
    pub fn from_str_or_unknown(s: &str) -> Self {
        match s {
            "green" => Self::Green,
            "amber" => Self::Amber,
            "red" => Self::Red,
            _ => Self::Unknown,
        }
    }
}

/// Per-process read-model projection.
///
/// One row per live or recently completed process, updated on every
/// `de.mako.*` event `obsd` receives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessProjection {
    /// UUID — `subject` field of the originating CloudEvent.
    pub process_id: Uuid,
    /// BDEW Prüfidentifikator.
    pub pid: u32,
    /// Process family (e.g. `"gpke"`, `"geli-gas"`, `"wim"`).
    pub family: String,
    /// Workflow name from the `makoworkflow` CE extension
    /// (e.g. `"gpke-lf-anmeldung"`, `"wim-geraetewechsel"`).
    pub workflow_name: String,
    /// Current lifecycle state.
    pub state: ProcessState,
    /// 11-digit MaLo-ID, if present in the event payload.
    pub malo_id: Option<String>,
    /// MP-ID of the counterparty (NB/GNB/MSB).
    pub partner_mp_id: Option<String>,
    /// Canonical Marktrolle from `marktrole` (e.g. `"LF"`, `"NB"`).
    pub mdm_role: Option<String>,
    /// The **business Antwortfrist**, from `mako_fristen::antwort`.
    ///
    /// `None` when no Festlegung this codebase has read quantifies the window
    /// for this PID. That is *unknown*, not unbounded — such a process is
    /// deliberately absent from every breach sweep rather than measured against
    /// an instant nobody can cite.
    pub deadline_at: Option<OffsetDateTime>,
    /// Citation for `deadline_at`, carried so an alert and a BNetzA answer can
    /// both name the Festlegung rather than asserting a number.
    pub deadline_source: Option<String>,
    /// Risk classification at the time of the last update.
    pub deadline_risk: DeadlineRisk,
    /// UTC timestamp of the first event seen for this process.
    pub started_at: OffsetDateTime,
    /// UTC timestamp of the most recently received event.
    pub last_event_at: OffsetDateTime,
    /// BDEW ERC error code when `state == Rejected` (e.g. `"E01"`, `"Z29"`).
    pub erc_code: Option<String>,
    /// § 7a Abs. 5 EnWG parity flag: `true` when the initiating Lieferant is
    /// part of the same vertically integrated undertaking as this operator.
    ///
    /// Set on `de.mako.process.initiated` for Lieferbeginn PIDs by comparing
    /// `data.new_supplier` against `[identity] own_mp_ids`. It is the grouping
    /// key of the Gleichbehandlungsbericht evidence — see
    /// [`ParityGroup`].
    pub initiator_is_affiliate: bool,
    /// Operator tenant — MP-ID of the deploying market participant.
    pub tenant: String,
}

/// Query filters for process projections.
#[derive(Debug, Clone)]
pub struct ObsQuery {
    pub state: Option<ProcessState>,
    pub pid: Option<u32>,
    pub family: Option<String>,
    pub partner_mp_id: Option<String>,
    pub mdm_role: Option<String>,
    /// Include only processes started on or after this time.
    pub since: Option<OffsetDateTime>,
    /// Filter by operator tenant (MP-ID). `None` = no tenant filter.
    pub tenant: Option<String>,
    /// Maximum number of results.
    pub limit: u32,
}

impl Default for ObsQuery {
    fn default() -> Self {
        Self {
            state: None,
            pid: None,
            family: None,
            partner_mp_id: None,
            mdm_role: None,
            since: None,
            tenant: None,
            limit: 100,
        }
    }
}

/// Process KPIs for one PID in one calendar period.
///
/// The two clocks of the module note are two separate fields. Nothing here
/// carries a regulatory target: the BNetzA publishes no numeric threshold for
/// any of these figures, so a target belongs to the operator's own escalation
/// policy and is configured, never asserted here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KpiReport {
    pub pid: u32,
    pub period_from: Date,
    pub period_to: Date,
    /// Processes whose `started_at` falls in the period.
    pub total_initiated: u64,
    pub total_completed: u64,
    pub total_rejected: u64,
    pub total_failed: u64,
    /// Processes currently in [`ProcessState::AperakTimeout`] — the
    /// **technical** acknowledgement clock.
    pub total_aperak_timeout: u64,
    /// Processes that passed their **business Antwortfrist** without closing,
    /// or closed after it. Counted only where a Frist is published.
    pub total_frist_breached: u64,
    /// Processes in the period that carry a published Antwortfrist at all.
    ///
    /// The denominator of [`Self::frist_compliance_rate`]. Reported separately
    /// because a low count is the interesting number: it means most of the
    /// bucket is unmeasurable, not that most of it is compliant.
    pub total_with_frist: u64,
    /// `1 - total_frist_breached / total_with_frist`, or `None` when nothing in
    /// the bucket carries a published Frist.
    pub frist_compliance_rate: Option<f64>,
    /// Mean cycle time in hours (started → terminal), or `None` when nothing in
    /// the bucket has closed.
    pub avg_cycle_time_hours: Option<f64>,
    /// 95th-percentile cycle time in hours, or `None` when nothing has closed.
    pub p95_cycle_time_hours: Option<f64>,
}

// ── § 7a Abs. 5 EnWG parity ──────────────────────────────────────────────────

/// One side of a parity comparison: the affiliate group or the third-party one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParityGroup {
    pub total: i64,
    pub completed: i64,
    pub rejected: i64,
    /// Processes that breached their published Antwortfrist.
    pub frist_breached: i64,
}

impl ParityGroup {
    /// Completion rate in \[0, 1\], or `None` for an empty group.
    ///
    /// `None` rather than `0.0`: an empty group has no rate, and a zero here
    /// reads as "we completed none of them".
    #[must_use]
    pub fn completion_rate(&self) -> Option<f64> {
        (self.total > 0).then(|| self.completed as f64 / self.total as f64)
    }
}

/// The smallest group size at which a parity gap is worth stating.
///
/// Below it the comparison is noise: one affiliate process that happened to be
/// rejected reads as a 100-percentage-point gap. Not a regulatory figure.
pub const PARITY_MIN_SAMPLE: i64 = 10;

/// A parity comparison between affiliate- and third-party-initiated processes.
///
/// **Sign convention, stated once and used everywhere.** `gap_pp` is
/// `affiliate − third_party`, in percentage points. A **positive** gap means the
/// affiliate fared better, which is the discrimination concern § 6a / § 7a EnWG
/// exists to catch. Three surfaces in `obsd` previously computed this, two with
/// one sign and one with the other, so the CloudEvent and the MCP tool
/// disagreed about which side was favoured.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ParityComparison {
    pub affiliate: ParityGroup,
    pub third_party: ParityGroup,
    /// `affiliate_rate − third_party_rate`, in percentage points, rounded to
    /// one decimal. `None` when either group is below [`PARITY_MIN_SAMPLE`] or
    /// empty — an unstatable gap, not a zero one.
    pub gap_pp: Option<f64>,
}

impl ParityComparison {
    /// Compare two groups under the sign convention above.
    #[must_use]
    pub fn new(affiliate: ParityGroup, third_party: ParityGroup) -> Self {
        let gap_pp = (affiliate.total >= PARITY_MIN_SAMPLE
            && third_party.total >= PARITY_MIN_SAMPLE)
            .then(|| {
                let a = affiliate.completion_rate()?;
                let t = third_party.completion_rate()?;
                Some(((a - t) * 1000.0).round() / 10.0)
            })
            .flatten();
        Self {
            affiliate,
            third_party,
            gap_pp,
        }
    }

    /// Which side the gap favours, when there is one.
    #[must_use]
    pub fn favours(&self) -> Option<&'static str> {
        match self.gap_pp? {
            g if g > 0.0 => Some("affiliate"),
            g if g < 0.0 => Some("third_party"),
            _ => Some("neither"),
        }
    }

    /// Whether the gap exceeds an **operator-configured** escalation threshold.
    ///
    /// `None` when the gap is unstatable. The threshold is the operator's own
    /// number: no BNetzA publication sets one for this figure.
    #[must_use]
    pub fn exceeds(&self, threshold_pp: f64) -> Option<bool> {
        Some(self.gap_pp?.abs() >= threshold_pp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_wire_spellings_round_trip() {
        for s in ProcessState::ALL {
            assert_eq!(ProcessState::from_str_exact(s.as_str()), Some(s));
        }
        assert_eq!(ProcessState::from_str_exact("cancelled"), None);
        assert_eq!(ProcessState::from_str_exact(""), None);
    }

    /// A missed APERAK is not the end of a process.
    #[test]
    fn an_aperak_timeout_is_not_terminal() {
        assert!(!ProcessState::AperakTimeout.is_terminal());
        assert!(ProcessState::Failed.is_terminal());
    }

    /// No deadline is not the same as plenty of time.
    #[test]
    fn an_absent_deadline_is_unknown_risk_not_green() {
        let now = OffsetDateTime::UNIX_EPOCH;
        assert_eq!(
            DeadlineRisk::classify_opt(None, now),
            DeadlineRisk::Unknown,
            "a PID with no published Frist must not read as healthy"
        );
    }

    /// The sign convention, pinned. Two surfaces disagreed about it.
    #[test]
    fn a_favoured_affiliate_is_a_positive_gap() {
        let c = ParityComparison::new(
            ParityGroup {
                total: 50,
                completed: 50,
                ..ParityGroup::default()
            },
            ParityGroup {
                total: 50,
                completed: 40,
                ..ParityGroup::default()
            },
        );
        assert_eq!(c.gap_pp, Some(20.0));
        assert_eq!(c.favours(), Some("affiliate"));
        assert_eq!(c.exceeds(5.0), Some(true));
    }

    #[test]
    fn a_favoured_third_party_is_a_negative_gap() {
        let c = ParityComparison::new(
            ParityGroup {
                total: 100,
                completed: 70,
                ..ParityGroup::default()
            },
            ParityGroup {
                total: 100,
                completed: 90,
                ..ParityGroup::default()
            },
        );
        assert_eq!(c.gap_pp, Some(-20.0));
        assert_eq!(c.favours(), Some("third_party"));
    }

    /// A group too small to compare yields no gap — not a gap of zero, and not
    /// a gap of a hundred points off one process.
    #[test]
    fn a_group_below_the_minimum_sample_states_no_gap() {
        let tiny = ParityGroup {
            total: 1,
            completed: 0,
            ..ParityGroup::default()
        };
        let big = ParityGroup {
            total: 100,
            completed: 96,
            ..ParityGroup::default()
        };
        let c = ParityComparison::new(tiny, big);
        assert_eq!(c.gap_pp, None);
        assert_eq!(c.favours(), None);
        assert_eq!(c.exceeds(5.0), None);
    }

    /// An empty group has no completion rate.
    #[test]
    fn an_empty_group_has_no_rate() {
        assert_eq!(ParityGroup::default().completion_rate(), None);
    }
}

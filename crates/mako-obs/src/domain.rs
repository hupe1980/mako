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
    /// `de.mako.aperak.timeout` received — APERAK deadline missed.
    AperakTimeout,
    /// `de.mako.process.completed` received — happy path finished.
    Completed,
    /// `de.mako.aperak.rejected` received — ERC code rejection.
    Rejected,
    /// `de.mako.process.failed` received — unrecoverable failure.
    Cancelled,
}

impl ProcessState {
    /// Parse from the CE type string.
    #[must_use]
    pub fn from_ce_type(ce_type: &str) -> Option<Self> {
        match ce_type {
            "de.mako.process.initiated" => Some(Self::Initiated),
            "de.mako.aperak.accepted" => Some(Self::Running),
            "de.mako.aperak.rejected" => Some(Self::Rejected),
            "de.mako.aperak.timeout" => Some(Self::AperakTimeout),
            "de.mako.process.completed" => Some(Self::Completed),
            "de.mako.process.failed" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// `true` for terminal states that will receive no further events.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Rejected | Self::Cancelled)
    }
}

/// Deadline risk classification for a live process.
///
/// Computed from `deadline_at` relative to `now()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeadlineRisk {
    /// More than 24 h (wall-clock) before deadline.
    Green,
    /// Less than 24 h before deadline (including Amber-on-Saturday logic).
    Amber,
    /// Deadline has passed and process is still open.
    Red,
}

impl DeadlineRisk {
    /// Classify risk given the deadline and current UTC time.
    #[must_use]
    pub fn classify(deadline: OffsetDateTime, now: OffsetDateTime) -> Self {
        if now > deadline {
            Self::Red
        } else if (deadline - now).whole_hours() < 24 {
            Self::Amber
        } else {
            Self::Green
        }
    }
}

/// Per-process read-model projection.
///
/// One row per live or recently completed process.  Updated on every
/// `de.mako.*` event received by `obsd`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessProjection {
    /// UUID v4 — `subject` field of the originating CloudEvent.
    pub process_id: Uuid,
    /// BDEW Prüfidentifikator.
    pub pid: u32,
    /// Process family (e.g. `"gpke"`, `"geli-gas"`, `"wim"`).
    pub family: String,
    /// Workflow name from the `makoworkflow` CE extension
    /// (e.g. `"gpke-lf-anmeldung"`, `"wim-device-change"`).
    pub workflow_name: String,
    /// Current lifecycle state.
    pub state: ProcessState,
    /// 11-digit MaLo-ID, if present in the event payload.
    pub malo_id: Option<String>,
    /// GLN of the counterparty (NB/GNB/MSB).
    pub partner_mp_id: Option<String>,
    /// Canonical Marktrolle from `marktrole` (e.g. `"LF"`, `"NB"`).
    pub mdm_role: Option<String>,
    /// Regulatory deadline in UTC (derived from process family + started_at).
    pub deadline_at: Option<OffsetDateTime>,
    /// Risk classification at time of last update.
    pub deadline_risk: DeadlineRisk,
    /// UTC timestamp of the first `process.initiated` event.
    pub started_at: OffsetDateTime,
    /// UTC timestamp of the most recently received event.
    pub last_event_at: OffsetDateTime,
    /// BDEW ERC error code when `state == Rejected` (e.g. `"E01"`, `"Z29"`).
    pub erc_code: Option<String>,
    /// §20 EnWG parity flag: `true` when the initiating LF MP-ID equals the
    /// operator's own MP-ID (vertically integrated utility deployment).
    ///
    /// Set by `obsd` on `de.mako.process.initiated` for Lieferbeginn PIDs
    /// (55001, 55016, 44001) by comparing `data.new_supplier` to the
    /// `[identity].own_mp_id` config value.
    ///
    /// Used for BNetzA §20 EnWG Diskriminierungsfreiheitspflicht audit reports.
    pub initiator_is_affiliate: bool,
    /// Operator tenant — MP-ID (GLN) of the deploying market participant.
    /// Used for multi-tenant deployments and DB-level row isolation.
    pub tenant: String,
}

/// Query filters for process projections.
#[derive(Debug, Clone)]
pub struct ObsQuery {
    pub state: Option<ProcessState>,
    pub pid: Option<u32>,
    pub partner_mp_id: Option<String>,
    pub mdm_role: Option<String>,
    /// Include only processes started on or after this time.
    pub since: Option<OffsetDateTime>,
    /// Filter by operator tenant (MP-ID). `None` = no tenant filter.
    pub tenant: Option<String>,
    /// Maximum number of results (default 100).
    pub limit: u32,
}

impl Default for ObsQuery {
    fn default() -> Self {
        Self {
            state: None,
            pid: None,
            partner_mp_id: None,
            mdm_role: None,
            since: None,
            tenant: None,
            limit: 100,
        }
    }
}

/// Regulatory KPI report for one PID in one calendar period.
///
/// Suitable for BNetzA voluntary reporting and §4a MsbG monitoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KpiReport {
    pub pid: u32,
    pub period_from: Date,
    pub period_to: Date,
    pub total_initiated: u64,
    pub total_completed: u64,
    pub total_rejected: u64,
    pub total_aperak_timeout: u64,
    pub total_cancelled: u64,
    /// APERAK Frist compliance rate (0.0 – 1.0).
    /// = (total_initiated - total_aperak_timeout) / total_initiated
    pub aperak_compliance_rate: f64,
    /// Mean process cycle time in hours (initiated → completed/rejected).
    pub avg_cycle_time_hours: f64,
    /// 95th percentile process cycle time in hours.
    pub p95_cycle_time_hours: f64,
}

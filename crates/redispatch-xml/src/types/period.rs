use serde::{Deserialize, Serialize};

use crate::types::{AttrV, Decimal3, TimeInterval};

// ── Quarter-hour Reason ───────────────────────────────────────────────────────

/// A reason code + optional free-text explanation, attached to an
/// `ActivationDocument` interval (within a `Period`).
///
/// Up to 2 `Reason` elements may appear per `Interval` in an
/// `ActivationDocument`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reason {
    /// ENTSO-E reason code (e.g. `"A44"`, `"A95"`, `"Z05"`).
    #[serde(rename = "ReasonCode")]
    pub code: AttrV<String>,
    /// Optional free-text reason description (max 512 chars).
    #[serde(
        rename = "ReasonText",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub text: Option<String>,
}

// ── Quarter-hour Interval ─────────────────────────────────────────────────────

/// A single quarter-hour (or longer) power value within a `Period`.
///
/// `pos` is 1-indexed (1–100 for a 25-hour DST day). `qty` is the power or
/// percentage value for the interval, in the unit specified by the parent
/// time series (`MeasureUnit`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Interval {
    /// Quarter-hour position within the day (1–100).
    #[serde(rename = "Pos")]
    pub pos: AttrV<u8>,
    /// Power or percentage quantity for this interval.
    #[serde(rename = "Qty")]
    pub qty: AttrV<Decimal3>,
    /// Optional reason codes (0–2); present only in `ActivationDocument`.
    #[serde(rename = "Reason", default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<Reason>,
}

// ── Period (attr-v document family) ──────────────────────────────────────────

/// A quarter-hour delivery period used in `ActivationDocument`,
/// `PlannedResourceScheduleDocument`, `NetworkConstraintDocument`, and
/// `Kostenblatt`.
///
/// The `time_interval` covers one complete calendar day (UTC).
/// `resolution` is always `"PT15M"` (15-minute granularity), giving 92–100
/// intervals per day depending on DST transitions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Period {
    /// The UTC day this period covers (`yyyy-mm-ddThh:mmZ/yyyy-mm-ddThh:mmZ`).
    #[serde(rename = "TimeInterval")]
    pub time_interval: AttrV<TimeInterval>,
    /// Sampling resolution; always `"PT15M"`.
    #[serde(rename = "Resolution")]
    pub resolution: AttrV<String>,
    /// Quarter-hour intervals for this period (92–100 per standard day).
    #[serde(rename = "Interval", default)]
    pub intervals: Vec<Interval>,
}

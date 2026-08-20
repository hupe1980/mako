//! The typed Sperr-/Entsperrauftrag domain — enums instead of loose strings.
//!
//! Every value here is one the ORDERS or IFTSTA AHB defines, and carries the
//! EDIFACT code it maps to. Typed rather than `String` + a database `CHECK`, so
//! a bad value is a 400 at the boundary instead of an SQL error in the response
//! body.

use serde::{Deserialize, Serialize};

/// What the Netzbetreiber was asked to do — BGM 1001 on the ORDERS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum OrderType {
    /// `BGM+Z51` — Sperrung. Arrives as ORDERS **17115** (Sperrauftrag).
    Sperrung,
    /// `BGM+Z52` — Entsperrung. Arrives as ORDERS **17117** (Entsperrauftrag).
    Entsperrung,
}

impl OrderType {
    /// The ORDERS Prüfidentifikator this order type arrives on.
    #[must_use]
    pub const fn pid(self) -> i32 {
        match self {
            Self::Sperrung => 17115,
            Self::Entsperrung => 17117,
        }
    }

    /// Derive the order type from an inbound ORDERS Prüfidentifikator.
    ///
    /// 17116 (Anfrage Sperrung, NB→MSB) is deliberately not mapped: it is the
    /// NB asking the Messstellenbetreiber whether the meter is reachable, not an
    /// order for this queue to execute.
    #[must_use]
    pub const fn from_pid(pid: u32) -> Option<Self> {
        match pid {
            17115 => Some(Self::Sperrung),
            17117 => Some(Self::Entsperrung),
            _ => None,
        }
    }

    /// The IFTSTA `SG15 STS DE9015` qualifier the outcome is reported under:
    /// `Z37` Auftragsstatus Sperren or `Z38` Auftragsstatus Entsperren. Exactly
    /// one of the two appears per SG14 (AHB 2.1 conditions \[78\]/\[79\]).
    #[must_use]
    pub const fn iftsta_qualifier(self) -> &'static str {
        match self {
            Self::Sperrung => "Z37",
            Self::Entsperrung => "Z38",
        }
    }

    /// The `makod` ERP command that starts the order in the NB-role workflow.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sperrung => "sperrung",
            Self::Entsperrung => "entsperrung",
        }
    }
}

/// Where the order stands in this service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum OrderStatus {
    /// Waiting for the field team.
    Pending,
    /// Carried out — IFTSTA `DE4405 = Z14 erfolgreich`.
    Executed,
    /// Attempted and not carried out — IFTSTA `DE4405 = Z13 gescheitert`.
    Failed,
    /// Withdrawn before execution. No IFTSTA: nothing happened to report.
    Cancelled,
}

impl OrderStatus {
    /// The IFTSTA `SG15 STS DE4405` status code, where the outcome has one.
    ///
    /// A cancelled order has none — it never reached the field, so there is no
    /// Auftragsstatus to report. (`Z32 abgelehnt` is a *refusal of the order*
    /// and belongs on the ORDRSP 19117 the workflow sends before execution, not
    /// on an execution report.)
    #[must_use]
    pub const fn iftsta_code(self) -> Option<&'static str> {
        match self {
            Self::Executed => Some("Z14"),
            Self::Failed => Some("Z13"),
            Self::Pending | Self::Cancelled => None,
        }
    }

    /// Whether the order still needs an IFTSTA 21039 sent to the Lieferant.
    #[must_use]
    pub const fn needs_iftsta(self) -> bool {
        matches!(self, Self::Executed | Self::Failed)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Executed => "executed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl std::str::FromStr for OrderStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "executed" => Ok(Self::Executed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(format!(
                "unknown status {other:?} — expected pending, executed, failed or cancelled"
            )),
        }
    }
}

/// `IMD+7081` on the Entsperrauftrag — whether the reconnection may be carried
/// out outside normal working hours.
///
/// The ORDERS AHB makes this a **Muss** on 17117: § 41f Abs. 7 EnWG requires the
/// restoration to be *unverzüglich*, and out-of-hours execution is what the
/// Lieferant asks for (and pays for) when the customer paid on a Friday evening.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Arbeitszeit {
    /// `Z53` — innerhalb der Arbeitszeit.
    Innerhalb,
    /// `Z54` — auch außerhalb der Arbeitszeit.
    AuchAusserhalb,
}

impl Arbeitszeit {
    /// The `IMD 7081` code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Innerhalb => "Z53",
            Self::AuchAusserhalb => "Z54",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_and_order_type_round_trip() {
        for t in [OrderType::Sperrung, OrderType::Entsperrung] {
            assert_eq!(
                OrderType::from_pid(u32::try_from(t.pid()).unwrap()),
                Some(t)
            );
        }
    }

    #[test]
    fn anfrage_sperrung_is_not_an_executable_order() {
        // 17116 is the NB asking the MSB whether the meter is reachable. Mapping
        // it into this queue would put a question in front of a field team as
        // though it were a disconnection order.
        assert_eq!(OrderType::from_pid(17116), None);
        assert_eq!(OrderType::from_pid(0), None);
    }

    #[test]
    fn only_terminal_outcomes_carry_an_iftsta_status() {
        assert_eq!(OrderStatus::Executed.iftsta_code(), Some("Z14"));
        assert_eq!(OrderStatus::Failed.iftsta_code(), Some("Z13"));
        // A cancelled order was never executed, so there is no Auftragsstatus.
        assert_eq!(OrderStatus::Cancelled.iftsta_code(), None);
        assert_eq!(OrderStatus::Pending.iftsta_code(), None);
    }

    #[test]
    fn needs_iftsta_matches_the_status_codes() {
        for s in [
            OrderStatus::Pending,
            OrderStatus::Executed,
            OrderStatus::Failed,
            OrderStatus::Cancelled,
        ] {
            assert_eq!(
                s.needs_iftsta(),
                s.iftsta_code().is_some(),
                "{s:?}: an outcome with an IFTSTA status code is exactly one that \
                 has to be reported to the LF",
            );
        }
    }

    #[test]
    fn sperren_and_entsperren_report_under_different_qualifiers() {
        // AHB 2.1 [78]/[79]: STS+Z37 and STS+Z38 are mutually exclusive per SG14.
        assert_ne!(
            OrderType::Sperrung.iftsta_qualifier(),
            OrderType::Entsperrung.iftsta_qualifier()
        );
    }

    #[test]
    fn status_parses_from_the_wire_and_rejects_anything_else() {
        assert_eq!(
            "executed".parse::<OrderStatus>().unwrap(),
            OrderStatus::Executed
        );
        // A query string is caller-controlled; an unknown value must be a 400,
        // not a filter that silently matches nothing.
        assert!("EXECUTED".parse::<OrderStatus>().is_err());
        assert!("done".parse::<OrderStatus>().is_err());
    }

    #[test]
    fn arbeitszeit_codes_are_the_imd_7081_values() {
        assert_eq!(Arbeitszeit::Innerhalb.code(), "Z53");
        assert_eq!(Arbeitszeit::AuchAusserhalb.code(), "Z54");
    }
}

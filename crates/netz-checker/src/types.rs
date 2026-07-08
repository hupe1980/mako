//! Core types for `netz-checker`.
//!
//! All types are `Clone + Debug + Serialize + Deserialize` so that callers can
//! log inputs/outputs and store audit records without extra conversions.

use serde::{Deserialize, Serialize};
use time::Date;
use uuid::Uuid;

use mako_markt::domain::Sparte;

// ── AnmeldungAnfrage ──────────────────────────────────────────────────────────

/// Classification of metering point.
///
/// Used to apply the correct Mindestvorlauffrist rule:
/// - `Slp`: SLP (Standardlastprofil) — 15:00 CET/CEST cutoff applies.
/// - `Rlm`: RLM (Registrierende Lastgangmessung) — 2 Werktage minimum lead.
/// - `Imsys`: intelligentes Messsystem (iMSys) — treated as SLP for Vorlauffrist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Messtyp {
    /// Standardlastprofil metering.
    Slp,
    /// Registrierende Lastgangmessung (interval metering).
    Rlm,
    /// Intelligentes Messsystem.
    Imsys,
}

impl std::fmt::Display for Messtyp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Slp => write!(f, "SLP"),
            Self::Rlm => write!(f, "RLM"),
            Self::Imsys => write!(f, "IMSYS"),
        }
    }
}

/// Parsed fields from a `de.mako.process.initiated` event for a Lieferbeginn PID.
///
/// All fields that `netz-checker` needs are extracted at the transport boundary
/// by `processd` before calling `evaluate`.  No raw CloudEvent JSON arrives here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnmeldungAnfrage {
    /// BDEW Prüfidentifikator:
    /// - `55001` GPKE Lieferbeginn Standard (Strom)
    /// - `55016` GPKE Lieferbeginn Netzentnahme (Strom)
    /// - `44001` GeLi Gas Lieferbeginn (Gas)
    pub pid: u32,
    /// mako process UUID (from `subject` CE field).
    pub process_id: Uuid,
    /// 11-digit Marktlokations-ID (Strom) or Gas-MaLo-ID.
    pub malo_id: String,
    /// GLN of the requesting new Lieferant.
    pub new_supplier_gln: String,
    /// GLN of the grid operator to whom the request is directed.
    ///
    /// Must equal the operator's own GLN; otherwise the event is misdirected.
    pub grid_operator_gln: String,
    /// Bilanzierungsgebiet-EIC provided in the UTILMD message (`LOC+237`).
    ///
    /// `None` when not present in the EDIFACT message (optional in some process variants).
    pub bilanzierungsgebiet: Option<String>,
    /// Requested Lieferbeginn date.
    pub process_date: Date,
    /// Energy commodity (Strom / Gas). Derived from PID.
    pub sparte: Sparte,
    /// Metering classification (SLP / RLM / iMSys).
    ///
    /// For Gas processes this is always `Slp` (GeLi Gas operates on gas MaLos
    /// which are billed as SLP equivalents unless explicitly flagged as RLM Gas).
    pub messtyp: Messtyp,
}

// ── MaloGridRecord ────────────────────────────────────────────────────────────

/// NB grid topology record for a MaLo.
///
/// Written by the NB's NIS/GIS adapter or provisioned manually via
/// `PUT /api/v1/malo/{id}/grid` on `marktd`. Read by `processd` NB module.
///
/// NOTE: This is NOT MaStR data. MaStR covers generation/consumption units,
/// not NB grid topology or Bilanzierungsgebiet assignments.
///
/// Absence of this record triggers `NetzCheckResult::Escalate` (rule 1) — the
/// NB cannot auto-decide without grid topology.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaloGridRecord {
    /// 11-digit Marktlokations-ID (Strom) or Gas-MaLo-ID.
    pub malo_id: String,
    /// GLN of the Netzbetreiber that owns this MaLo.
    pub nb_mp_id: String,
    /// Bilanzierungsgebiet-EIC (`LOC+237` in UTILMD).
    ///
    /// `None` means the Bilanzierungsgebiet is unknown — check 4 is skipped
    /// (treated as passing) when both this field and the UTILMD value are `None`.
    pub bilanzierungsgebiet: Option<String>,
    /// Netzgebiet code (optional; NB-specific identifier).
    pub netzgebiet: Option<String>,
}

// ── NetzCheckResult ───────────────────────────────────────────────────────────

/// Outcome of the NB Anmeldung validation pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NetzCheckResult {
    /// All checks passed.  If `auto_accept = true`, dispatch `bestaetigen`.
    Accept,
    /// A deterministic, verifiable rule failed.
    ///
    /// Dispatch `ablehnen` with `reason.erc_code` as the EDIFACT ERC.
    Reject(RejectReason),
    /// Validation could not complete — data is missing or ambiguous.
    ///
    /// Do NOT auto-decide.  Write `anmeldung_decisions` with
    /// `decision = "Escalate"` and alert the operator.
    Escalate {
        /// Human-readable explanation for the operator alert.
        reason: String,
    },
}

/// A structured rejection with the standard BDEW ERC code and a human-readable
/// explanation for the BNetzA audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectReason {
    /// BDEW ERC code (e.g. `"A02"`, `"A05"`, `"A06"`, `"A97"`, `"A99"`).
    ///
    /// Source: GPKE AHB / GeLi Gas AHB ERC decision trees.
    pub erc_code: String,
    /// Human-readable explanation for the operator and BNetzA audit log.
    pub detail: String,
    /// Which check number failed (1–6).
    pub check_number: u8,
}

impl NetzCheckResult {
    /// Returns the ERC code if this is a `Reject` result.
    #[must_use]
    pub fn erc_code(&self) -> Option<&str> {
        match self {
            Self::Reject(r) => Some(&r.erc_code),
            _ => None,
        }
    }

    /// Returns `true` if the decision is `Accept`.
    #[must_use]
    pub fn is_accept(&self) -> bool {
        matches!(self, Self::Accept)
    }

    /// Returns `true` if the decision is `Reject`.
    #[must_use]
    pub fn is_reject(&self) -> bool {
        matches!(self, Self::Reject(_))
    }

    /// Returns `true` if the decision requires operator escalation.
    #[must_use]
    pub fn is_escalate(&self) -> bool {
        matches!(self, Self::Escalate { .. })
    }
}

// ── Conversion from mako-markt repository type ────────────────────────────────

impl From<mako_markt::repository::MaloGridRecord> for MaloGridRecord {
    fn from(r: mako_markt::repository::MaloGridRecord) -> Self {
        Self {
            malo_id: r.malo_id.to_string(),
            nb_mp_id: r.nb_mp_id,
            bilanzierungsgebiet: r.bilanzierungsgebiet,
            netzgebiet: r.netzgebiet,
        }
    }
}

impl From<&mako_markt::repository::MaloGridRecord> for MaloGridRecord {
    fn from(r: &mako_markt::repository::MaloGridRecord) -> Self {
        Self {
            malo_id: r.malo_id.to_string(),
            nb_mp_id: r.nb_mp_id.clone(),
            bilanzierungsgebiet: r.bilanzierungsgebiet.clone(),
            netzgebiet: r.netzgebiet.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_helpers() {
        assert!(NetzCheckResult::Accept.is_accept());
        assert!(!NetzCheckResult::Accept.is_reject());
        assert!(!NetzCheckResult::Accept.is_escalate());

        let reject = NetzCheckResult::Reject(RejectReason {
            erc_code: "A06".to_owned(),
            detail: "Conflicting supply".to_owned(),
            check_number: 2,
        });
        assert!(reject.is_reject());
        assert_eq!(reject.erc_code(), Some("A06"));

        let escalate = NetzCheckResult::Escalate {
            reason: "Grid record missing".to_owned(),
        };
        assert!(escalate.is_escalate());
        assert!(escalate.erc_code().is_none());
    }

    #[test]
    fn messtyp_display() {
        assert_eq!(Messtyp::Slp.to_string(), "SLP");
        assert_eq!(Messtyp::Rlm.to_string(), "RLM");
        assert_eq!(Messtyp::Imsys.to_string(), "IMSYS");
    }
}

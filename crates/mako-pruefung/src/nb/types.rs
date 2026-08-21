//! Inputs and outputs of the Netzbetreiber's `E_0622` / `E_0607` decisions.
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
/// - `Slp`: SLP (Standardlastprofil) — LFW24 day rule applies (spätester ÜT
///   ist der Tag vor dem letzten WT vor dem Zuordnungsbeginn).
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
/// All fields that `mako-pruefung` needs are extracted at the transport boundary
/// by `processd` before calling `evaluate`.  No raw CloudEvent JSON arrives here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnmeldungAnfrage {
    /// BDEW Prüfidentifikator:
    /// - `55001` Anmeldung verbrauchende Marktlokation (Strom)
    /// - `55077` Anmeldung erzeugende Marktlokation (Strom)
    /// - `44001` Anmeldung NN (Gas)
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
    /// SG4 STS Transaktionsgrund (DE9013) from the UTILMD, when transmitted —
    /// e.g. `E01` Ein-/Auszug (Umzug), `E03` Lieferantenwechsel, `E06`
    /// Ersatzbelieferung.
    ///
    /// Drives the date-plausibility rules (check 3): GPKE permits a
    /// retroactive Lieferbeginn for Ein-/Auszug within the statutory
    /// backdating window, but not for a regular Wechsel. `None` (legacy
    /// messages or extraction failure) is treated conservatively.
    pub transaktionsgrund: Option<String>,
    /// `true` when the Anmeldung is for an **Erzeugende Marktlokation**
    /// (EEG-/KWKG-Einspeise-MaLo) — signalled by the UTILMD SG4 STS
    /// Transaktionsgrundergänzung `9013=ZW3`. Switches Check 4 to the §10c EEG
    /// Monatserster date rule. Kept separate from [`transaktionsgrund`] (the main
    /// Anmeldegrund) because a message carries both codes as distinct STS.
    ///
    /// [`transaktionsgrund`]: Self::transaktionsgrund
    #[serde(default)]
    pub ist_erzeugende_marktlokation: bool,
}

// ── AbmeldungAnfrage ──────────────────────────────────────────────────────────

/// Parsed fields from a `de.mako.process.initiated` event for an **Abmeldung**
/// PID — Strom `55004`, Gas `44004`.
///
/// Separate from [`AnmeldungAnfrage`] because the two carry different facts: an
/// Anmeldung names the *incoming* supplier and a Bilanzierungsgebiet to check
/// against the grid record; an Abmeldung names the *outgoing* one and nothing
/// to reconcile topology with. Folding them into one struct would leave half
/// its fields meaningless in either direction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbmeldungAnfrage {
    /// BDEW Prüfidentifikator: `55004` (Strom) or `44004` (Gas).
    pub pid: u32,
    /// mako process UUID (from the CloudEvent `subject`).
    pub process_id: Uuid,
    /// 11-digit Marktlokations-ID (Strom) or Gas-MaLo-ID.
    pub malo_id: String,
    /// MP-ID of the supplier ending the assignment.
    pub lf_mp_id: String,
    /// GLN of the grid operator the Abmeldung is directed to.
    pub grid_operator_gln: String,
    /// Requested Zuordnungsende („Abmeldedatum").
    pub abmeldedatum: Date,
    /// Energy commodity, derived from the PID.
    pub sparte: Sparte,
    /// Metering classification — drives the Gas retroactivity rules.
    pub messtyp: Messtyp,
    /// SG4 STS Transaktionsgrund (DE9013) — `E01`/`E02` Auszug, `E03`
    /// Lieferantenwechsel. Drives the Gas date rules and the `A09`/`A10` split.
    pub transaktionsgrund: Option<String>,
    /// `true` for an Erzeugende (EEG-/KWKG-) Marktlokation (`9013=ZW3`), whose
    /// Zuordnungsende must be a Monatserster a month ahead.
    #[serde(default)]
    pub ist_erzeugende_marktlokation: bool,
}

// ── MaloGridRecord ────────────────────────────────────────────────────────────

/// NB grid topology record for a MaLo.
///
/// Written by the NB's NIS/GIS adapter or provisioned manually via
/// `PUT /api/v1/malos/{id}/grid` on `marktd`. Read by `processd` NB module.
///
/// NOTE: This is NOT MaStR data. MaStR covers generation/consumption units,
/// not NB grid topology or Bilanzierungsgebiet assignments.
///
/// Absence of this record triggers `NbEntscheidung::Escalate` (rule 1) — the
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

// ── NbEntscheidung ───────────────────────────────────────────────────────────

/// Outcome of an NB decision tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NbEntscheidung {
    /// All checks passed.  If `auto_accept = true`, dispatch `bestaetigen`.
    Accept,
    /// A deterministic, verifiable rule failed.
    ///
    /// Dispatch `ablehnen` with `reason.antwortcode` — it renders into
    /// `SG4 STS+E01++<code>:<ebd>` of the answering UTILMD.
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

/// A structured rejection: the BDEW **Antwortcode**, the EBD it comes from, and
/// a human-readable explanation for the BNetzA audit log.
///
/// The EBD is part of the value, not context the caller is expected to carry:
/// `A02` means three different things across `E_0607`, `E_0622` and the LF's
/// `E_0609`, and a combined deployment runs all three.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectReason {
    /// `SG4 STS+E01` DE 9013 — the Antwortcode (e.g. `"A02"`, `"A05"`, `"E17"`).
    ///
    /// Not an ERC code: `ERC` is the APERAK/CONTRL processability segment.
    pub antwortcode: String,
    /// `SG4 STS+E01` DE 1131 — the EBD that publishes the code (`"E_0622"`,
    /// `"E_0607"`), or `None` on the Gas Codelisten the MIG leaves unnamed.
    pub ebd: Option<String>,
    /// Human-readable explanation for the operator and BNetzA audit log.
    pub detail: String,
    /// Which check number failed (1–6).
    pub check_number: u8,
}

impl NbEntscheidung {
    /// Returns the Antwortcode if this is a `Reject` result.
    #[must_use]
    pub fn antwortcode(&self) -> Option<&str> {
        match self {
            Self::Reject(r) => Some(&r.antwortcode),
            _ => None,
        }
    }

    /// Returns the EBD the Antwortcode belongs to, if this is a `Reject`.
    #[must_use]
    pub fn ebd(&self) -> Option<&str> {
        match self {
            Self::Reject(r) => r.ebd.as_deref(),
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
        assert!(NbEntscheidung::Accept.is_accept());
        assert!(!NbEntscheidung::Accept.is_reject());
        assert!(!NbEntscheidung::Accept.is_escalate());

        let reject = NbEntscheidung::Reject(RejectReason {
            antwortcode: "A06".to_owned(),
            ebd: Some("E_0622".into()),
            detail: "Conflicting supply".to_owned(),
            check_number: 2,
        });
        assert!(reject.is_reject());
        assert_eq!(reject.antwortcode(), Some("A06"));

        let escalate = NbEntscheidung::Escalate {
            reason: "Grid record missing".to_owned(),
        };
        assert!(escalate.is_escalate());
        assert!(escalate.antwortcode().is_none());
    }

    #[test]
    fn messtyp_display() {
        assert_eq!(Messtyp::Slp.to_string(), "SLP");
        assert_eq!(Messtyp::Rlm.to_string(), "RLM");
        assert_eq!(Messtyp::Imsys.to_string(), "IMSYS");
    }
}

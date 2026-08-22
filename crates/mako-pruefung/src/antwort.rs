//! The wire form of a decided answer — shared by every prüfende Rolle.
//!
//! An answer is a code plus the tree that publishes it. Both are needed on the
//! wire (`SG4 STS+E01` DE 9013 and DE 1131) and both are needed in the audit
//! log, because a code alone does not identify a meaning: `A02` is published by
//! `E_0607`, `E_0622`, `E_0249` and `E_0250` with four unrelated meanings.
//!
//! Lives here rather than under a role module because the MSB, the NB and the
//! LF all answer, and a per-role copy of this shape is how the four fields
//! start to drift.

use serde::{Deserialize, Serialize};

use crate::codes::{AntwortCode, Cluster};

/// A resolved Antwortcode: the code, the tree that publishes it, and the BDEW's
/// own wording.
///
/// Built only from an [`AntwortCode`] in [`crate::codes`], so a code can never
/// travel under a tree that does not define it: `G_0011` answers with
/// `A16` / `E13` / `ZC5` where `E_0622` answers `A02` / `A05` / `A06`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AntwortDetail {
    /// The EBD id the code was **resolved against** — always present, and the
    /// key [`crate::codes::lookup`] takes.
    ///
    /// Distinct from [`Self::ebd`]: the Gas Codelisten are not named in `STS`
    /// DE 1131, so a Gas answer carries no EBD *on the wire* while still
    /// belonging to exactly one tree.
    pub tree: String,
    /// `SG4 STS+E01` DE 9013 — the Antwortcode.
    ///
    /// Not an ERC code: `ERC` is the APERAK/CONTRL processability segment.
    pub antwortcode: String,
    /// `SG4 STS+E01` DE 1131 — the EBD that publishes the code (`"E_0622"`,
    /// `"E_0607"`), or `None` on the Gas Codelisten, which the MIG does not
    /// require to be named.
    pub ebd: Option<String>,
    /// The BDEW's own wording for the code, for the operator queue and the
    /// § 20 EnWG audit log.
    pub bedeutung: String,
    /// `true` when the BDEW requires a written Erläuterung (`FTX+ACB`)
    /// alongside the code. Sending one of these bare is an incomplete answer.
    pub braucht_bemerkung: bool,
    /// The date the answer states, when the code asserts a Terminänderung.
    ///
    /// `Z01` („Zustimmung mit Terminänderung"), `Z12` (the nächstmöglicher
    /// Kündigungszeitpunkt) and `Z14` (the corrected Abmeldetermin) each mean
    /// „to a different date than you asked for", and each is incomplete
    /// without naming it — `Z12`'s Anmerkung says so explicitly. `None` on
    /// every code that changes no date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abweichender_termin: Option<time::Date>,
}

impl AntwortDetail {
    /// Resolve a published code into its wire form, naming the tree it came from.
    #[must_use]
    pub fn new(tree: &'static str, code: &'static AntwortCode) -> Self {
        Self {
            tree: tree.to_owned(),
            antwortcode: code.code.to_owned(),
            ebd: code.ebd.map(ToOwned::to_owned),
            bedeutung: code.bedeutung.to_owned(),
            braucht_bemerkung: code.braucht_bemerkung,
            abweichender_termin: None,
        }
    }

    /// The same answer, stating the date it moves the process to.
    #[must_use]
    pub fn mit_termin(mut self, termin: time::Date) -> Self {
        self.abweichender_termin = Some(termin);
        self
    }
}

/// A structured rejection: the resolved BDEW **Antwortcode**, the EBD
/// Prüfschritt that produced it, and a human-readable explanation for the
/// BNetzA audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectReason {
    /// The resolved Antwortcode.
    #[serde(flatten)]
    pub antwort: AntwortDetail,
    /// Human-readable explanation for the operator and the § 20 EnWG audit log.
    ///
    /// When [`AntwortDetail::braucht_bemerkung`] is set this is also what goes
    /// into `FTX+ACB` on the wire.
    pub detail: String,
    /// The **EBD Prüfschritt** number that produced the code — `15`, `270`,
    /// `806`. Not an ordinal of mako's own checks: an auditor holding the
    /// published tree can find the row this decision came from.
    pub pruefschritt: u16,
}

impl RejectReason {
    /// Build a rejection from a published Ablehnungscode.
    ///
    /// # Panics
    ///
    /// In debug builds, when `code` is a Zustimmung — that would put an
    /// agreement code on an Ablehnungs-PID.
    #[must_use]
    pub fn new(
        tree: &'static str,
        code: &'static AntwortCode,
        pruefschritt: u16,
        detail: impl Into<String>,
    ) -> Self {
        debug_assert_eq!(
            code.cluster,
            Cluster::Ablehnung,
            "{} is a Zustimmungscode and cannot carry a rejection",
            code.code
        );
        Self {
            antwort: AntwortDetail::new(tree, code),
            detail: detail.into(),
            pruefschritt,
        }
    }

    /// The same rejection, naming the date the answer points the sender at.
    #[must_use]
    pub fn mit_termin(mut self, termin: time::Date) -> Self {
        self.antwort.abweichender_termin = Some(termin);
        self
    }
}

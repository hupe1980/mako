//! The shape of a decided MaBiS answer.
//!
//! Three MaBiS clusters are refusals and they are **not interchangeable**: an
//! Abweisung is not forwarded, an Ablehnung der gesamten Liste carries no
//! positions, and a Korrekturliste is itself a list. The `zustimmung: bool` of
//! [`crate::lf::LfAntwort`] loses all three, so the answer carries its
//! [`Cluster`].

use serde::{Deserialize, Serialize};

use crate::codes::{AntwortCode, Cluster};

/// What the tree decided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MabisEntscheidung {
    /// Send this Antwortcode. Its [`Cluster`] decides the answer message —
    /// the caller never chooses that separately.
    Antwort(Box<MabisAntwort>),
    /// Nothing is owed. `E_0100` publishes only Reklamationsgründe, so an
    /// acceptable profile is acknowledged with silence; a Zustimmung there
    /// would be an unsolicited message.
    Schweigen,
    /// The tree reached a Prüfschritt the caller's records cannot answer.
    ///
    /// Queue it for an operator. Do **not** invent a code: a MaBiS answer is a
    /// binding statement about a Bilanzkreisabrechnung that settles in money.
    Eskalation {
        /// What the operator must decide, in the EBD's own terms.
        grund: String,
        /// The Prüfschritt the walk stopped at.
        pruefschritt: u16,
    },
}

/// A resolved MaBiS answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MabisAntwort {
    /// The EBD the code was resolved against.
    pub tree: String,
    /// DE 9013 — the Antwortcode.
    pub code: String,
    /// DE 1131 — the EBD that publishes it.
    pub ebd: Option<String>,
    /// Which cluster the code sits in.
    pub cluster: Cluster,
    /// The BDEW's own wording, for the operator queue and the audit log.
    pub bedeutung: String,
    /// `FTX+ACB` Erläuterung, required by some codes.
    pub bemerkung: Option<String>,
    /// `true` when the Codeliste requires a written Erläuterung alongside the
    /// code. Sending one of these bare is an incomplete answer.
    pub braucht_bemerkung: bool,
    /// The Prüfschritt that produced the code.
    pub pruefschritt: u16,
}

impl MabisAntwort {
    /// Build an answer from a catalogue entry, for callers that decide a
    /// Prüfschritt outside this crate. Pass `0` for `pruefschritt` when the
    /// decision did not come from a tree walk.
    #[must_use]
    pub fn from_code(
        tree: &'static str,
        code: &'static AntwortCode,
        pruefschritt: u16,
        bemerkung: Option<String>,
    ) -> Self {
        Self::new(tree, code, pruefschritt, bemerkung)
    }

    pub(crate) fn new(
        tree: &'static str,
        code: &'static AntwortCode,
        pruefschritt: u16,
        bemerkung: Option<String>,
    ) -> Self {
        Self {
            tree: tree.to_owned(),
            code: code.code.to_owned(),
            ebd: code.ebd.map(ToOwned::to_owned),
            cluster: code.cluster,
            bedeutung: code.bedeutung.to_owned(),
            bemerkung,
            braucht_bemerkung: code.braucht_bemerkung,
            pruefschritt,
        }
    }

    /// `true` when the answer agrees.
    #[must_use]
    pub fn ist_zustimmung(&self) -> bool {
        self.cluster == Cluster::Zustimmung
    }

    /// `true` when a Prüfmitteilung carrying this answer is **forwarded** to the
    /// next market partner.
    ///
    /// MaBiS Kap. 9.8.2 Nr. 2: an abgewiesene Prüfmitteilung is not forwarded.
    /// Branching on [`Self::ist_zustimmung`] instead forwards traffic the BIKO
    /// must never see.
    #[must_use]
    pub fn wird_weitergeleitet(&self) -> bool {
        !self.cluster.ist_abweisung()
    }

    /// `true` when the answer is itself a list of disputed positions.
    ///
    /// A `KorrekturlisteWegenAblehnung` answer without positions is malformed,
    /// and an `AblehnungDerGesamtenListe` answer *with* positions is too.
    #[must_use]
    pub fn traegt_positionen(&self) -> bool {
        self.cluster == Cluster::KorrekturlisteWegenAblehnung
    }
}

impl MabisEntscheidung {
    /// Build an answer from a catalogue entry.
    ///
    /// # Panics
    ///
    /// When `tree` does not publish `code` — that is a bug in the walk, not a
    /// runtime condition.
    pub(crate) fn antwort(tree: &'static str, code: &str, pruefschritt: u16) -> Self {
        Self::antwort_mit(tree, code, pruefschritt, None)
    }

    pub(crate) fn antwort_mit(
        tree: &'static str,
        code: &str,
        pruefschritt: u16,
        bemerkung: Option<String>,
    ) -> Self {
        let entry = super::codes::lookup(tree, code)
            .unwrap_or_else(|| panic!("{tree} does not publish {code}"));
        Self::Antwort(Box::new(MabisAntwort::new(
            tree,
            entry,
            pruefschritt,
            bemerkung,
        )))
    }

    pub(crate) fn eskalation(grund: impl Into<String>, pruefschritt: u16) -> Self {
        Self::Eskalation {
            grund: grund.into(),
            pruefschritt,
        }
    }

    /// The resolved answer, or `None` on Schweigen or an Eskalation.
    #[must_use]
    pub fn antwort_ref(&self) -> Option<&MabisAntwort> {
        match self {
            Self::Antwort(a) => Some(a),
            Self::Schweigen | Self::Eskalation { .. } => None,
        }
    }
}

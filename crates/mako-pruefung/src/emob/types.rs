//! What a Modell-2 tree walk lands on.

use serde::{Deserialize, Serialize};

use crate::codes::{AntwortCode, Cluster};

/// A resolved Modell-2 answer, ready for `SG4 STS+E01` of the outbound UTILMD.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmobAntwort {
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
    /// `FTX+ACB` Erläuterung, required by `A99`.
    pub bemerkung: Option<String>,
    /// `true` when the Codeliste requires a written Erläuterung alongside the
    /// code. Sending one of these bare is an incomplete answer.
    pub braucht_bemerkung: bool,
    /// The Prüfschritt that produced the code.
    pub pruefschritt: u16,
}

impl EmobAntwort {
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

    /// `true` when this answer agrees with the request.
    ///
    /// Reads the cluster, never the bare code — `A01` agrees in `E_0511` and
    /// refuses in `E_0510`.
    #[must_use]
    pub const fn ist_zustimmung(&self) -> bool {
        matches!(self.cluster, Cluster::Zustimmung)
    }
}

/// The outcome of walking one Modell-2 Entscheidungsbaum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmobEntscheidung {
    /// Send this Antwortcode. Its [`Cluster`] decides nothing further here —
    /// both Modell-2 answers ride the *same* Prüfidentifikator (55239 for the
    /// Anmeldung, 55243 for the Abmeldung), and the cluster is carried inside
    /// it as `SG4 STS+E01` DE 9013.
    Antwort(Box<EmobAntwort>),

    /// No answer is owed **yet**: this step hands the message to another tree.
    ///
    /// The only producer is `E_0513` („Prüfen, ob Anmeldung direkt
    /// ablehnbar"), whose „nein" branch leads to `E_0514`
    /// („Beendigung der Zuordnung prüfen"), which publishes no tree because no
    /// answer is given there. The VNB's actual answer to the Anmeldung comes
    /// later, from `E_0510`, once the LF's own window has run.
    ///
    /// A caller must **not** read this as agreement. Nothing has been
    /// confirmed; the 55240 leg to the LF is what is owed next.
    Weiter {
        /// The EBD the message was handed to (`E_0514`).
        naechster_baum: &'static str,
    },

    /// The tree reached a Prüfschritt the caller's records cannot answer.
    ///
    /// Queue it for an operator. Do **not** reach for `A99`: it is an
    /// *Ablehnung*, so using it as a stand-in for „we do not know" refuses a
    /// counterparty's lawful Anmeldung — and it stops being available at all on
    /// 01.04.2027 ([`super::codes::A99_NUTZUNGSMOEGLICHKEIT_ENDE`]).
    Eskalation {
        /// What the operator must decide, in the EBD's own terms.
        grund: String,
        /// The Prüfschritt the walk stopped at.
        pruefschritt: u16,
    },
}

impl EmobEntscheidung {
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
        Self::Antwort(Box::new(EmobAntwort::new(
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

    /// The resolved answer, or `None` on a handover or an escalation.
    #[must_use]
    pub fn antwort_ref(&self) -> Option<&EmobAntwort> {
        match self {
            Self::Antwort(a) => Some(a),
            Self::Weiter { .. } | Self::Eskalation { .. } => None,
        }
    }

    /// `true` when an answer message goes out now.
    #[must_use]
    pub const fn ist_antwort(&self) -> bool {
        matches!(self, Self::Antwort(_))
    }
}

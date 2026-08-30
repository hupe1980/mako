//! The Antwortcode catalogue — what a counterparty is entitled to answer with.
//!
//! A Prüfidentifikator says which message answers a request, not what the
//! answer may state. That is published per Entscheidungsbaum as a Codeliste,
//! and rides `SG4 STS+E01`: the code in DE 9013, the list in DE 1131.
//!
//! Three traps make it worth binding rather than writing a code into a test as
//! a string. A code has no meaning outside its tree — `A02` differs in
//! `E_0607`, `E_0622` and `E_0609` — so [`antwort_code`] resolves within one.
//! Whether a code agrees is the code's own property, not a boolean the caller
//! supplies. And DE 1131 is not always the EBD number: the WiM trees publish
//! through `S_xxxx` / `G_xxxx` lists picked by the cluster, and Gas names none
//! at all.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyModule;

use mako_pruefung::codes::{self, Cluster};

/// One published Antwortcode, resolved against the tree that publishes it.
#[pyclass(get_all, frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct AntwortCode {
    /// `SG4 STS+E01` DE 9013 — the code itself (`"A06"`, `"Z12"`, …).
    pub code: String,
    /// The Entscheidungsbaum the code was resolved against, e.g. `"E_0622"`.
    ///
    /// This is the code's *identity*. It is not necessarily what goes on the
    /// wire — see [`Self::wire_codeliste`].
    pub tree: String,
    /// `SG4 STS+E01` DE 1131 / ORDRSP `SG2 AJT` DE 1082 — the value the answer
    /// must actually carry, which is the EBD number for every GPKE and GeLi Gas
    /// tree and a `S_xxxx` / `G_xxxx` Codeliste for the WiM ones.
    ///
    /// `None` where the answer names no list at all — the Gas Codelisten are
    /// not required in DE 1131.
    pub wire_codeliste: Option<String>,
    /// `"ZUSTIMMUNG"`, `"ABLEHNUNG"`, `"ABWEISUNG"`, `"REKLAMATION"`,
    /// `"AENDERUNG_DER_DATEN"`, `"KEINE_AENDERUNG_DER_DATEN"`,
    /// `"ABLEHNUNG_DER_GESAMTEN_LISTE"` or `"KORREKTURLISTE_WEGEN_ABLEHNUNG"`.
    pub cluster: String,
    /// The BDEW's own wording for the code.
    pub bedeutung: String,
    /// `True` when the BDEW requires a written Erläuterung (`FTX+ACB`) beside
    /// the code. Sending one of these bare is an incomplete answer.
    pub braucht_bemerkung: bool,
}

#[pymethods]
impl AntwortCode {
    /// `True` when the code agrees, `False` when it refuses, `None` off the
    /// agreement axis.
    ///
    /// `None` is not "unknown": `E_0595` states whether a Stammdatenänderung
    /// follows, and a MaBiS Profil-Reklamation does not invalidate the profile
    /// it complains about. A caller deriving the answer PID from agreement has
    /// to handle it rather than read `None` as a refusal.
    #[getter]
    fn ist_zustimmung(&self) -> Option<bool> {
        cluster_from_str(&self.cluster).and_then(|c| match c {
            Cluster::Zustimmung => Some(true),
            Cluster::Ablehnung
            | Cluster::Abweisung
            | Cluster::AblehnungDerGesamtenListe
            | Cluster::KorrekturlisteWegenAblehnung => Some(false),
            Cluster::AenderungDerDaten | Cluster::KeineAenderungDerDaten | Cluster::Reklamation => {
                None
            }
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "AntwortCode({} {} [{}] — {})",
            self.tree, self.code, self.cluster, self.bedeutung
        )
    }
}

fn cluster_key(cluster: Cluster) -> &'static str {
    match cluster {
        Cluster::Zustimmung => "ZUSTIMMUNG",
        Cluster::Ablehnung => "ABLEHNUNG",
        Cluster::AenderungDerDaten => "AENDERUNG_DER_DATEN",
        Cluster::KeineAenderungDerDaten => "KEINE_AENDERUNG_DER_DATEN",
        Cluster::Abweisung => "ABWEISUNG",
        Cluster::AblehnungDerGesamtenListe => "ABLEHNUNG_DER_GESAMTEN_LISTE",
        Cluster::KorrekturlisteWegenAblehnung => "KORREKTURLISTE_WEGEN_ABLEHNUNG",
        Cluster::Reklamation => "REKLAMATION",
    }
}

fn cluster_from_str(key: &str) -> Option<Cluster> {
    Some(match key {
        "ZUSTIMMUNG" => Cluster::Zustimmung,
        "ABLEHNUNG" => Cluster::Ablehnung,
        "AENDERUNG_DER_DATEN" => Cluster::AenderungDerDaten,
        "KEINE_AENDERUNG_DER_DATEN" => Cluster::KeineAenderungDerDaten,
        "ABWEISUNG" => Cluster::Abweisung,
        "ABLEHNUNG_DER_GESAMTEN_LISTE" => Cluster::AblehnungDerGesamtenListe,
        "KORREKTURLISTE_WEGEN_ABLEHNUNG" => Cluster::KorrekturlisteWegenAblehnung,
        "REKLAMATION" => Cluster::Reklamation,
        _ => return None,
    })
}

fn convert(tree: &str, c: &codes::AntwortCode) -> AntwortCode {
    AntwortCode {
        code: c.code.to_owned(),
        tree: tree.to_owned(),
        wire_codeliste: c.wire_codeliste().map(ToOwned::to_owned),
        cluster: cluster_key(c.cluster).to_owned(),
        bedeutung: c.bedeutung.to_owned(),
        braucht_bemerkung: c.braucht_bemerkung,
    }
}

/// Every Entscheidungsbaum the compiled catalogue carries codes for, sorted.
#[pyfunction]
pub fn entscheidungsbaeume() -> Vec<String> {
    let mut out: Vec<String> = codes::CODELISTEN
        .iter()
        .map(|(tree, _)| (*tree).to_owned())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The Antwortcode `code` as `tree` publishes it, or `None`.
///
/// `None` means the tree does not publish that code — which is a defect in the
/// *test*, not in the platform: a counterparty answering with it would be
/// sending a code the Entscheidungsbaum has no leaf for.
#[pyfunction]
pub fn antwort_code(tree: &str, code: &str) -> Option<AntwortCode> {
    codes::lookup(tree, code).map(|c| convert(tree, c))
}

/// Every code `tree` publishes, in catalogue order.
///
/// This is the tree's whole outcome space, which is what makes "which process
/// tests should exist" a derivable question: every published code is one
/// observable outcome a platform has to handle.
///
/// Raises `ValueError` for a tree the catalogue does not carry, rather than
/// returning an empty list — an empty outcome space and an unknown tree are
/// different answers, and only one of them is a test defect.
#[pyfunction]
pub fn antwort_codes(tree: &str) -> PyResult<Vec<AntwortCode>> {
    codes::CODELISTEN
        .iter()
        .find(|(id, _)| *id == tree)
        .map(|(id, list)| list.iter().map(|c| convert(id, c)).collect())
        .ok_or_else(|| {
            PyValueError::new_err(format!(
                "no Entscheidungsbaum {tree:?} in the compiled catalogue — see \
                 entscheidungsbaeume() for the trees this build carries"
            ))
        })
}

/// The Antwortcodes of the Entscheidungsbaum the answer-Frist table names for
/// the inbound `trigger_pid`.
///
/// The join the BDEW publishes across three separate documents: the
/// Prüfidentifikator names the process, the Festlegung names the
/// Entscheidungsbaum that decides its answer, and the Codeliste names the
/// outcomes that tree can reach. Empty when the table names no tree for that
/// PID, or the catalogue carries none for the tree it names.
///
/// **One PID can be decided by a chain of trees, and this returns the first.**
/// A GPKE Anmeldung runs the Vorprüfung `E_0622`, which publishes refusals
/// only, before `E_0623` decides the Lieferbeginn and supplies the agreement
/// codes — so `antwort_codes_for_pid(55001)` is 55001's *refusal* space, not
/// its whole one. Every returned code names its own `tree`, and a caller that
/// needs a later stage asks [`antwort_codes`] for it by name. Inferring the
/// rest of the chain here would mean inventing a mapping no document states.
#[pyfunction]
pub fn antwort_codes_for_pid(trigger_pid: u32) -> Vec<AntwortCode> {
    let Some(obligation) = mako_fristen::antwort::antwort_obligation(trigger_pid) else {
        return Vec::new();
    };
    let Some(tree) = obligation.ebd else {
        return Vec::new();
    };
    antwort_codes(tree).unwrap_or_default()
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(entscheidungsbaeume, m)?)?;
    m.add_function(wrap_pyfunction!(antwort_code, m)?)?;
    m.add_function(wrap_pyfunction!(antwort_codes, m)?)?;
    m.add_function(wrap_pyfunction!(antwort_codes_for_pid, m)?)?;
    m.add_class::<AntwortCode>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reason a code is resolved within a tree and never on its own.
    #[test]
    fn one_code_means_different_things_in_different_trees() {
        let anmeldung = antwort_code("E_0622", "A02").expect("E_0622 publishes A02");
        let abmeldung = antwort_code("E_0607", "A02").expect("E_0607 publishes A02");
        assert_ne!(
            anmeldung.bedeutung, abmeldung.bedeutung,
            "A02 carries one meaning per tree"
        );
        assert!(antwort_code("E_0622", "NOSUCH").is_none());
    }

    /// The cluster is a property of the code, so a simulator does not get to
    /// call a refusal an agreement.
    #[test]
    fn the_cluster_decides_which_pid_carries_the_answer() {
        let zustimmung = antwort_code("E_0623", "A51").expect("E_0623 confirms with A51");
        assert_eq!(zustimmung.cluster, "ZUSTIMMUNG");
        assert_eq!(zustimmung.ist_zustimmung(), Some(true));

        let ablehnung = antwort_code("E_0622", "A02").expect("A02 refuses");
        assert_eq!(ablehnung.cluster, "ABLEHNUNG");
        assert_eq!(ablehnung.ist_zustimmung(), Some(false));
    }

    /// A GPKE Anmeldung is decided by **two** trees in sequence, and only the
    /// first is what the obligation table names: `E_0622` is the Vorprüfung and
    /// publishes refusals only, `E_0623` decides the Lieferbeginn and is where
    /// the agreement codes live.
    ///
    /// Pinned because it is the shape that makes "the tree for this PID" an
    /// incomplete question — a caller confirming a 55001 has to name `E_0623`
    /// rather than expect the obligation's tree to carry an agreement.
    #[test]
    fn the_vorpruefung_tree_publishes_no_agreement() {
        let vorpruefung = antwort_codes("E_0622").unwrap();
        assert!(!vorpruefung.is_empty());
        assert!(
            vorpruefung
                .iter()
                .all(|c| c.ist_zustimmung() == Some(false)),
            "E_0622 is the Vorprüfung: every leaf refuses"
        );
        let lieferbeginn = antwort_codes("E_0623").unwrap();
        assert!(
            lieferbeginn
                .iter()
                .any(|c| c.ist_zustimmung() == Some(true))
        );
        assert!(
            lieferbeginn
                .iter()
                .any(|c| c.ist_zustimmung() == Some(false))
        );
    }

    /// `E_0595` is off the agreement axis: neither of its clusters is a
    /// Zustimmung, and reading `None` as a refusal would invert the answer.
    #[test]
    fn a_tree_off_the_agreement_axis_answers_none() {
        let off = antwort_codes("E_0595")
            .expect("E_0595 is in the catalogue")
            .into_iter()
            .find(|c| c.cluster.contains("AENDERUNG_DER_DATEN"))
            .expect("E_0595 states whether data follows");
        assert_eq!(off.ist_zustimmung(), None);
    }

    /// DE 1131 carries the *Codeliste*, and for the WiM trees that is not the
    /// EBD number. Writing the EBD there is a rejected message.
    #[test]
    fn the_wim_trees_name_a_codeliste_rather_than_their_ebd() {
        let codes = antwort_codes("E_0200").expect("the WiM Kündigung tree");
        let zustimmung = codes
            .iter()
            .find(|c| c.cluster == "ZUSTIMMUNG")
            .expect("a Kündigung can be confirmed");
        assert_eq!(zustimmung.wire_codeliste.as_deref(), Some("S_0090"));
        let ablehnung = codes
            .iter()
            .find(|c| c.cluster == "ABLEHNUNG")
            .expect("a Kündigung can be refused");
        assert_eq!(ablehnung.wire_codeliste.as_deref(), Some("S_0054"));

        // A GPKE tree names itself, so the two agree there.
        let gpke = antwort_code("E_0622", "A02").unwrap();
        assert_eq!(gpke.wire_codeliste.as_deref(), Some("E_0622"));
    }

    /// The join that exists in no single BDEW document: inbound PID → the tree
    /// the Festlegung assigns it → the outcomes that tree publishes.
    #[test]
    fn an_inbound_pid_resolves_to_the_outcomes_of_its_named_tree() {
        let outcomes = antwort_codes_for_pid(55_001);
        assert!(
            outcomes.len() > 3,
            "55001 is decided by E_0622, got {}",
            outcomes.len()
        );
        assert!(outcomes.iter().all(|c| c.tree == "E_0622"));

        // A PID with no published obligation resolves to nothing rather than
        // guessing a tree.
        assert!(antwort_codes_for_pid(44_020).is_empty());

        // A tree that decides on both axes reaches both from the PID.
        let abmeldung = antwort_codes_for_pid(55_004);
        assert!(abmeldung.iter().any(|c| c.ist_zustimmung() == Some(true)));
        assert!(abmeldung.iter().any(|c| c.ist_zustimmung() == Some(false)));
    }

    /// The NB trees must be in the catalogue without `role-nb`.
    ///
    /// Only the Codelisten are bound here, and they are not role-gated — which
    /// is what lets the wheel leave out the one feature that would drag an
    /// async runtime into an extension module that never runs one.
    #[test]
    fn codelisten_do_not_depend_on_the_role_features() {
        for tree in ["E_0622", "E_0623", "E_0607", "E_0608"] {
            assert!(
                !antwort_codes(tree).unwrap_or_default().is_empty(),
                "{tree} is a Netzbetreiber tree and must be catalogued anyway"
            );
        }
    }

    /// An unknown tree is a different answer from a tree with no codes.
    #[test]
    fn an_unknown_tree_is_refused_rather_than_reported_empty() {
        assert!(antwort_codes("E_9999").is_err());
        assert!(!entscheidungsbaeume().is_empty());
        assert!(entscheidungsbaeume().windows(2).all(|w| w[0] < w[1]));
    }

    /// Every catalogue entry has to survive the round trip through the wire
    /// key, or `ist_zustimmung` silently answers `None` for a real cluster.
    #[test]
    fn every_cluster_key_round_trips() {
        for (tree, list) in codes::CODELISTEN {
            for c in *list {
                let bound = convert(tree, c);
                assert_eq!(
                    cluster_from_str(&bound.cluster),
                    Some(c.cluster),
                    "{tree}/{}: cluster key does not round trip",
                    c.code
                );
                assert_eq!(bound.ist_zustimmung(), c.ist_zustimmung());
            }
        }
    }
}

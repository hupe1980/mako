//! The trees whose **prüfende Rolle** is the Netzbetreiber.
//!
//! | Function | Inbound PIDs | EBD |
//! |---|---|---|
//! | [`evaluate`] | 55001, 55077, 44001 | `E_0622` / `E_3005` „Prüfen, ob Anmeldung direkt ablehnbar" |
//! | [`evaluate_abmeldung`] | 55004, 44004 | `E_0607` / `E_3019` „Abmeldung prüfen" |
//! | [`evaluate_neuanlage`] | 55600, 55601 | `E_0608` „Anmeldung einer Zuordnung" |
//!
//! They are separate functions because the trees have separate Codelisten:
//! `A02` is „Marktlokation nimmt nicht an der Marktkommunikation teil" in
//! `E_0622` and „Vorlauffrist nicht eingehalten" in `E_0607`. Resolve a code
//! against its tree with [`crate::codes::lookup`].
//!
//! # `evaluate` — the Anmeldung
//!
//! `E_0622` Prüfschritt 10 splits Strom into two branches that share no code,
//! and Gas answers from `G_0011`:
//!
//! | Condition | verbrauchend/ruhend | erzeugend | Gas |
//! |---|---|---|---|
//! | Vorlauffrist | `A07` | `A34`/`A28`/`A29`/`A30`/`A32`/`A35`/`A44` | `E17` |
//! | nimmt nicht an der MaKo teil | `A02` | — | `A16` |
//! | Zuordnungsermächtigung | `A05` | `A25` | `E13` |
//! | andere Anmeldung in Bearbeitung | `A06` | `A45` | `ZC5` |
//! | Zustimmung | `A51` (`E_0623`) | `A58` (`E_0623`) | `E15` (`G_0012`) |
//!
//! `E_0622` and `E_3005` are **Vorprüfungen** — every code they publish is an
//! Ablehnung, and a surviving message is confirmed out of `E_0623` / `E_3007`.
//! `SG4 STS+E01` is Muss on every Antwortnachricht, so `Accept` carries a code.
//!
//! # `evaluate_neuanlage` — the Neuanlage, and its third outcome
//!
//! `E_0608` Prüfschritte 110 / 590 are a **loop**: an Anmeldung whose
//! Marktlokation cannot yet be identified is re-checked daily for 60 Werktage
//! before it may be refused. [`NeuanlageEntscheidung::Vertagen`] is that state —
//! the NB answers nothing at all that day.
//!
pub mod abmeldung;
pub mod anmeldung;
pub mod config;
pub mod lieferbeginn;
pub mod neuanlage;
pub mod types;

/// The Entscheidungsbaum each NB walk answers from, per Sparte.
///
/// The walks below resolve their codes against these, so a caller that needs
/// to name the tree of a decision it did not finish — an escalation the
/// operator answers — reads it here rather than keeping its own table.
///
/// Source: BDEW *Entscheidungsbaum-Diagramme und Codelisten* 4.3 — Kap. 6.6.1
/// (`E_0622`), 6.6.4 (`E_0623`), 13.6.1 (`E_3005`), 13.6.4 (`E_3007`), 6.3.1
/// (`E_0607`), 13.4.1 (`E_3019`).
pub mod baum {
    use crate::codes as c;
    use mako_markt::domain::Sparte;

    /// „Prüfen, ob Anmeldung direkt ablehnbar" — the Vorprüfung
    /// [`super::evaluate`] walks: `E_0622` / `E_3005`. Ablehnungen only.
    #[must_use]
    pub const fn vorpruefung(sparte: Sparte) -> &'static str {
        match sparte {
            Sparte::Strom => c::EBD_ANMELDUNG_DIREKT_ABLEHNBAR,
            Sparte::Gas => c::EBD_ANMELDUNG_DIREKT_ABLEHNBAR_GAS,
        }
    }

    /// Lieferbeginn prüfen — what [`super::evaluate_lieferbeginn`] walks
    /// once the Vorprüfung is passed: `E_0623` / `E_3007`.
    #[must_use]
    pub const fn lieferbeginn(sparte: Sparte) -> &'static str {
        match sparte {
            Sparte::Strom => c::EBD_LIEFERBEGINN,
            Sparte::Gas => c::EBD_LIEFERBEGINN_GAS,
        }
    }

    /// Abmeldung prüfen — what [`super::evaluate_abmeldung`] walks:
    /// `E_0607` / `E_3019`.
    #[must_use]
    pub const fn abmeldung(sparte: Sparte) -> &'static str {
        match sparte {
            Sparte::Strom => c::EBD_ABMELDUNG_NB,
            Sparte::Gas => c::EBD_ABMELDUNG_GAS_NB,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The Vorprüfung hands a surviving Anmeldung to exactly the
        /// Lieferbeginn tree named here, in both Sparten — so naming the stage
        /// a walk stopped in names the tree its codes come from.
        #[test]
        fn the_vorpruefung_hands_over_to_the_lieferbeginn_tree() {
            for sparte in [Sparte::Strom, Sparte::Gas] {
                assert_eq!(
                    c::zustimmung_tree_of(vorpruefung(sparte)),
                    Some(lieferbeginn(sparte))
                );
                for tree in [vorpruefung(sparte), lieferbeginn(sparte), abmeldung(sparte)] {
                    assert!(
                        c::codes_im_cluster(tree, c::Cluster::Ablehnung).count() > 0,
                        "{tree} publishes no Ablehnung"
                    );
                }
            }
        }
    }
}

pub use abmeldung::evaluate_abmeldung;
pub use anmeldung::evaluate;
pub use config::NetzCheckConfig;
pub use lieferbeginn::{CODES_REQUIRING_DRITTER, evaluate_lieferbeginn};
pub use neuanlage::{
    Identifikation, NeuanlageAnfrage, NeuanlageBefund, NeuanlageEntscheidung, evaluate_neuanlage,
};
pub use types::{
    Abmeldeanfrage, AbmeldungAnfrage, AnmeldungAnfrage, AntwortDetail, ErzeugungsAnmeldung,
    Geschaeftsvorfall, LfaAntwort, MaloGridRecord, Marktlokationsart, Messtyp, NbEntscheidung,
    RejectReason, RuhendeAnmeldung, TranchenAntwort, TranchenLage, TranchenZuordnung,
    Veraeusserungsform,
};

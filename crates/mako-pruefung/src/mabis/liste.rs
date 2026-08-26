//! `Marktlokationen mit ‹Liste› abgleichen` — the Clearinglisten trees.
//!
//! A list answer is **itself a list**, and that is the distinction the two
//! refusal clusters carry:
//!
//! | Cluster | Positions | What the sender must do |
//! |---|---|---|
//! | [`Cluster::AblehnungDerGesamtenListe`] | none | resend a whole list |
//! | [`Cluster::KorrekturlisteWegenAblehnung`] | one per disputed Marktlokation | reconcile the named positions |
//!
//! [`Cluster::AblehnungDerGesamtenListe`]: crate::codes::Cluster::AblehnungDerGesamtenListe
//! [`Cluster::KorrekturlisteWegenAblehnung`]: crate::codes::Cluster::KorrekturlisteWegenAblehnung
//!
//! The whole-list Prüfschritte run **first and to completion**: a list that was
//! never subscribed is refused entire, and no position in it is assessed. The
//! reply is owed either way — silence reads as acceptance of what the sender
//! filed, so an empty [`ListenEntscheidung::Korrekturliste`] is the normal
//! „reconciled, nothing to correct" outcome.
//!
//! Source: BDEW *Entscheidungsbaum-Diagramme und Codelisten* 4.3.

use crate::codes::AntwortCode;

use super::types::{MabisAntwort, MabisEntscheidung};

/// Why a single position is disputed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Korrekturgrund {
    /// The receiver holds a Marktlokation the list does not name.
    Ergaenzt,
    /// The list assigns the Marktlokation to the wrong Lieferant resp.
    /// Netzbetreiber. Which of the two a tree names follows from the list, not
    /// from the caller.
    FalscheZuordnung,
    /// The list names a Marktlokation the receiver does not hold.
    Entfallen,
    /// Bilanzierungsrelevante Daten are wrong or missing.
    DatenFehlerhaft,
    /// The Marktlokation is not known at all (`E_0070` only).
    MaloUnbekannt,
}

/// One disputed position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Korrekturposition {
    /// The Marktlokations-ID the correction is about.
    pub malo: String,
    /// Why it is disputed.
    pub grund: Korrekturgrund,
}

/// The facts a Listenabgleich needs. `None` is unknown and escalates.
#[derive(Debug, Clone, Default)]
pub struct ListenPruefung<'a> {
    /// Abonnement-Listen (`E_0004`, `E_0049`, `E_0052`): was it subscribed?
    pub abonnement_bestellt: Option<bool>,
    /// Einzelanforderungs- and AACL-Listen: is the period plausible?
    pub zeitraum_plausibel: Option<bool>,
    /// Einzelanforderungs-Listen: does the list answer the MaBiS-ZP that was
    /// asked for?
    pub mabis_zp_passt: Option<bool>,
    /// `E_0070`: did the list arrive inside the Clearingphase DZÜ?
    pub innerhalb_clearingphase: Option<bool>,
    /// Is the list's Versionsangabe permitted?
    pub version_zugelassen: Option<bool>,
    /// The positions the receiver disputes, already reconciled by the caller.
    pub positionen: &'a [Korrekturposition],
}

/// The outcome of a Listenabgleich.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenEntscheidung {
    /// The whole list is refused; it carries **no** positions.
    GesamtAblehnung(Box<MabisAntwort>),
    /// The list was assessed. One entry per disputed position — empty when
    /// nothing was found, which is still an answer that must be sent.
    Korrekturliste(Vec<KorrekturAntwort>),
    /// A Prüfschritt the caller's records cannot answer.
    Eskalation {
        /// What the operator must decide.
        grund: String,
        /// The Prüfschritt the walk stopped at.
        pruefschritt: u16,
    },
}

/// One position of a Korrekturliste.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KorrekturAntwort {
    /// The Marktlokation the entry is about.
    pub malo: String,
    /// The resolved code, drawn from the tree that decided the list.
    pub antwort: MabisAntwort,
}

/// The whole-list Prüfschritte a tree runs, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gesamt {
    /// „Wurde das Abonnement bestellt?"
    Abonnement,
    /// „Ist der Zeitraum plausibel?"
    Zeitraum,
    /// „Entspricht der MaBiS-ZP dem angefragten?"
    MabisZp,
    /// „Ist die Version zugelassen?"
    Version,
    /// „Liegt der Eingang innerhalb der Clearingphase DZÜ?"
    Clearingphase,
}

/// `(ebd, whole-list steps, (Ergaenzt, FalscheZuordnung, Entfallen,
/// DatenFehlerhaft, MaloUnbekannt) codes)`.
///
/// A `None` in the Korrekturgrund tuple means the tree does not publish that
/// ground — asking for it is a caller bug and escalates rather than emitting a
/// neighbouring code.
type Tabelle = (
    &'static str,
    &'static [(Gesamt, &'static str, u16)],
    [Option<(&'static str, u16)>; 5],
);

const TABELLEN: &[Tabelle] = &[
    // ── Abonnement-Listen ─────────────────────────────────────────────────────
    (
        super::codes::EBD_LF_CL_ERSTABO,
        &[(Gesamt::Abonnement, "A01", 1), (Gesamt::Version, "A02", 2)],
        [
            Some(("A03", 3)),
            Some(("A04", 4)),
            Some(("A05", 5)),
            Some(("A06", 5)),
            None,
        ],
    ),
    (
        // `A06` is not published by this tree — the Daten-Korrekturgrund is `A07`.
        super::codes::EBD_LF_CL_FOLGEABO,
        &[(Gesamt::Abonnement, "A01", 1), (Gesamt::Version, "A02", 2)],
        [
            Some(("A03", 3)),
            Some(("A04", 4)),
            Some(("A05", 5)),
            Some(("A07", 5)),
            None,
        ],
    ),
    (
        super::codes::EBD_BG_CL_ABO,
        &[(Gesamt::Abonnement, "A01", 1), (Gesamt::Version, "A02", 2)],
        [
            Some(("A03", 3)),
            Some(("A04", 4)),
            Some(("A05", 5)),
            Some(("A06", 5)),
            None,
        ],
    ),
    // ── Einzelanforderungs-Listen ─────────────────────────────────────────────
    (
        super::codes::EBD_LF_CL_EINZEL,
        &[
            (Gesamt::Zeitraum, "A01", 1),
            (Gesamt::MabisZp, "A02", 2),
            (Gesamt::Version, "A03", 3),
        ],
        [
            Some(("A04", 4)),
            Some(("A05", 5)),
            Some(("A06", 6)),
            Some(("A07", 6)),
            None,
        ],
    ),
    (
        super::codes::EBD_LF_CL_CLEARING,
        &[
            (Gesamt::Zeitraum, "A01", 1),
            (Gesamt::MabisZp, "A02", 2),
            (Gesamt::Version, "A03", 3),
        ],
        [
            Some(("A04", 4)),
            Some(("A05", 5)),
            Some(("A06", 6)),
            Some(("A07", 6)),
            None,
        ],
    ),
    (
        super::codes::EBD_BG_CL_EINZEL,
        &[
            (Gesamt::Zeitraum, "A01", 1),
            (Gesamt::MabisZp, "A02", 2),
            (Gesamt::Version, "A03", 3),
        ],
        [
            Some(("A04", 4)),
            Some(("A05", 5)),
            Some(("A06", 6)),
            Some(("A07", 6)),
            None,
        ],
    ),
    (
        super::codes::EBD_LF_AACL_EINZEL,
        &[
            (Gesamt::Zeitraum, "A01", 1),
            (Gesamt::MabisZp, "A02", 2),
            (Gesamt::Version, "A03", 3),
        ],
        [
            Some(("A04", 4)),
            Some(("A05", 5)),
            Some(("A06", 6)),
            Some(("A07", 6)),
            None,
        ],
    ),
    // ── AACL-Abonnement ───────────────────────────────────────────────────────
    (
        super::codes::EBD_LF_AACL,
        &[(Gesamt::Zeitraum, "A01", 1), (Gesamt::Version, "A02", 2)],
        [
            Some(("A03", 3)),
            Some(("A04", 4)),
            Some(("A05", 5)),
            Some(("A06", 5)),
            None,
        ],
    ),
    // ── DZÜ-Liste ─────────────────────────────────────────────────────────────
    (
        // The only Clearingliste whose whole-list refusal is a Frist, and the
        // only one that publishes „Marktlokation ist nicht bekannt".
        super::codes::EBD_DZUE_LISTE,
        &[(Gesamt::Clearingphase, "A01", 1)],
        [None, None, None, Some(("A03", 2)), Some(("A02", 2))],
    ),
];

/// The code a tree publishes for one Korrekturgrund, without walking the
/// whole-list Prüfschritte.
///
/// [`pruefe_liste`] is the full walk and needs the whole-list facts. A caller
/// that has *already* established the list is assessable — because it is
/// building a Korrekturliste rather than refusing the list entire — asks this
/// instead, rather than asserting facts it never checked.
///
/// `None` when the tree does not publish that Korrekturgrund: `E_0070` has no
/// „ergänzte Marktlokation", and asking for one is a caller bug.
#[must_use]
pub fn korrekturcode(ebd: &str, grund: Korrekturgrund) -> Option<(&'static AntwortCode, u16)> {
    let (_, _, gruende) = TABELLEN.iter().find(|(id, _, _)| *id == ebd)?;
    let (code, nr) = gruende[grund_index(grund)]?;
    Some((super::codes::lookup(ebd, code)?, nr))
}

const fn grund_index(grund: Korrekturgrund) -> usize {
    match grund {
        Korrekturgrund::Ergaenzt => 0,
        Korrekturgrund::FalscheZuordnung => 1,
        Korrekturgrund::Entfallen => 2,
        Korrekturgrund::DatenFehlerhaft => 3,
        Korrekturgrund::MaloUnbekannt => 4,
    }
}

/// Walk a Clearinglisten-Tree.
///
/// # Panics
///
/// When `ebd` is not a catalogued Listen-Tree.
#[must_use]
pub fn pruefe_liste(ebd: &'static str, p: &ListenPruefung<'_>) -> ListenEntscheidung {
    let (_, gesamt, gruende) = TABELLEN
        .iter()
        .find(|(id, _, _)| *id == ebd)
        .unwrap_or_else(|| panic!("{ebd} is not a Listenabgleich tree"));

    // The whole-list steps run first and to completion.
    for &(schritt, code, nr) in *gesamt {
        let (fakt, frage) = match schritt {
            Gesamt::Abonnement => (p.abonnement_bestellt, "Wurde das Abonnement bestellt?"),
            Gesamt::Zeitraum => (p.zeitraum_plausibel, "Ist der Zeitraum plausibel?"),
            Gesamt::MabisZp => (
                p.mabis_zp_passt,
                "Entspricht der MaBiS-ZP dem angefragten MaBiS-ZP?",
            ),
            Gesamt::Version => (p.version_zugelassen, "Ist die Version zugelassen?"),
            Gesamt::Clearingphase => (
                p.innerhalb_clearingphase,
                "Liegt der Eingang innerhalb der Clearingphase DZÜ?",
            ),
        };
        match fakt {
            Some(true) => {}
            Some(false) => {
                let entry = super::codes::lookup(ebd, code)
                    .unwrap_or_else(|| panic!("{ebd} does not publish {code}"));
                return ListenEntscheidung::GesamtAblehnung(Box::new(MabisAntwort::new(
                    ebd, entry, nr, None,
                )));
            }
            None => {
                return ListenEntscheidung::Eskalation {
                    grund: frage.to_owned(),
                    pruefschritt: nr,
                };
            }
        }
    }

    // The list is assessable; every disputed position gets its own code.
    let mut liste = Vec::with_capacity(p.positionen.len());
    for pos in p.positionen {
        let Some((code, nr)) = gruende[grund_index(pos.grund)] else {
            return ListenEntscheidung::Eskalation {
                grund: format!("{ebd} publishes no code for {:?}", pos.grund),
                pruefschritt: 0,
            };
        };
        let entry = super::codes::lookup(ebd, code)
            .unwrap_or_else(|| panic!("{ebd} does not publish {code}"));
        liste.push(KorrekturAntwort {
            malo: pos.malo.clone(),
            antwort: MabisAntwort::new(ebd, entry, nr, None),
        });
    }
    ListenEntscheidung::Korrekturliste(liste)
}

impl ListenEntscheidung {
    /// `true` when the answer carries positions.
    #[must_use]
    pub fn ist_korrekturliste(&self) -> bool {
        matches!(self, Self::Korrekturliste(_))
    }

    /// How many positions the answer disputes; `0` for a whole-list refusal.
    #[must_use]
    pub fn korrekturen(&self) -> usize {
        match self {
            Self::Korrekturliste(l) => l.len(),
            Self::GesamtAblehnung(_) | Self::Eskalation { .. } => 0,
        }
    }
}

/// A [`MabisEntscheidung`] view of a whole-list refusal, for callers that
/// handle every MaBiS answer uniformly.
impl From<ListenEntscheidung> for Option<MabisEntscheidung> {
    fn from(e: ListenEntscheidung) -> Self {
        match e {
            ListenEntscheidung::GesamtAblehnung(a) => Some(MabisEntscheidung::Antwort(a)),
            ListenEntscheidung::Eskalation {
                grund,
                pruefschritt,
            } => Some(MabisEntscheidung::Eskalation {
                grund,
                pruefschritt,
            }),
            ListenEntscheidung::Korrekturliste(_) => None,
        }
    }
}

/// `E_0068` (Einzelanforderung prüfen) and `E_0104` (Listeninhalt prüfen).
///
/// Both publish exactly one code, `A01` „Kein Lieferant zugeordnet". They are
/// the NB's answer when an anfragender Marktpartner asks for data on a
/// Marktlokation that has no Lieferant at the requested time — there is no
/// Zustimmungscode because the positive outcome is the requested list itself.
///
/// Returns [`MabisEntscheidung::Schweigen`] when a Lieferant is assigned, which
/// the caller reads as „deliver the data".
///
/// # Panics
///
/// When `ebd` is neither `E_0068` nor `E_0104`.
#[must_use]
pub fn pruefe_lieferantenzuordnung(
    ebd: &'static str,
    lieferant_zugeordnet: Option<bool>,
) -> MabisEntscheidung {
    assert!(
        matches!(ebd, "E_0068" | "E_0104"),
        "{ebd} is not a Lieferantenzuordnungs-Tree"
    );
    match lieferant_zugeordnet {
        Some(true) => MabisEntscheidung::Schweigen,
        Some(false) => MabisEntscheidung::antwort(ebd, "A01", 1),
        None => MabisEntscheidung::eskalation(
            "Ist der Marktlokation zum angefragten Zeitpunkt ein Lieferant zugeordnet?",
            1,
        ),
    }
}

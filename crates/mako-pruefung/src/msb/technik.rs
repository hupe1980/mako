//! Änderung der Technik an einer Messlokation — the MSB's side of six trees on
//! three message pairs.
//!
//! # Two regulatory documents, one ORDRSP pair
//!
//! WiM Strom Teil 1 Kap. 3.3 has the NB or the LF order a Messlokationsänderung
//! outright. The BDEW *AWH Prozesse zur Änderung der Technik an Lokationen* 1.1
//! puts a quotation round in front of the same order:
//!
//! ```text
//! WiM Teil 1 Kap. 3.3   ORDERS 17011 (ZO-T15) ─────────────▶ ORDRSP 19005/19006   E_0249 · E_0250
//!
//! AWH 1.1               REQOTE 35005 ─▶ QUOTES 15005                              E_0278 · E_0281
//!                                    └▶ IFTSTA 21033 (Ablehnung)
//!                       ORDERS 17011 (ZG-T24) ─────────────▶ ORDRSP 19005/19006   E_0279 · E_0283
//!                       Durchführung ──────────────────────▶ IFTSTA 21027 / 21025 E_0286
//! ```
//!
//! **The answer PIDs are the same in both.** ORDRSP 19005 „Bestätigung Auftrag
//! Änderung Technik" and 19006 „Ablehnung" carry all four Bestellungs-Bäume, so
//! a code cannot be resolved from the answer PID, and the sender's Marktrolle
//! is not enough either: `E_0249` and `E_0279` are both NB → MSB → NB.
//!
//! What separates them is the **Zuordnung zu einem Objekt** the AHB gives the
//! inbound ORDERS (Anwendungsübersicht der Prüfidentifikatoren 4.0):
//!
//! | lfd. Nr. | Prozess | 17011 Zuordnung | Antwort-EBD |
//! |---|---|---|---|
//! | 30660 | WiM Teil 1, vom NB | `ZO-T15` (öffnet den Vorgang) | `E_0249` |
//! | 30720 | WiM Teil 1, vom LF | `ZO-T15` | `E_0250` |
//! | 36030 | AWH, vom NB | `ZG-T24` (Antwort im Vorgang) | `E_0279` |
//! | 36120 | AWH, vom LF | `ZG-T24` | `E_0283` |
//!
//! # Why this is not a cosmetic distinction
//!
//! `A02` is the **Zustimmung** of `E_0249` („Änderung kann durchgeführt
//! werden") and the **Ablehnung** of `E_0279` („MSB ist der betroffenen
//! Lokation zum Beginn des Umsetzungszeitraums nicht mehr zugeordnet"). Same
//! spelling, same PID, same direction, opposite meaning. Answering an AWH
//! Bestellung out of `E_0249` sends a confirmation the counterparty reads as a
//! refusal, and the process ends in a Klärfall neither side can see the cause
//! of.
//!
//! # The Durchführung is a leg of its own
//!
//! `E_0286` is not an answer to a message — it is the MSB reporting, after the
//! field visit, that the change could not be made. Success publishes no code
//! („Stammdatenänderung vom MSB ausgehend versenden"), so
//! [`melde_technik_durchfuehrung`] returns `None` for it. `E_0284`, `E_0285`
//! and `E_0287` are the same tree under three further numbers.
//!
//! # Sources
//!
//! - BK6-22-024 Anlage 2a, WiM Strom Teil 1 Kap. 3.3
//! - BDEW *AWH Prozesse zur Änderung der Technik an Lokationen* V1.1 (31.03.2025)
//! - *Entscheidungsbaum-Diagramme und Codelisten* 4.3 Kap. 8.6, 8.7 und 9.1, 9.2
//! - Anwendungsübersicht der Prüfidentifikatoren 4.0, lfd. Nr. 30660–30740, 36000–36160

use serde::{Deserialize, Serialize};
use time::Date;

use mako_fristen::HolidayCalendar;
use mako_fristen::vorlauf::{VorlaufShape, VorlaufVerdict};

use crate::antwort::RejectReason;
use crate::codes::{
    EBD_TECHNIK_DURCHFUEHRUNG, aenderung_der_technik_baum, lookup, technik_anfrage_baum,
};

use super::types::MsbEntscheidung;

pub use crate::codes::{TechnikBeauftragung as Beauftragungsart, TechnikBesteller as Besteller};

/// Mindestvorlauffrist of a Beauftragung zur Änderung der Technik, in Werktagen.
///
/// WiM Teil 1 Kap. 3.3.1.2 Nr. 1 and the `E_0249`/`E_0250` Prüfschritt that
/// reads it: „Liegt das gewünschte Änderungsdatum mindestens 20 WT nach dem
/// Nachrichteneingangsdatum?"
///
/// It is a rejection here and **not** a date the MSB may move — unlike the
/// Abmeldung of Kap. 2.4.2 Nr. 2, where the NB sets the nächstmögliches
/// Zuordnungsende and confirms with `Z01`. `E_0249` publishes no such code.
pub const AENDERUNG_VORLAUF_WT: u32 = 20;

/// The Messlokationsänderung a request refers to — the facts every tree in this
/// family asks about.
///
/// `Option` means „not established", and every one of them escalates rather
/// than defaulting: refusing a lawful order and confirming an impossible one
/// are both binding statements to the market.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MesslokationsAenderung {
    /// The Messlokation whose technology is to change.
    pub melo_id: String,
    /// Who ordered it — it picks between the NB and the LF tree.
    pub besteller: Besteller,
    /// Whether this MSB offers the requested Leistung per its Preisblatt.
    pub leistung_im_preisblatt: Option<bool>,
    /// Whether the Besteller is entitled to request this product.
    ///
    /// For an LF-initiated change the entitlement runs through the
    /// Marktlokations-Zuordnung or a Vollmacht — see [`TechnikAnfrage`].
    pub besteller_berechtigt: Option<bool>,
    /// Whether the requested technology is already installed.
    pub technik_liegt_vor: Option<bool>,
    /// Whether the requested technology is possible at this Lokation at all.
    pub technik_moeglich: Option<bool>,
}

/// An inbound REQOTE 35005 „Anfrage Angebot Änderung Technik" (AWH only).
///
/// The Vollmacht Prüfschritte are LF-only in the published tree, and they come
/// **before** the Technik questions — which is why `E_0281` numbers its codes
/// differently from `E_0278` rather than extending it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TechnikAnfrage {
    /// The change being priced.
    pub aenderung: MesslokationsAenderung,
    /// Whether the LF is the Marktlokation's assigned Lieferant in the
    /// requested Umsetzungszeitraum (`E_0281` Prüfschritt 10). Ignored for an
    /// NB-initiated Anfrage.
    pub lf_ist_zugeordnet: Option<bool>,
    /// Whether a Vollmacht des Letztverbrauchers bzw. Erzeugers is on file
    /// (`E_0281` Prüfschritt 20). Only read when the LF is not assigned.
    pub vollmacht_liegt_vor: Option<bool>,
    /// Whether that Vollmacht is plausible and valid (`E_0281` Prüfschritt 30).
    pub vollmacht_gueltig: Option<bool>,
}

/// An inbound ORDERS 17011 „Bestellung Angebot Änderung Technik".
///
/// [`Self::art`] is what selects `E_0249`/`E_0250` from `E_0279`/`E_0283` —
/// read it off the ORDERS' Zuordnung zu einem Objekt, never guessed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TechnikBestellung {
    /// The change being ordered.
    pub aenderung: MesslokationsAenderung,
    /// Whether the ORDERS opens the Vorgang (WiM Teil 1) or answers one (AWH).
    pub art: Beauftragungsart,
    /// `SG?` — the requested Änderungstermin resp. the start of the
    /// Umsetzungszeitraum.
    pub gewuenschter_termin: Date,
    /// Whether this MSB is still the assigned MSB at that date
    /// (`E_0279`/`E_0283` Prüfschritt 20). Not asked by the WiM Teil 1 trees.
    pub noch_zugeordnet: Option<bool>,
    /// Whether the change can be realised inside the requested
    /// Umsetzungszeitraum (`E_0279`/`E_0283` Prüfschritt 40).
    pub im_zeitraum_realisierbar: Option<bool>,
    /// Whether the Preisblatt version the order names is acceptable
    /// (`E_0279`/`E_0283` Prüfschritt 50).
    pub preisblatt_version_akzeptiert: Option<bool>,
}

/// Why a Messlokationsänderung failed on site — the input to `E_0286`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TechnikDurchfuehrung {
    /// Prüfschritt 60 „nein" — the change was made. No code; the MSB sends a
    /// Stammdatenänderung instead.
    Erfolgreich,
    /// Prüfschritt 10 — the Monteur could not get to the Lokation.
    KeinZugang,
    /// Prüfschritt 30 — no usable Kommunikationsverbindung.
    KeineKommunikationsverbindung,
    /// Prüfschritt 50 — the ordered SR/NeLo→TR mapping cannot be realised on
    /// site.
    MappingNichtUmsetzbar,
    /// Prüfschritt 60 „ja" — anything else; the answer must describe it.
    Sonstiges,
}

/// One Prüfschritt: a yes/no fact, the answer it triggers, and what to say when
/// it is not established.
///
/// Every step in this family has the same shape — read a fact, refuse on one
/// polarity, escalate when it is unknown, otherwise walk on — and writing it
/// out per step buried the trees under boilerplate. `Ok(())` means „continue".
///
/// The `None` arm is never a default. Refusing a lawful order and confirming an
/// impossible one are both binding statements to the market, so an unread fact
/// stops the walk and asks an operator.
// `MsbEntscheidung` is the crate's answer type and carries the BDEW wording;
// boxing it here to satisfy `result_large_err` would put an allocation on the
// hot path of a pure decision function and hide the type at every call site.
#[allow(clippy::result_large_err)]
fn schritt(
    fact: Option<bool>,
    refuse_when: bool,
    tree: &'static str,
    code: &'static str,
    pruefschritt: u16,
    detail: impl FnOnce() -> String,
    unbekannt: impl FnOnce() -> String,
) -> Result<(), MsbEntscheidung> {
    match fact {
        Some(v) if v == refuse_when => Err(MsbEntscheidung::Reject(RejectReason::new(
            tree,
            lookup(tree, code).expect("code is published in the resolved tree"),
            pruefschritt,
            detail(),
        ))),
        Some(_) => Ok(()),
        None => Err(MsbEntscheidung::Escalate {
            reason: unbekannt(),
        }),
    }
}

/// Decide the MSB's answer to a REQOTE 35005 Anfrage.
///
/// A `None` return is „Angebot versenden": `E_0278`/`E_0281` publish
/// Ablehnungscodes only, and the agreement is the QUOTES 15005 itself
/// (see [`crate::codes::ABLEHNUNG_ONLY_TREES`]). A `Some` rides IFTSTA 21033.
///
/// # Panics
///
/// Only if the tree's Codeliste is missing a code this function names, which a
/// test in this module rules out.
// One block per published Prüfschritt, in the tree's own order. Splitting it
// would break the line-by-line correspondence with `E_0278`/`E_0281`, which is
// what makes the walk auditable against the EBD.
#[allow(clippy::too_many_lines, clippy::result_large_err)]
#[must_use]
pub fn pruefe_technik_anfrage(anfrage: &TechnikAnfrage) -> Option<MsbEntscheidung> {
    let a = &anfrage.aenderung;
    let tree = technik_anfrage_baum(a.besteller);
    let melo = || a.melo_id.clone();
    let lf = a.besteller == Besteller::Lieferant;

    // The published Prüfschritt order differs by Besteller, and so do the code
    // numbers. `E_0281` asks the Vollmacht first (10–30) and only then the
    // Leistung (40) and the Berechtigung (50); `E_0278` has no Vollmacht step
    // and opens at the Leistung (10). Walking one order for both would emit
    // codes out of the tree's own sequence under the other tree's spelling.
    let (leistung, berechtigung, liegt_vor, moeglich) = if lf {
        ("A05", "A06", "A03", "A04")
    } else {
        ("A03", "A04", "A01", "A02")
    };
    let nr = |lf_schritt: u16, nb_schritt: u16| if lf { lf_schritt } else { nb_schritt };

    let walk = || -> Result<(), MsbEntscheidung> {
        if lf {
            // Prüfschritt 10 — a LF that supplies the Marktlokation needs no
            // Vollmacht; one that does not must produce a valid one.
            let Some(zugeordnet) = anfrage.lf_ist_zugeordnet else {
                return Err(MsbEntscheidung::Escalate {
                    reason: format!(
                        "Zuordnung des anfragenden LF zur Marktlokation der Messlokation {} im \
                         gewünschten Umsetzungszeitraum ist nicht feststellbar \
                         (E_0281 Prüfschritt 10)",
                        melo()
                    ),
                });
            };
            if !zugeordnet {
                schritt(
                    anfrage.vollmacht_liegt_vor,
                    false,
                    tree,
                    "A01",
                    20,
                    || {
                        format!(
                            "Der LF ist der Marktlokation der Messlokation {} nicht zugeordnet \
                             und es liegt keine Vollmacht des Letztverbrauchers bzw. Erzeugers vor",
                            melo()
                        )
                    },
                    || {
                        format!(
                            "Ob dem MSB eine Vollmacht für die Messlokation {} vorliegt, ist \
                             nicht feststellbar (E_0281 Prüfschritt 20)",
                            melo()
                        )
                    },
                )?;
                schritt(
                    anfrage.vollmacht_gueltig,
                    false,
                    tree,
                    "A02",
                    30,
                    || {
                        format!(
                            "Die Vollmacht für die Messlokation {} ist nicht plausibel und gültig",
                            melo()
                        )
                    },
                    || {
                        format!(
                            "Gültigkeit und Plausibilität der Vollmacht für die Messlokation {} \
                             sind nicht feststellbar (E_0281 Prüfschritt 30)",
                            melo()
                        )
                    },
                )?;
            }
        }

        schritt(
            a.leistung_im_preisblatt,
            false,
            tree,
            leistung,
            nr(40, 10),
            || "Der MSB bietet die angefragte Leistung nicht gemäß Preisblatt an".to_owned(),
            || {
                format!(
                    "Ob der MSB die angefragte Leistung gemäß Preisblatt anbietet, ist nicht \
                     feststellbar (Messlokation {}, {tree})",
                    melo()
                )
            },
        )?;
        schritt(
            a.besteller_berechtigt,
            false,
            tree,
            berechtigung,
            nr(50, 20),
            || "Der Besteller hat keine Berechtigung, dieses Produkt anzufragen".to_owned(),
            || {
                format!(
                    "Berechtigung des Bestellers für dieses Produkt ist nicht feststellbar \
                     (Messlokation {}, {tree})",
                    melo()
                )
            },
        )?;
        schritt(
            a.technik_liegt_vor,
            true,
            tree,
            liegt_vor,
            nr(60, 30),
            || {
                format!(
                    "Die angefragte Technik liegt an der Messlokation {} bereits vor",
                    melo()
                )
            },
            || {
                format!(
                    "Bestand der angefragten Technik an der Messlokation {} ist nicht \
                     feststellbar ({tree})",
                    melo()
                )
            },
        )?;
        schritt(
            a.technik_moeglich,
            false,
            tree,
            moeglich,
            nr(70, 40),
            || {
                format!(
                    "Die gewünschte Technik ist an der Messlokation {} nicht möglich",
                    melo()
                )
            },
            || {
                format!(
                    "Machbarkeit der gewünschten Technik an der Messlokation {} ist nicht \
                     feststellbar ({tree})",
                    melo()
                )
            },
        )
    };

    // `Ok` is „Angebot versenden" — the QUOTES 15005 is the answer and carries
    // no code.
    walk().err()
}

/// Decide the MSB's answer to an ORDERS 17011 Bestellung.
///
/// The tree is [`aenderung_der_technik_baum`]`(besteller, art)` — both inputs,
/// always. See the module note for why the Marktrolle alone is not enough.
///
/// # Panics
///
/// Only if the resolved tree's Codeliste is missing a code this function names,
/// which a test in this module rules out.
// One block per published Prüfschritt across four trees — see the note on
// [`pruefe_technik_anfrage`].
#[allow(clippy::too_many_lines, clippy::result_large_err)]
#[must_use]
pub fn pruefe_technik_bestellung(
    bestellung: &TechnikBestellung,
    eingangsdatum: Date,
    cal: HolidayCalendar,
) -> MsbEntscheidung {
    let a = &bestellung.aenderung;
    let tree = aenderung_der_technik_baum(a.besteller, bestellung.art);
    let code = |c: &str| lookup(tree, c).expect("code is published in the Technik-Bestellung tree");
    let esc = |reason: String| MsbEntscheidung::Escalate { reason };

    match bestellung.art {
        // ── WiM Teil 1 Kap. 3.3 — one Prüfschritt, the Vorlauffrist ───────────
        //
        // `E_0250` adds the Vollmacht steps 20/30 in front of it; those are
        // decided at [`pruefe_technik_anfrage`]-time facts, carried here on the
        // same struct.
        Beauftragungsart::DirekteBeauftragung => {
            if a.besteller == Besteller::Lieferant {
                match a.besteller_berechtigt {
                    Some(false) => {
                        return MsbEntscheidung::Reject(RejectReason::new(
                            tree,
                            code("A03"),
                            20,
                            format!(
                                "Vollmacht des Letztverbrauchers bzw. Erzeugers für die \
                                 Messlokation {} liegt nicht vor",
                                a.melo_id
                            ),
                        ));
                    }
                    None => {
                        return esc(format!(
                            "Berechtigung des LF für die Messlokation {} ist nicht feststellbar \
                             (E_0250 Prüfschritt 20)",
                            a.melo_id
                        ));
                    }
                    Some(true) => {}
                }
            }
            match VorlaufShape::LatestWerktageBefore(AENDERUNG_VORLAUF_WT).check(
                eingangsdatum,
                bestellung.gewuenschter_termin,
                cal,
            ) {
                VorlaufVerdict::TooLate { .. } => MsbEntscheidung::Reject(RejectReason::new(
                    tree,
                    code("A01"),
                    if a.besteller == Besteller::Lieferant {
                        40
                    } else {
                        1
                    },
                    format!(
                        "Frist nicht eingehalten — das gewünschte Änderungsdatum {} liegt \
                         weniger als {AENDERUNG_VORLAUF_WT} Werktage nach dem \
                         Nachrichteneingang {eingangsdatum}",
                        bestellung.gewuenschter_termin
                    ),
                )),
                VorlaufVerdict::Ok | VorlaufVerdict::TooEarly { .. } => {
                    MsbEntscheidung::accept(tree, code("A02"))
                }
            }
        }

        // ── AWH Kap. 9.1.2 / 9.2.2 — six Prüfschritte, `A06` is the yes ───────
        //
        // No Vorlauffrist step at all: the Umsetzungszeitraum was agreed in the
        // Angebot, so applying `E_0249`'s 20 Werktage here refuses a lawful
        // order.
        Beauftragungsart::BestellungNachAngebot => {
            let melo = || a.melo_id.clone();
            let termin = bestellung.gewuenschter_termin;
            let walk = || -> Result<(), MsbEntscheidung> {
                schritt(
                    a.technik_liegt_vor,
                    true,
                    tree,
                    "A01",
                    10,
                    || {
                        format!(
                            "Die bestellte Technik liegt an der Messlokation {} bereits vor",
                            melo()
                        )
                    },
                    || {
                        format!(
                            "Bestand der bestellten Technik an der Messlokation {} ist nicht \
                             feststellbar ({tree} Prüfschritt 10)",
                            melo()
                        )
                    },
                )?;
                schritt(
                    bestellung.noch_zugeordnet,
                    false,
                    tree,
                    "A02",
                    20,
                    || {
                        format!(
                            "Der MSB ist der Messlokation {} zum Beginn des Umsetzungszeitraums \
                             ({termin}) nicht mehr zugeordnet",
                            melo()
                        )
                    },
                    || {
                        format!(
                            "Fortbestand der eigenen Zuordnung zur Messlokation {} zum {termin} \
                             ist nicht feststellbar ({tree} Prüfschritt 20)",
                            melo()
                        )
                    },
                )?;
                schritt(
                    a.technik_moeglich,
                    false,
                    tree,
                    "A03",
                    30,
                    || {
                        format!(
                            "Die gewünschte Technik ist an der Messlokation {} nicht möglich",
                            melo()
                        )
                    },
                    || {
                        format!(
                            "Machbarkeit der gewünschten Technik an der Messlokation {} ist \
                             nicht feststellbar ({tree} Prüfschritt 30)",
                            melo()
                        )
                    },
                )?;
                schritt(
                    bestellung.im_zeitraum_realisierbar,
                    false,
                    tree,
                    "A04",
                    40,
                    || {
                        "Realisierung ist im gewünschten Umsetzungszeitraum nicht möglich — ein \
                         alternativer Umsetzungszeitraum ist anzugeben"
                            .to_owned()
                    },
                    || {
                        format!(
                            "Realisierbarkeit im gewünschten Umsetzungszeitraum ist für die \
                             Messlokation {} nicht feststellbar ({tree} Prüfschritt 40)",
                            melo()
                        )
                    },
                )?;
                schritt(
                    bestellung.preisblatt_version_akzeptiert,
                    false,
                    tree,
                    "A05",
                    50,
                    || {
                        "Das angegebene Preisblatt kann in der angegebenen Version nicht \
                         akzeptiert werden"
                            .to_owned()
                    },
                    || {
                        format!(
                            "Die in der Bestellung genannte Preisblatt-Version ist nicht prüfbar \
                             (Messlokation {}, {tree} Prüfschritt 50)",
                            melo()
                        )
                    },
                )
            };
            walk().map_or_else(|e| e, |()| MsbEntscheidung::accept(tree, code("A06")))
        }
    }
}

/// The `E_0286` code for how a Messlokationsänderung ended.
///
/// `None` on [`TechnikDurchfuehrung::Erfolgreich`]: the tree's „nein" branch at
/// Prüfschritt 60 publishes no code and says „Stammdatenänderung vom MSB
/// ausgehend versenden" instead. A caller that renders `None` as an IFTSTA
/// would report a failure that did not happen.
///
/// The Ablehnung rides IFTSTA **21027** toward the NB and **21025** toward the
/// LF — [`durchfuehrung_pid`].
///
/// # Panics
///
/// Only if the `E_0286` Codeliste is missing a code this function names, which
/// a test in this module rules out.
#[must_use]
pub fn melde_technik_durchfuehrung(
    ergebnis: TechnikDurchfuehrung,
    melo_id: &str,
) -> Option<MsbEntscheidung> {
    let tree = EBD_TECHNIK_DURCHFUEHRUNG;
    let code = |c: &str| lookup(tree, c).expect("code is published in E_0286");
    let (c, schritt, detail): (&str, u16, String) = match ergebnis {
        TechnikDurchfuehrung::Erfolgreich => return None,
        TechnikDurchfuehrung::KeinZugang => (
            "A01",
            10,
            format!("Der Monteur hatte keinen Zugang zur Messlokation {melo_id}"),
        ),
        TechnikDurchfuehrung::KeineKommunikationsverbindung => (
            "A02",
            30,
            format!(
                "Die Änderung der Technik an der Messlokation {melo_id} ist aufgrund einer \
                 fehlenden oder mangelhaften Kommunikationsverbindung nicht möglich"
            ),
        ),
        TechnikDurchfuehrung::MappingNichtUmsetzbar => (
            "A03",
            50,
            format!(
                "Die bestellte Zuordnung der genannten TR zur SR/NeLo lässt sich an der \
                 Messlokation {melo_id} vor Ort nicht realisieren"
            ),
        ),
        TechnikDurchfuehrung::Sonstiges => (
            "A99",
            60,
            format!(
                "Die Änderung an der Messlokation {melo_id} ist aus einem zuvor nicht \
                 spezifizierten Grund gescheitert — der Grund ist in der Antwort zu benennen"
            ),
        ),
    };
    Some(MsbEntscheidung::Reject(RejectReason::new(
        tree,
        code(c),
        schritt,
        detail,
    )))
}

/// The IFTSTA Prüfidentifikator an `E_0286` Scheitermeldung rides.
///
/// **21027** to the Netzbetreiber, **21025** to the Lieferant — the recipient
/// decides, not the reason (PID-Übersicht 4.0 lfd. Nr. 30710 and 30770).
#[must_use]
pub const fn durchfuehrung_pid(besteller: Besteller) -> u32 {
    match besteller {
        Besteller::Netzbetreiber => 21_027,
        Besteller::Lieferant => 21_025,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Month;

    const CAL: HolidayCalendar = HolidayCalendar::BdewMaKo;

    fn d(y: i32, m: Month, day: u8) -> Date {
        Date::from_calendar_date(y, m, day).expect("valid date")
    }

    fn aenderung(besteller: Besteller) -> MesslokationsAenderung {
        MesslokationsAenderung {
            melo_id: "DE0000000001234567890000000000001".to_owned(),
            besteller,
            leistung_im_preisblatt: Some(true),
            besteller_berechtigt: Some(true),
            technik_liegt_vor: Some(false),
            technik_moeglich: Some(true),
        }
    }

    fn bestellung(besteller: Besteller, art: Beauftragungsart) -> TechnikBestellung {
        TechnikBestellung {
            aenderung: aenderung(besteller),
            art,
            gewuenschter_termin: d(2026, Month::October, 1),
            noch_zugeordnet: Some(true),
            im_zeitraum_realisierbar: Some(true),
            preisblatt_version_akzeptiert: Some(true),
        }
    }

    /// The finding this module exists for: four trees on one answer PID pair,
    /// and the Marktrolle alone resolves only half of it.
    #[test]
    fn the_ordrsp_pair_resolves_to_four_distinct_trees() {
        use Beauftragungsart::{BestellungNachAngebot, DirekteBeauftragung};
        use Besteller::{Lieferant, Netzbetreiber};
        let trees = [
            aenderung_der_technik_baum(Netzbetreiber, DirekteBeauftragung),
            aenderung_der_technik_baum(Lieferant, DirekteBeauftragung),
            aenderung_der_technik_baum(Netzbetreiber, BestellungNachAngebot),
            aenderung_der_technik_baum(Lieferant, BestellungNachAngebot),
        ];
        assert_eq!(trees, ["E_0249", "E_0250", "E_0279", "E_0283"]);
        let mut sorted = trees.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 4, "the four trees must stay distinct");
    }

    /// `A02` is the Zustimmung in `E_0249` and an Ablehnung in `E_0279`. If the
    /// resolution ever collapses, this is the assertion that catches it before
    /// a confirmation goes out reading as a refusal.
    #[test]
    fn a02_means_opposite_things_in_the_two_nb_trees() {
        let direkt = lookup("E_0249", "A02").expect("E_0249 publishes A02");
        let awh = lookup("E_0279", "A02").expect("E_0279 publishes A02");
        assert_eq!(direkt.ist_zustimmung(), Some(true));
        assert_eq!(awh.ist_zustimmung(), Some(false));
    }

    #[test]
    fn a_compliant_direct_beauftragung_is_a02() {
        let b = bestellung(
            Besteller::Netzbetreiber,
            Beauftragungsart::DirekteBeauftragung,
        );
        let uet = mako_fristen::sub_werktage(b.gewuenschter_termin, AENDERUNG_VORLAUF_WT, CAL);
        let e = pruefe_technik_bestellung(&b, uet, CAL);
        assert_eq!(e.antwortcode(), Some("A02"));
        assert_eq!(e.ebd(), Some("E_0249"));
    }

    #[test]
    fn a_short_lead_time_on_a_direct_beauftragung_is_a01() {
        let b = bestellung(
            Besteller::Netzbetreiber,
            Beauftragungsart::DirekteBeauftragung,
        );
        let uet = mako_fristen::sub_werktage(b.gewuenschter_termin, 5, CAL);
        assert_eq!(
            pruefe_technik_bestellung(&b, uet, CAL).antwortcode(),
            Some("A01")
        );
    }

    /// The AWH Bestellung confirms with `A06`, not `A02` — the same message
    /// shape, a different alphabet.
    #[test]
    fn a_compliant_awh_bestellung_is_a06() {
        for besteller in [Besteller::Netzbetreiber, Besteller::Lieferant] {
            let b = bestellung(besteller, Beauftragungsart::BestellungNachAngebot);
            let e = pruefe_technik_bestellung(&b, d(2026, Month::January, 5), CAL);
            assert_eq!(e.antwortcode(), Some("A06"), "besteller={besteller:?}");
            assert_eq!(e.ist_zustimmung(), Some(true));
        }
    }

    /// The AWH tree has no Vorlauffrist Prüfschritt at all — the corridor was
    /// agreed in the Angebot. Applying `E_0249`'s 20 WT here would refuse a
    /// lawful order.
    #[test]
    fn the_awh_bestellung_has_no_vorlauffrist_step() {
        let b = bestellung(
            Besteller::Netzbetreiber,
            Beauftragungsart::BestellungNachAngebot,
        );
        let tomorrow = d(2026, Month::September, 30);
        assert_eq!(
            pruefe_technik_bestellung(&b, tomorrow, CAL).antwortcode(),
            Some("A06")
        );
    }

    #[test]
    fn an_unassigned_msb_refuses_the_awh_bestellung_with_a02() {
        let mut b = bestellung(
            Besteller::Netzbetreiber,
            Beauftragungsart::BestellungNachAngebot,
        );
        b.noch_zugeordnet = Some(false);
        let e = pruefe_technik_bestellung(&b, d(2026, Month::January, 5), CAL);
        assert_eq!(e.antwortcode(), Some("A02"));
        assert_eq!(e.ist_zustimmung(), Some(false));
    }

    #[test]
    fn an_unpriced_leistung_refuses_the_anfrage_per_besteller_alphabet() {
        for (besteller, want) in [
            (Besteller::Netzbetreiber, "A03"),
            (Besteller::Lieferant, "A05"),
        ] {
            let mut a = aenderung(besteller);
            a.leistung_im_preisblatt = Some(false);
            let anfrage = TechnikAnfrage {
                aenderung: a,
                lf_ist_zugeordnet: Some(true),
                vollmacht_liegt_vor: None,
                vollmacht_gueltig: None,
            };
            let e = pruefe_technik_anfrage(&anfrage).expect("a refusal");
            assert_eq!(e.antwortcode(), Some(want), "besteller={besteller:?}");
        }
    }

    /// „Angebot versenden" carries no code — the QUOTES 15005 is the agreement.
    #[test]
    fn a_servable_anfrage_returns_no_code() {
        let anfrage = TechnikAnfrage {
            aenderung: aenderung(Besteller::Netzbetreiber),
            lf_ist_zugeordnet: None,
            vollmacht_liegt_vor: None,
            vollmacht_gueltig: None,
        };
        assert!(pruefe_technik_anfrage(&anfrage).is_none());
    }

    /// An LF that is the Marktlokation's assigned Lieferant needs no Vollmacht;
    /// one that is not needs a valid one.
    #[test]
    fn the_lf_vollmacht_is_only_asked_when_the_lf_is_not_assigned() {
        let base = |zugeordnet, vorhanden| TechnikAnfrage {
            aenderung: aenderung(Besteller::Lieferant),
            lf_ist_zugeordnet: Some(zugeordnet),
            vollmacht_liegt_vor: vorhanden,
            vollmacht_gueltig: vorhanden,
        };
        assert!(pruefe_technik_anfrage(&base(true, None)).is_none());
        assert_eq!(
            pruefe_technik_anfrage(&base(false, Some(false)))
                .expect("a refusal")
                .antwortcode(),
            Some("A01")
        );
    }

    #[test]
    fn a_successful_durchfuehrung_publishes_no_code() {
        assert!(melde_technik_durchfuehrung(TechnikDurchfuehrung::Erfolgreich, "X").is_none());
    }

    #[test]
    fn each_failure_mode_maps_to_its_published_code() {
        for (ergebnis, want) in [
            (TechnikDurchfuehrung::KeinZugang, "A01"),
            (TechnikDurchfuehrung::KeineKommunikationsverbindung, "A02"),
            (TechnikDurchfuehrung::MappingNichtUmsetzbar, "A03"),
            (TechnikDurchfuehrung::Sonstiges, "A99"),
        ] {
            let e = melde_technik_durchfuehrung(ergebnis, "X").expect("a Scheitermeldung");
            assert_eq!(e.antwortcode(), Some(want));
            assert_eq!(e.ebd(), Some(EBD_TECHNIK_DURCHFUEHRUNG));
        }
    }

    #[test]
    fn the_scheitermeldung_pid_follows_the_recipient() {
        assert_eq!(durchfuehrung_pid(Besteller::Netzbetreiber), 21_027);
        assert_eq!(durchfuehrung_pid(Besteller::Lieferant), 21_025);
    }

    #[test]
    fn every_named_code_is_published() {
        for (tree, codes) in [
            ("E_0278", &["A01", "A02", "A03", "A04", "A99"][..]),
            (
                "E_0281",
                &["A01", "A02", "A03", "A04", "A05", "A06", "A99"][..],
            ),
            (
                "E_0279",
                &["A01", "A02", "A03", "A04", "A05", "A06", "A99"][..],
            ),
            (
                "E_0283",
                &["A01", "A02", "A03", "A04", "A05", "A06", "A99"][..],
            ),
            ("E_0286", &["A01", "A02", "A03", "A99"][..]),
            ("E_0249", &["A01", "A02"][..]),
            ("E_0250", &["A01", "A02", "A03", "A04"][..]),
        ] {
            for c in codes {
                assert!(lookup(tree, c).is_some(), "{c} left the {tree} Codeliste");
            }
        }
    }

    /// `E_0284`/`E_0285`/`E_0287` print one sentence and delegate. A caller
    /// holding one of them must land on a real Codeliste.
    #[test]
    fn the_durchfuehrung_aliases_resolve_to_one_tree() {
        use crate::codes::EBD_TECHNIK_DURCHFUEHRUNG_ALIASES as ALIASES;
        assert_eq!(ALIASES, ["E_0284", "E_0285", "E_0287"]);
        for a in ALIASES {
            assert!(
                lookup(a, "A01").is_none(),
                "{a} must not carry its own Codeliste — it delegates to E_0286"
            );
        }
    }
}

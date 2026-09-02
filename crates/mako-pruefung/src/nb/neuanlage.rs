//! The NB's **Neuanlage** decision — `E_0608` „Anmeldung einer Zuordnung".
//!
//! A Lieferant registers a Marktlokation that is being commissioned for the
//! first time (UTILMD **55600** verbrauchend / **55601** erzeugend, GPKE Teil 2
//! § 2.2). The NB answers 55602/55604 or 55603/55605.
//!
//! # The tree has a third outcome
//!
//! Prüfschritte 110 and 590 are a **loop, not a refusal**: an Anmeldung whose
//! Marktlokation cannot yet be identified is re-checked *daily* and may only be
//! answered `A07` / `A16` once it has been open for **more than 60 Werktage**.
//! That is why GPKE Teil 2 § 2.2.2 states the answer window as „spätester ÜZ ist
//! 00:00 Uhr des 61. WT nach dem ÜT" and why this module returns
//! [`NeuanlageEntscheidung::Vertagen`] alongside Accept / Reject / Escalate. A
//! two-outcome engine has to call the unidentifiable case *something*, and both
//! available answers are wrong: refusing breaks § 20 EnWG, confirming assigns a
//! Lieferant to a Marktlokation the NB cannot find.
//!
//! # Two branches, two code spaces
//!
//! Prüfschritt 10 splits them and they share nothing:
//!
//! | Prüfschritt | Question | verbrauchend | erzeugend |
//! |---|---|---|---|
//! | 20 / 500 | Vorlauffrist eingehalten | `A01` | `A10` |
//! | 40 / 520 | nimmt an der Marktkommunikation teil | `A02` | `A11` |
//! | 55 / 535 | Keine- oder Mehrfachidentifizierung | `A08` | `A17` |
//! | 60 / 540 | erstmalige Inbetriebnahme | `A03` | `A12` |
//! | 545 | viertelstündliche Messtechnik | — | `A19` |
//! | 70 / 550 | bereits ein LF zugeordnet | `A04` | `A13` |
//! | 80 / 560 | im Netzgebiet des NB | `A05` | `A14` |
//! | 90 / 570 | zwingend notwendige Anforderungen | `A06` | `A15` |
//! | 110 / 590 | länger als 60 WT offen | `A07` | `A16` |
//! | 130 / 610 | Zustimmung | `A09` | `A18` |
//!
//! # Vorlauffrist (Prüfschritt 20 / 500)
//!
//! GPKE Teil 2 § 2.2.2 Nr. 1 states two:
//!
//! - **Direktvermarktung ab Inbetriebnahmedatum**: „spätester ÜT liegt 1 Monat
//!   vor dem voraussichtlichen Zuordnungsbeginn."
//! - **Alle anderen Marktlokationen und Tranchen**: „spätester ÜT ist der Tag
//!   vor dem letzten WT vor dem voraussichtlichen Zuordnungsbeginn."
//!
//! On a Neuanlage the assignment always starts at commissioning, so an
//! erzeugende Marktlokation entering a Direktvermarktungsform takes the month.
//!
//! # Sources
//!
//! - BK6-24-174 GPKE Teil 2 § 2.2 (UC/SD Neuanlage)
//! - Entscheidungsbaum-Diagramme und Codelisten 4.3, Kap. 6.5.1 (`E_0608`)

use time::Date;

use crate::codes::{self, AntwortCode, EBD_NEUANLAGE};

use super::anmeldung::{has_werktag_strictly_between, months_before};
use super::config::NetzCheckConfig;
use super::types::{
    AntwortDetail, ErzeugungsAnmeldung, Marktlokationsart, RejectReason, Veraeusserungsform,
};

/// How many Werktage the NB re-checks an unidentifiable Neuanlage before it may
/// refuse it (`E_0608` Prüfschritte 110 / 590).
pub const IDENTIFIKATION_WERKTAGE: u32 = 60;

// ── Inputs ────────────────────────────────────────────────────────────────────

/// What the NB's identification run produced — `E_0608` Prüfschritte 30 / 50 /
/// 55 (and 510 / 530 / 535).
///
/// The NB identifies a newly commissioned Marktlokation from the address and
/// device data the Anmeldung carries, against its own NIS/GIS. That is not a
/// market-communication lookup and not something this crate can do, so the
/// caller supplies the outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identifikation {
    /// Exactly one Marktlokation matched.
    Eindeutig {
        /// The MaLo-ID the NB assigned.
        malo_id: String,
    },
    /// Several matched. `teilnehmende` counts how many of them take part in
    /// market communication — the tree accepts the case only when that is
    /// exactly one.
    Mehrdeutig {
        /// How many of the matches participate in Marktkommunikation.
        teilnehmende: u32,
    },
    /// Nothing matched. The tree's retry path.
    Keine,
}

/// The NB's own facts about the identified Marktlokation.
///
/// Every field answers one Prüfschritt. `None` for the whole struct means the
/// identification has not produced a Marktlokation to look up yet.
///
/// Six booleans, deliberately: `E_0608` asks six yes/no questions and each has
/// its own Antwortcode. Grouping them into a bitflag or a state enum would make
/// the mapping from field to Prüfschritt — the thing an auditor checks — a
/// lookup instead of a name.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct NeuanlageBefund {
    /// Prüfschritt 40 / 520. „Nicht teilnehmend" is stillgelegt or
    /// Modell-2-zugeordnet.
    pub nimmt_an_mako_teil: bool,
    /// Prüfschritt 60 / 540 — is this really a first commissioning?
    pub erstmalige_inbetriebnahme: bool,
    /// Prüfschritt 70 / 550.
    pub lf_bereits_zugeordnet: bool,
    /// Prüfschritt 80 / 560 — still in this NB's Netzgebiet at receipt.
    pub im_netzgebiet: bool,
    /// Prüfschritt 90 / 570 — insbesondere die Zuordnungsermächtigung.
    pub anforderungen_erfuellt: bool,
    /// Prüfschritt 545, erzeugende Marktlokation only: viertelstündliche
    /// Messtechnik an allen relevanten Messeinrichtungen.
    pub viertelstundenmessung: bool,
}

/// Parsed fields of an inbound 55600 / 55601.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeuanlageAnfrage {
    /// `55600` verbrauchende, `55601` erzeugende Marktlokation.
    pub pid: u32,
    /// Which branch of `E_0608` answers.
    pub marktlokationsart: Marktlokationsart,
    /// MP-ID of the registering Lieferant.
    pub lf_mp_id: String,
    /// The voraussichtlicher Zuordnungsbeginn the LF named.
    pub zuordnungsbeginn: Date,
    /// The Übertragungstag — when the Anmeldung arrived. The 60-Werktage
    /// Prüflauf counts from it, so it is a field and not `now`.
    pub uebertragungstag: Date,
    /// The Veräußerungsform facts, on an erzeugende Marktlokation.
    pub erzeugung: Option<ErzeugungsAnmeldung>,
}

// ── Outcome ───────────────────────────────────────────────────────────────────

/// Outcome of `E_0608`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NeuanlageEntscheidung {
    /// Every applicable Prüfschritt passed — `A09` / `A18`.
    Accept(AntwortDetail),
    /// A Prüfschritt refused.
    Reject(RejectReason),
    /// Prüfschritte 110 / 590: not identifiable yet, and the 60-Werktage window
    /// has not run out. **Answer nothing today** and re-check tomorrow.
    Vertagen {
        /// The last day on which a refusal would still be premature.
        letzter_pruefungstag: Date,
        /// Werktage still left in the window.
        verbleibende_werktage: u32,
    },
    /// A Prüfschritt needs a fact the caller did not supply.
    Escalate {
        /// What is missing, for the operator queue.
        reason: String,
    },
}

impl NeuanlageEntscheidung {
    /// The Antwortcode this decision puts on the wire, where it has one.
    #[must_use]
    pub fn antwortcode(&self) -> Option<&str> {
        match self {
            Self::Accept(a) => Some(&a.antwortcode),
            Self::Reject(r) => Some(&r.antwort.antwortcode),
            Self::Vertagen { .. } | Self::Escalate { .. } => None,
        }
    }

    /// `true` when the NB must answer nothing today.
    #[must_use]
    pub const fn is_vertagen(&self) -> bool {
        matches!(self, Self::Vertagen { .. })
    }
}

// ── Code helpers ──────────────────────────────────────────────────────────────

fn code(c: &'static str) -> &'static AntwortCode {
    codes::lookup(EBD_NEUANLAGE, c)
        .unwrap_or_else(|| panic!("{c} is not published by {EBD_NEUANLAGE} — see crate::codes"))
}

fn reject(c: &'static str, pruefschritt: u16, detail: String) -> NeuanlageEntscheidung {
    NeuanlageEntscheidung::Reject(RejectReason::new(
        EBD_NEUANLAGE,
        code(c),
        pruefschritt,
        detail,
    ))
}

/// The code pair for a Prüfschritt, by branch.
struct Codes {
    vorlauffrist: (&'static str, u16),
    mako: (&'static str, u16),
    identifikation: (&'static str, u16),
    inbetriebnahme: (&'static str, u16),
    lf_zugeordnet: (&'static str, u16),
    netzgebiet: (&'static str, u16),
    anforderungen: (&'static str, u16),
    abgelaufen: (&'static str, u16),
    zustimmung: &'static str,
}

const VERBRAUCHEND: Codes = Codes {
    vorlauffrist: ("A01", 20),
    mako: ("A02", 40),
    identifikation: ("A08", 55),
    inbetriebnahme: ("A03", 60),
    lf_zugeordnet: ("A04", 70),
    netzgebiet: ("A05", 80),
    anforderungen: ("A06", 90),
    abgelaufen: ("A07", 110),
    zustimmung: "A09",
};

const ERZEUGEND: Codes = Codes {
    vorlauffrist: ("A10", 500),
    mako: ("A11", 520),
    identifikation: ("A17", 535),
    inbetriebnahme: ("A12", 540),
    lf_zugeordnet: ("A13", 550),
    netzgebiet: ("A14", 560),
    anforderungen: ("A15", 570),
    abgelaufen: ("A16", 590),
    zustimmung: "A18",
};

// ── evaluate ──────────────────────────────────────────────────────────────────

/// Walk `E_0608` for one inbound Neuanlage.
///
/// # Parameters
///
/// - `anfrage` — the parsed 55600 / 55601.
/// - `identifikation` — what the NB's identification run against its own
///   NIS/GIS produced today.
/// - `befund` — the NB's facts about the identified Marktlokation. `None` when
///   nothing was identified; ignored otherwise only if the tree does not reach
///   the questions it answers.
/// - `today` — the current date in German local time.
/// - `config` — the holiday calendar and the EEG lead.
///
/// # Returns
///
/// [`NeuanlageEntscheidung::Vertagen`] whenever the Marktlokation is not
/// identifiable and the 60-Werktage window is still open — the NB answers
/// nothing that day and re-runs the check tomorrow.
#[must_use]
pub fn evaluate_neuanlage(
    anfrage: &NeuanlageAnfrage,
    identifikation: &Identifikation,
    befund: Option<&NeuanlageBefund>,
    today: Date,
    config: &NetzCheckConfig,
) -> NeuanlageEntscheidung {
    let erzeugend = anfrage.marktlokationsart == Marktlokationsart::Erzeugend;
    let c = if erzeugend { ERZEUGEND } else { VERBRAUCHEND };

    // ── 20 / 500: Wurde die Vorlauffrist eingehalten? ────────────────────────
    if let Some(d) = check_vorlauffrist(anfrage, &c, *config) {
        return d;
    }

    // ── 30 / 510, 50 / 530, 55 / 535: Identifikation ─────────────────────────
    match identifikation {
        Identifikation::Mehrdeutig { teilnehmende } if *teilnehmende != 1 => {
            return reject(
                c.identifikation.0,
                c.identifikation.1,
                format!(
                    "Keine- oder Mehrfachidentifizierung: {teilnehmende} of the matched \
                     Marktlokationen take part in market communication; exactly one must."
                ),
            );
        }
        Identifikation::Keine => {
            // ── 110 / 590: „Ist die Anmeldung vor mehr als 60 WT eingegangen?"
            //
            // Nein → back to Prüfschritt 30: re-check tomorrow. The tree refuses
            // *only* once the window has run out, so a Neuanlage that is merely
            // not findable yet must not be answered at all.
            let letzter = mako_fristen::add_werktage(
                anfrage.uebertragungstag,
                IDENTIFIKATION_WERKTAGE,
                config.holiday_calendar,
            );
            if today > letzter {
                return reject(
                    c.abgelaufen.0,
                    c.abgelaufen.1,
                    format!(
                        "Neu angelegte Marktlokation konnte nicht identifiziert werden: the \
                         Anmeldung arrived {} and the {IDENTIFIKATION_WERKTAGE}-Werktage \
                         Prüflauf ended {letzter} (today {today}).",
                        anfrage.uebertragungstag
                    ),
                );
            }
            return NeuanlageEntscheidung::Vertagen {
                letzter_pruefungstag: letzter,
                verbleibende_werktage: werktage_between(today, letzter, *config),
            };
        }
        Identifikation::Eindeutig { .. } | Identifikation::Mehrdeutig { .. } => {}
    }

    befund_pruefschritte(anfrage, &c, befund, erzeugend)
}

/// `E_0608` Prüfschritte 40–130 / 520–610 — everything the tree asks about the
/// Marktlokation once it has been identified.
fn befund_pruefschritte(
    anfrage: &NeuanlageAnfrage,
    c: &Codes,
    befund: Option<&NeuanlageBefund>,
    erzeugend: bool,
) -> NeuanlageEntscheidung {
    let Some(b) = befund else {
        return NeuanlageEntscheidung::Escalate {
            reason: format!(
                "A Marktlokation was identified for the Neuanlage of LF {}, but the NB's \
                 facts about it (Marktkommunikationsteilnahme, erstmalige Inbetriebnahme, \
                 Netzgebiet, Zuordnungsermächtigung) were not supplied — {EBD_NEUANLAGE} \
                 Prüfschritte {}–{} cannot be answered.",
                anfrage.lf_mp_id, c.mako.1, c.anforderungen.1
            ),
        };
    };

    // ── 40 / 520: nimmt an der Marktkommunikation teil? ──────────────────────
    if !b.nimmt_an_mako_teil {
        return reject(
            c.mako.0,
            c.mako.1,
            "Identifizierte Marktlokation nimmt nicht an der Marktkommunikation teil; \
             weiterhin handelt es sich nicht um eine Neuanlage."
                .to_owned(),
        );
    }

    // ── 60 / 540: erstmalige Inbetriebnahme? ─────────────────────────────────
    if !b.erstmalige_inbetriebnahme {
        return reject(
            c.inbetriebnahme.0,
            c.inbetriebnahme.1,
            "Keine Neuanlage, falscher Anwendungsfall — die Marktlokation ist bereits in \
             Betrieb."
                .to_owned(),
        );
    }

    // ── 545: viertelstündliche Messtechnik (erzeugende Marktlokation) ────────
    if erzeugend && !b.viertelstundenmessung {
        return reject(
            "A19",
            545,
            "Es liegt nicht an allen Messeinrichtungen, die für die Energiemengenermittlung \
             der Marktlokation notwendig sind, die Messtechnik für eine viertelstündliche \
             Messung vor."
                .to_owned(),
        );
    }

    // ── 70 / 550: ist bereits ein LF zugeordnet? ─────────────────────────────
    if b.lf_bereits_zugeordnet {
        return reject(
            c.lf_zugeordnet.0,
            c.lf_zugeordnet.1,
            "Falscher Anwendungsfall — es ist bereits ein LF zugeordnet.".to_owned(),
        );
    }

    // ── 80 / 560: im Netzgebiet des NB? ──────────────────────────────────────
    if !b.im_netzgebiet {
        return reject(
            c.netzgebiet.0,
            c.netzgebiet.1,
            "Marktlokation befindet sich zum Eingangsdatum der Meldung nicht mehr im \
             Netzgebiet des NB."
                .to_owned(),
        );
    }

    // ── 90 / 570: zwingend notwendige Anforderungen erfüllt? ─────────────────
    if !b.anforderungen_erfuellt {
        return reject(
            c.anforderungen.0,
            c.anforderungen.1,
            "Anforderungen können nicht erfüllt werden — insbesondere fehlt die \
             Zuordnungsermächtigung (Bilanzkreis/Bilanzierungsverfahren). Die Abweichungen \
             sind zu benennen."
                .to_owned(),
        );
    }

    // ── 130 / 610: Zustimmung ────────────────────────────────────────────────
    NeuanlageEntscheidung::Accept(AntwortDetail::new(EBD_NEUANLAGE, code(c.zustimmung)))
}

/// Prüfschritt 20 / 500 — „Wurde die Vorlauffrist eingehalten?"
///
/// GPKE Teil 2 § 2.2.2 Nr. 1: a Direktvermarktung ab Inbetriebnahmedatum takes
/// a month, everything else the Tag-vor-dem-letzten-WT rule.
///
/// Measured against the **Übertragungstag**, not today: the Frist is a property
/// of the message, so a case re-evaluated on day 40 of the Prüflauf must reach
/// the same verdict it did on day one.
fn check_vorlauffrist(
    anfrage: &NeuanlageAnfrage,
    c: &Codes,
    config: NetzCheckConfig,
) -> Option<NeuanlageEntscheidung> {
    if direktvermarktung_ab_inbetriebnahme(anfrage) {
        let latest_ut = months_before(
            anfrage.zuordnungsbeginn,
            config.eeg_zuordnung_vorlauf_monate,
        );
        return (anfrage.uebertragungstag > latest_ut).then(|| {
            reject(
                c.vorlauffrist.0,
                c.vorlauffrist.1,
                format!(
                    "Vorlauffrist wurde nicht eingehalten: Zuordnungsbeginn {}, spätester ÜT \
                     {latest_ut} (1 Monat davor, GPKE Teil 2 § 2.2.2 Nr. 1 für die \
                     Direktvermarktung ab Inbetriebnahmedatum); ÜT war {}.",
                    anfrage.zuordnungsbeginn, anfrage.uebertragungstag
                ),
            )
        });
    }
    (!has_werktag_strictly_between(
        anfrage.uebertragungstag,
        anfrage.zuordnungsbeginn,
        config.holiday_calendar,
    ))
    .then(|| {
        reject(
            c.vorlauffrist.0,
            c.vorlauffrist.1,
            format!(
                "Vorlauffrist wurde nicht eingehalten: Zuordnungsbeginn {}, ÜT {}. Spätester \
                 ÜT ist der Tag vor dem letzten WT vor dem voraussichtlichen \
                 Zuordnungsbeginn (GPKE Teil 2 § 2.2.2 Nr. 1).",
                anfrage.zuordnungsbeginn, anfrage.uebertragungstag
            ),
        )
    })
}

/// „Bei DV ab Inbetriebnahmedatum gilt …" — an erzeugende Neuanlage entering a
/// Direktvermarktungsform. On a Neuanlage the assignment always starts at
/// commissioning, so the form alone decides.
fn direktvermarktung_ab_inbetriebnahme(anfrage: &NeuanlageAnfrage) -> bool {
    anfrage.marktlokationsart == Marktlokationsart::Erzeugend
        && anfrage.erzeugung.as_ref().is_some_and(|e| {
            matches!(
                e.angemeldete_veraeusserungsform,
                Veraeusserungsform::Marktpraemie | Veraeusserungsform::SonstigeDirektvermarktung
            )
        })
}

/// Werktage from `from` (exclusive) to `to` (inclusive), for the operator view.
fn werktage_between(from: Date, to: Date, config: NetzCheckConfig) -> u32 {
    let mut n = 0;
    let mut cur = from;
    while cur < to {
        let Some(next) = cur.next_day() else { break };
        cur = next;
        if mako_fristen::is_werktag(cur, config.holiday_calendar) {
            n += 1;
        }
    }
    n
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use time::Month;

    use super::super::types::Geschaeftsvorfall;

    fn d(y: i32, m: Month, day: u8) -> Date {
        Date::from_calendar_date(y, m, day).expect("valid date")
    }

    fn cfg() -> NetzCheckConfig {
        NetzCheckConfig::default()
    }

    fn anfrage(pid: u32, ut: Date, zuordnungsbeginn: Date) -> NeuanlageAnfrage {
        NeuanlageAnfrage {
            pid,
            marktlokationsart: if pid == 55_601 {
                Marktlokationsart::Erzeugend
            } else {
                Marktlokationsart::Verbrauchend
            },
            lf_mp_id: "9900357000004".to_owned(),
            zuordnungsbeginn,
            uebertragungstag: ut,
            erzeugung: None,
        }
    }

    fn ok_befund() -> NeuanlageBefund {
        NeuanlageBefund {
            nimmt_an_mako_teil: true,
            erstmalige_inbetriebnahme: true,
            lf_bereits_zugeordnet: false,
            im_netzgebiet: true,
            anforderungen_erfuellt: true,
            viertelstundenmessung: true,
        }
    }

    fn eindeutig() -> Identifikation {
        Identifikation::Eindeutig {
            malo_id: "51238696012".to_owned(),
        }
    }

    // ÜT Wed 2026-03-04, Zuordnungsbeginn Mon 2026-03-09.
    const UT: (i32, Month, u8) = (2026, Month::March, 4);
    const BEGINN: (i32, Month, u8) = (2026, Month::March, 9);

    fn clean() -> NeuanlageAnfrage {
        anfrage(55_600, d(UT.0, UT.1, UT.2), d(BEGINN.0, BEGINN.1, BEGINN.2))
    }

    #[test]
    fn a_clean_neuanlage_is_confirmed_with_a09() {
        let r = evaluate_neuanlage(
            &clean(),
            &eindeutig(),
            Some(&ok_befund()),
            d(UT.0, UT.1, UT.2),
            &cfg(),
        );
        assert_eq!(r.antwortcode(), Some("A09"), "{r:?}");
    }

    /// The erzeugende branch confirms with `A18`, not `A09`.
    #[test]
    fn the_erzeugende_branch_confirms_with_a18() {
        let mut a = anfrage(55_601, d(2026, Month::March, 4), d(2026, Month::March, 9));
        a.erzeugung = Some(ErzeugungsAnmeldung {
            geschaeftsvorfall: Geschaeftsvorfall::Eins,
            // Einspeisevergütung is not a Direktvermarktung, so the Werktag rule
            // applies and the short lead above is fine.
            angemeldete_veraeusserungsform: Veraeusserungsform::Einspeiseverguetung,
            bestehende_veraeusserungsform: None,
            nicht_eeg_kwkg: false,
            ausfallverguetung: false,
            // Untranchiert: the Anmeldung is for the whole Marktlokation.
            gewuenschter_prozentsatz: None,
            tranchen_prozent: std::collections::BTreeMap::new(),
            direktvermarktungspflichtig: None,
        });
        let r = evaluate_neuanlage(
            &a,
            &eindeutig(),
            Some(&ok_befund()),
            d(2026, Month::March, 4),
            &cfg(),
        );
        assert_eq!(r.antwortcode(), Some("A18"), "{r:?}");
    }

    /// Every refusal uses its own branch's code. `A01` and `A10` answer the same
    /// question.
    #[test]
    fn each_branch_has_its_own_code_for_the_same_question() {
        // ÜT Wed, Zuordnungsbeginn Thu — no full Werktag between.
        let late = anfrage(55_600, d(2026, Month::March, 4), d(2026, Month::March, 5));
        let r = evaluate_neuanlage(
            &late,
            &eindeutig(),
            Some(&ok_befund()),
            d(2026, Month::March, 4),
            &cfg(),
        );
        assert_eq!(r.antwortcode(), Some("A01"));

        let mut late_erz = anfrage(55_601, d(2026, Month::March, 4), d(2026, Month::March, 5));
        late_erz.erzeugung = Some(ErzeugungsAnmeldung {
            geschaeftsvorfall: Geschaeftsvorfall::Eins,
            angemeldete_veraeusserungsform: Veraeusserungsform::Einspeiseverguetung,
            bestehende_veraeusserungsform: None,
            nicht_eeg_kwkg: false,
            ausfallverguetung: false,
            // Untranchiert: the Anmeldung is for the whole Marktlokation.
            gewuenschter_prozentsatz: None,
            tranchen_prozent: std::collections::BTreeMap::new(),
            direktvermarktungspflichtig: None,
        });
        let r = evaluate_neuanlage(
            &late_erz,
            &eindeutig(),
            Some(&ok_befund()),
            d(2026, Month::March, 4),
            &cfg(),
        );
        assert_eq!(r.antwortcode(), Some("A10"));
    }

    /// A Direktvermarktung ab Inbetriebnahmedatum takes the month, not the
    /// Werktag rule — GPKE Teil 2 § 2.2.2 Nr. 1.
    #[test]
    fn a_direktvermarktung_neuanlage_needs_a_month_of_lead() {
        let mut a = anfrage(55_601, d(2026, Month::March, 4), d(2026, Month::March, 9));
        a.erzeugung = Some(ErzeugungsAnmeldung {
            geschaeftsvorfall: Geschaeftsvorfall::Eins,
            angemeldete_veraeusserungsform: Veraeusserungsform::Marktpraemie,
            bestehende_veraeusserungsform: None,
            nicht_eeg_kwkg: false,
            ausfallverguetung: false,
            // Untranchiert: the Anmeldung is for the whole Marktlokation.
            gewuenschter_prozentsatz: None,
            tranchen_prozent: std::collections::BTreeMap::new(),
            direktvermarktungspflichtig: None,
        });
        let r = evaluate_neuanlage(
            &a,
            &eindeutig(),
            Some(&ok_befund()),
            d(2026, Month::March, 4),
            &cfg(),
        );
        assert_eq!(r.antwortcode(), Some("A10"), "five days is not a month");

        // A Zuordnungsbeginn a clear month out passes.
        a.zuordnungsbeginn = d(2026, Month::May, 1);
        let r = evaluate_neuanlage(
            &a,
            &eindeutig(),
            Some(&ok_befund()),
            d(2026, Month::March, 4),
            &cfg(),
        );
        assert_eq!(r.antwortcode(), Some("A18"), "{r:?}");
    }

    /// The Vorlauffrist is a property of the message, so the verdict does not
    /// drift as the Prüflauf runs.
    #[test]
    fn the_vorlauffrist_verdict_does_not_change_during_the_pruflauf() {
        let late = anfrage(55_600, d(2026, Month::March, 4), d(2026, Month::March, 5));
        for day in [4_u8, 20, 60] {
            let r = evaluate_neuanlage(
                &late,
                &eindeutig(),
                Some(&ok_befund()),
                d(2026, Month::March, 4)
                    .replace_day(day.min(31))
                    .unwrap_or(d(2026, Month::March, 4)),
                &cfg(),
            );
            assert_eq!(r.antwortcode(), Some("A01"), "day {day}");
        }
    }

    // ── The 60-Werktage Prüflauf ─────────────────────────────────────────────

    /// An unidentifiable Marktlokation is **not** refused. The tree loops back
    /// to Prüfschritt 30 and the NB re-checks daily.
    #[test]
    fn an_unidentifiable_malo_is_deferred_not_refused() {
        let r = evaluate_neuanlage(
            &clean(),
            &Identifikation::Keine,
            None,
            d(2026, Month::March, 10),
            &cfg(),
        );
        let NeuanlageEntscheidung::Vertagen {
            letzter_pruefungstag,
            verbleibende_werktage,
        } = r
        else {
            panic!("expected Vertagen, got {r:?}");
        };
        assert!(letzter_pruefungstag > d(2026, Month::May, 1), "60 WT out");
        assert!(verbleibende_werktage > 0);
        assert!(r.antwortcode().is_none(), "nothing goes on the wire");
    }

    /// Once the window has run out, and only then, the answer is `A07` / `A16`.
    #[test]
    fn the_refusal_only_lands_after_sixty_werktage() {
        let a = clean();
        let letzter = mako_fristen::add_werktage(
            a.uebertragungstag,
            IDENTIFIKATION_WERKTAGE,
            cfg().holiday_calendar,
        );

        let still_open = evaluate_neuanlage(&a, &Identifikation::Keine, None, letzter, &cfg());
        assert!(still_open.is_vertagen(), "the last day is still open");

        let expired = evaluate_neuanlage(
            &a,
            &Identifikation::Keine,
            None,
            letzter.next_day().expect("next day"),
            &cfg(),
        );
        assert_eq!(expired.antwortcode(), Some("A07"), "{expired:?}");
    }

    #[test]
    fn the_erzeugende_branch_refuses_with_a16() {
        let mut a = anfrage(55_601, d(2026, Month::March, 4), d(2026, Month::May, 1));
        a.erzeugung = Some(ErzeugungsAnmeldung {
            geschaeftsvorfall: Geschaeftsvorfall::Eins,
            angemeldete_veraeusserungsform: Veraeusserungsform::Marktpraemie,
            bestehende_veraeusserungsform: None,
            nicht_eeg_kwkg: false,
            ausfallverguetung: false,
            // Untranchiert: the Anmeldung is for the whole Marktlokation.
            gewuenschter_prozentsatz: None,
            tranchen_prozent: std::collections::BTreeMap::new(),
            direktvermarktungspflichtig: None,
        });
        let letzter = mako_fristen::add_werktage(
            a.uebertragungstag,
            IDENTIFIKATION_WERKTAGE,
            cfg().holiday_calendar,
        );
        let expired = evaluate_neuanlage(
            &a,
            &Identifikation::Keine,
            None,
            letzter.next_day().expect("next day"),
            &cfg(),
        );
        assert_eq!(expired.antwortcode(), Some("A16"), "{expired:?}");
    }

    // ── Identification and Befund ────────────────────────────────────────────

    #[test]
    fn a_multiple_match_is_refused_unless_exactly_one_participates() {
        let r = evaluate_neuanlage(
            &clean(),
            &Identifikation::Mehrdeutig { teilnehmende: 2 },
            Some(&ok_befund()),
            d(2026, Month::March, 4),
            &cfg(),
        );
        assert_eq!(r.antwortcode(), Some("A08"));

        let r = evaluate_neuanlage(
            &clean(),
            &Identifikation::Mehrdeutig { teilnehmende: 1 },
            Some(&ok_befund()),
            d(2026, Month::March, 4),
            &cfg(),
        );
        assert_eq!(r.antwortcode(), Some("A09"), "exactly one is the ja path");
    }

    #[test]
    fn an_identified_malo_without_facts_escalates() {
        let r = evaluate_neuanlage(
            &clean(),
            &eindeutig(),
            None,
            d(2026, Month::March, 4),
            &cfg(),
        );
        assert!(matches!(r, NeuanlageEntscheidung::Escalate { .. }), "{r:?}");
    }

    #[test]
    fn each_befund_fact_has_its_own_code() {
        /// One Prüfschritt: break a fact, expect its own code.
        type Fall = (fn(&mut NeuanlageBefund), &'static str);
        let cases: &[Fall] = &[
            (|b| b.nimmt_an_mako_teil = false, "A02"),
            (|b| b.erstmalige_inbetriebnahme = false, "A03"),
            (|b| b.lf_bereits_zugeordnet = true, "A04"),
            (|b| b.im_netzgebiet = false, "A05"),
            (|b| b.anforderungen_erfuellt = false, "A06"),
        ];
        for (mutate, expected) in cases {
            let mut b = ok_befund();
            mutate(&mut b);
            let r = evaluate_neuanlage(
                &clean(),
                &eindeutig(),
                Some(&b),
                d(2026, Month::March, 4),
                &cfg(),
            );
            assert_eq!(r.antwortcode(), Some(*expected), "{r:?}");
        }
    }

    /// Prüfschritt 545 exists only on the erzeugende branch.
    #[test]
    fn the_quarter_hour_check_is_erzeugend_only() {
        let mut b = ok_befund();
        b.viertelstundenmessung = false;

        // A verbrauchende Neuanlage never reaches it.
        let r = evaluate_neuanlage(
            &clean(),
            &eindeutig(),
            Some(&b),
            d(2026, Month::March, 4),
            &cfg(),
        );
        assert_eq!(r.antwortcode(), Some("A09"));

        let mut a = anfrage(55_601, d(2026, Month::March, 4), d(2026, Month::March, 9));
        a.erzeugung = Some(ErzeugungsAnmeldung {
            geschaeftsvorfall: Geschaeftsvorfall::Eins,
            angemeldete_veraeusserungsform: Veraeusserungsform::Einspeiseverguetung,
            bestehende_veraeusserungsform: None,
            nicht_eeg_kwkg: false,
            ausfallverguetung: false,
            // Untranchiert: the Anmeldung is for the whole Marktlokation.
            gewuenschter_prozentsatz: None,
            tranchen_prozent: std::collections::BTreeMap::new(),
            direktvermarktungspflichtig: None,
        });
        let r = evaluate_neuanlage(&a, &eindeutig(), Some(&b), d(2026, Month::March, 4), &cfg());
        assert_eq!(r.antwortcode(), Some("A19"), "{r:?}");
    }

    /// Every code this module emits is published by `E_0608`.
    #[test]
    fn every_code_belongs_to_e0608() {
        for c in [
            VERBRAUCHEND.vorlauffrist.0,
            VERBRAUCHEND.mako.0,
            VERBRAUCHEND.identifikation.0,
            VERBRAUCHEND.inbetriebnahme.0,
            VERBRAUCHEND.lf_zugeordnet.0,
            VERBRAUCHEND.netzgebiet.0,
            VERBRAUCHEND.anforderungen.0,
            VERBRAUCHEND.abgelaufen.0,
            VERBRAUCHEND.zustimmung,
            ERZEUGEND.vorlauffrist.0,
            ERZEUGEND.mako.0,
            ERZEUGEND.identifikation.0,
            ERZEUGEND.inbetriebnahme.0,
            ERZEUGEND.lf_zugeordnet.0,
            ERZEUGEND.netzgebiet.0,
            ERZEUGEND.anforderungen.0,
            ERZEUGEND.abgelaufen.0,
            ERZEUGEND.zustimmung,
            "A19",
        ] {
            assert!(
                codes::lookup(EBD_NEUANLAGE, c).is_some(),
                "{c} is not published by {EBD_NEUANLAGE}"
            );
        }
        // The two branches share no code.
        let verb = [
            VERBRAUCHEND.vorlauffrist.0,
            VERBRAUCHEND.mako.0,
            VERBRAUCHEND.identifikation.0,
            VERBRAUCHEND.zustimmung,
        ];
        let erz = [
            ERZEUGEND.vorlauffrist.0,
            ERZEUGEND.mako.0,
            ERZEUGEND.identifikation.0,
            ERZEUGEND.zustimmung,
        ];
        for v in verb {
            assert!(!erz.contains(&v), "{v} appears in both branches");
        }
    }
}

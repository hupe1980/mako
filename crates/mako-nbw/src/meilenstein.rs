//! The published **Prozess- und Fristenübersicht**, one [`Meilenstein`] per row.
//!
//! Strom Kap. 5 and Gas Kap. 4.1.2 are the same kind of table with different
//! columns, and the difference matters:
//!
//! | Column | Strom Kap. 5 | Gas Kap. 4.1.2 |
//! |---|---|---|
//! | Thema | yes, spanning several rows | — |
//! | Kapitel | yes | implied by the Prozess |
//! | Prozess | yes | yes |
//! | Beteiligte Rollen | yes | yes |
//! | „eine (erstmalige) Übermittlung sollte spätestens stattfinden" | yes | „Frist zum initialen Austausch" |
//! | „vorab durchzuführen (Kapitel-Nr.)" | yes | — |
//! | „Kommunikation auf Lokationsebene" | yes | — |
//!
//! Where Gas has no column this crate does not fill one in: the Gas rows carry
//! [`Lokationsebene::NichtAusgewiesen`], and their [`Vorbedingung`]s come from
//! the Use-Case „Vorbedingung" fields rather than from a Fristenübersicht
//! column, with the Fundstelle saying so.
//!
//! # Every row runs on a process that already exists
//!
//! Kap. 6 to 9 name, for each row, the Use-Case that carries it — „Übermittlung
//! von Informationen" (GPKE Teil 4), „Stammdatenänderung vom NB verantwortlich
//! (ausgehend)" (GPKE Teil 4), „Übermittlung Preisblatt NB an LF" (GPKE Teil 2),
//! the MaBiS Profil-Use-Cases, „Übermittlung der Berechnungsformel" (WiM Strom
//! Teil 2), „Ende Messstellenbetrieb" (WiM Strom Teil 1). Two rows are
//! explicitly not standardised: the Liste der Lokationen (Kap. 6.2, 9.1) and the
//! Information von NB an weiteren Datenberechtigten (Kap. 7.4), both of which
//! state „Der in diesem Prozess beschriebene Informationsaustausch erfolgt nicht
//! in einem standardisierten, durch EDI@Energy beschriebenen
//! Datenaustauschformat." [`Meilenstein::prozessquelle`] and
//! [`Meilenstein::uebertragung`] carry that.

use serde::{Deserialize, Serialize};
use time::Date;

use crate::{Aenderungszeitpunkt, Sparte, monate_zurueck};

/// Werktag calendar the two Werktag-denominated Gas Fristen count in.
///
/// Both Anwendungshilfen use the market's one Werktagsdefinition; no separate
/// calendar is stated for a NB-Wechsel.
pub const KALENDER: mako_fristen::HolidayCalendar = mako_fristen::HolidayCalendar::BdewMaKo;

/// Werktage within which a change discovered after the first transmission must
/// be passed on, in the Gas Sparte (Gas Kap. 4.2.2 Nr. 2, 4.3.2 Nr. 3/4,
/// 4.4.2 Nr. 2, 4.5.2 Nr. 2).
///
/// „Unverzüglich, spätestens jedoch 3 WT nach Kenntnisnahme." It applies to
/// every one of the four Gas Use-Cases that has a Nachmelde-Schritt: newly
/// added Datenberechtigte, newly added or dropped Lokationen, and changed
/// Stammdaten. The Strom Anwendungshilfe states its equivalent step only as
/// „Unverzüglich", with no Werktag figure.
pub const GAS_AKTUALISIERUNG_WT: u32 = 3;

/// A Rolle taking part in a NB-Wechsel.
///
/// The Rollen come from the BDEW-Anwendungshilfe „Rollenmodell für die
/// Marktkommunikation im deutschen Energiemarkt", Version 2.1 (Strom Kap. 2,
/// Gas Literaturverzeichnis \[1\]). NBA/NBN and gMSBA/gMSBN are not separate
/// Rollen but the two sides of one Rolle at a Lokation, which is why they are
/// listed here as they appear in the Übersicht — „NB (NBA)", „MSB (gMSBA)".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Rolle {
    /// Netzbetreiber alt — „trägt die Verantwortung für eine betroffene
    /// Lokation bis zum Zuordnungsende, auch nach dem zeitlichen Erreichen des
    /// Zuordnungsendes" (Kap. 2.2).
    Nba,
    /// Netzbetreiber neu — „trägt die Verantwortung für eine betroffene
    /// Lokation ab dem Zuordnungsbeginn" (Kap. 2.2).
    Nbn,
    /// Lieferant.
    Lf,
    /// Messstellenbetreiber.
    Msb,
    /// Grundzuständiger Messstellenbetreiber des NBA (Strom Kap. 4
    /// Rahmenbedingung 9).
    GMsbA,
    /// Grundzuständiger Messstellenbetreiber des NBN (Strom Kap. 4
    /// Rahmenbedingung 9).
    GMsbN,
    /// Messstellenbetreiber alt (Gas Kap. 6 Abkürzungsverzeichnis) — the
    /// outgoing side of Gas Kap. 4.6 „Übergang des Messstellenbetriebs".
    MsbA,
    /// Messstellenbetreiber neu (Gas Kap. 6 Abkürzungsverzeichnis).
    MsbN,
    /// Übertragungsnetzbetreiber (Strom only).
    Uenb,
    /// Bilanzkoordinator (Strom only).
    Biko,
    /// Bilanzkreisverantwortlicher.
    Bkv,
    /// Registerbetreiber — „hier: Umweltbundesamt (UBA)" (Strom Kap. 2.1
    /// Fußnote 1).
    Registerbetreiber,
    /// Einsatzverantwortlicher (Strom only).
    Eiv,
    /// Letztverbraucher mit Netznutzungsvertrag.
    Lv,
    /// Erzeuger.
    Ez,
    /// Marktgebietsverantwortlicher (Gas only).
    Mgv,
    /// Datenberechtigter — „kann eine unter Kapitel 2.1 genannte Rolle sein und
    /// verarbeitet und nutzt Abrechnungs-, Stamm- und Bewegungsdaten einer
    /// Lokation, die zur Erfüllung seiner vertraglichen bzw. gesetzlichen
    /// Verpflichtungen erforderlich sind" (Kap. 2.2).
    ///
    /// It is a capacity, not a Rolle of the Rollenmodell: the duty may be
    /// time-limited, and per Datum and Zeitpunkt there may be several DB using
    /// one Lokation's data at once.
    Datenberechtigter,
    /// „weiterer DB" — the collective the Strom Anwendungshilfe uses in
    /// Kap. 7.4 for BIKO, BKV, RB, EIV, LV and EZ.
    WeitererDb,
}

impl Rolle {
    /// The abbreviation the Anwendungshilfen use in their tables.
    #[must_use]
    pub const fn kuerzel(self) -> &'static str {
        match self {
            Self::Nba => "NBA",
            Self::Nbn => "NBN",
            Self::Lf => "LF",
            Self::Msb => "MSB",
            Self::GMsbA => "gMSBA",
            Self::GMsbN => "gMSBN",
            Self::MsbA => "MSBA",
            Self::MsbN => "MSBN",
            Self::Uenb => "ÜNB",
            Self::Biko => "BIKO",
            Self::Bkv => "BKV",
            Self::Registerbetreiber => "RB",
            Self::Eiv => "EIV",
            Self::Lv => "LV",
            Self::Ez => "EZ",
            Self::Mgv => "MGV",
            Self::Datenberechtigter => "DB",
            Self::WeitererDb => "weiterer DB",
        }
    }
}

impl std::fmt::Display for Rolle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.kuerzel())
    }
}

/// Who takes part, in the shape the Übersicht states it.
///
/// The distinction is in the tables themselves: some cells read „NB (NBA) **an**
/// NB (NBN)" and some read „NB (NBA) **und** NB (NBN)". The second is an
/// exchange in both directions — Kap. 6.1 and 7.2 are the initial setup and
/// upkeep of Kommunikationsdaten *between* two parties — and flattening it into
/// a sender and a receiver loses one of the two obligations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "art", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Beteiligte {
    /// „A an B".
    Gerichtet {
        /// The party or parties that send.
        absender: &'static [Rolle],
        /// The party or parties that receive.
        empfaenger: &'static [Rolle],
    },
    /// „A und B" — an exchange the table gives no direction for.
    Wechselseitig(&'static [Rolle]),
}

impl Beteiligte {
    /// Every Rolle named, regardless of direction.
    #[must_use]
    pub const fn rollen(self) -> (&'static [Rolle], &'static [Rolle]) {
        match self {
            Self::Gerichtet {
                absender,
                empfaenger,
            } => (absender, empfaenger),
            Self::Wechselseitig(rollen) => (rollen, rollen),
        }
    }

    /// Whether `rolle` takes part at all.
    #[must_use]
    pub fn beteiligt(self, rolle: Rolle) -> bool {
        let (a, e) = self.rollen();
        a.contains(&rolle) || e.contains(&rolle)
    }
}

/// Whether the communication happens per Lokation.
///
/// The Strom Kap.-5 column „Kommunikation auf Lokationsebene" has three values,
/// not two: `ja`, `nein` and — for Kap. 7.4 — „abhängig vom DB". The third is
/// real: an Informationsschreiben to a BIKO names a Bilanzierungsgebiet while
/// one to a Letztverbraucher names their Lokation, and the Anwendungshilfe
/// declines to fix it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Lokationsebene {
    /// „ja".
    Ja,
    /// „nein".
    Nein,
    /// „abhängig vom DB" (Strom Kap. 5, row Kap. 7.4).
    AbhaengigVomDb,
    /// The Gas Prozess- und Fristenübersicht (Kap. 4.1.2) has no such column.
    NichtAusgewiesen,
}

/// How the data travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Uebertragung {
    /// A standardised EDI@Energy format, through the Use-Case named in
    /// [`Meilenstein::prozessquelle`].
    Edifact,
    /// „Der in diesem Prozess beschriebene Informationsaustausch erfolgt nicht
    /// in einem standardisierten, durch EDI@Energy beschriebenen
    /// Datenaustauschformat."
    NonEdifact,
    /// „NBA und NBN haben sich im Vorfeld über das Datenformat und die
    /// Übertragungsform verständigt" (Gas Kap. 4.2.1, 4.4.1, 4.8.1).
    Bilateral,
}

impl Uebertragung {
    /// Whether an EDI@Energy format carries this step.
    #[must_use]
    pub const fn ist_edifact(self) -> bool {
        matches!(self, Self::Edifact)
    }
}

/// How binding a prerequisite is.
///
/// The Strom Kap.-5 column „vorab durchzuführen (Kapitel-Nr.)" qualifies some of
/// its entries — „ggf. 7.2", „teilw. 7.3" — and the qualifiers are not
/// decoration. „ggf. 7.2" is the setup of Kommunikationsdaten with a DB, owed
/// only „sofern die EDIFACT-Kommunikation zu diesem DB noch nicht aufgebaut ist"
/// (Kap. 7.2); „teilw. 7.3" is the Basisdaten, of which only the part a given
/// Abrechnungsdaten-Übermittlung actually needs has to be there first. Treating
/// either as unconditional blocks a milestone that is ready to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VorbedingungArt {
    /// The chapter is named without a qualifier.
    Erforderlich,
    /// „ggf." — required only where the condition the chapter itself states
    /// applies.
    Bedingt,
    /// „teilw." — only the part this milestone depends on.
    Teilweise,
}

/// One entry of the „vorab durchzuführen" column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Vorbedingung {
    /// The chapter that must have run first, as the Anwendungshilfe numbers it.
    pub kapitel: &'static str,
    /// Whether the table qualifies it.
    pub art: VorbedingungArt,
}

impl Vorbedingung {
    /// An unqualified prerequisite.
    #[must_use]
    pub const fn erforderlich(kapitel: &'static str) -> Self {
        Self {
            kapitel,
            art: VorbedingungArt::Erforderlich,
        }
    }

    /// A „ggf." prerequisite.
    #[must_use]
    pub const fn bedingt(kapitel: &'static str) -> Self {
        Self {
            kapitel,
            art: VorbedingungArt::Bedingt,
        }
    }

    /// A „teilw." prerequisite.
    #[must_use]
    pub const fn teilweise(kapitel: &'static str) -> Self {
        Self {
            kapitel,
            art: VorbedingungArt::Teilweise,
        }
    }
}

/// The lead time of a first transmission, as the Übersicht states it.
///
/// Every Strom row is stated in whole months. Gas states one row in months plus
/// Werktage, one in Werktage alone, and refers two rows elsewhere for their
/// Fristen — which is what [`Vorlauf::NichtBeziffert`] is for. Reading an
/// unquantified row as „no lead" would put it on the Änderungszeitpunkt itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "art", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Vorlauf {
    /// „spätestens `n` Monate vor dem Änderungszeitpunkt".
    Monate(u32),
    /// „spätestens `monate` Monate + `werktage` WT vor dem Änderungszeitpunkt"
    /// (Gas Kap. 4.1.2 / 4.3.2 Nr. 1 und 2).
    ///
    /// The two units compose in that order: the calendar interval first, then
    /// the Werktage back from the day it lands on.
    MonateUndWerktage {
        /// Calendar months before the Änderungszeitpunkt.
        monate: u32,
        /// Werktage before the day those months land on.
        werktage: u32,
    },
    /// „`n` WT vor Änderungszeitpunkt" (Gas Kap. 4.6.2 Nr. 1).
    Werktage(u32),
    /// The Übersicht gives no figure and says where the Frist comes from
    /// instead.
    NichtBeziffert(&'static str),
}

impl Vorlauf {
    /// The latest date a first transmission should happen, where the Übersicht
    /// quantifies one.
    ///
    /// `None` for [`Self::NichtBeziffert`] — the Frist exists but lives in
    /// another document, and inventing a date for it would put a deadline in a
    /// plan that the Anwendungshilfe does not set.
    #[must_use]
    pub fn faellig(self, aenderungszeitpunkt: Date) -> Option<Date> {
        match self {
            Self::Monate(n) => Some(monate_zurueck(aenderungszeitpunkt, n)),
            Self::MonateUndWerktage { monate, werktage } => Some(mako_fristen::sub_werktage(
                monate_zurueck(aenderungszeitpunkt, monate),
                werktage,
                KALENDER,
            )),
            Self::Werktage(n) => Some(mako_fristen::sub_werktage(aenderungszeitpunkt, n, KALENDER)),
            Self::NichtBeziffert(_) => None,
        }
    }
}

/// One row of the Prozess- und Fristenübersicht.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Meilenstein {
    /// Stable slug, used as the lookup key and in operator-facing reasons.
    pub key: &'static str,
    /// The Sparte whose Anwendungshilfe publishes this row.
    pub sparte: Sparte,
    /// The „Thema" cell, where the table has one. `None` in Gas.
    pub thema: Option<&'static str>,
    /// The chapter, as the Anwendungshilfe numbers it. Several rows may share
    /// one — Strom Kap. 7.3 has five and Kap. 7.5 five — and a prerequisite
    /// names a chapter, so it binds all of them.
    pub kapitel: &'static str,
    /// The „Prozess" cell.
    pub prozess: &'static str,
    /// Who takes part, and in which direction.
    pub beteiligte: Beteiligte,
    /// The lead time of the first transmission.
    pub vorlauf: Vorlauf,
    /// The chapters that must have run first.
    pub vorbedingungen: &'static [Vorbedingung],
    /// Whether communication is per Lokation.
    pub lokationsebene: Lokationsebene,
    /// How the data travels.
    pub uebertragung: Uebertragung,
    /// The existing Marktprozess Use-Case that carries this row, where one does.
    ///
    /// This is the answer to „which message do I send" — a NB-Wechsel has no
    /// message family of its own. `None` where the exchange is not a
    /// Marktprozess at all.
    pub prozessquelle: Option<&'static str>,
    /// Citation, for the audit trail.
    pub fundstelle: &'static str,
}

impl Meilenstein {
    /// The latest date a first transmission should happen.
    ///
    /// `None` where the Übersicht states no figure — see
    /// [`Vorlauf::NichtBeziffert`].
    #[must_use]
    pub fn faellig(&self, aenderungszeitpunkt: Aenderungszeitpunkt) -> Option<Date> {
        self.vorlauf.faellig(aenderungszeitpunkt.datum())
    }
}

// ── Strom: Prozess- und Fristenübersicht, Kap. 5 ─────────────────────────────

const STROM_FRIST_4M: &str = "Kap. 5 — „4 Monate vor dem Änderungszeitpunkt\"";
const STROM_FRIST_3M: &str = "Kap. 5 — „3 Monate vor dem Änderungszeitpunkt\"";
const STROM_FRIST_2M: &str = "Kap. 5 — „2 Monate vor dem Änderungszeitpunkt\"";
const STROM_FRIST_1M: &str = "Kap. 5 — „1 Monat vor dem Änderungszeitpunkt\"";

/// The „Basisdaten" prerequisites, shared by all five Kap.-7.3 rows.
const VOR_7_3: &[Vorbedingung] = &[
    Vorbedingung::erforderlich("6.3"),
    Vorbedingung::bedingt("7.2"),
];

/// The prerequisites of the two Abrechnungsdaten rows of Kap. 7.5.
const VOR_7_5_ABRECHNUNG: &[Vorbedingung] = &[
    Vorbedingung::erforderlich("6.4"),
    Vorbedingung::erforderlich("7.1"),
    Vorbedingung::bedingt("7.2"),
    Vorbedingung::teilweise("7.3"),
];

/// The prerequisites of the three remaining Kap.-7.5 rows.
const VOR_7_5: &[Vorbedingung] = &[
    Vorbedingung::erforderlich("6.4"),
    Vorbedingung::erforderlich("7.1"),
    Vorbedingung::bedingt("7.2"),
];

/// **Strom** — every row of the Prozess- und Fristenübersicht, Kap. 5.
///
/// Twenty-one rows in four Frist bands (4, 3, 2 and 1 Monat) plus Kap. 9.2,
/// whose Frist cell is „--" because the Use-Case „Ende Messstellenbetrieb"
/// brings its own Vorlauffrist from WiM Strom Teil 1.
///
/// Kept in the table's own order, so the published document can be read against
/// it row by row. [`crate::Fristenkalender::plan`] is what puts it into
/// dependency order.
pub const STROM: &[Meilenstein] = &[
    Meilenstein {
        key: "strom.kommunikationsdaten-nba-nbn",
        sparte: Sparte::Strom,
        thema: Some("Kommunikationsdaten NBA/NBN"),
        kapitel: "6.1",
        prozess: "Übermittlung von Informationen",
        beteiligte: Beteiligte::Wechselseitig(&[Rolle::Nba, Rolle::Nbn]),
        vorlauf: Vorlauf::Monate(4),
        vorbedingungen: &[],
        lokationsebene: Lokationsebene::Nein,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: Some("Übermittlung von Informationen (GPKE Teil 4)"),
        fundstelle: STROM_FRIST_4M,
    },
    Meilenstein {
        key: "strom.liste-der-lokationen-nba-nbn",
        sparte: Sparte::Strom,
        thema: Some("Lokationen einer Paket-ID"),
        kapitel: "6.2",
        prozess: "Liste der Lokationen von NBA an NBN (NON-EDIFACT)",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nba],
            empfaenger: &[Rolle::Nbn],
        },
        vorlauf: Vorlauf::Monate(4),
        vorbedingungen: &[],
        lokationsebene: Lokationsebene::Nein,
        uebertragung: Uebertragung::NonEdifact,
        prozessquelle: None,
        fundstelle: STROM_FRIST_4M,
    },
    Meilenstein {
        key: "strom.stammdatenaenderung-nba",
        sparte: Sparte::Strom,
        thema: Some("Lokationen einer Paket-ID"),
        kapitel: "7.1",
        prozess: "Stammdatenänderung vom NB verantwortlich (ausgehend)",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nba],
            empfaenger: &[Rolle::Lf, Rolle::Msb, Rolle::Uenb],
        },
        vorlauf: Vorlauf::Monate(4),
        vorbedingungen: &[],
        lokationsebene: Lokationsebene::Ja,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: Some("Stammdatenänderung vom NB verantwortlich (ausgehend) (GPKE Teil 4)"),
        fundstelle: "Kap. 5 und Kap. 7.1 — die Paket-ID wird für jede betroffene Marktlokation \
                     vom NBA an den DB (hier: LF, MSB einschließlich nicht aktiver gMSB im \
                     Lokationsbündel, ÜNB) übermittelt",
    },
    Meilenstein {
        key: "strom.liste-der-lokationen-gmsba-gmsbn",
        sparte: Sparte::Strom,
        thema: Some("Lokationen einer Paket-ID"),
        kapitel: "9.1",
        prozess: "Liste der Lokationen von gMSBA an gMSBN (NON-EDIFACT)",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::GMsbA],
            empfaenger: &[Rolle::GMsbN],
        },
        vorlauf: Vorlauf::Monate(4),
        vorbedingungen: &[Vorbedingung::erforderlich("7.1")],
        lokationsebene: Lokationsebene::Nein,
        uebertragung: Uebertragung::NonEdifact,
        prozessquelle: None,
        fundstelle: "Kap. 5 („4 Monate\", vorab 7.1) und Kap. 9.1.1 — Vorbedingung: „Die vom \
                     NB-Wechsel betroffenen Lokationen liegen dem gMSBA vom NBA vor (siehe \
                     Kapitel 7.1)\"",
    },
    Meilenstein {
        key: "strom.lokationsbuendelstruktur-und-db",
        sparte: Sparte::Strom,
        thema: Some("Lokationsbündelstruktur und DB"),
        kapitel: "6.3",
        prozess: "Lokationsbündelstruktur und DB von NBA an NBN",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nba],
            empfaenger: &[Rolle::Nbn],
        },
        vorlauf: Vorlauf::Monate(4),
        vorbedingungen: &[Vorbedingung::erforderlich("6.1")],
        lokationsebene: Lokationsebene::Ja,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: None,
        fundstelle: "Kap. 5 („4 Monate\", vorab 6.1) und Kap. 6.3.1 — Vorbedingung: „Die \
                     EDIFACT-Kommunikation ist aufgebaut\"",
    },
    Meilenstein {
        key: "strom.kommunikationsdaten-nbn-db",
        sparte: Sparte::Strom,
        thema: Some("Kommunikationsdaten NBN/DB"),
        kapitel: "7.2",
        prozess: "Übermittlung von Informationen",
        beteiligte: Beteiligte::Wechselseitig(&[Rolle::Nbn, Rolle::Datenberechtigter]),
        vorlauf: Vorlauf::Monate(3),
        vorbedingungen: &[Vorbedingung::erforderlich("6.3")],
        lokationsebene: Lokationsebene::Nein,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: Some("Übermittlung von Informationen (GPKE Teil 4)"),
        fundstelle: "Kap. 5 („3 Monate\", vorab 6.3) und Kap. 7.2 — „unverzüglich nach dem \
                     Use-Case „Lokationsbündelstruktur und DB von NBA an NBN\" …, sofern die \
                     EDIFACT-Kommunikation zu diesem DB noch nicht aufgebaut ist\"",
    },
    Meilenstein {
        key: "strom.profildefinitionen-lf",
        sparte: Sparte::Strom,
        thema: Some("Basisdaten"),
        kapitel: "7.3",
        prozess: "Übermittlung der Liste der Profildefinitionen vom NB an LF",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nbn],
            empfaenger: &[Rolle::Lf],
        },
        vorlauf: Vorlauf::Monate(3),
        vorbedingungen: VOR_7_3,
        lokationsebene: Lokationsebene::Nein,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: Some("Übermittlung der Liste der Profildefinitionen vom NB an LF (MaBiS)"),
        fundstelle: STROM_FRIST_3M,
    },
    Meilenstein {
        key: "strom.normierte-profile-und-profilscharen-lf",
        sparte: Sparte::Strom,
        thema: Some("Basisdaten"),
        kapitel: "7.3",
        prozess: "Übermittlung von normierten Profilen und Profilscharen vom NB an LF",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nbn],
            empfaenger: &[Rolle::Lf],
        },
        vorlauf: Vorlauf::Monate(3),
        vorbedingungen: VOR_7_3,
        lokationsebene: Lokationsebene::Nein,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: Some(
            "Übermittlung von normierten Profilen und Profilscharen vom NB an LF (MaBiS)",
        ),
        fundstelle: STROM_FRIST_3M,
    },
    Meilenstein {
        key: "strom.profildefinitionen-msb",
        sparte: Sparte::Strom,
        thema: Some("Basisdaten"),
        kapitel: "7.3",
        prozess: "Übermittlung der Liste der Profildefinitionen vom NB an MSB",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nbn],
            empfaenger: &[Rolle::Msb],
        },
        vorlauf: Vorlauf::Monate(3),
        vorbedingungen: VOR_7_3,
        lokationsebene: Lokationsebene::Nein,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: Some("Übermittlung der Liste der Profildefinitionen vom NB an MSB (MaBiS)"),
        fundstelle: STROM_FRIST_3M,
    },
    Meilenstein {
        key: "strom.normierte-profile-msb",
        sparte: Sparte::Strom,
        thema: Some("Basisdaten"),
        kapitel: "7.3",
        prozess: "Übermittlung von normierten Profilen vom NB an MSB",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nbn],
            empfaenger: &[Rolle::Msb],
        },
        vorlauf: Vorlauf::Monate(3),
        vorbedingungen: VOR_7_3,
        lokationsebene: Lokationsebene::Nein,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: Some("Übermittlung von normierten Profilen vom NB an MSB (MaBiS)"),
        fundstelle: "Kap. 5 — die MSB-Zeile nennt „normierte Profile\" ohne Profilscharen; nur \
                     die LF-Zeile führt beide",
    },
    Meilenstein {
        key: "strom.preisblatt-lf",
        sparte: Sparte::Strom,
        thema: Some("Basisdaten"),
        kapitel: "7.3",
        prozess: "Übermittlung Preisblatt NB an LF",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nbn],
            empfaenger: &[Rolle::Lf],
        },
        vorlauf: Vorlauf::Monate(3),
        vorbedingungen: VOR_7_3,
        lokationsebene: Lokationsebene::Nein,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: Some("Übermittlung Preisblatt NB an LF (GPKE Teil 2)"),
        fundstelle: STROM_FRIST_3M,
    },
    Meilenstein {
        key: "strom.information-weiterer-db",
        sparte: Sparte::Strom,
        thema: Some("Information an DB (NON-EDIFACT)"),
        kapitel: "7.4",
        prozess: "Information von NB an weiteren Datenberechtigten (NON-EDIFACT)",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nba, Rolle::Nbn],
            empfaenger: &[Rolle::WeitererDb],
        },
        vorlauf: Vorlauf::Monate(3),
        vorbedingungen: &[Vorbedingung::erforderlich("6.3")],
        lokationsebene: Lokationsebene::AbhaengigVomDb,
        uebertragung: Uebertragung::NonEdifact,
        prozessquelle: None,
        fundstelle: "Kap. 5 („3 Monate\", vorab 6.3, Lokationsebene „abhängig vom DB\") und \
                     Kap. 7.4.1 — der weitere DB ist BIKO, BKV, RB, EIV, LV oder EZ, informiert \
                     „in Textform\"; NBA und NBN können ein gemeinsames Informationsschreiben \
                     versenden",
    },
    Meilenstein {
        key: "strom.ergaenzende-daten-lokationsbuendel",
        sparte: Sparte::Strom,
        thema: Some("Ergänzende Daten zum Lokationsbündel"),
        kapitel: "6.4",
        prozess: "Ergänzende Daten zum Lokationsbündel von NBA an NBN",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nba],
            empfaenger: &[Rolle::Nbn],
        },
        vorlauf: Vorlauf::Monate(2),
        vorbedingungen: &[Vorbedingung::erforderlich("6.3")],
        lokationsebene: Lokationsebene::Ja,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: None,
        fundstelle: "Kap. 5 („2 Monate\", vorab 6.3) und Kap. 6.4.1 — die ergänzenden Daten sind \
                     Abrechnungsdaten Netznutzungsabrechnung, Abrechnungsdaten \
                     Bilanzkreisabrechnung, Stammdaten einer Stammdatenänderung vom NB \
                     verantwortlich (ausgehend) und die Berechnungsformel",
    },
    Meilenstein {
        key: "strom.abrechnungsdaten-netznutzungsabrechnung",
        sparte: Sparte::Strom,
        thema: Some("Ergänzende Daten zum Lokationsbündel"),
        kapitel: "7.5",
        prozess: "Abrechnungsdaten Netznutzungsabrechnung",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nba, Rolle::Nbn],
            empfaenger: &[Rolle::Lf],
        },
        vorlauf: Vorlauf::Monate(2),
        vorbedingungen: VOR_7_5_ABRECHNUNG,
        lokationsebene: Lokationsebene::Ja,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: Some("Abrechnungsdaten Netznutzungsabrechnung (GPKE Teil 2)"),
        fundstelle: STROM_FRIST_2M,
    },
    Meilenstein {
        key: "strom.abrechnungsdaten-bilanzkreisabrechnung",
        sparte: Sparte::Strom,
        thema: Some("Ergänzende Daten zum Lokationsbündel"),
        kapitel: "7.5",
        prozess: "Abrechnungsdaten Bilanzkreisabrechnung",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nba, Rolle::Nbn],
            empfaenger: &[Rolle::Lf, Rolle::Uenb],
        },
        vorlauf: Vorlauf::Monate(2),
        vorbedingungen: VOR_7_5_ABRECHNUNG,
        lokationsebene: Lokationsebene::Ja,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: Some("Abrechnungsdaten Bilanzkreisabrechnung (GPKE Teil 2)"),
        fundstelle: STROM_FRIST_2M,
    },
    Meilenstein {
        key: "strom.stammdaten-bilanzkreistreue",
        sparte: Sparte::Strom,
        thema: Some("Ergänzende Daten zum Lokationsbündel"),
        kapitel: "7.5",
        prozess: "Stammdaten zur Bilanzkreistreue",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nba, Rolle::Nbn],
            empfaenger: &[Rolle::Uenb],
        },
        vorlauf: Vorlauf::Monate(2),
        vorbedingungen: VOR_7_5,
        lokationsebene: Lokationsebene::Ja,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: Some("Stammdaten zur Bilanzkreistreue (GPKE Teil 4)"),
        fundstelle: STROM_FRIST_2M,
    },
    Meilenstein {
        key: "strom.stammdatenaenderung-nbn",
        sparte: Sparte::Strom,
        thema: Some("Ergänzende Daten zum Lokationsbündel"),
        kapitel: "7.5",
        prozess: "Stammdatenänderung vom NB verantwortlich (ausgehend)",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nbn],
            empfaenger: &[Rolle::Lf, Rolle::Msb, Rolle::Uenb],
        },
        vorlauf: Vorlauf::Monate(2),
        vorbedingungen: VOR_7_5,
        lokationsebene: Lokationsebene::Ja,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: Some("Stammdatenänderung vom NB verantwortlich (ausgehend) (GPKE Teil 4)"),
        fundstelle: STROM_FRIST_2M,
    },
    Meilenstein {
        key: "strom.berechnungsformel",
        sparte: Sparte::Strom,
        thema: Some("Ergänzende Daten zum Lokationsbündel"),
        kapitel: "7.5",
        prozess: "Berechnungsformel",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nbn],
            empfaenger: &[Rolle::Lf, Rolle::Msb],
        },
        vorlauf: Vorlauf::Monate(2),
        vorbedingungen: VOR_7_5,
        lokationsebene: Lokationsebene::Ja,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: Some("Übermittlung der Berechnungsformel (WiM Strom Teil 2)"),
        fundstelle: STROM_FRIST_2M,
    },
    Meilenstein {
        key: "strom.stammdatenaenderung-lf",
        sparte: Sparte::Strom,
        thema: Some("Ergänzende Daten zum Lokationsbündel"),
        kapitel: "8",
        prozess: "Stammdatenänderung vom LF verantwortlich (ausgehend)",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Lf],
            empfaenger: &[Rolle::Nbn],
        },
        vorlauf: Vorlauf::Monate(1),
        vorbedingungen: &[Vorbedingung::erforderlich("7.5")],
        lokationsebene: Lokationsebene::Ja,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: Some("Stammdatenänderung vom LF verantwortlich (ausgehend) (GPKE Teil 4)"),
        fundstelle: STROM_FRIST_1M,
    },
    Meilenstein {
        key: "strom.stammdatenaenderung-msb",
        sparte: Sparte::Strom,
        thema: Some("Ergänzende Daten zum Lokationsbündel"),
        kapitel: "8",
        prozess: "Stammdatenänderung vom MSB verantwortlich (ausgehend)",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Msb],
            empfaenger: &[Rolle::Nbn],
        },
        vorlauf: Vorlauf::Monate(1),
        vorbedingungen: &[Vorbedingung::erforderlich("7.5")],
        lokationsebene: Lokationsebene::Ja,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: Some("Stammdatenänderung vom MSB verantwortlich (ausgehend) (GPKE Teil 4)"),
        fundstelle: "Kap. 5 („1 Monat\", vorab 7.5) und Kap. 8 — „Die im Rahmen des Use-Cases \
                     „Stammdatenänderung vom MSB verantwortlich (ausgehend)\" durchzuführende \
                     Übermittlung von Werten bezieht sich auf den Änderungszeitpunkt\"",
    },
    Meilenstein {
        key: "strom.ende-messstellenbetrieb",
        sparte: Sparte::Strom,
        thema: Some("Übergang Grundzuständigkeit Messstellenbetrieb"),
        kapitel: "9.2",
        prozess: "Ende Messstellenbetrieb",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::GMsbA],
            empfaenger: &[Rolle::Nbn],
        },
        vorlauf: Vorlauf::NichtBeziffert(
            "Kap. 5 lässt die Frist-Spalte leer („--\"); Kap. 9.2 verweist auf den Use-Case \
             „Ende Messstellenbetrieb\" (WiM Strom Teil 1), dessen eigene Vorlauffrist gilt",
        ),
        vorbedingungen: &[
            Vorbedingung::erforderlich("6.3"),
            Vorbedingung::erforderlich("7.1"),
            Vorbedingung::bedingt("7.2"),
        ],
        lokationsebene: Lokationsebene::Ja,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: Some("Ende Messstellenbetrieb (WiM Strom Teil 1)"),
        fundstelle: "Kap. 5 (Frist „--\", vorab 6.3, 7.1, ggf. 7.2) und Kap. 9.2 — die \
                     Verpflichtungsanfrage ist an den gMSBN zu richten; der gMSBA tritt als MSBA \
                     am Objekt Messlokation auf, der gMSBN als gMSB, und das gewünschte \
                     Zuordnungsende ist der Änderungszeitpunkt",
    },
];

// ── Gas: Prozess- und Fristenübersicht, Kap. 4.1.2 ───────────────────────────

/// **Gas** — every row of the Prozess- und Fristenübersicht, Kap. 4.1.2.
///
/// Seven rows, and three shapes of Frist the Strom table does not have: months
/// plus Werktage (Kap. 4.3), Werktage alone (Kap. 4.6) and two rows that name
/// another document instead of a figure (Kap. 4.8, 4.9).
///
/// The Gas Übersicht has no „vorab durchzuführen" and no „Lokationsebene"
/// column. The prerequisites below therefore come from the Use-Cases'
/// „Vorbedingung" fields, and every row carries
/// [`Lokationsebene::NichtAusgewiesen`].
pub const GAS: &[Meilenstein] = &[
    Meilenstein {
        key: "gas.kontaktdaten-db",
        sparte: Sparte::Gas,
        thema: None,
        kapitel: "4.2",
        prozess: "Übergabe der Kontaktdaten der DB",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nba],
            empfaenger: &[Rolle::Nbn],
        },
        vorlauf: Vorlauf::Monate(4),
        vorbedingungen: &[],
        lokationsebene: Lokationsebene::NichtAusgewiesen,
        uebertragung: Uebertragung::Bilateral,
        prozessquelle: None,
        fundstelle: "Kap. 4.1.2 — „Spätestens 4 Monate vor dem Änderungszeitpunkt\"; Kap. 4.2.1 \
                     Vorbedingung: „NBA und NBN haben sich im Vorfeld über das Datenformat und \
                     die Übertragungsform verständigt\"",
    },
    Meilenstein {
        key: "gas.information-db",
        sparte: Sparte::Gas,
        thema: None,
        kapitel: "4.3",
        prozess: "Information der DB",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nba, Rolle::Nbn],
            empfaenger: &[Rolle::Datenberechtigter],
        },
        vorlauf: Vorlauf::MonateUndWerktage {
            monate: 3,
            werktage: 10,
        },
        vorbedingungen: &[Vorbedingung::erforderlich("4.2")],
        lokationsebene: Lokationsebene::NichtAusgewiesen,
        uebertragung: Uebertragung::NonEdifact,
        prozessquelle: None,
        fundstelle: "Kap. 4.1.2 und Kap. 4.3.2 Nr. 1/2 — „Spätestens 3 Monate + 10 WT vor dem \
                     Änderungszeitpunkt\", je einmal für Abgabe und Übernahme; Kap. 4.3.1 \
                     Vorbedingung: „Der NBN hat die Kontaktdaten aller zu informierenden DB … \
                     vom NBA erhalten\"; die Information erfolgt „in Textform\" und nicht in \
                     einem durch EDI@Energy beschriebenen Datenaustauschformat",
    },
    Meilenstein {
        key: "gas.uebergabe-stammdaten",
        sparte: Sparte::Gas,
        thema: None,
        kapitel: "4.4",
        prozess: "Übergabe der Stammdaten",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nba],
            empfaenger: &[Rolle::Nbn],
        },
        vorlauf: Vorlauf::Monate(3),
        vorbedingungen: &[],
        lokationsebene: Lokationsebene::NichtAusgewiesen,
        uebertragung: Uebertragung::Bilateral,
        prozessquelle: None,
        fundstelle: "Kap. 4.1.2 und Kap. 4.4.2 Nr. 1 — „Spätestens 3 Monate vor dem \
                     Änderungszeitpunkt\", „Es müssen alle Stammdaten (Mindestumfang) übergeben \
                     werden\"; Kap. 4.4.1 verpflichtet den NBA zudem, auf Anforderung des NBN \
                     spätestens 4 Monate vor dem Änderungszeitpunkt Testdatensätze \
                     bereitzustellen",
    },
    Meilenstein {
        key: "gas.uebermittlung-stammdaten",
        sparte: Sparte::Gas,
        thema: None,
        kapitel: "4.5",
        prozess: "Übermittlung der Stammdaten",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nbn],
            empfaenger: &[Rolle::Datenberechtigter],
        },
        vorlauf: Vorlauf::Monate(2),
        vorbedingungen: &[Vorbedingung::erforderlich("4.4")],
        lokationsebene: Lokationsebene::NichtAusgewiesen,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: Some("EDI@Energy UTILMD Anwendungshandbuch Gas"),
        fundstelle: "Kap. 4.1.2 und Kap. 4.5.2 Nr. 1 — „Spätestens 2 Monate vor dem \
                     Änderungszeitpunkt\"; Kap. 4.5.1 Vorbedingung: der NBN besitzt die \
                     Stammdaten aller betroffenen Lokationen im Mindestumfang und kennt deren DB",
    },
    Meilenstein {
        key: "gas.uebergang-messstellenbetrieb",
        sparte: Sparte::Gas,
        thema: None,
        kapitel: "4.6",
        prozess: "Übergang des Messstellenbetriebs",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::MsbA],
            empfaenger: &[Rolle::MsbN],
        },
        vorlauf: Vorlauf::Werktage(25),
        vorbedingungen: &[],
        lokationsebene: Lokationsebene::NichtAusgewiesen,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: Some("Beginn Messstellenbetrieb (WiM Gas)"),
        fundstelle: "Kap. 4.6.2 Nr. 1 — „Stammdaten zu betroffenen Messlokationen: 25 WT vor \
                     Änderungszeitpunkt\". Die Übersicht Kap. 4.1.2 verweist für den \
                     Folgeprozess auf „Fristen gemäß der jeweils gültigen Fassung WiM Gas\"",
    },
    Meilenstein {
        key: "gas.werteuebermittlung-nb",
        sparte: Sparte::Gas,
        thema: None,
        kapitel: "4.8",
        prozess: "Werteübermittlung Gas an NB",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nba],
            empfaenger: &[Rolle::Nbn],
        },
        vorlauf: Vorlauf::NichtBeziffert(
            "Kap. 4.1.2 — „Fristen gemäß Abstimmung zwischen NBA und NBN\"",
        ),
        vorbedingungen: &[],
        lokationsebene: Lokationsebene::NichtAusgewiesen,
        uebertragung: Uebertragung::Bilateral,
        prozessquelle: None,
        fundstelle: "Kap. 4.1.2 und Kap. 4.8.1 — „Das Format des Datenaustauschs zwischen NBA \
                     und NBN ist bilateral abzustimmen\"; der Schlusszählerstand beim NBA und \
                     der Anfangszählerstand beim NBN sind bei einer Geräteübernahme identisch",
    },
    Meilenstein {
        key: "gas.werteuebermittlung-db",
        sparte: Sparte::Gas,
        thema: None,
        kapitel: "4.9",
        prozess: "Werteübermittlung Gas an DB",
        beteiligte: Beteiligte::Gerichtet {
            absender: &[Rolle::Nbn],
            empfaenger: &[Rolle::Datenberechtigter],
        },
        vorlauf: Vorlauf::NichtBeziffert(
            "Kap. 4.1.2 — „Fristen gemäß der jeweils gültigen Fassung WiM Gas\"",
        ),
        vorbedingungen: &[],
        lokationsebene: Lokationsebene::NichtAusgewiesen,
        uebertragung: Uebertragung::Edifact,
        prozessquelle: Some("Aufbereitung und Übermittlung von Werten (WiM Gas)"),
        fundstelle: "Kap. 4.1.2 und Kap. 4.9.2 — „Vorgehen gemäß BDEW/GEODE/VKU-Leitfaden \
                     „Wechselprozesse im Messwesen Gas\", Use-Case „Aufbereitung und \
                     Übermittlung von Werten\"\"",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use time::Month;

    fn d(y: i32, m: Month, day: u8) -> Date {
        Date::from_calendar_date(y, m, day).expect("valid date")
    }

    /// Strom Kap. 5 publishes 21 rows in four Frist bands plus the one with an
    /// empty Frist cell; Gas Kap. 4.1.2 publishes 7.
    #[test]
    fn both_tables_have_the_published_row_count() {
        assert_eq!(STROM.len(), 21);
        assert_eq!(GAS.len(), 7);
    }

    /// Strom Kap. 5 — the four Frist bands, counted row by row against the
    /// published table.
    #[test]
    fn the_strom_frist_bands_hold_the_published_rows() {
        let bands = |n: u32| {
            STROM
                .iter()
                .filter(|m| m.vorlauf == Vorlauf::Monate(n))
                .count()
        };
        // 6.1, 6.2, 7.1, 9.1, 6.3
        assert_eq!(bands(4), 5, "4 Monate");
        // 7.2, 7.3 ×5, 7.4
        assert_eq!(bands(3), 7, "3 Monate");
        // 6.4, 7.5 ×5
        assert_eq!(bands(2), 6, "2 Monate");
        // 8 ×2
        assert_eq!(bands(1), 2, "1 Monat");
        // 9.2 — „--"
        assert_eq!(
            STROM
                .iter()
                .filter(|m| matches!(m.vorlauf, Vorlauf::NichtBeziffert(_)))
                .count(),
            1
        );
    }

    /// Strom Kap. 5 — the „Kommunikation auf Lokationsebene" column is
    /// three-valued, and Kap. 7.4 is the row that makes it so („abhängig vom
    /// DB").
    #[test]
    fn only_kapitel_7_4_depends_on_the_datenberechtigter() {
        let abhaengig: Vec<_> = STROM
            .iter()
            .filter(|m| m.lokationsebene == Lokationsebene::AbhaengigVomDb)
            .map(|m| m.kapitel)
            .collect();
        assert_eq!(abhaengig, ["7.4"]);
    }

    /// Strom Kap. 6.2, 9.1 and 7.4 state „Der in diesem Prozess beschriebene
    /// Informationsaustausch erfolgt nicht in einem standardisierten, durch
    /// EDI@Energy beschriebenen Datenaustauschformat" — those three and no
    /// others.
    #[test]
    fn exactly_three_strom_rows_are_non_edifact() {
        let non: Vec<_> = STROM
            .iter()
            .filter(|m| m.uebertragung == Uebertragung::NonEdifact)
            .map(|m| m.kapitel)
            .collect();
        assert_eq!(non, ["6.2", "9.1", "7.4"]);
    }

    /// Gas Kap. 4.1.2 / 4.3.2 Nr. 1 — „Spätestens 3 Monate + 10 WT vor dem
    /// Änderungszeitpunkt". The two units compose in that order, and reading it
    /// as three months alone hands the DB ten Werktage of notice it does not
    /// have.
    #[test]
    fn the_gas_information_frist_composes_months_then_werktage() {
        // Änderungszeitpunkt Fri 2027-01-01; 3 Monate back is Thu 2026-10-01;
        // 10 WT before that is Thu 2026-09-17 (03.10. is a Feiertag).
        let az = d(2027, Month::January, 1);
        let vorlauf = Vorlauf::MonateUndWerktage {
            monate: 3,
            werktage: 10,
        };
        assert_eq!(vorlauf.faellig(az), Some(d(2026, Month::September, 17)));
        assert_ne!(vorlauf.faellig(az), Vorlauf::Monate(3).faellig(az));
    }

    /// Gas Kap. 4.6.2 Nr. 1 — „25 WT vor Änderungszeitpunkt".
    #[test]
    fn the_gas_messstellenbetrieb_frist_is_twenty_five_werktage() {
        let az = d(2027, Month::January, 1);
        assert_eq!(
            Vorlauf::Werktage(25).faellig(az),
            Some(mako_fristen::sub_werktage(az, 25, KALENDER))
        );
    }

    /// A row whose Frist cell names another document has no due date here.
    /// Inventing one would put a deadline in a plan that the Anwendungshilfe
    /// does not set.
    #[test]
    fn an_unquantified_frist_yields_no_date() {
        assert_eq!(
            Vorlauf::NichtBeziffert("irgendwo anders").faellig(d(2027, Month::January, 1)),
            None
        );
    }

    /// Every key is unique across both tables — the calendar looks milestones up
    /// by key, and a duplicate silently shadows.
    #[test]
    fn every_key_is_unique() {
        let mut keys: Vec<_> = STROM.iter().chain(GAS).map(|m| m.key).collect();
        keys.sort_unstable();
        let vorher = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), vorher, "doppelter Meilenstein-Key");
    }

    /// Every row cites the chapter its figures come from, and every row belongs
    /// to the Sparte of the table it sits in.
    #[test]
    fn every_row_cites_a_chapter_and_names_its_sparte() {
        for (tabelle, sparte) in [(STROM, Sparte::Strom), (GAS, Sparte::Gas)] {
            for m in tabelle {
                assert_eq!(m.sparte, sparte, "{}", m.key);
                assert!(
                    m.fundstelle.contains("Kap."),
                    "{} nennt kein Kapitel: {}",
                    m.key,
                    m.fundstelle
                );
                assert!(!m.prozess.is_empty(), "{}", m.key);
                assert!(!m.kapitel.is_empty(), "{}", m.key);
            }
        }
    }

    /// Kap. 5's „Thema" column spans rows; Gas has no such column, and filling
    /// one in would be this crate's invention.
    #[test]
    fn only_strom_carries_a_thema() {
        assert!(STROM.iter().all(|m| m.thema.is_some()));
        assert!(GAS.iter().all(|m| m.thema.is_none()));
        assert!(
            GAS.iter()
                .all(|m| m.lokationsebene == Lokationsebene::NichtAusgewiesen)
        );
    }

    /// „ggf." and „teilw." are not decoration: Kap. 7.2 is owed only where the
    /// EDIFACT-Kommunikation to that DB is not already up, and „teilw. 7.3"
    /// binds only the Basisdaten a given Abrechnungsdaten-Übermittlung needs.
    #[test]
    fn the_qualified_prerequisites_are_kept_apart() {
        let netznutzung = STROM
            .iter()
            .find(|m| m.key == "strom.abrechnungsdaten-netznutzungsabrechnung")
            .expect("in der Tabelle");
        assert_eq!(
            netznutzung.vorbedingungen,
            &[
                Vorbedingung::erforderlich("6.4"),
                Vorbedingung::erforderlich("7.1"),
                Vorbedingung::bedingt("7.2"),
                Vorbedingung::teilweise("7.3"),
            ]
        );
        // The three non-Abrechnungs rows of Kap. 7.5 carry no „teilw. 7.3".
        let bilanzkreistreue = STROM
            .iter()
            .find(|m| m.key == "strom.stammdaten-bilanzkreistreue")
            .expect("in der Tabelle");
        assert!(
            !bilanzkreistreue
                .vorbedingungen
                .iter()
                .any(|v| v.kapitel == "7.3")
        );
    }

    /// The MSB row of Kap. 7.3 names „normierte Profile" and the LF row
    /// „normierte Profile und Profilscharen". Copying the LF wording onto the
    /// MSB row would oblige the NBN to send a Profilschar no chapter asks for.
    #[test]
    fn profilscharen_go_only_to_the_lieferant() {
        let mit_scharen: Vec<_> = STROM
            .iter()
            .filter(|m| m.prozess.contains("Profilscharen"))
            .map(|m| m.key)
            .collect();
        assert_eq!(
            mit_scharen,
            ["strom.normierte-profile-und-profilscharen-lf"]
        );
    }

    /// Kap. 6.1 and 7.2 say „und", not „an" — Kommunikationsdaten are exchanged
    /// in both directions, and flattening them into one sender loses an
    /// obligation.
    #[test]
    fn the_kommunikationsdaten_rows_are_an_exchange() {
        for key in [
            "strom.kommunikationsdaten-nba-nbn",
            "strom.kommunikationsdaten-nbn-db",
        ] {
            let m = STROM.iter().find(|m| m.key == key).expect("in der Tabelle");
            assert!(
                matches!(m.beteiligte, Beteiligte::Wechselseitig(_)),
                "{key}"
            );
        }
        assert!(
            STROM
                .iter()
                .find(|m| m.key == "strom.kommunikationsdaten-nba-nbn")
                .unwrap()
                .beteiligte
                .beteiligt(Rolle::Nbn)
        );
    }

    /// Kap. 7.4's Rollen are BIKO, BKV, RB, EIV, LV and EZ — collected by the
    /// Anwendungshilfe under „weiterer DB", and both NB send.
    #[test]
    fn both_netzbetreiber_inform_the_weiterer_db() {
        let m = STROM
            .iter()
            .find(|m| m.key == "strom.information-weiterer-db")
            .expect("in der Tabelle");
        let (absender, empfaenger) = m.beteiligte.rollen();
        assert_eq!(absender, &[Rolle::Nba, Rolle::Nbn]);
        assert_eq!(empfaenger, &[Rolle::WeitererDb]);
    }

    /// Gas Kap. 4.2.2 Nr. 2, 4.3.2 Nr. 3/4, 4.4.2 Nr. 2 and 4.5.2 Nr. 2 all say
    /// „Unverzüglich, spätestens jedoch 3 WT nach Kenntnisnahme".
    #[test]
    fn gas_updates_are_due_within_three_werktage() {
        assert_eq!(GAS_AKTUALISIERUNG_WT, 3);
    }
}

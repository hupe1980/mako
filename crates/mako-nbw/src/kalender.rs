//! The **Fristenkalender** — the published milestones of one Sparte, and the
//! dated plan they turn into.
//!
//! # The ordering column is a constraint, not a hint
//!
//! Strom Kap. 5 carries a column „vorab durchzuführen (Kapitel-Nr.)", and the
//! chapters it names are genuine preconditions rather than a suggested reading
//! order. Kap. 6.3 („Lokationsbündelstruktur und DB") states as its Vorbedingung
//! that „Die EDIFACT-Kommunikation ist aufgebaut" — which is Kap. 6.1. Kap. 9.1
//! states that „Die vom NB-Wechsel betroffenen Lokationen liegen dem gMSBA vom
//! NBA vor (siehe Kapitel 7.1)". Running a milestone whose prerequisite has not
//! happened sends data to a party that cannot place it.
//!
//! So [`Fristenkalender::plan`] does two things a sorted list would not:
//!
//! - it orders the milestones **topologically**, so a caller walking the plan
//!   never reaches a step before what it depends on; and
//! - it **refuses** — not warns — a table in which a prerequisite falls after
//!   the milestone depending on it. A plan that cannot be executed in the order
//!   it prescribes is not a plan, and a warning on a months-long migration is
//!   read once and then never again.
//!
//! Milestones sharing a Frist are not an ordering violation: Kap. 5 puts 6.1 and
//! 6.3 both at four months, and 6.4 and 7.5 both at two, so a prerequisite due
//! on the *same* day as its dependent is exactly what the published table says.

use std::collections::BTreeSet;

use time::Date;

use crate::{Aenderungszeitpunkt, Meilenstein, Sparte, Vorbedingung, meilenstein};

/// The milestones of one Sparte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fristenkalender {
    sparte: Sparte,
    meilensteine: &'static [Meilenstein],
}

impl Fristenkalender {
    /// The Strom calendar — BDEW-Anwendungshilfe „Marktprozesse
    /// Netzbetreiberwechsel Sparte Strom" V1.2, Kap. 5.
    #[must_use]
    pub const fn strom() -> Self {
        Self {
            sparte: Sparte::Strom,
            meilensteine: meilenstein::STROM,
        }
    }

    /// The Gas calendar — BDEW/VKU/GEODE-Anwendungshilfe „Marktprozesse
    /// Netzbetreiberwechsel Sparte Gas" V1.0, Kap. 4.1.2.
    #[must_use]
    pub const fn gas() -> Self {
        Self {
            sparte: Sparte::Gas,
            meilensteine: meilenstein::GAS,
        }
    }

    /// The calendar of a Sparte.
    #[must_use]
    pub const fn fuer(sparte: Sparte) -> Self {
        match sparte {
            Sparte::Strom => Self::strom(),
            Sparte::Gas => Self::gas(),
        }
    }

    /// The Sparte this calendar belongs to.
    #[must_use]
    pub const fn sparte(self) -> Sparte {
        self.sparte
    }

    /// Every milestone, in the order the published table lists them.
    #[must_use]
    pub const fn meilensteine(self) -> &'static [Meilenstein] {
        self.meilensteine
    }

    /// Look a milestone up by its slug.
    #[must_use]
    pub fn meilenstein(self, key: &str) -> Option<&'static Meilenstein> {
        self.meilensteine.iter().find(|m| m.key == key)
    }

    /// Every milestone of one chapter. Strom Kap. 7.3 and 7.5 have five each,
    /// Kap. 8 has two, so a prerequisite naming a chapter binds all of them.
    pub fn kapitel(self, kapitel: &str) -> impl Iterator<Item = &'static Meilenstein> {
        let kapitel = kapitel.to_owned();
        self.meilensteine
            .iter()
            .filter(move |m| m.kapitel == kapitel)
    }

    /// Check the table against itself, without dating it.
    ///
    /// Catches the two defects that make a plan meaningless: a prerequisite
    /// naming a chapter no milestone has, and a cycle in the chapter graph.
    ///
    /// # Errors
    ///
    /// [`KalenderFehler`] naming the milestone at fault.
    pub fn validieren(self) -> Result<(), KalenderFehler> {
        for m in self.meilensteine {
            for v in m.vorbedingungen {
                if !self.meilensteine.iter().any(|k| k.kapitel == v.kapitel) {
                    return Err(KalenderFehler::UnbekannteVorbedingung {
                        meilenstein: m.key,
                        kapitel: v.kapitel,
                    });
                }
                if v.kapitel == m.kapitel {
                    return Err(KalenderFehler::VorbedingungAufSichSelbst {
                        meilenstein: m.key,
                        kapitel: v.kapitel,
                    });
                }
            }
        }
        // A chapter graph without a topological order has a cycle.
        if let Err(offen) = self.topologisch(&self.faelligkeiten_leer()) {
            return Err(KalenderFehler::Zyklus { meilenstein: offen });
        }
        Ok(())
    }

    /// Date every milestone against an Änderungszeitpunkt and put them in
    /// dependency order.
    ///
    /// Ties are broken by due date and then by the row's position in the
    /// published table, so the plan is stable and reads in the order the
    /// Anwendungshilfe prints. Milestones whose Frist the table does not
    /// quantify sort last within their dependency level — they have no date, and
    /// pretending otherwise would put a deadline on them.
    ///
    /// # Errors
    ///
    /// [`PlanFehler::FalscheSparte`] when the Änderungszeitpunkt was validated
    /// against a different Anwendungshilfe; [`PlanFehler::Kalender`] for a
    /// defect in the table itself; [`PlanFehler::VorbedingungZuSpaet`] when a
    /// prerequisite falls after the milestone depending on it.
    pub fn plan(
        self,
        aenderungszeitpunkt: Aenderungszeitpunkt,
    ) -> Result<Vec<GeplanterMeilenstein>, PlanFehler> {
        if aenderungszeitpunkt.sparte() != self.sparte {
            return Err(PlanFehler::FalscheSparte {
                kalender: self.sparte,
                zeitpunkt: aenderungszeitpunkt.sparte(),
            });
        }
        self.validieren()?;

        let faellig: Vec<Option<Date>> = self
            .meilensteine
            .iter()
            .map(|m| m.faellig(aenderungszeitpunkt))
            .collect();

        // Teeth: a prerequisite that is due *after* what depends on it makes the
        // published order unexecutable.
        for (i, m) in self.meilensteine.iter().enumerate() {
            let Some(ziel) = faellig[i] else { continue };
            for v in m.vorbedingungen {
                for (j, vorher) in self.meilensteine.iter().enumerate() {
                    if vorher.kapitel != v.kapitel {
                        continue;
                    }
                    let Some(vorher_faellig) = faellig[j] else {
                        continue;
                    };
                    if vorher_faellig > ziel {
                        return Err(PlanFehler::VorbedingungZuSpaet {
                            meilenstein: m.key,
                            faellig: ziel,
                            vorbedingung: vorher.key,
                            vorbedingung_faellig: vorher_faellig,
                        });
                    }
                }
            }
        }

        let reihenfolge = self
            .topologisch(&faellig)
            .map_err(|meilenstein| PlanFehler::Kalender(KalenderFehler::Zyklus { meilenstein }))?;

        Ok(reihenfolge
            .into_iter()
            .map(|i| GeplanterMeilenstein {
                meilenstein: &self.meilensteine[i],
                faellig: faellig[i],
            })
            .collect())
    }

    /// All-`None` due dates, so [`Self::validieren`] can reuse the topological
    /// sort without an Änderungszeitpunkt.
    fn faelligkeiten_leer(self) -> Vec<Option<Date>> {
        vec![None; self.meilensteine.len()]
    }

    /// Kahn's algorithm over the chapter graph, tie-broken by due date and then
    /// by table position.
    ///
    /// Returns the offending milestone's key when no order exists.
    fn topologisch(self, faellig: &[Option<Date>]) -> Result<Vec<usize>, &'static str> {
        let n = self.meilensteine.len();
        // `kanten[i]` = the indices `i` must follow.
        let kanten: Vec<Vec<usize>> = self
            .meilensteine
            .iter()
            .map(|m| {
                self.meilensteine
                    .iter()
                    .enumerate()
                    .filter(|(_, k)| {
                        m.vorbedingungen
                            .iter()
                            .any(|v: &Vorbedingung| v.kapitel == k.kapitel)
                    })
                    .map(|(j, _)| j)
                    .collect()
            })
            .collect();

        let mut erledigt = vec![false; n];
        let mut reihenfolge = Vec::with_capacity(n);
        while reihenfolge.len() < n {
            let naechster = (0..n)
                .filter(|&i| !erledigt[i])
                .filter(|&i| kanten[i].iter().all(|&j| erledigt[j]))
                // `None` sorts after `Some` in `Option`'s own ordering, which is
                // the wrong way round here: an undated milestone must come last.
                .min_by_key(|&i| (faellig[i].map_or(Date::MAX, |d| d), i));
            if let Some(i) = naechster {
                erledigt[i] = true;
                reihenfolge.push(i);
            } else {
                let offen = (0..n)
                    .find(|&i| !erledigt[i])
                    .map_or("", |i| self.meilensteine[i].key);
                return Err(offen);
            }
        }
        Ok(reihenfolge)
    }
}

/// One milestone with the date it is due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeplanterMeilenstein {
    /// The published row.
    pub meilenstein: &'static Meilenstein,
    /// The latest date the first transmission should happen, where the
    /// Anwendungshilfe quantifies the Frist.
    ///
    /// `None` for a row whose Frist cell names another document — Strom Kap. 9.2
    /// („Ende Messstellenbetrieb", WiM Strom Teil 1) and the two Gas
    /// Werteübermittlungen (Kap. 4.8, 4.9).
    pub faellig: Option<Date>,
}

/// A defect in the milestone table itself, independent of any date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KalenderFehler {
    /// A prerequisite names a chapter no milestone carries.
    #[error(
        "{meilenstein} verlangt Kapitel {kapitel} vorab, aber kein Meilenstein trägt dieses \
         Kapitel — die Vorbedingung wäre wirkungslos und der Meilenstein liefe ungeprüft vor \
         seiner Voraussetzung"
    )]
    UnbekannteVorbedingung {
        /// The milestone carrying the prerequisite.
        meilenstein: &'static str,
        /// The chapter it names.
        kapitel: &'static str,
    },
    /// A prerequisite names the milestone's own chapter.
    #[error(
        "{meilenstein} verlangt sein eigenes Kapitel {kapitel} vorab — jede Vorbedingung bindet \
         alle Zeilen des genannten Kapitels, sodass der Meilenstein auf sich selbst wartet"
    )]
    VorbedingungAufSichSelbst {
        /// The milestone.
        meilenstein: &'static str,
        /// Its own chapter.
        kapitel: &'static str,
    },
    /// The chapter graph has a cycle.
    #[error(
        "der Abhängigkeitsgraph enthält einen Zyklus, offen ab {meilenstein} — kein Meilenstein \
         des Zyklus kann je starten"
    )]
    Zyklus {
        /// One milestone that can never start.
        meilenstein: &'static str,
    },
}

/// Why an Änderungszeitpunkt cannot be planned against this calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PlanFehler {
    /// The Änderungszeitpunkt belongs to the other Anwendungshilfe.
    #[error(
        "der Kalender führt die Sparte {kalender}, der Änderungszeitpunkt wurde gegen {zeitpunkt} \
         geprüft — die beiden Anwendungshilfen nummerieren ihre Kapitel unterschiedlich und \
         veröffentlichen unterschiedliche Fristen"
    )]
    FalscheSparte {
        /// The calendar's Sparte.
        kalender: Sparte,
        /// The Änderungszeitpunkt's Sparte.
        zeitpunkt: Sparte,
    },
    /// The table itself is defective.
    #[error(transparent)]
    Kalender(#[from] KalenderFehler),
    /// A prerequisite is due after the milestone depending on it.
    #[error(
        "{meilenstein} ist am {faellig} fällig, seine Vorbedingung {vorbedingung} aber erst am \
         {vorbedingung_faellig} — der Plan wäre in der vorgeschriebenen Reihenfolge nicht \
         ausführbar"
    )]
    VorbedingungZuSpaet {
        /// The dependent milestone.
        meilenstein: &'static str,
        /// Its due date.
        faellig: Date,
        /// The prerequisite milestone.
        vorbedingung: &'static str,
        /// The prerequisite's due date.
        vorbedingung_faellig: Date,
    },
}

/// Every chapter named anywhere in a calendar, as a set — the vocabulary a
/// prerequisite may draw on.
#[must_use]
pub fn kapitelverzeichnis(kalender: Fristenkalender) -> BTreeSet<&'static str> {
    kalender.meilensteine().iter().map(|m| m.kapitel).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Lokationsebene, Rolle, Uebertragung, Vorlauf};
    use time::Month;

    fn d(y: i32, m: Month, day: u8) -> Date {
        Date::from_calendar_date(y, m, day).expect("valid date")
    }

    fn heute() -> Date {
        d(2026, Month::January, 15)
    }

    fn az(sparte: Sparte) -> Aenderungszeitpunkt {
        Aenderungszeitpunkt::neu(d(2027, Month::January, 1), heute(), sparte)
            .expect("gültiger Änderungszeitpunkt")
    }

    /// Both published tables are internally consistent: every „vorab
    /// durchzuführen" chapter exists, none names its own row's chapter, and the
    /// graph has a topological order.
    #[test]
    fn both_published_calendars_validate() {
        Fristenkalender::strom().validieren().expect("Strom Kap. 5");
        Fristenkalender::gas().validieren().expect("Gas Kap. 4.1.2");
    }

    /// The dependency graph is acyclic. A cycle would mean no milestone of the
    /// cycle could ever start, and the topological sort is what proves it.
    #[test]
    fn the_dependency_graph_is_acyclic() {
        for kalender in [Fristenkalender::strom(), Fristenkalender::gas()] {
            let ordnung = kalender
                .topologisch(&kalender.faelligkeiten_leer())
                .expect("azyklisch");
            assert_eq!(ordnung.len(), kalender.meilensteine().len());

            // Independently of the algorithm: every prerequisite appears before
            // every row that depends on it.
            let position = |i: usize| ordnung.iter().position(|&x| x == i).expect("enthalten");
            for (i, m) in kalender.meilensteine().iter().enumerate() {
                for v in m.vorbedingungen {
                    for (j, k) in kalender.meilensteine().iter().enumerate() {
                        if k.kapitel == v.kapitel {
                            assert!(
                                position(j) < position(i),
                                "{} steht nach {} obwohl {} vorab durchzuführen ist",
                                k.key,
                                m.key,
                                v.kapitel
                            );
                        }
                    }
                }
            }
        }
    }

    /// Strom Kap. 5, planned against a concrete Änderungszeitpunkt: every due
    /// date, in the order the plan produces them.
    ///
    /// Änderungszeitpunkt 01.01.2027 — 4 Monate is 01.09.2026, 3 Monate
    /// 01.10.2026, 2 Monate 01.11.2026, 1 Monat 01.12.2026.
    #[test]
    fn the_strom_plan_dates_every_published_milestone() {
        let plan = Fristenkalender::strom()
            .plan(az(Sparte::Strom))
            .expect("planbar");
        assert_eq!(plan.len(), 21);

        let vier = d(2026, Month::September, 1);
        let drei = d(2026, Month::October, 1);
        let zwei = d(2026, Month::November, 1);
        let eins = d(2026, Month::December, 1);

        let erwartet: &[(&str, Option<Date>)] = &[
            ("strom.kommunikationsdaten-nba-nbn", Some(vier)),
            ("strom.liste-der-lokationen-nba-nbn", Some(vier)),
            ("strom.stammdatenaenderung-nba", Some(vier)),
            ("strom.liste-der-lokationen-gmsba-gmsbn", Some(vier)),
            ("strom.lokationsbuendelstruktur-und-db", Some(vier)),
            ("strom.kommunikationsdaten-nbn-db", Some(drei)),
            ("strom.profildefinitionen-lf", Some(drei)),
            ("strom.normierte-profile-und-profilscharen-lf", Some(drei)),
            ("strom.profildefinitionen-msb", Some(drei)),
            ("strom.normierte-profile-msb", Some(drei)),
            ("strom.preisblatt-lf", Some(drei)),
            ("strom.information-weiterer-db", Some(drei)),
            ("strom.ergaenzende-daten-lokationsbuendel", Some(zwei)),
            ("strom.abrechnungsdaten-netznutzungsabrechnung", Some(zwei)),
            ("strom.abrechnungsdaten-bilanzkreisabrechnung", Some(zwei)),
            ("strom.stammdaten-bilanzkreistreue", Some(zwei)),
            ("strom.stammdatenaenderung-nbn", Some(zwei)),
            ("strom.berechnungsformel", Some(zwei)),
            ("strom.stammdatenaenderung-lf", Some(eins)),
            ("strom.stammdatenaenderung-msb", Some(eins)),
            ("strom.ende-messstellenbetrieb", None),
        ];

        let ist: Vec<(&str, Option<Date>)> = plan
            .iter()
            .map(|g| (g.meilenstein.key, g.faellig))
            .collect();
        assert_eq!(ist.as_slice(), erwartet);
    }

    /// Gas Kap. 4.1.2, planned against the same Änderungszeitpunkt. Three of the
    /// seven rows are not stated in whole months, and each is dated the way its
    /// own chapter states it.
    #[test]
    fn the_gas_plan_dates_every_published_milestone() {
        let plan = Fristenkalender::gas()
            .plan(az(Sparte::Gas))
            .expect("planbar");
        assert_eq!(plan.len(), 7);

        let ist: Vec<(&str, Option<Date>)> = plan
            .iter()
            .map(|g| (g.meilenstein.key, g.faellig))
            .collect();

        // Kap. 4.2 — 4 Monate vor 01.01.2027.
        assert_eq!(
            ist[0],
            ("gas.kontaktdaten-db", Some(d(2026, Month::September, 1)))
        );
        // Kap. 4.3 — 3 Monate + 10 WT: 01.10.2026, dann 10 Werktage zurück.
        assert_eq!(
            ist[1],
            (
                "gas.information-db",
                Some(mako_fristen::sub_werktage(
                    d(2026, Month::October, 1),
                    10,
                    meilenstein::KALENDER
                ))
            )
        );
        // Kap. 4.4 — 3 Monate.
        assert_eq!(
            ist[2],
            ("gas.uebergabe-stammdaten", Some(d(2026, Month::October, 1)))
        );
        // Kap. 4.5 — 2 Monate.
        assert_eq!(
            ist[3],
            (
                "gas.uebermittlung-stammdaten",
                Some(d(2026, Month::November, 1))
            )
        );
        // Kap. 4.6 — 25 WT vor dem Änderungszeitpunkt: 24.11.2026, also *nach*
        // der 2-Monats-Zeile, weil 25 Werktage weniger als zwei Monate sind.
        assert_eq!(
            ist[4],
            (
                "gas.uebergang-messstellenbetrieb",
                Some(d(2026, Month::November, 24))
            )
        );
        assert_eq!(
            ist[4].1,
            Some(mako_fristen::sub_werktage(
                d(2027, Month::January, 1),
                25,
                meilenstein::KALENDER
            ))
        );
        // Kap. 4.8 und 4.9 nennen ein anderes Dokument statt einer Frist.
        assert_eq!(ist[5], ("gas.werteuebermittlung-nb", None));
        assert_eq!(ist[6], ("gas.werteuebermittlung-db", None));
    }

    /// Gas Kap. 4.3 is due 3 Monate **+ 10 WT** before, so it falls before the
    /// 3-Monate row of Kap. 4.4 even though both bands read „3 Monate".
    #[test]
    fn the_gas_information_precedes_the_three_month_row() {
        let plan = Fristenkalender::gas()
            .plan(az(Sparte::Gas))
            .expect("planbar");
        let hole = |key: &str| {
            plan.iter()
                .find(|g| g.meilenstein.key == key)
                .and_then(|g| g.faellig)
                .expect("datiert")
        };
        assert!(hole("gas.information-db") < hole("gas.uebergabe-stammdaten"));
    }

    /// Kap. 5 puts prerequisite and dependent on the same date more than once —
    /// 6.1 and 6.3 both at four months, 6.4 and 7.5 both at two. An equal date
    /// is what the published table says and must not be refused.
    #[test]
    fn a_prerequisite_due_on_the_same_day_is_admissible() {
        let plan = Fristenkalender::strom()
            .plan(az(Sparte::Strom))
            .expect("planbar");
        let hole = |key: &str| {
            plan.iter()
                .find(|g| g.meilenstein.key == key)
                .and_then(|g| g.faellig)
                .expect("datiert")
        };
        assert_eq!(
            hole("strom.kommunikationsdaten-nba-nbn"),
            hole("strom.lokationsbuendelstruktur-und-db")
        );
        assert_eq!(
            hole("strom.ergaenzende-daten-lokationsbuendel"),
            hole("strom.stammdatenaenderung-nbn")
        );
        // …but they still come out in dependency order.
        let position = |key: &str| {
            plan.iter()
                .position(|g| g.meilenstein.key == key)
                .expect("enthalten")
        };
        assert!(
            position("strom.kommunikationsdaten-nba-nbn")
                < position("strom.lokationsbuendelstruktur-und-db")
        );
    }

    /// A prerequisite falling *after* its dependent is refused, not warned
    /// about: the plan would not be executable in the order it prescribes.
    #[test]
    fn a_prerequisite_due_later_than_its_dependent_is_refused() {
        static SPAET: &[Meilenstein] = &[
            Meilenstein {
                key: "test.vorher",
                sparte: Sparte::Strom,
                thema: None,
                kapitel: "1",
                prozess: "Vorbedingung",
                beteiligte: crate::Beteiligte::Gerichtet {
                    absender: &[Rolle::Nba],
                    empfaenger: &[Rolle::Nbn],
                },
                // Only one month of lead — later than the row that needs it.
                vorlauf: Vorlauf::Monate(1),
                vorbedingungen: &[],
                lokationsebene: Lokationsebene::Nein,
                uebertragung: Uebertragung::Edifact,
                prozessquelle: None,
                fundstelle: "Kap. 5 — Testtabelle",
            },
            Meilenstein {
                key: "test.nachher",
                sparte: Sparte::Strom,
                thema: None,
                kapitel: "2",
                prozess: "Abhängiger Schritt",
                beteiligte: crate::Beteiligte::Gerichtet {
                    absender: &[Rolle::Nba],
                    empfaenger: &[Rolle::Nbn],
                },
                vorlauf: Vorlauf::Monate(3),
                vorbedingungen: &[Vorbedingung::erforderlich("1")],
                lokationsebene: Lokationsebene::Nein,
                uebertragung: Uebertragung::Edifact,
                prozessquelle: None,
                fundstelle: "Kap. 5 — Testtabelle",
            },
        ];
        let kalender = Fristenkalender {
            sparte: Sparte::Strom,
            meilensteine: SPAET,
        };
        // The table itself is well-formed and acyclic …
        kalender.validieren().expect("azyklisch");
        // … but it cannot be executed in the order it prescribes.
        match kalender.plan(az(Sparte::Strom)) {
            Err(PlanFehler::VorbedingungZuSpaet {
                meilenstein,
                vorbedingung,
                faellig,
                vorbedingung_faellig,
            }) => {
                assert_eq!(meilenstein, "test.nachher");
                assert_eq!(vorbedingung, "test.vorher");
                assert!(vorbedingung_faellig > faellig);
            }
            other => panic!("erwartet VorbedingungZuSpaet, war {other:?}"),
        }
    }

    /// A cycle in the chapter graph is a defect of the table, caught without any
    /// date at all.
    #[test]
    fn a_cyclic_table_is_refused() {
        static ZYKLUS: &[Meilenstein] = &[
            Meilenstein {
                key: "test.a",
                sparte: Sparte::Strom,
                thema: None,
                kapitel: "1",
                prozess: "A",
                beteiligte: crate::Beteiligte::Gerichtet {
                    absender: &[Rolle::Nba],
                    empfaenger: &[Rolle::Nbn],
                },
                vorlauf: Vorlauf::Monate(4),
                vorbedingungen: &[Vorbedingung::erforderlich("2")],
                lokationsebene: Lokationsebene::Nein,
                uebertragung: Uebertragung::Edifact,
                prozessquelle: None,
                fundstelle: "Kap. 5 — Testtabelle",
            },
            Meilenstein {
                key: "test.b",
                sparte: Sparte::Strom,
                thema: None,
                kapitel: "2",
                prozess: "B",
                beteiligte: crate::Beteiligte::Gerichtet {
                    absender: &[Rolle::Nba],
                    empfaenger: &[Rolle::Nbn],
                },
                vorlauf: Vorlauf::Monate(4),
                vorbedingungen: &[Vorbedingung::erforderlich("1")],
                lokationsebene: Lokationsebene::Nein,
                uebertragung: Uebertragung::Edifact,
                prozessquelle: None,
                fundstelle: "Kap. 5 — Testtabelle",
            },
        ];
        let kalender = Fristenkalender {
            sparte: Sparte::Strom,
            meilensteine: ZYKLUS,
        };
        assert!(matches!(
            kalender.validieren(),
            Err(KalenderFehler::Zyklus { .. })
        ));
        assert!(matches!(
            kalender.plan(az(Sparte::Strom)),
            Err(PlanFehler::Kalender(KalenderFehler::Zyklus { .. }))
        ));
    }

    /// A prerequisite naming a chapter nothing carries would be silently
    /// ineffective, so it is refused.
    #[test]
    fn an_unknown_prerequisite_chapter_is_refused() {
        static UNBEKANNT: &[Meilenstein] = &[Meilenstein {
            key: "test.a",
            sparte: Sparte::Strom,
            thema: None,
            kapitel: "1",
            prozess: "A",
            beteiligte: crate::Beteiligte::Gerichtet {
                absender: &[Rolle::Nba],
                empfaenger: &[Rolle::Nbn],
            },
            vorlauf: Vorlauf::Monate(4),
            vorbedingungen: &[Vorbedingung::erforderlich("99")],
            lokationsebene: Lokationsebene::Nein,
            uebertragung: Uebertragung::Edifact,
            prozessquelle: None,
            fundstelle: "Kap. 5 — Testtabelle",
        }];
        let kalender = Fristenkalender {
            sparte: Sparte::Strom,
            meilensteine: UNBEKANNT,
        };
        assert!(matches!(
            kalender.validieren(),
            Err(KalenderFehler::UnbekannteVorbedingung { kapitel: "99", .. })
        ));
    }

    /// The two Anwendungshilfen are not interchangeable, so a Gas
    /// Änderungszeitpunkt cannot be planned against the Strom calendar.
    #[test]
    fn a_calendar_refuses_the_other_spartes_zeitpunkt() {
        assert!(matches!(
            Fristenkalender::strom().plan(az(Sparte::Gas)),
            Err(PlanFehler::FalscheSparte {
                kalender: Sparte::Strom,
                zeitpunkt: Sparte::Gas
            })
        ));
    }

    /// Strom Kap. 7.3 and 7.5 have five rows each and Kap. 8 two — a
    /// prerequisite names a chapter, so it binds all of them.
    #[test]
    fn a_chapter_can_hold_several_milestones() {
        let strom = Fristenkalender::strom();
        assert_eq!(strom.kapitel("7.3").count(), 5);
        assert_eq!(strom.kapitel("7.5").count(), 5);
        assert_eq!(strom.kapitel("8").count(), 2);
        assert_eq!(strom.kapitel("6.1").count(), 1);

        // Both Kap.-8 rows wait for all five Kap.-7.5 rows.
        let plan = strom.plan(az(Sparte::Strom)).expect("planbar");
        let position = |key: &str| {
            plan.iter()
                .position(|g| g.meilenstein.key == key)
                .expect("enthalten")
        };
        for kap75 in strom.kapitel("7.5") {
            assert!(position(kap75.key) < position("strom.stammdatenaenderung-lf"));
            assert!(position(kap75.key) < position("strom.stammdatenaenderung-msb"));
        }
    }

    /// Every milestone is reachable by its own key.
    #[test]
    fn every_milestone_resolves_by_key() {
        for kalender in [Fristenkalender::strom(), Fristenkalender::gas()] {
            for m in kalender.meilensteine() {
                assert_eq!(
                    kalender.meilenstein(m.key).map(|k| k.prozess),
                    Some(m.prozess),
                    "{}",
                    m.key
                );
            }
            assert!(kalender.meilenstein("gibt-es-nicht").is_none());
        }
    }

    /// Strom Kap. 5 names nine distinct chapters, Gas Kap. 4.1.2 seven.
    #[test]
    fn the_chapter_vocabularies_match_the_published_tables() {
        assert_eq!(
            kapitelverzeichnis(Fristenkalender::strom())
                .into_iter()
                .collect::<Vec<_>>(),
            [
                "6.1", "6.2", "6.3", "6.4", "7.1", "7.2", "7.3", "7.4", "7.5", "8", "9.1", "9.2"
            ]
        );
        assert_eq!(
            kapitelverzeichnis(Fristenkalender::gas())
                .into_iter()
                .collect::<Vec<_>>(),
            ["4.2", "4.3", "4.4", "4.5", "4.6", "4.8", "4.9"]
        );
    }
}

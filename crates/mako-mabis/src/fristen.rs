//! The MaBiS Fristenkalender — BK6-24-174 Anlage 3 Kap. 3.10, Tabelle 2.
//!
//! # There is no per-message Prüfmitteilung deadline
//!
//! This module exists because the obvious model is wrong. A Summenzeitreihe
//! arrives and a Prüfmitteilung goes back, so it is tempting to hang a
//! response Frist off the arrival the way GPKE and WiM do. MaBiS does not work
//! that way, and the Festlegung says so twice:
//!
//! - **Kap. 9.8.2 Nr. 1** — „Prüfmitteilung BG-SZR (Kategorie B)", Frist **„–"**.
//!   The NB *may* („kann") answer positively or negatively. Every other
//!   Prüfmitteilung use case in the document carries the same empty Frist cell.
//! - **Kap. 13.8.2** — the section the 1-Werktag claim used to cite — defines no
//!   BKV answer at all. Its two rows are the **BIKO's own** dispatch Fristen
//!   (18. WT vorläufig / 42. WT endgültig) and „Abrechnungssummenzeitreihe
//!   fehlerhaft — im Bedarfsfall", which has no Frist either.
//!
//! What actually bounds a Prüfmitteilung is the **clearing window** of Tabelle 2:
//! once it closes, no further version and no further Prüfmitteilung can change
//! the settlement. That window is a date range anchored on the end of the
//! Bilanzierungsmonat, not a countdown from an arrival instant, and it differs
//! between the BG-SZR and the BK-SZR by two Werktage at each end.
//!
//! The two places a **1 Werktag** Frist genuinely appears are both obligations
//! of the **BIKO**, not of the answering party:
//!
//! | Obligation | Frist | Source |
//! |---|---|---|
//! | BIKO forwards a Prüfmitteilung to the responsible party | „Spätestens am folgenden WT" | Kap. 9.8.2 Nr. 3 |
//! | BIKO sends the Datenstatus | „Spätestens am folgenden WT" | Kap. 9.9.2 Nr. 1 |
//!
//! [`BIKO_WEITERLEITUNG_WERKTAGE`] and [`BIKO_DATENSTATUS_WERKTAGE`] carry them.
//!
//! # Tabelle 2
//!
//! Every Frist below is anchored on the **end of the Bilanzierungsmonat** and
//! counted in Werktage per the GPKE Werktagsdefinition (Kap. 3.1). „Sie beziehen
//! sich auf den Zeitpunkt des **Eingangs** einer Meldung beim BIKO" — the clock
//! measures arrival at the BIKO, not dispatch.
//!
//! | Zeitreihe | BKA Erstaufschlag | BKA Clearing | KBKA |
//! |---|---|---|---|
//! | BG-SZR (Kategorie B) | 1.–10. WT | 11.–30. WT | 31. WT – Ende 7. Monat |
//! | BK-SZR (Kategorie A und B) | 1.–12. WT | 13.–30. WT | 31. WT – Ende 7. Monat |
//! | DZÜ | — | 31.–34. WT | 1.–8. WT des 8. Monats |
//!
//! # Kapitel 17 has a second Fristentabelle
//!
//! The Redispatch-Ausfallarbeit series do **not** appear in Tabelle 2 — they have
//! their own table in Kap. 17.3.1.3, with the BK-SZR windows and one row that has
//! no analogue anywhere else:
//!
//! | Zeitreihe | BKA Erstaufschlag | BKA Clearing | KBKA |
//! |---|---|---|---|
//! | monatliche AAÜZ · LF-AASZR | 1.–12. WT | 13.–30. WT | 31. WT – Ende 7. Monat |
//! | **tägliche AAÜZ** | **Folgetag (täglich)** | — | — |
//!
//! The tägliche AAÜZ is the only MaBiS series with a *daily* Frist: it is due the
//! day after the Liefertag, and it has no Clearingphase because Kap. 17.2 is
//! Bilanzkreismonitoring rather than settlement. It is also the series that
//! disappears on 30.09.2026 ([`crate::zeitreihen::KAPITEL_17_2_ENDE`]).
//!
//! | Abrechnungsstichtag | BKA | KBKA |
//! |---|---|---|
//! | Vorläufige Bilanzierung | 18. WT, Datenstand 15. WT | 8. WT des 5. Monats, Datenstand Ende 4. Monat |
//! | Abrechnungsrelevante Bilanzierung | 42. WT, Datenstand 30. WT | Ende 8. Monat, Datenstand Ende 7. Monat |
//!
//! # Why the Erstaufschlag window is load-bearing
//!
//! Kap. 3.8.3: a version that reaches the BIKO **inside** the Erstaufschlag
//! window is assigned „Abrechnungsdaten" automatically. One that arrives after
//! it gets „Prüfdaten" and only a **positive** Prüfmitteilung promotes it. Filing
//! a day late therefore does not merely miss a deadline — it changes the
//! settlement path, and silently, because the message is accepted either way.
//! [`Bilanzierungsmonat::phase`] is what tells the two apart.

use time::Date;

use crate::zeitreihen::{Familie, Kategorie, Zeitreihe};

/// Werktag calendar MaBiS counts in — the GPKE definition (Kap. 3.1).
pub const KALENDER: mako_fristen::HolidayCalendar = mako_fristen::HolidayCalendar::BdewMaKo;

// ── The two genuine 1-Werktag obligations ───────────────────────────────────

/// The BIKO forwards a received Prüfmitteilung „spätestens am folgenden WT"
/// (Kap. 9.8.2 Nr. 3). An **abgewiesene** Prüfmitteilung is not forwarded at all
/// (Nr. 2), so this Frist only starts once the Abweisung check has passed.
pub const BIKO_WEITERLEITUNG_WERKTAGE: u32 = 1;

/// The BIKO sends the Datenstatus „spätestens am folgenden WT" (Kap. 9.9.2
/// Nr. 1), and sends it „unabhängig davon, ob er sich geändert hat oder nicht".
pub const BIKO_DATENSTATUS_WERKTAGE: u32 = 1;

// No deadline label accompanies the two 1-Werktag Fristen above: they are the
// **BIKO's** obligations, and mako does not play BIKO. The constants are here
// because the numbers matter for reasoning about what the counterparty owes —
// a Datenstatus that has not arrived a Werktag after a Prüfmitteilung is late,
// and that is worth knowing — but registering a deadline for an obligation this
// participant does not hold would fire into a workflow with no arm to answer it.

/// Deadline label for the close of a clearing window — the point after which a
/// Summenzeitreihe version can no longer change the settlement.
pub const CLEARING_ENDE_LABEL: &str = "mabis-clearingfenster-ende";

// ── Abrechnungslauf ─────────────────────────────────────────────────────────

/// Which settlement run a Frist belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Abrechnungslauf {
    /// Bilanzkreisabrechnung ohne Korrektur — the ordinary monthly run.
    Bka,
    /// Korrekturbilanzkreisabrechnung — the later correction run.
    Kbka,
}

// ── Phase ───────────────────────────────────────────────────────────────────

/// Where a date sits in the settlement lifecycle of one Bilanzierungsmonat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Before the Bilanzierungsmonat has ended — nothing is due yet.
    Vorlaufend,
    /// Erstaufschlag: a version arriving here is assigned „Abrechnungsdaten"
    /// automatically (Kap. 3.8.3).
    Erstaufschlag,
    /// BKA clearing: a new version arrives as „Prüfdaten" and needs a positive
    /// Prüfmitteilung to be promoted.
    Clearing,
    /// Between the close of the BKA clearing window and the start of the KBKA.
    ZwischenLaeufen,
    /// KBKA clearing.
    Kbka,
    /// Both windows have closed; no version can still change the settlement.
    Geschlossen,
}

impl Phase {
    /// Whether a version arriving in this phase is assigned „Abrechnungsdaten"
    /// automatically under the Erstaufschlagsrecht (Kap. 3.8.3).
    #[must_use]
    pub fn ist_erstaufschlag(self) -> bool {
        self == Self::Erstaufschlag
    }

    /// Whether a new version may still be filed at all.
    #[must_use]
    pub fn nimmt_versionen_an(self) -> bool {
        matches!(self, Self::Erstaufschlag | Self::Clearing | Self::Kbka)
    }
}

// ── Fenster ─────────────────────────────────────────────────────────────────

/// A closed date window `[von, bis]`, both bounds inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Fenster {
    /// First day of the window.
    pub von: Date,
    /// Last day of the window.
    pub bis: Date,
}

impl Fenster {
    /// Whether `date` falls inside the window.
    #[must_use]
    pub fn enthaelt(self, date: Date) -> bool {
        self.von <= date && date <= self.bis
    }
}

// ── Bilanzierungsmonat ──────────────────────────────────────────────────────

/// One settlement month, and every Frist Tabelle 2 hangs off it.
///
/// Construct from the **last day** of the Bilanzierungsmonat; all Werktag counts
/// start there, so the *n*-th Werktag is `n` Werktage after that day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bilanzierungsmonat {
    /// Last calendar day of the Bilanzierungsmonat.
    monatsende: Date,
}

impl Bilanzierungsmonat {
    /// Build from the last calendar day of the month.
    #[must_use]
    pub fn new(monatsende: Date) -> Self {
        Self { monatsende }
    }

    /// Build from any day inside the month.
    ///
    /// # Panics
    ///
    /// Never for a date the `time` crate can represent — the month length is
    /// looked up from the calendar.
    #[must_use]
    pub fn enthaltend(tag: Date) -> Self {
        let letzter = time::util::days_in_month(tag.month(), tag.year());
        Self::new(
            Date::from_calendar_date(tag.year(), tag.month(), letzter)
                .expect("last day of a real month"),
        )
    }

    /// The last calendar day of the Bilanzierungsmonat.
    #[must_use]
    pub fn monatsende(self) -> Date {
        self.monatsende
    }

    /// The *n*-th Werktag after the Bilanzierungsmonat.
    #[must_use]
    pub fn werktag(self, n: u32) -> Date {
        mako_fristen::add_werktage(self.monatsende, n, KALENDER)
    }

    /// The last calendar day of the *n*-th month after the Bilanzierungsmonat.
    ///
    /// Tabelle 2 states the KBKA bounds as „Ende 7. Monat" / „Ende 8. Monat",
    /// counted from the Bilanzierungsmonat itself: month 1 is the following
    /// month, so „Ende 7. Monat" is the end of the seventh month after it.
    ///
    /// # Panics
    ///
    /// Never for a Bilanzierungsmonat in the representable calendar range.
    #[must_use]
    pub fn monatsende_nach(self, n: u32) -> Date {
        let mut d = self.monatsende;
        for _ in 0..n {
            // `replace_day(1)` first so a 31-day month does not overflow into
            // the month after next when the following month is shorter.
            d = d
                .replace_day(1)
                .expect("day 1 is valid in every month")
                .checked_add(time::Duration::days(32))
                .expect("date overflow")
                .replace_day(1)
                .expect("day 1 is valid in every month");
            let letzter = time::util::days_in_month(d.month(), d.year());
            d = d.replace_day(letzter).expect("last day of a real month");
        }
        d
    }

    /// The *n*-th Werktag of the *m*-th month after the Bilanzierungsmonat.
    ///
    /// Used for the KBKA DZÜ window, which Tabelle 2 states as „1. WT des
    /// 8. Monats – 8. WT des 8. Monats".
    #[must_use]
    pub fn werktag_im_monat(self, monat: u32, n: u32) -> Date {
        let vormonatsende = self.monatsende_nach(monat.saturating_sub(1));
        mako_fristen::add_werktage(vormonatsende, n, KALENDER)
    }

    // ── Datenlieferungsfristen (Tabelle 2, upper block) ─────────────────────

    /// The Erstaufschlag window for `zeitreihe`, or `None` where Tabelle 2
    /// defines none.
    ///
    /// Only the BKA has an Erstaufschlag; the KBKA column reads „./." for both
    /// Summenzeitreihen rows, and the DZÜ has no Erstaufschlag in either run.
    #[must_use]
    pub fn erstaufschlag(self, zeitreihe: Zeitreihe, lauf: Abrechnungslauf) -> Option<Fenster> {
        if lauf != Abrechnungslauf::Bka {
            return None;
        }
        let bis = match tabellenzeile(zeitreihe)? {
            Zeile::BgSzr => 10,
            Zeile::BkSzr => 12,
            Zeile::Dzue => return None,
            // „Folgetag (täglich)" — the whole obligation is one day wide, and
            // there is no later phase to fall into.
            Zeile::TaeglicheAauez => {
                return Some(Fenster {
                    von: self.monatsende.next_day()?,
                    bis: self.monatsende.next_day()?,
                });
            }
        };
        Some(Fenster {
            von: self.werktag(1),
            bis: self.werktag(bis),
        })
    }

    /// The clearing window for `zeitreihe` in `lauf`, or `None` where Tabelle 2
    /// defines none.
    #[must_use]
    pub fn clearing(self, zeitreihe: Zeitreihe, lauf: Abrechnungslauf) -> Option<Fenster> {
        let zeile = tabellenzeile(zeitreihe)?;
        Some(match (zeile, lauf) {
            (Zeile::BgSzr, Abrechnungslauf::Bka) => Fenster {
                von: self.werktag(11),
                bis: self.werktag(30),
            },
            (Zeile::BkSzr, Abrechnungslauf::Bka) => Fenster {
                von: self.werktag(13),
                bis: self.werktag(30),
            },
            (Zeile::Dzue, Abrechnungslauf::Bka) => Fenster {
                von: self.werktag(31),
                bis: self.werktag(34),
            },
            (Zeile::BgSzr | Zeile::BkSzr, Abrechnungslauf::Kbka) => Fenster {
                von: self.werktag(31),
                bis: self.monatsende_nach(7),
            },
            (Zeile::Dzue, Abrechnungslauf::Kbka) => Fenster {
                von: self.werktag_im_monat(8, 1),
                bis: self.werktag_im_monat(8, 8),
            },
            // Kap. 17.3.1.3 gives the tägliche AAÜZ no Clearingphase: Kap. 17.2
            // is Bilanzkreismonitoring, one direction only, and it carries
            // neither Prüfmitteilung nor Datenstatus.
            (Zeile::TaeglicheAauez, _) => return None,
        })
    }

    // ── Abrechnungsstichtage (Tabelle 2, lower block) ───────────────────────

    /// Vorläufige Bilanzierung — the day by which the BIKO must have dispatched
    /// the preliminary Abrechnungssummenzeitreihen, and the Datenstand it is
    /// computed on (Kap. 13.8.2 Nr. 1).
    #[must_use]
    pub fn vorlaeufige_bilanzierung(self, lauf: Abrechnungslauf) -> Stichtag {
        match lauf {
            Abrechnungslauf::Bka => Stichtag {
                faellig: self.werktag(18),
                datenstand: self.werktag(15),
            },
            Abrechnungslauf::Kbka => Stichtag {
                faellig: self.werktag_im_monat(5, 8),
                datenstand: self.monatsende_nach(4),
            },
        }
    }

    /// Abrechnungsrelevante Bilanzierung — the day the settled versions receive
    /// the Datenstatus „abgerechnete Daten" bzw. „abgerechnete Daten KBKA".
    #[must_use]
    pub fn abrechnungsrelevante_bilanzierung(self, lauf: Abrechnungslauf) -> Stichtag {
        match lauf {
            Abrechnungslauf::Bka => Stichtag {
                faellig: self.werktag(42),
                datenstand: self.werktag(30),
            },
            Abrechnungslauf::Kbka => Stichtag {
                faellig: self.monatsende_nach(8),
                datenstand: self.monatsende_nach(7),
            },
        }
    }

    // ── Phase ───────────────────────────────────────────────────────────────

    /// Where `date` sits in the lifecycle of this month for `zeitreihe`.
    ///
    /// The answer drives the Datenstatus a newly filed version receives
    /// (Kap. 3.8.3), so it must come from the calendar rather than from a flag
    /// on the message.
    #[must_use]
    pub fn phase(self, zeitreihe: Zeitreihe, date: Date) -> Phase {
        if date <= self.monatsende {
            return Phase::Vorlaufend;
        }
        if self
            .erstaufschlag(zeitreihe, Abrechnungslauf::Bka)
            .is_some_and(|f| f.enthaelt(date))
        {
            return Phase::Erstaufschlag;
        }
        if self
            .clearing(zeitreihe, Abrechnungslauf::Bka)
            .is_some_and(|f| f.enthaelt(date))
        {
            return Phase::Clearing;
        }
        if self
            .clearing(zeitreihe, Abrechnungslauf::Kbka)
            .is_some_and(|f| f.enthaelt(date))
        {
            return Phase::Kbka;
        }
        // Between the BKA clearing close and the KBKA start — for the DZÜ the
        // BKA window ends at the 34. WT and the KBKA opens in month 8.
        let bka_ende = self
            .clearing(zeitreihe, Abrechnungslauf::Bka)
            .map(|f| f.bis);
        let kbka_start = self
            .clearing(zeitreihe, Abrechnungslauf::Kbka)
            .map(|f| f.von);
        match (bka_ende, kbka_start) {
            (Some(ende), Some(start)) if date > ende && date < start => Phase::ZwischenLaeufen,
            _ => Phase::Geschlossen,
        }
    }
}

/// A settlement milestone: the day it is due and the data cut-off it uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Stichtag {
    /// Latest day the dispatch may happen.
    pub faellig: Date,
    /// Datenstand the figures are computed on.
    pub datenstand: Date,
}

// ── Tabelle-2 rows ──────────────────────────────────────────────────────────

/// The Datenlieferungs rows of the two Fristentabellen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zeile {
    /// Tabelle 2: „Erstaufschlag der abrechnungsrelevanten BG-SZR (Kategorie B)".
    BgSzr,
    /// Tabelle 2: „Erstaufschlag der abrechnungsrelevanten BK-SZR (Kategorie A
    /// und Kategorie B)". Kap. 17.3.1.3 gives the monatliche AAÜZ and the
    /// LF-AASZR the same windows.
    BkSzr,
    /// Tabelle 2: „Clearingphase für DZÜ".
    Dzue,
    /// Kap. 17.3.1.3: the tägliche AAÜZ, due „Folgetag (täglich)".
    TaeglicheAauez,
}

/// Which Fristentabellen row governs `zeitreihe`, or `None` if it has none.
///
/// Tabelle 2 names only the **abrechnungsrelevanten** monthly series, and
/// Kap. 17.3.1.3 adds the three Ausfallarbeit ones. A Kategorie-C series is
/// daily and settles nothing (Tabelle 1), the LF-SZR is never
/// settlement-relevant, the NZR's Abstimmung rides the BG-SZR row, and the
/// Abrechnungssummenzeitreihe is what the BIKO *produces* at the
/// Abrechnungsstichtage rather than something filed into a window.
fn tabellenzeile(zeitreihe: Zeitreihe) -> Option<Zeile> {
    match (zeitreihe.familie(), zeitreihe.kategorie()) {
        // „Abstimmung und Übermittlung der NZR" shares the 1.–10. WT cell with
        // the BG-SZR (Kategorie B) Erstaufschlag.
        (Familie::Nzr, _) => Some(Zeile::BgSzr),
        (Familie::BgSzr, Some(Kategorie::B)) => Some(Zeile::BgSzr),
        (Familie::BkSzr, Some(Kategorie::A | Kategorie::B)) => Some(Zeile::BkSzr),
        (Familie::Dzue, _) => Some(Zeile::Dzue),
        // Kap. 17.3.1.3 puts the monatliche AAÜZ and the LF-AASZR on the
        // BK-SZR windows.
        (Familie::Aauez | Familie::LfAaszr, _) => Some(Zeile::BkSzr),
        (Familie::TaeglicheAauez, _) => Some(Zeile::TaeglicheAauez),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zeitreihen::Familie;
    use time::Month;

    fn d(y: i32, m: Month, day: u8) -> Date {
        Date::from_calendar_date(y, m, day).expect("valid date")
    }

    fn bg() -> Zeitreihe {
        Zeitreihe::new(Familie::BgSzr, Some(Kategorie::B)).unwrap()
    }

    fn bk() -> Zeitreihe {
        Zeitreihe::new(Familie::BkSzr, Some(Kategorie::B)).unwrap()
    }

    fn dzue() -> Zeitreihe {
        Zeitreihe::new(Familie::Dzue, None).unwrap()
    }

    #[test]
    fn enthaltend_finds_the_month_end() {
        assert_eq!(
            Bilanzierungsmonat::enthaltend(d(2026, Month::February, 9)).monatsende(),
            d(2026, Month::February, 28)
        );
        // Leap year.
        assert_eq!(
            Bilanzierungsmonat::enthaltend(d(2028, Month::February, 1)).monatsende(),
            d(2028, Month::February, 29)
        );
    }

    #[test]
    fn bg_and_bk_erstaufschlag_windows_differ_by_two_werktage() {
        let m = Bilanzierungsmonat::enthaltend(d(2026, Month::September, 15));
        let gebiet = m.erstaufschlag(bg(), Abrechnungslauf::Bka).unwrap();
        let kreis = m.erstaufschlag(bk(), Abrechnungslauf::Bka).unwrap();
        assert_eq!(gebiet.von, kreis.von, "both open on the 1. WT");
        assert_eq!(gebiet.bis, m.werktag(10));
        assert_eq!(kreis.bis, m.werktag(12));
        assert_ne!(gebiet.bis, kreis.bis, "10. WT vs 12. WT");
    }

    #[test]
    fn clearing_starts_the_werktag_after_the_erstaufschlag_closes() {
        let m = Bilanzierungsmonat::enthaltend(d(2026, Month::September, 15));
        for z in [bg(), bk()] {
            let e = m.erstaufschlag(z, Abrechnungslauf::Bka).unwrap();
            let c = m.clearing(z, Abrechnungslauf::Bka).unwrap();
            assert_eq!(
                c.von,
                mako_fristen::add_werktage(e.bis, 1, KALENDER),
                "no gap and no overlap between the two windows for {z}"
            );
            assert_eq!(c.bis, m.werktag(30), "both close on the 30. WT");
        }
    }

    #[test]
    fn kbka_opens_on_the_31st_werktag_and_closes_at_the_end_of_month_seven() {
        let m = Bilanzierungsmonat::enthaltend(d(2026, Month::January, 10));
        let k = m.clearing(bg(), Abrechnungslauf::Kbka).unwrap();
        assert_eq!(k.von, m.werktag(31));
        assert_eq!(
            k.bis,
            d(2026, Month::August, 31),
            "Ende 7. Monat nach Januar"
        );
    }

    #[test]
    fn dzue_has_no_erstaufschlag_and_its_own_windows() {
        let m = Bilanzierungsmonat::enthaltend(d(2026, Month::January, 10));
        assert!(m.erstaufschlag(dzue(), Abrechnungslauf::Bka).is_none());
        let bka = m.clearing(dzue(), Abrechnungslauf::Bka).unwrap();
        assert_eq!(bka.von, m.werktag(31));
        assert_eq!(bka.bis, m.werktag(34));
        let kbka = m.clearing(dzue(), Abrechnungslauf::Kbka).unwrap();
        // „1. WT des 8. Monats – 8. WT des 8. Monats": month 8 after January is
        // September 2026, so the window opens on the first Werktag of September.
        assert_eq!(kbka.von, d(2026, Month::September, 1));
        assert_eq!(kbka.bis, d(2026, Month::September, 10));
    }

    #[test]
    fn abrechnungsstichtage_match_tabelle_2() {
        let m = Bilanzierungsmonat::enthaltend(d(2026, Month::January, 10));
        let vor = m.vorlaeufige_bilanzierung(Abrechnungslauf::Bka);
        assert_eq!(vor.faellig, m.werktag(18));
        assert_eq!(vor.datenstand, m.werktag(15));
        let end = m.abrechnungsrelevante_bilanzierung(Abrechnungslauf::Bka);
        assert_eq!(end.faellig, m.werktag(42));
        assert_eq!(end.datenstand, m.werktag(30));

        let kvor = m.vorlaeufige_bilanzierung(Abrechnungslauf::Kbka);
        assert_eq!(kvor.datenstand, d(2026, Month::May, 31), "Ende 4. Monat");
        let kend = m.abrechnungsrelevante_bilanzierung(Abrechnungslauf::Kbka);
        assert_eq!(kend.faellig, d(2026, Month::September, 30), "Ende 8. Monat");
        assert_eq!(kend.datenstand, d(2026, Month::August, 31), "Ende 7. Monat");
    }

    #[test]
    fn monatsende_nach_survives_short_months() {
        // From 31 January, one month on must be 28/29 February, not 3 March.
        let m = Bilanzierungsmonat::new(d(2026, Month::January, 31));
        assert_eq!(m.monatsende_nach(1), d(2026, Month::February, 28));
        assert_eq!(m.monatsende_nach(2), d(2026, Month::March, 31));
        let leap = Bilanzierungsmonat::new(d(2028, Month::January, 31));
        assert_eq!(leap.monatsende_nach(1), d(2028, Month::February, 29));
    }

    #[test]
    fn phase_separates_erstaufschlag_from_clearing() {
        let m = Bilanzierungsmonat::enthaltend(d(2026, Month::September, 15));
        assert_eq!(m.phase(bg(), m.monatsende()), Phase::Vorlaufend);
        assert_eq!(m.phase(bg(), m.werktag(1)), Phase::Erstaufschlag);
        assert_eq!(m.phase(bg(), m.werktag(10)), Phase::Erstaufschlag);
        assert_eq!(m.phase(bg(), m.werktag(11)), Phase::Clearing);
        // The 11. and 12. WT are still Erstaufschlag for the BK-SZR — the
        // two-Werktag offset is exactly what a single shared window would lose.
        assert_eq!(m.phase(bk(), m.werktag(11)), Phase::Erstaufschlag);
        assert_eq!(m.phase(bk(), m.werktag(12)), Phase::Erstaufschlag);
        assert_eq!(m.phase(bk(), m.werktag(13)), Phase::Clearing);
    }

    #[test]
    fn phase_closes_after_the_kbka() {
        let m = Bilanzierungsmonat::enthaltend(d(2026, Month::January, 10));
        assert_eq!(m.phase(bg(), m.werktag(31)), Phase::Kbka);
        assert_eq!(m.phase(bg(), d(2026, Month::August, 31)), Phase::Kbka);
        assert_eq!(
            m.phase(bg(), d(2026, Month::September, 1)),
            Phase::Geschlossen
        );
    }

    #[test]
    fn dzue_has_a_gap_between_its_two_runs() {
        let m = Bilanzierungsmonat::enthaltend(d(2026, Month::January, 10));
        let after_bka = mako_fristen::add_werktage(m.werktag(34), 1, KALENDER);
        assert_eq!(m.phase(dzue(), after_bka), Phase::ZwischenLaeufen);
    }

    #[test]
    fn the_ausfallarbeit_series_have_their_own_table() {
        // Kap. 17.3.1.3, not Tabelle 2. The monatliche AAÜZ and the LF-AASZR
        // ride the BK-SZR windows; the tägliche AAÜZ is due the following day
        // and has no Clearingphase at all.
        let m = Bilanzierungsmonat::enthaltend(d(2026, Month::September, 15));
        for familie in [Familie::Aauez, Familie::LfAaszr] {
            let z = Zeitreihe::new(familie, None).unwrap();
            assert_eq!(
                m.erstaufschlag(z, Abrechnungslauf::Bka),
                m.erstaufschlag(bk(), Abrechnungslauf::Bka),
                "{z}"
            );
            assert_eq!(
                m.clearing(z, Abrechnungslauf::Bka),
                m.clearing(bk(), Abrechnungslauf::Bka),
                "{z}"
            );
        }

        let taeglich = Zeitreihe::new(Familie::TaeglicheAauez, None).unwrap();
        let fenster = m.erstaufschlag(taeglich, Abrechnungslauf::Bka).unwrap();
        assert_eq!(fenster.von, fenster.bis, "Folgetag is one day wide");
        assert_eq!(fenster.von, m.monatsende().next_day().unwrap());
        assert!(m.clearing(taeglich, Abrechnungslauf::Bka).is_none());
        assert!(m.clearing(taeglich, Abrechnungslauf::Kbka).is_none());
    }

    #[test]
    fn series_without_a_tabelle_2_row_are_always_closed() {
        let m = Bilanzierungsmonat::enthaltend(d(2026, Month::January, 10));
        for z in [
            Zeitreihe::new(Familie::BgSzr, Some(Kategorie::C)).unwrap(),
            Zeitreihe::new(Familie::LfSzr, Some(Kategorie::A)).unwrap(),
            Zeitreihe::new(Familie::Abrechnungssummenzeitreihe, None).unwrap(),
        ] {
            assert!(m.erstaufschlag(z, Abrechnungslauf::Bka).is_none(), "{z}");
            assert!(m.clearing(z, Abrechnungslauf::Bka).is_none(), "{z}");
            assert_eq!(m.phase(z, m.werktag(5)), Phase::Geschlossen, "{z}");
        }
    }

    #[test]
    fn nzr_shares_the_bg_szr_row() {
        let m = Bilanzierungsmonat::enthaltend(d(2026, Month::January, 10));
        let nzr = Zeitreihe::new(Familie::Nzr, None).unwrap();
        assert_eq!(
            m.erstaufschlag(nzr, Abrechnungslauf::Bka),
            m.erstaufschlag(bg(), Abrechnungslauf::Bka)
        );
    }

    #[test]
    fn phase_predicates_agree_with_the_variants() {
        assert!(Phase::Erstaufschlag.ist_erstaufschlag());
        assert!(!Phase::Clearing.ist_erstaufschlag());
        for p in [Phase::Erstaufschlag, Phase::Clearing, Phase::Kbka] {
            assert!(p.nimmt_versionen_an(), "{p:?}");
        }
        for p in [
            Phase::Vorlaufend,
            Phase::ZwischenLaeufen,
            Phase::Geschlossen,
        ] {
            assert!(!p.nimmt_versionen_an(), "{p:?}");
        }
    }
}

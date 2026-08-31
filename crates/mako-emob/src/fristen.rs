//! The Modell-2 deadlines, in one place.
//!
//! The **answer** windows live in [`mako_fristen::antwort`] with every other
//! Antwortfrist, keyed by trigger PID, so `makod`, `processd`, `obsd` and
//! `agentd` cannot disagree about them. What this module adds is the three
//! windows that are not answers to an inbound message and therefore have no
//! entry there.
//!
//! | Frist | Value | Source |
//! |---|---|---|
//! | An-/Abmeldung in/aus Modell 2 | zum Monatsersten, ≥ 1 Monat Vorlauf | AWH Kap. 2.1.2 Nr. 1 / 2.2.2 Nr. 1 |
//! | VNB → LF: Beendigung der Zuordnung | ≤ 3 WT nach Eingang der Anmeldung | AWH Kap. 2.1.2 Nr. 2 |
//! | Aktivierung des MaBiS-ZP für die tägliche BK-SZR eMob | ≥ 1 WT vor dem ersten Versand | AWH Kap. 5.6.1.2 Nr. 1 |
//! | Tägliche BK-SZR eMob | täglich für den Vortag bis 14:00 | AWH Kap. 5.6.3.2 Nr. 1 |
//! | Zuordnung ZP der NGZ zur NZR | ≥ 5 WT vor dem ersten Datenversand | NGZ-AWH Kap. 1.8.2 Nr. 1 |
//! | Antwort darauf | ≤ 1 WT nach Erhalt | NGZ-AWH Kap. 1.8.2 Nr. 2 |
//! | Information an den ÜNB | ≥ 1 WT vor dem Datenversand | NGZ-AWH Kap. 1.8.2 Nr. 4 |

use time::{Date, Time};

use mako_fristen::HolidayCalendar;
use mako_mabis::Bilanzierungsmonat;

pub use mako_fristen::antwort::{MODELL_2_ANMELDUNG_ANTWORT_WT, MODELL_2_DREI_WERKTAGE};

/// Werktage within which the VNB must tell the MaLo's LF that its Zuordnung
/// ends (55240), counted from the Eingang der Anmeldung (AWH Kap. 2.1.2 Nr. 2).
pub const BEENDIGUNG_AN_LF_WT: u32 = 3;

/// Calendar months of lead time an An-/Abmeldung needs (AWH Kap. 2.1.2 Nr. 1,
/// 2.2.2 Nr. 1): „zum Beginn eines Monats mit einer Frist von einem Monat in
/// die Zukunft möglich".
pub const MODELLWECHSEL_VORLAUF_MONATE: u32 = 1;

/// Werktage before the first NGZ send by which the Zuordnung des Zählpunkts der
/// NGZ zur NZR must go out (NGZ-AWH Kap. 1.8.2 Nr. 1).
pub const ZP_NGZ_ZUORDNUNG_VORLAUF_WT: u32 = 5;

/// Werktage within which the neighbouring NB answers that Zuordnung
/// (NGZ-AWH Kap. 1.8.2 Nr. 2). Answered with `E_0102`; the Beendigung with
/// `E_0103` — both already in `mako_pruefung::mabis`.
pub const ZP_NGZ_ANTWORT_WT: u32 = 1;

/// Werktage before the NGZ send by which the ÜNB must have been informed
/// (NGZ-AWH Kap. 1.8.2 Nr. 4).
pub const ZP_NGZ_UENB_INFO_WT: u32 = 1;

/// Werktage before the first daily BK-SZR eMob by which its MaBiS-Zählpunkt
/// must be active (AWH Kap. 5.6.1.2 Nr. 1).
///
/// The activation is owed „spätestens unverzüglich nach der Zuordnung des
/// ersten Ladevorgangs zu einem BK, wenn für die **Kombination aus BK und BG**
/// noch kein MaBiS-ZP für die tägliche BK-SZR eMob aktiv ist" — one Zählpunkt
/// per (Bilanzkreis, Bilanzierungsgebiet) pair, not one per supplier and not
/// one per BG.
pub const BK_SZR_ZP_AKTIVIERUNG_VORLAUF_WT: u32 = 1;

/// The wall-clock instant by which the tägliche BK-SZR eMob for the previous
/// day must have been sent: **14:00** Europe/Berlin (AWH Kap. 5.6.3.2 Nr. 1).
///
/// „Diese Zeitreihe wird nur einmalig für den Vortag ermittelt und versendet.
/// Änderungen an den Basiswerten werden anschließend nur noch in der
/// monatlichen Übermittlung berücksichtigt." So a late CDR does **not** produce
/// a second daily series — it produces a difference the monthly BK-SZR carries.
pub const TAEGLICHE_BK_SZR_UHRZEIT: Time = time::macros::time!(14:00);

/// The earliest Modellwechseltermin an An-/Abmeldung sent on `uebertragungstag`
/// may request.
///
/// „Zum Beginn eines Monats mit einer Frist von einem Monat in die Zukunft":
/// one whole month of lead time, landing on a Monatserster. A message sent on
/// any day in November therefore cannot move a Marktlokation before 1 January.
#[must_use]
pub fn fruehester_modellwechsel(uebertragungstag: Date) -> Date {
    let ein_monat_spaeter = crate::bg::naechster_monatserster(uebertragungstag);
    crate::bg::naechster_monatserster(ein_monat_spaeter)
}

/// The instant the tägliche BK-SZR eMob for `liefertag` is due.
///
/// The series covers `liefertag` and is owed by 14:00 Europe/Berlin on the
/// following calendar day. Not a Werktag rule: the AWH says „täglich".
///
/// # Panics
///
/// Only if `liefertag` is [`Date::MAX`], which the `time` crate places in the
/// year 9999.
#[must_use]
pub fn taegliche_bk_szr_faellig(liefertag: Date) -> time::OffsetDateTime {
    let folgetag = liefertag
        .next_day()
        .expect("a delivery day is never the last representable date");
    mako_fristen::berlin_at(folgetag, TAEGLICHE_BK_SZR_UHRZEIT)
}

/// The last day a corrected allocation may be filed for `monat`.
///
/// MaBiS Kap. 3.10 closes the Korrekturfenster at the **end of month M+7**
/// relative to the Bilanzierungsmonat, and the Modell-2 series are ordinary
/// MaBiS Zeitreihen — the KBKA bound is theirs too. Sourced from
/// [`Bilanzierungsmonat::monatsende_nach`] rather than restated, so it moves
/// with the Festlegung and not with this crate.
#[must_use]
pub fn korrekturfrist(monat: Bilanzierungsmonat) -> Date {
    monat.monatsende_nach(KORREKTURFENSTER_MONATE)
}

/// Months after the Bilanzierungsmonat that the MaBiS Korrekturfenster stays
/// open (Kap. 3.10, „Ende 7. Monat").
pub const KORREKTURFENSTER_MONATE: u32 = 7;

/// The last day the Zuordnung des ZP der NGZ zur NZR may be sent if the first
/// NGZ goes out on `erster_datenversand`.
#[must_use]
pub fn spaetester_zp_ngz_versand(erster_datenversand: Date) -> Date {
    mako_fristen::sub_werktage(
        erster_datenversand,
        ZP_NGZ_ZUORDNUNG_VORLAUF_WT,
        HolidayCalendar::BdewMaKo,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Month;

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::from_calendar_date(y, Month::try_from(m).unwrap(), day).unwrap()
    }

    /// A month of lead time, landing on a Monatserster — so any day in
    /// November reaches 1 January, never 1 December.
    #[test]
    fn a_month_of_lead_time_lands_two_month_starts_ahead() {
        assert_eq!(fruehester_modellwechsel(d(2026, 11, 1)), d(2027, 1, 1));
        assert_eq!(fruehester_modellwechsel(d(2026, 11, 30)), d(2027, 1, 1));
        assert_eq!(fruehester_modellwechsel(d(2026, 12, 15)), d(2027, 2, 1));
    }

    #[test]
    fn the_daily_series_is_due_at_1400_the_next_day() {
        let due = taegliche_bk_szr_faellig(d(2026, 11, 3));
        assert_eq!(mako_fristen::berlin_date(due), d(2026, 11, 4));
        assert_eq!(due.to_offset(due.offset()).time(), TAEGLICHE_BK_SZR_UHRZEIT);
    }

    #[test]
    fn the_zp_zuordnung_needs_five_werktage() {
        // Mon 2026-11-16 minus 5 Werktage → Mon 2026-11-09.
        assert_eq!(spaetester_zp_ngz_versand(d(2026, 11, 16)), d(2026, 11, 9));
    }

    /// The Korrekturfenster ends at the end of the seventh month after the
    /// Bilanzierungsmonat, not seven months after its last day.
    #[test]
    fn the_korrekturfrist_closes_at_the_end_of_month_seven() {
        let november = Bilanzierungsmonat::enthaltend(d(2026, 11, 12));
        assert_eq!(korrekturfrist(november), d(2027, 6, 30));
        // A February start still lands on a month end, short month or not.
        let februar = Bilanzierungsmonat::enthaltend(d(2027, 2, 3));
        assert_eq!(korrekturfrist(februar), d(2027, 9, 30));
    }

    /// The answer windows are the ones `mako-fristen` publishes, not copies.
    #[test]
    fn the_answer_windows_come_from_the_one_table() {
        assert_eq!(MODELL_2_ANMELDUNG_ANTWORT_WT, 7);
        assert_eq!(MODELL_2_DREI_WERKTAGE, 3);
        assert_eq!(BEENDIGUNG_AN_LF_WT, MODELL_2_DREI_WERKTAGE);
    }
}

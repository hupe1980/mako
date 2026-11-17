//! `E_0240` / `E_2006` — the grundzuständiger MSB's answer to a
//! **Verpflichtungsanfrage** (UTILMD 55168 / 44168, NB → gMSB).
//!
//! # Only the Frist is decidable
//!
//! WiM Teil 1 Kap. 2.4.2 Nr. 4 leaves the answer to the gMSB's own judgement —
//! „Der gMSB hat nach eigenem Ermessen erforderliche vorbereitende Maßnahmen zu
//! ergreifen" — so whether it takes over or wants the MSBA weiterverpflichtet
//! is the operator's call. What the message does decide is whether it came in
//! time: Nr. 3 sends it „Frühester ÜT ist der 8. WT und spätester der 5. WT vor
//! dem vorläufig bestätigten Zuordnungsende", and `E_0240` / `E_2006` publish
//! `E17` „Ablehnung wg. Fristüberschreitung" (EBD 4.3 Kap. 8.3.3, S. 540).
//!
//! The anchor rides the message itself: `SG4 DTM+76` „Datum zum geplanten
//! Leistungsbeginn" (UTILMD AHB Strom 2.2, 55168) is the day the gMSB would
//! begin — the day the MSBA's vorläufig bestätigte Zuordnung ends at 00:00.
//!
//! A request sent **before** the window opens has overshot no Frist, so it is
//! not refused `E17`: it escalates with the finding, beside the Ermessens-
//! decision it needs anyway.
//!
//! A late **Gas** request is refused `E17` out of `E_2006` (Codeliste `G_0071`,
//! EBD 4.3 Kap. 14.3.6, S. 776), carried by 44170 (PID-Übersicht 4.0
//! Lesefassung Stand 12.08.2026, Tabelle 1 Lfd. Nr. 39260, S. 76).

use mako_fristen::HolidayCalendar;
use mako_fristen::vorlauf::{VorlaufVerdict, vorlauf};
use time::Date;

use crate::antwort::RejectReason;
use crate::codes::{EBD_VERPFLICHTUNGSANFRAGE, EBD_VERPFLICHTUNGSANFRAGE_GAS, lookup};

use super::types::{MsbEntscheidung, Sparte};

/// The catalogued window the Verpflichtungsanfrage is sent in.
const VORLAUF_KEY: &str = "wim.verpflichtungsanfrage";

/// Judge the Verpflichtungsanfrage's ÜT against its window.
///
/// `Some(Reject(E17))` when the request arrived after the 5. WT vor the
/// Zuordnungsende, `Some(Escalate)` when it arrived before the 8., `None` when
/// it is in time and the gMSB's own decision is all that is left.
///
/// # Panics
///
/// Only if the `wim.verpflichtungsanfrage` row or the `E17` code leaves its
/// catalogue, which a test in this module rules out.
#[must_use]
pub fn pruefe_verpflichtungsanfrage_frist(
    sparte: Sparte,
    uebertragungstag: Date,
    vorlaeufiges_zuordnungsende: Date,
    cal: HolidayCalendar,
) -> Option<MsbEntscheidung> {
    let row = vorlauf(VORLAUF_KEY).expect("the Verpflichtungsanfrage row is catalogued");
    match row
        .shape
        .check(uebertragungstag, vorlaeufiges_zuordnungsende, cal)
    {
        VorlaufVerdict::Ok => None,
        VorlaufVerdict::TooLate { shortfall_wt, .. } => {
            let befund = format!(
                "Verpflichtungsanfrage am {uebertragungstag} übertragen, {shortfall_wt} WT nach \
                 dem spätesten ÜT vor dem vorläufig bestätigten Zuordnungsende \
                 {vorlaeufiges_zuordnungsende} ({})",
                row.source
            );
            let tree = match sparte {
                Sparte::Strom => EBD_VERPFLICHTUNGSANFRAGE,
                Sparte::Gas => EBD_VERPFLICHTUNGSANFRAGE_GAS,
            };
            Some(MsbEntscheidung::Reject(RejectReason::new(
                tree,
                lookup(tree, "E17").expect("E17 is published for the Verpflichtungsanfrage"),
                4,
                befund,
            )))
        }
        VorlaufVerdict::TooEarly { excess_wt } => Some(MsbEntscheidung::Escalate {
            reason: format!(
                "Verpflichtungsanfrage am {uebertragungstag} übertragen, {excess_wt} WT vor dem \
                 frühesten ÜT vor dem vorläufig bestätigten Zuordnungsende \
                 {vorlaeufiges_zuordnungsende} ({}); E17 gilt einer überschrittenen Frist, \
                 nicht einer zu frühen Anfrage — der gMSB entscheidet",
                row.source
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    const CAL: HolidayCalendar = HolidayCalendar::BdewMaKo;

    /// Zuordnungsende Mo 2026-12-14: the 5. WT before it is Mo 07.12., the
    /// 8. WT Mi 02.12.
    const ENDE: Date = date!(2026 - 12 - 14);

    #[test]
    fn a_request_inside_the_window_leaves_the_decision_to_the_gmsb() {
        for uet in [date!(2026 - 12 - 02), date!(2026 - 12 - 07)] {
            assert_eq!(
                pruefe_verpflichtungsanfrage_frist(Sparte::Strom, uet, ENDE, CAL),
                None,
                "ÜT {uet}"
            );
        }
    }

    #[test]
    fn a_late_strom_request_is_refused_e17() {
        match pruefe_verpflichtungsanfrage_frist(Sparte::Strom, date!(2026 - 12 - 08), ENDE, CAL) {
            Some(MsbEntscheidung::Reject(r)) => {
                assert_eq!(r.antwort.antwortcode, "E17");
                assert_eq!(r.antwort.tree, EBD_VERPFLICHTUNGSANFRAGE);
            }
            other => panic!("{other:?}"),
        }
    }

    /// A late Gas request is refused `E17` out of `E_2006`, carried by 44170.
    #[test]
    fn a_late_gas_request_is_refused_e17() {
        match pruefe_verpflichtungsanfrage_frist(Sparte::Gas, date!(2026 - 12 - 08), ENDE, CAL) {
            Some(MsbEntscheidung::Reject(r)) => {
                assert_eq!(r.antwort.antwortcode, "E17");
                assert_eq!(r.antwort.tree, EBD_VERPFLICHTUNGSANFRAGE_GAS);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_early_request_escalates_rather_than_refusing() {
        assert!(matches!(
            pruefe_verpflichtungsanfrage_frist(Sparte::Strom, date!(2026 - 12 - 01), ENDE, CAL),
            Some(MsbEntscheidung::Escalate { .. })
        ));
    }
}

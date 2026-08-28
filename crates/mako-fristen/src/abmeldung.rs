//! **Vorlauffristen der NB-seitigen Abmeldung** — how far ahead the grid
//! operator must announce that a supplier's assignment ends.
//!
//! `E_0609` Prüfschritt 40 („Wurde die Vorlauffrist eingehalten?", Hinweis: „Es
//! ist die maximale und die minimale Vorlauffrist zu prüfen") and its
//! erzeugend twin at 520 are the only steps in the LF's trees whose answer is
//! not a contract fact — it is arithmetic on two dates the message itself
//! carries. Without this table `mako_pruefung` can only escalate, and `A03` /
//! `A22` („Vorlauffrist wurde nicht eingehalten") are unreachable.
//!
//! # The windows
//!
//! GPKE Teil 2 § 2.5.2 SD „Lieferende von NB an LF" Nr. 1 states four, keyed on
//! the Beendigungsgrund:
//!
//! | Grund | Spätester ÜT |
//! |---|---|
//! | `Z33` Auszug wegen Stilllegung, and every other MaLo/Tranche | Tag vor dem letzten WT vor dem Zuordnungsende |
//! | `ZT0` Abmeldung wegen geändertem ZRT | Tag vor dem letzten WT vor dem Zuordnungsende |
//! | EEG-Marktlokationen und Tranchen von EEG-Marktlokationen | 1 Monat vor dem Zuordnungsende |
//! | `ZQ7` Abmeldung wegen Deaktivierung der Zuordnungsermächtigung | anchored on the ÜT der Deaktivierungsmeldung |
//!
//! The `ZQ7` window is deliberately **not** derivable here: its anchor is the
//! transmission day of a Deaktivierungsmeldung between BKV and NB, which never
//! reaches the supplier. `E_0609` checks that case at Prüfschritt 120 instead
//! („Eingangsdatum nach dem 5. WT des Monats, in dem die Zuordnungsermächtigung
//! endet?" → `A09`), so Prüfschritt 40 has nothing to evaluate for it:
//! [`AbmeldungVorlauf::AnPruefschritt120`] is a *pass through the step*, not an
//! unknown.
//!
//! # Sources
//!
//! - BK6-24-174 GPKE Teil 2, § 2.5.2 SD „Lieferende von NB an LF" Nr. 1
//! - BDEW *Entscheidungsbaum-Diagramme und Codelisten* 4.3, `E_0609` 40 / 520

use time::Date;

use crate::HolidayCalendar;
use crate::vorlauf::{VorlaufShape, VorlaufVerdict};

/// `Z33` — Auszug wegen Stilllegung.
pub const STILLLEGUNG: &str = "Z33";
/// `ZQ7` — Abmeldung wegen fehlender Zuordnungsermächtigung (BKV-Deaktivierung).
pub const BKV_DEAKTIVIERUNG: &str = "ZQ7";
/// `ZT0` — Abmeldung wegen fehlender Zuordnungsermächtigung aufgrund Änderung ZRT.
pub const ZRT_AENDERUNG: &str = "ZT0";

/// Which of the four windows applies to a Vorgang.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbmeldungVorlauf {
    /// „Spätester ÜT ist der Tag vor dem letzten WT vor dem Zuordnungsende."
    TagVorDemLetztenWerktag,
    /// „Spätester ÜT liegt 1 Monat vor dem Zuordnungsende" — EEG-Marktlokationen
    /// und Tranchen von EEG-Marktlokationen.
    EinMonat,
    /// The window exists but its anchor is not in the message.
    ///
    /// The caller must escalate rather than treat this as a pass: an
    /// unevaluated Vorlauffrist is not a kept one.
    Unbestimmt {
        /// What is missing, in the Festlegung's own terms.
        grund: &'static str,
    },
    /// `ZQ7` — the Frist for this Grund is a **different Prüfschritt**.
    ///
    /// Its Vorlauffrist hangs on the ÜT of the Deaktivierungsmeldung between
    /// BKV and NB, which never reaches the supplier, so the LF cannot refuse
    /// at Prüfschritt 40 on it. `E_0609` gives the Grund its own, measurable
    /// Frist further down — Prüfschritt 120 / 600, „Liegt das Eingangsdatum der
    /// Abmeldung nach dem 5. WT des Monats, in dem die Zuordnungsermächtigung
    /// endet?" → `A09` / `A28`.
    ///
    /// So this is neither *kept* nor *unknown*: Prüfschritt 40 has nothing to
    /// evaluate and the walk continues. Escalating here would make the whole
    /// BKV-Deaktivierungs-Zweig — Prüfschritte 85, 100, 120 and their `A05` /
    /// `A07` / `A09` — unreachable.
    AnPruefschritt120,
}

impl AbmeldungVorlauf {
    /// The window that governs a Vorgang.
    ///
    /// `eeg` says whether the object is an EEG-Marktlokation or a Tranche of
    /// one. It only ever matters for an erzeugende Marktlokation or a Tranche,
    /// so a caller answering for a verbrauchende or ruhende Marktlokation may
    /// pass `Some(false)` without knowing anything about the EEG.
    #[must_use]
    pub fn fuer(transaktionsgrund: &str, eeg: Option<bool>) -> Self {
        if transaktionsgrund == BKV_DEAKTIVIERUNG {
            return Self::AnPruefschritt120;
        }
        match eeg {
            Some(true) => Self::EinMonat,
            Some(false) => Self::TagVorDemLetztenWerktag,
            None => Self::Unbestimmt {
                grund: "Für EEG-Marktlokationen und Tranchen von EEG-Marktlokationen gilt \
                        ein Monat statt des letzten Werktags (GPKE Teil 2 § 2.5.2 Nr. 1); \
                        ob die Marktlokation eine EEG-Marktlokation ist, ist nicht bekannt.",
            },
        }
    }

    /// The shape, where one applies.
    #[must_use]
    pub const fn shape(self) -> Option<VorlaufShape> {
        match self {
            Self::TagVorDemLetztenWerktag => Some(VorlaufShape::TagVorDemLetztenWerktagVor),
            Self::EinMonat => Some(VorlaufShape::LatestMonateBefore(1)),
            Self::Unbestimmt { .. } | Self::AnPruefschritt120 => None,
        }
    }

    /// `true` when Prüfschritt 40 has nothing to evaluate because the Frist for
    /// this Grund lives at Prüfschritt 120.
    ///
    /// Distinguishes [`Self::AnPruefschritt120`] from [`Self::Unbestimmt`]:
    /// both return `None` from [`Self::check`], but only the second is an
    /// unevaluated window and therefore an escalation.
    #[must_use]
    pub const fn ist_an_pruefschritt_120(self) -> bool {
        matches!(self, Self::AnPruefschritt120)
    }

    /// Check an Übertragungstag against the Zuordnungsende.
    ///
    /// `None` means the window could not be evaluated — never that it passed.
    #[must_use]
    pub fn check(
        self,
        uebertragungstag: Date,
        zuordnungsende: Date,
        cal: HolidayCalendar,
    ) -> Option<VorlaufVerdict> {
        Some(self.shape()?.check(uebertragungstag, zuordnungsende, cal))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    const CAL: HolidayCalendar = HolidayCalendar::BdewMaKo;

    /// Zuordnungsende Tuesday 2026-09-01 → last Werktag before it is Monday
    /// 2026-08-31, so the latest ÜT is Sunday 2026-08-30.
    #[test]
    fn the_general_window_ends_the_day_before_the_last_werktag() {
        let w = AbmeldungVorlauf::fuer(STILLLEGUNG, Some(false));
        assert_eq!(
            w.check(date!(2026 - 08 - 30), date!(2026 - 09 - 01), CAL),
            Some(VorlaufVerdict::Ok)
        );
        let late = w
            .check(date!(2026 - 08 - 31), date!(2026 - 09 - 01), CAL)
            .expect("evaluable");
        assert!(!late.is_ok(), "the last Werktag itself is already too late");
    }

    /// The EEG window is a calendar month, not a Werktag count.
    #[test]
    fn the_eeg_window_is_one_calendar_month() {
        let w = AbmeldungVorlauf::fuer(STILLLEGUNG, Some(true));
        assert_eq!(
            w.check(date!(2026 - 08 - 01), date!(2026 - 09 - 01), CAL),
            Some(VorlaufVerdict::Ok)
        );
        assert!(
            !w.check(date!(2026 - 08 - 02), date!(2026 - 09 - 01), CAL)
                .expect("evaluable")
                .is_ok()
        );
    }

    /// A 31st clamps to the shorter month rather than overflowing (§ 188 Abs. 3 BGB).
    #[test]
    fn a_month_shift_clamps_to_the_last_day() {
        let w = AbmeldungVorlauf::fuer(STILLLEGUNG, Some(true));
        // 2026-03-31 minus one month is 2026-02-28.
        assert_eq!(
            w.check(date!(2026 - 02 - 28), date!(2026 - 03 - 31), CAL),
            Some(VorlaufVerdict::Ok)
        );
    }

    /// The BKV deactivation has no window Prüfschritt 40 can measure — its
    /// anchor is the ÜT of a Deaktivierungsmeldung the supplier never sees —
    /// and `E_0609` gives the Grund its **own** Frist at Prüfschritt 120. So it
    /// is neither kept nor unknown: the step has nothing to evaluate.
    #[test]
    fn the_bkv_window_is_checked_at_pruefschritt_120() {
        let w = AbmeldungVorlauf::fuer(BKV_DEAKTIVIERUNG, Some(false));
        assert_eq!(w, AbmeldungVorlauf::AnPruefschritt120);
        assert!(w.ist_an_pruefschritt_120());
        assert!(
            w.check(date!(2026 - 08 - 31), date!(2026 - 09 - 01), CAL)
                .is_none(),
            "there is still no verdict to report from this step"
        );
        assert!(!AbmeldungVorlauf::fuer(STILLLEGUNG, None).ist_an_pruefschritt_120());
    }

    /// An unknown EEG status is the same kind of unknown.
    #[test]
    fn an_unknown_eeg_status_is_unbestimmt() {
        assert!(matches!(
            AbmeldungVorlauf::fuer(ZRT_AENDERUNG, None),
            AbmeldungVorlauf::Unbestimmt { .. }
        ));
    }
}

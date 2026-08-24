//! Runtime configuration for the NB Anmeldung checks.
//!
//! `mako-pruefung` stays a pure, I/O-free library: the only tunables that vary
//! by operator or by regulatory ambiguity are collected here and passed in by
//! value. Defaults reproduce the exact behaviour the crate shipped with before
//! the config seam existed, so `NetzCheckConfig::default()` is always safe.

use mako_fristen::HolidayCalendar;

/// Tunable parameters for [`crate::evaluate`].
///
/// All fields have regulatory defaults; construct with
/// [`NetzCheckConfig::default`] and override only what an operator needs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetzCheckConfig {
    /// Holiday calendar used for every Werktag computation in the date checks.
    ///
    /// Defaults to [`HolidayCalendar::BdewMaKo`] — the BDEW-defined,
    /// Germany-wide MaKo calendar. A bare Mon–Fri approximation silently
    /// accepts dates the exact calendar pushes out.
    pub holiday_calendar: HolidayCalendar,

    /// Gas Bearbeitungsfrist (in Werktage) added to the 6-week retroactive
    /// window for non-Wechsel SLP Anmeldungen.
    ///
    /// The AWH GeLi Gas 2.0 quantifies the Bearbeitungsfrist only for the
    /// Ersatz-/Grundversorgung (3 WT); the same value is applied to An-/
    /// Abmeldungen as the documented default. Operators whose AWH reading
    /// differs may override it. Defaults to `3`.
    pub gas_bearbeitungsfrist_wt: u32,

    /// EEG-Marktlokation Zuordnungs-Vorlauf, in whole months.
    ///
    /// The assignment of an EEG-/KWKG-Einspeise-MaLo to a Bilanzkreis
    /// (signalled by the `ZW3` „Erzeugende Marktlokation" Transaktionsgrund-
    /// ergänzung; §10c EEG) is only permitted to the **first of a month** and
    /// requires at least this many months of lead. Defaults to `1`.
    pub eeg_zuordnung_vorlauf_monate: u32,
}

impl Default for NetzCheckConfig {
    fn default() -> Self {
        Self {
            holiday_calendar: HolidayCalendar::BdewMaKo,
            gas_bearbeitungsfrist_wt: super::anmeldung::GAS_BEARBEITUNGSFRIST_WT_DEFAULT,
            eeg_zuordnung_vorlauf_monate: super::anmeldung::EEG_ZUORDNUNG_VORLAUF_MONATE_DEFAULT,
        }
    }
}

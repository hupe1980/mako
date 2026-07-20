//! Which rules were in force for a delivery period.
//!
//! German network-charge law is not one timeline but several, each turning over
//! on its own date:
//!
//! | Axis | Turns over | Because |
//! |---|---|---|
//! | Netzzugang | 31.12.2025 | StromNZV and GasNZV lapsed; the competence moved to §20 Abs. 3 EnWG, exercised through BNetzA Festlegungen |
//! | Entgeltbildung | 31.12.2028 | StromNEV and ARegV lapse; the successor is the BNetzA framework Festlegung *AgNeS* |
//! | Dezentrale Erzeugung | 2026–2028 | §18 StromNEV payments are being phased out in quarterly steps (GBK-25-02-1#1) |
//! | Umlagen | annually | the ÜNB publish new rates each October |
//!
//! A settlement that spans one of those dates is governed by different rules at
//! its start and its end, and a settlement produced today for a 2024 period must
//! still be computed under 2024's rules.
//!
//! ## Why the regime is resolved once
//!
//! Scattering `if period_to <= some_date` through the calculation is how a rule
//! change becomes a bug: each site has to be found and each has to agree. Here
//! the dates are read **once**, at the edge, into [`RegulatoryRegime`]; every
//! calculation then matches on an enum. Adding the AgNeS turnover means changing
//! the resolution and exhaustively matching the new variant — the compiler names
//! every site that has to decide.
//!
//! This also makes the regime an *input*, so a settlement can be recomputed
//! under a stated regime rather than under whatever today's calendar implies.

use time::Date;

/// The network-access regime — what governs Mehr-/Mindermengen and the
/// Lieferantenwechsel processes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub enum NetzzugangRegime {
    /// **To 31.12.2025.** StromNZV §13 Abs. 3 (Strom) and GasNZV §25 (Gas).
    ///
    /// Both Verordnungen ceased to have effect with the end of 31.12.2025 —
    /// Art. 15 Abs. 4 (Strom) and Abs. 6 (Gas) of the Gesetz v. 22.12.2023,
    /// BGBl. 2023 I Nr. 405.
    Nzv,
    /// **From 01.01.2026.** §20 Abs. 3 EnWG, exercised through the BNetzA
    /// Festlegungen — GPKE (BK6-24-174) for Strom, GaBi Gas 2.1
    /// (BK7-24-01-008) for Gas.
    EnwgFestlegung,
}

/// The charge-setting regime — what governs how Netzentgelte are formed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub enum EntgeltRegime {
    /// **To 31.12.2028.** Verordnungsrecht: StromNEV / GasNEV together with the
    /// ARegV, and the §19 Abs. 2 individual charges decided by BNetzA
    /// Beschlusskammer 4 (BK4-22-089).
    Verordnung,
    /// **From 01.01.2029.** The BNetzA framework Festlegung *AgNeS*
    /// (Allgemeine Netzentgeltsystematik Strom), which replaces StromNEV and
    /// ARegV when they lapse at the end of 2028.
    ///
    /// Modelled ahead of time so a settlement for a 2029 period is refused
    /// explicitly rather than computed under rules that no longer exist. The
    /// substantive methodology is not implemented: the framework Festlegung was
    /// still in consultation when this was written.
    AgNeS,
}

/// The rules in force for one delivery period.
///
/// Construct with [`RegulatoryRegime::for_period`] and pass it down; do not
/// re-derive it from dates further in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RegulatoryRegime {
    netzzugang: NetzzugangRegime,
    entgelt: EntgeltRegime,
    /// The calendar year the annual rates are taken from.
    tarifjahr: i32,
}

/// Last day on which StromNZV and GasNZV applied.
const NZV_LETZTER_TAG: Date = time::macros::date!(2025 - 12 - 31);

/// Last day on which StromNEV and ARegV apply.
const VERORDNUNG_LETZTER_TAG: Date = time::macros::date!(2028 - 12 - 31);

impl RegulatoryRegime {
    /// Resolve the regime governing a delivery period.
    ///
    /// A period is governed by the rules in force **at its end**: the settlement
    /// documents a supply that has finished, and it is the completed supply that
    /// the current rules attach to. A period straddling a turnover is reported
    /// by [`Self::straddles_turnover`] so the caller can split it rather than
    /// silently pick one side.
    #[must_use]
    pub fn for_period(period_from: Date, period_to: Date) -> Self {
        let _ = period_from;
        Self {
            netzzugang: if period_to <= NZV_LETZTER_TAG {
                NetzzugangRegime::Nzv
            } else {
                NetzzugangRegime::EnwgFestlegung
            },
            entgelt: if period_to <= VERORDNUNG_LETZTER_TAG {
                EntgeltRegime::Verordnung
            } else {
                EntgeltRegime::AgNeS
            },
            tarifjahr: period_from.year(),
        }
    }

    /// Build a regime explicitly, for recomputing a settlement under stated rules.
    ///
    /// Use this to reproduce a historical settlement exactly, rather than letting
    /// today's calendar decide what applied then.
    #[must_use]
    pub const fn new(netzzugang: NetzzugangRegime, entgelt: EntgeltRegime, tarifjahr: i32) -> Self {
        Self {
            netzzugang,
            entgelt,
            tarifjahr,
        }
    }

    /// The network-access regime.
    #[must_use]
    pub const fn netzzugang(&self) -> NetzzugangRegime {
        self.netzzugang
    }

    /// The charge-setting regime.
    #[must_use]
    pub const fn entgelt(&self) -> EntgeltRegime {
        self.entgelt
    }

    /// The calendar year annual rates are taken from.
    #[must_use]
    pub const fn tarifjahr(&self) -> i32 {
        self.tarifjahr
    }

    /// `true` when the period crosses a regime turnover.
    ///
    /// Such a period is governed by different rules at its start and its end, so
    /// a single settlement over it applies the wrong rules to part of the
    /// supply. The caller should split the period at the turnover.
    #[must_use]
    pub fn straddles_turnover(period_from: Date, period_to: Date) -> bool {
        let start = Self::for_period(period_from, period_from);
        let end = Self::for_period(period_to, period_to);
        start.netzzugang != end.netzzugang || start.entgelt != end.entgelt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    /// The NZVs governed periods ending on or before 31.12.2025.
    #[test]
    fn the_nzv_regime_ends_with_2025() {
        let last = RegulatoryRegime::for_period(date!(2025 - 12 - 01), date!(2025 - 12 - 31));
        assert_eq!(last.netzzugang(), NetzzugangRegime::Nzv);

        let first_after =
            RegulatoryRegime::for_period(date!(2026 - 01 - 01), date!(2026 - 01 - 31));
        assert_eq!(first_after.netzzugang(), NetzzugangRegime::EnwgFestlegung);
    }

    /// StromNEV and ARegV govern through 2028; AgNeS takes over after.
    #[test]
    fn the_verordnung_regime_ends_with_2028() {
        let last = RegulatoryRegime::for_period(date!(2028 - 12 - 01), date!(2028 - 12 - 31));
        assert_eq!(last.entgelt(), EntgeltRegime::Verordnung);

        let first_after =
            RegulatoryRegime::for_period(date!(2029 - 01 - 01), date!(2029 - 01 - 31));
        assert_eq!(first_after.entgelt(), EntgeltRegime::AgNeS);
    }

    /// A period ending after a turnover is governed by the later rules.
    #[test]
    fn a_period_is_governed_by_the_rules_at_its_end() {
        let straddling = RegulatoryRegime::for_period(date!(2025 - 12 - 15), date!(2026 - 01 - 15));
        assert_eq!(straddling.netzzugang(), NetzzugangRegime::EnwgFestlegung);
    }

    /// …and a straddling period is reported, so it can be split rather than
    /// half-billed under rules that did not apply.
    #[test]
    fn a_straddling_period_is_detected() {
        assert!(RegulatoryRegime::straddles_turnover(
            date!(2025 - 12 - 15),
            date!(2026 - 01 - 15)
        ));
        assert!(RegulatoryRegime::straddles_turnover(
            date!(2028 - 12 - 15),
            date!(2029 - 01 - 15)
        ));
        assert!(!RegulatoryRegime::straddles_turnover(
            date!(2026 - 01 - 01),
            date!(2026 - 12 - 31)
        ));
    }

    /// An explicit regime reproduces a historical settlement regardless of today.
    #[test]
    fn an_explicit_regime_overrides_the_calendar() {
        let historical =
            RegulatoryRegime::new(NetzzugangRegime::Nzv, EntgeltRegime::Verordnung, 2024);
        assert_eq!(historical.netzzugang(), NetzzugangRegime::Nzv);
        assert_eq!(historical.tarifjahr(), 2024);
    }

    /// The Tarifjahr follows the period's start — annual rates attach to the
    /// year the supply began, not to when the settlement was produced.
    #[test]
    fn the_tarifjahr_follows_the_period_start() {
        let r = RegulatoryRegime::for_period(date!(2026 - 03 - 01), date!(2026 - 03 - 31));
        assert_eq!(r.tarifjahr(), 2026);
    }
}

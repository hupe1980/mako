//! The LPB's own **Bilanzierungsgebiet** — a regelzonenweites BG that exists
//! only to hold charging energy.
//!
//! Anlage 6 §II gives the Ladepunktbetreiber a claim against the
//! Bilanzkoordinator for a Bilanzierungsgebiet covering a whole Regelzone, and
//! against every Verteilnetzbetreiber for treating a registered Übergabestelle
//! as an exchange between the VNB's BG and this one. AWH Kap. 5.3 runs the
//! lifecycle through the ordinary MaBiS use cases; what this module adds is the
//! three invariants that make it a *virtual* BG rather than a grid area.

use serde::{Deserialize, Serialize};
use time::{Date, Month};

use mako_mabis::{BilanzierungsgebietId, BilanzkreisId};

use crate::error::EmobError;

/// A German Regelzone, named by its ÜNB.
///
/// A `String` would let two spellings of the same zone hold two BGs, which is
/// precisely the invariant [`VirtualBalancingArea`] enforces. There are four
/// and there have been four since 2010.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Regelzone {
    /// 50Hertz Transmission.
    FiftyHertz,
    /// Amprion.
    Amprion,
    /// TenneT TSO.
    TenneT,
    /// TransnetBW.
    TransnetBw,
}

impl Regelzone {
    /// Every Regelzone, in a stable order.
    pub const ALL: [Self; 4] = [
        Self::FiftyHertz,
        Self::Amprion,
        Self::TenneT,
        Self::TransnetBw,
    ];

    /// The operator's name, as the market writes it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FiftyHertz => "50Hertz",
            Self::Amprion => "Amprion",
            Self::TenneT => "TenneT",
            Self::TransnetBw => "TransnetBW",
        }
    }
}

impl std::fmt::Display for Regelzone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `true` when `date` is the first day of a calendar month.
#[must_use]
pub const fn ist_monatserster(date: Date) -> bool {
    date.day() == 1
}

/// The first day of the month after `date`.
///
/// # Panics
///
/// Only if `date` is within a month of [`Date::MAX`], which the `time` crate
/// places in the year 9999.
#[must_use]
pub fn naechster_monatserster(date: Date) -> Date {
    let (year, month) = if date.month() == Month::December {
        (date.year() + 1, Month::January)
    } else {
        (date.year(), date.month().next())
    };
    Date::from_calendar_date(year, month, 1).expect("the first of a month always exists")
}

/// The LPB's virtual Bilanzierungsgebiet in one Regelzone.
///
/// # Invariants
///
/// - **Monthly lifecycle.** „Die Bildung und Änderung von einem BG erfolgt nur
///   zum Ersten eines Monats … Die Beendigung eines BG erfolgt jeweils zum
///   Monatsletzten" (AWH Kap. 5.3.1.3). [`Self::neu`] refuses anything else,
///   which is why `valid_to` is stored as the **first day after** the BG rather
///   than as its last day: the two spellings differ by a day and only one of
///   them composes with [`Self::gilt_am`].
/// - **One per Regelzone.** [`BgRegistry`] enforces it across a set.
/// - **A Delta-Bilanzkreis before the first Übergabestelle.** Anlage 6 §IV.2
///   books the Restmenge into a Bilanzkreis the LPB names, at its own cost.
///   Making it non-optional here means there is no state in which allocated
///   energy has nowhere to go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualBalancingArea {
    /// The Y-EIC the BIKO issued, as Local Issuing Office (BDEW AWH
    /// EIC-Vergabe V1.0).
    pub eic: BilanzierungsgebietId,
    /// Which Regelzone the BG spans. A BG is regelzonenweit by construction.
    pub regelzone: Regelzone,
    /// The Bilanzkreis every unassigned Restmenge lands in, at the LPB's cost.
    pub delta_bk: BilanzkreisId,
    /// First day the BG is valid — always the first of a month.
    pub valid_from: Date,
    /// First day the BG is **no longer** valid, or `None` while it is open.
    /// Always the first of a month; the BG's last day is the day before.
    pub valid_to: Option<Date>,
}

impl VirtualBalancingArea {
    /// Open a Bilanzierungsgebiet.
    ///
    /// # Errors
    ///
    /// [`EmobError::NotFirstOfMonth`] when `valid_from` is not a Monatserster.
    pub fn neu(
        eic: BilanzierungsgebietId,
        regelzone: Regelzone,
        delta_bk: BilanzkreisId,
        valid_from: Date,
    ) -> Result<Self, EmobError> {
        if !ist_monatserster(valid_from) {
            return Err(EmobError::NotFirstOfMonth {
                was: "the Gültigkeitsbeginn of a Bilanzierungsgebiet",
                date: valid_from,
            });
        }
        Ok(Self {
            eic,
            regelzone,
            delta_bk,
            valid_from,
            valid_to: None,
        })
    }

    /// End the BG „zum Monatsletzten".
    ///
    /// `letzter_tag` is the BG's **last** valid day; the stored `valid_to`
    /// becomes the day after, so [`Self::gilt_am`] stays a half-open test.
    ///
    /// # Errors
    ///
    /// [`EmobError::NotFirstOfMonth`] when `letzter_tag` is not the last day of
    /// a month, or when the end would precede the start.
    pub fn beenden(&mut self, letzter_tag: Date) -> Result<(), EmobError> {
        let folgetag = letzter_tag.next_day().ok_or(EmobError::NotFirstOfMonth {
            was: "the Beendigung of a Bilanzierungsgebiet",
            date: letzter_tag,
        })?;
        if !ist_monatserster(folgetag) || folgetag <= self.valid_from {
            return Err(EmobError::NotFirstOfMonth {
                was: "the Beendigung of a Bilanzierungsgebiet",
                date: letzter_tag,
            });
        }
        self.valid_to = Some(folgetag);
        Ok(())
    }

    /// `true` when the BG is valid on `date`.
    #[must_use]
    pub fn gilt_am(&self, date: Date) -> bool {
        date >= self.valid_from && self.valid_to.is_none_or(|to| date < to)
    }
}

/// Every Bilanzierungsgebiet one Ladepunktbetreiber holds.
///
/// The registry exists for one invariant that no single BG can state about
/// itself: **at most one BG per Regelzone at any instant** (AWH Kap. 5.3).
/// Holding two would split one Regelzone's charging energy across two
/// Bilanzkreise-Zuordnungen with no rule for which a session belongs to.
///
/// Historic BGs are kept: a BG that ended is still the right one for a
/// Bilanzierungsmonat inside its validity, and MaBiS accepts corrections for
/// seven months after.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BgRegistry {
    bgs: Vec<VirtualBalancingArea>,
}

impl BgRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `bg`.
    ///
    /// # Errors
    ///
    /// [`EmobError::DoppeltesBilanzierungsgebiet`] when its validity overlaps
    /// another BG in the same Regelzone.
    pub fn register(&mut self, bg: VirtualBalancingArea) -> Result<(), EmobError> {
        if self.bgs.iter().any(|held| {
            held.regelzone == bg.regelzone
                && held.valid_from < bg.valid_to.unwrap_or(Date::MAX)
                && bg.valid_from < held.valid_to.unwrap_or(Date::MAX)
        }) {
            return Err(EmobError::DoppeltesBilanzierungsgebiet {
                regelzone: bg.regelzone.to_string(),
            });
        }
        self.bgs.push(bg);
        Ok(())
    }

    /// The BG valid in `regelzone` on `date`, if any.
    #[must_use]
    pub fn at(&self, regelzone: Regelzone, date: Date) -> Option<&VirtualBalancingArea> {
        self.bgs
            .iter()
            .find(|bg| bg.regelzone == regelzone && bg.gilt_am(date))
    }

    /// Every registered BG, in registration order.
    pub fn iter(&self) -> impl Iterator<Item = &VirtualBalancingArea> {
        self.bgs.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::from_calendar_date(y, Month::try_from(m).unwrap(), day).unwrap()
    }

    fn bk() -> BilanzkreisId {
        BilanzkreisId::new("11XSUEDWESTSTRO8").unwrap_or_else(|_| {
            // Fall back to whatever the type accepts; the tests below never
            // read the value, only the lifecycle around it.
            panic!("test Bilanzkreis id must parse")
        })
    }

    fn bg_id() -> BilanzierungsgebietId {
        BilanzierungsgebietId::new("11YN-0000-0001-Q").expect("test BG id must parse")
    }

    fn bg(from: Date) -> VirtualBalancingArea {
        VirtualBalancingArea::neu(bg_id(), Regelzone::TenneT, bk(), from).expect("valid start")
    }

    #[test]
    fn a_bg_starts_only_on_the_first() {
        assert!(VirtualBalancingArea::neu(bg_id(), Regelzone::TenneT, bk(), d(2026, 9, 1)).is_ok());
        assert!(
            VirtualBalancingArea::neu(bg_id(), Regelzone::TenneT, bk(), d(2026, 9, 15)).is_err()
        );
    }

    #[test]
    fn a_bg_ends_on_a_monatsletzten() {
        let mut b = bg(d(2026, 9, 1));
        assert!(b.beenden(d(2026, 11, 15)).is_err(), "mid-month");
        assert!(b.beenden(d(2026, 11, 30)).is_ok());
        assert_eq!(b.valid_to, Some(d(2026, 12, 1)));
    }

    /// The last valid day is inside, the stored `valid_to` is outside.
    #[test]
    fn validity_is_half_open() {
        let mut b = bg(d(2026, 9, 1));
        b.beenden(d(2026, 11, 30)).unwrap();
        assert!(b.gilt_am(d(2026, 9, 1)));
        assert!(b.gilt_am(d(2026, 11, 30)));
        assert!(!b.gilt_am(d(2026, 12, 1)));
        assert!(!b.gilt_am(d(2026, 8, 31)));
    }

    #[test]
    fn one_bg_per_regelzone_at_a_time() {
        let mut reg = BgRegistry::new();
        reg.register(bg(d(2026, 9, 1))).unwrap();
        let err = reg.register(bg(d(2027, 1, 1))).unwrap_err();
        assert!(matches!(
            err,
            EmobError::DoppeltesBilanzierungsgebiet { .. }
        ));
    }

    /// A BG that has ended frees its Regelzone for the next one.
    #[test]
    fn a_closed_bg_frees_the_regelzone() {
        let mut reg = BgRegistry::new();
        let mut first = bg(d(2026, 9, 1));
        first.beenden(d(2026, 12, 31)).unwrap();
        reg.register(first).unwrap();
        reg.register(bg(d(2027, 1, 1))).expect("no overlap");
        assert_eq!(reg.iter().count(), 2);
        assert_eq!(
            reg.at(Regelzone::TenneT, d(2026, 10, 1))
                .unwrap()
                .valid_from,
            d(2026, 9, 1)
        );
        assert_eq!(
            reg.at(Regelzone::TenneT, d(2027, 2, 1)).unwrap().valid_from,
            d(2027, 1, 1)
        );
    }

    /// Different Regelzonen never collide — the LPB is expected to hold four.
    #[test]
    fn regelzonen_are_independent() {
        let mut reg = BgRegistry::new();
        for rz in Regelzone::ALL {
            let b = VirtualBalancingArea::neu(bg_id(), rz, bk(), d(2026, 9, 1)).unwrap();
            reg.register(b).expect("one per Regelzone");
        }
        assert_eq!(reg.iter().count(), 4);
    }

    #[test]
    fn naechster_monatserster_crosses_the_year() {
        assert_eq!(naechster_monatserster(d(2026, 12, 3)), d(2027, 1, 1));
        assert_eq!(naechster_monatserster(d(2026, 1, 31)), d(2026, 2, 1));
    }
}

//! Ladevorgänge, and how one becomes quarter-hour energies.
//!
//! # The quarter-hour grid is DST-safe by construction
//!
//! A [`Viertelstunde`] is an *instant* plus fifteen minutes of real time, not a
//! wall-clock label. German local time is UTC+1 or UTC+2, both whole hours, so
//! a UTC-aligned quarter hour is also aligned in Europe/Berlin — and the
//! 92-slot and 100-slot days need no special case, because they are simply days
//! with fewer or more instants in them. Nothing here counts „96".
//!
//! # Provenance is not metadata
//!
//! Two very different things can produce a session's energy, and the difference
//! is visible in the result. A charge point that reports **clock-aligned meter
//! values** every 900 s (OCPP `AlignedDataCtrlr` / `ClockAlignedDataInterval`)
//! measures each quarter hour. A **CDR** reports one total for the whole
//! session, and splitting it assumes constant power, which a tapering charge
//! curve is not. The second is an estimate in the shape of a measurement, so
//! [`Provenance`] rides on every value.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use time::{Date, Duration, OffsetDateTime};

use metering::allocation::{AllocationBasis, AllocationPart, allocate};

use crate::error::EmobError;
use crate::ids::{SessionId, TokenRef, VirtualMaloId};

/// The length of one quarter hour.
pub const VIERTELSTUNDE: Duration = Duration::minutes(15);

/// The most quarter hours one [`Ladevorgang`] may span — a calendar year.
///
/// A corruption guard, not a regulatory bound. [`Ladevorgang::viertelstunden`]
/// walks the grid one slot at a time, so a backend that reports an `ende` in
/// the year 9999 would allocate until the process dies. The longest real
/// session is a few days; a year is three orders of magnitude past it and
/// still small enough to hold.
pub const MAX_SLOTS_JE_LADEVORGANG: u64 = 366 * 96;

/// One quarter hour of the German market grid, named by the instant it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Viertelstunde {
    start: OffsetDateTime,
}

impl Viertelstunde {
    /// The quarter hour `at` falls in.
    ///
    /// Truncates toward the past, so an instant exactly on a boundary belongs
    /// to the slot it opens.
    ///
    /// # Panics
    ///
    /// Never for a value obtained from an [`OffsetDateTime`]: truncating a
    /// representable Unix timestamp toward the past stays representable.
    #[must_use]
    pub fn containing(at: OffsetDateTime) -> Self {
        let secs = at.unix_timestamp();
        let slot = secs.div_euclid(900) * 900;
        Self {
            start: OffsetDateTime::from_unix_timestamp(slot)
                .expect("a truncated valid timestamp is valid"),
        }
    }

    /// The instant the quarter hour opens.
    #[must_use]
    pub const fn start(self) -> OffsetDateTime {
        self.start
    }

    /// The instant the quarter hour closes — the start of the next one.
    #[must_use]
    pub fn end(self) -> OffsetDateTime {
        self.start + VIERTELSTUNDE
    }

    /// The next quarter hour.
    #[must_use]
    pub fn next(self) -> Self {
        Self { start: self.end() }
    }

    /// The Europe/Berlin calendar day this quarter hour is settled under.
    #[must_use]
    pub fn berlin_day(self) -> Date {
        mako_fristen::berlin_date(self.start)
    }

    /// Seconds of this quarter hour that lie inside `[von, bis)`.
    ///
    /// Zero when they do not overlap; never negative.
    #[must_use]
    pub fn overlap_secs(self, von: OffsetDateTime, bis: OffsetDateTime) -> i64 {
        let from = self.start.max(von);
        let to = self.end().min(bis);
        (to - from).whole_seconds().max(0)
    }
}

/// Where a quarter-hour energy came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Provenance {
    /// Clock-aligned meter values from the charge point, one per quarter hour.
    ///
    /// The only provenance that *measures* the slot. Preferred wherever the
    /// station delivers it.
    ClockAlignedMeterValues,
    /// One CDR total, split across the slots it spans in proportion to time.
    ///
    /// An estimate: it assumes constant power across the session, which
    /// tapering charge curves violate. Correct in aggregate over a session,
    /// wrong within it — and the error lands on whichever supplier held the
    /// slot boundary.
    CdrProRata,
    /// A station-local log, neither clock-aligned nor a settled CDR.
    DeviceLog,
}

impl Provenance {
    /// `true` when the value measures its quarter hour rather than estimating it.
    #[must_use]
    pub const fn ist_gemessen(self) -> bool {
        matches!(self, Self::ClockAlignedMeterValues)
    }
}

/// One quarter hour's worth of one session's energy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotEnergie {
    /// The quarter hour.
    pub slot: Viertelstunde,
    /// Energy in kWh, always non-negative.
    pub kwh: Decimal,
    /// How this value was arrived at.
    pub provenance: Provenance,
}

/// A charging session, as the CPO backend reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ladevorgang {
    /// The backend's own id, for deduplicating a late CDR.
    pub id: SessionId,
    /// Which virtual Marktlokation — and so which supplier — this belongs to.
    pub virtual_malo: VirtualMaloId,
    /// The contract token, as an opaque keyed hash. `None` for an
    /// unauthenticated draw.
    pub token: Option<TokenRef>,
    /// When charging began.
    pub beginn: OffsetDateTime,
    /// When charging ended.
    pub ende: OffsetDateTime,
    /// Total energy drawn, in kWh.
    pub energie_kwh: Decimal,
    /// Where [`Self::energie_kwh`] came from.
    pub provenance: Provenance,
}

/// A session split across the quarter hours it spans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSplit {
    /// One entry per quarter hour the session touched, in time order.
    pub slots: Vec<SlotEnergie>,
    /// Energy the split could not place, in kWh.
    ///
    /// **Not an error and not silent.** A proportional split cuts each share to
    /// six decimal places toward zero (`metering::allocation::ALLOCATION_DP`),
    /// so a session spanning many slots can leave a millionth of a kWh
    /// unplaced. That energy was really drawn, so it does not vanish: it stays
    /// out of every supplier's Bilanzkreis and lands in the Deltamenge, which
    /// Anlage 6 §IV.2 books to the LPB's own Bilanzkreis at its cost. Reported
    /// here so an operator can see the magnitude rather than discover it in a
    /// yearly reconciliation.
    pub nicht_zugeordnet_kwh: Decimal,
}

impl Ladevorgang {
    /// How many quarter hours this session touches.
    ///
    /// Computed rather than counted, so an absurd `ende` is caught before any
    /// allocation is made.
    #[must_use]
    fn slot_count(&self) -> u64 {
        if self.ende <= self.beginn {
            return 0;
        }
        let erster = Viertelstunde::containing(self.beginn).start();
        // `ende` is exclusive, so the last slot is the one containing the
        // instant just before it — round the span up instead.
        let spanne = (self.ende - erster).whole_seconds().max(0);
        u64::try_from(spanne.div_euclid(900) + i64::from(spanne.rem_euclid(900) != 0))
            .unwrap_or(u64::MAX)
    }

    /// The quarter hours this session touches, in time order.
    ///
    /// Empty when the session has no duration.
    ///
    /// # Errors
    ///
    /// [`EmobError::LadevorgangZuLang`] when the session spans more than
    /// [`MAX_SLOTS_JE_LADEVORGANG`] quarter hours.
    pub fn viertelstunden(&self) -> Result<Vec<Viertelstunde>, EmobError> {
        let count = self.slot_count();
        if count > MAX_SLOTS_JE_LADEVORGANG {
            return Err(EmobError::LadevorgangZuLang {
                id: self.id.to_string(),
                slots: count,
                max: MAX_SLOTS_JE_LADEVORGANG,
            });
        }
        let mut out = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
        let mut slot = Viertelstunde::containing(self.beginn);
        for _ in 0..count {
            out.push(slot);
            slot = slot.next();
        }
        Ok(out)
    }

    /// Split [`Self::energie_kwh`] across those quarter hours, pro rata by
    /// overlap.
    ///
    /// The arithmetic is `metering::allocation::allocate` with the overlap in
    /// seconds as the weight, which is what guarantees
    /// `Σ slots + nicht_zugeordnet = energie_kwh` **exactly** rather than
    /// approximately.
    ///
    /// A session already delivered as clock-aligned meter values should not go
    /// through here at all — it is already per-slot. Passing one anyway keeps
    /// its [`Provenance`], because a value that was measured stays measured
    /// even if it is re-split.
    ///
    /// # Errors
    ///
    /// [`EmobError::Allocation`] when the energy is negative — an Einspeisung
    /// belongs in its own direction, not in a negative Bezug — and
    /// [`EmobError::LadevorgangZuLang`] when the session spans an implausible
    /// number of quarter hours.
    pub fn in_viertelstunden(&self) -> Result<SessionSplit, EmobError> {
        if self.energie_kwh < Decimal::ZERO {
            return Err(EmobError::Allocation(format!(
                "session {} carries negative energy {}; model Einspeisung as its own \
                 Richtung rather than as a negative Bezug",
                self.id, self.energie_kwh
            )));
        }
        let slots = self.viertelstunden()?;
        if slots.is_empty() {
            return Ok(SessionSplit {
                slots: Vec::new(),
                nicht_zugeordnet_kwh: self.energie_kwh,
            });
        }

        let parts: Vec<AllocationPart> = slots
            .iter()
            .map(|s| {
                AllocationPart::new(
                    s.start().unix_timestamp().to_string(),
                    Decimal::from(s.overlap_secs(self.beginn, self.ende)),
                )
            })
            .collect();

        let row = allocate(self.energie_kwh, parts, AllocationBasis::Proportional)?;

        let placed = slots
            .into_iter()
            .zip(row.parts.iter())
            .map(|(slot, part)| SlotEnergie {
                slot,
                kwh: part.allocated,
                provenance: self.provenance,
            })
            .collect();

        Ok(SessionSplit {
            slots: placed,
            nicht_zugeordnet_kwh: row.residual,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;
    use time::macros::datetime;

    fn session(beginn: OffsetDateTime, ende: OffsetDateTime, kwh: Decimal) -> Ladevorgang {
        Ladevorgang {
            id: SessionId::new("s1"),
            virtual_malo: VirtualMaloId::new("veh-1").unwrap(),
            token: None,
            beginn,
            ende,
            energie_kwh: kwh,
            provenance: Provenance::CdrProRata,
        }
    }

    #[test]
    fn a_slot_is_aligned_to_the_quarter_hour() {
        let v = Viertelstunde::containing(datetime!(2026-11-03 08:07:31 UTC));
        assert_eq!(v.start(), datetime!(2026-11-03 08:00:00 UTC));
        assert_eq!(v.end(), datetime!(2026-11-03 08:15:00 UTC));
    }

    #[test]
    fn an_instant_on_the_boundary_opens_its_slot() {
        let v = Viertelstunde::containing(datetime!(2026-11-03 08:15:00 UTC));
        assert_eq!(v.start(), datetime!(2026-11-03 08:15:00 UTC));
    }

    /// Alignment must survive the pre-1970 sign, which integer division does not.
    #[test]
    fn alignment_uses_euclidean_division() {
        let v = Viertelstunde::containing(datetime!(1969-12-31 23:52:00 UTC));
        assert_eq!(v.start(), datetime!(1969-12-31 23:45:00 UTC));
    }

    #[test]
    fn a_session_inside_one_slot_stays_whole() {
        let s = session(
            datetime!(2026-11-03 08:02:00 UTC),
            datetime!(2026-11-03 08:10:00 UTC),
            dec!(4),
        );
        let split = s.in_viertelstunden().unwrap();
        assert_eq!(split.slots.len(), 1);
        assert_eq!(split.slots[0].kwh, dec!(4));
        assert_eq!(split.nicht_zugeordnet_kwh, Decimal::ZERO);
    }

    #[test]
    fn a_session_spanning_two_slots_splits_by_overlap() {
        // 08:00–08:30 exactly: two full slots, half each.
        let s = session(
            datetime!(2026-11-03 08:00:00 UTC),
            datetime!(2026-11-03 08:30:00 UTC),
            dec!(10),
        );
        let split = s.in_viertelstunden().unwrap();
        assert_eq!(split.slots.len(), 2);
        assert_eq!(split.slots[0].kwh, dec!(5));
        assert_eq!(split.slots[1].kwh, dec!(5));
        assert_eq!(split.nicht_zugeordnet_kwh, Decimal::ZERO);
    }

    #[test]
    fn a_partial_first_slot_gets_its_share() {
        // 08:10–08:20: 5 min in the 08:00 slot, 5 min in the 08:15 slot.
        let s = session(
            datetime!(2026-11-03 08:10:00 UTC),
            datetime!(2026-11-03 08:20:00 UTC),
            dec!(2),
        );
        let split = s.in_viertelstunden().unwrap();
        assert_eq!(split.slots.len(), 2);
        assert_eq!(split.slots[0].kwh, dec!(1));
        assert_eq!(split.slots[1].kwh, dec!(1));
    }

    /// The identity is the point: nothing is created and nothing disappears.
    #[test]
    fn the_split_conserves_energy_exactly() {
        // A number that does not divide evenly by three slots.
        let s = session(
            datetime!(2026-11-03 08:00:00 UTC),
            datetime!(2026-11-03 08:45:00 UTC),
            dec!(10),
        );
        let split = s.in_viertelstunden().unwrap();
        let sum: Decimal = split.slots.iter().map(|s| s.kwh).sum();
        assert_eq!(sum + split.nicht_zugeordnet_kwh, dec!(10));
        assert!(split.nicht_zugeordnet_kwh >= Decimal::ZERO);
    }

    /// A long session across a whole day still conserves.
    #[test]
    fn a_long_session_conserves_too() {
        let s = session(
            datetime!(2026-11-03 00:00:00 UTC),
            datetime!(2026-11-04 00:00:00 UTC),
            dec!(77.77),
        );
        let split = s.in_viertelstunden().unwrap();
        assert_eq!(split.slots.len(), 96);
        let sum: Decimal = split.slots.iter().map(|s| s.kwh).sum();
        assert_eq!(sum + split.nicht_zugeordnet_kwh, dec!(77.77));
    }

    /// The clocks change and the day is 23 or 25 hours long; nothing special
    /// happens, because a quarter hour is fifteen minutes of real time.
    #[test]
    fn the_grid_needs_no_dst_special_case() {
        // Europe/Berlin spring-forward 2027-03-28: 02:00 local jumps to 03:00,
        // which is 01:00 UTC. A session across it is still 15-minute slots.
        let s = session(
            datetime!(2027-03-28 00:30:00 UTC),
            datetime!(2027-03-28 01:30:00 UTC),
            dec!(8),
        );
        let split = s.in_viertelstunden().unwrap();
        assert_eq!(split.slots.len(), 4);
        assert_eq!(split.slots.iter().map(|s| s.kwh).sum::<Decimal>(), dec!(8));
    }

    #[test]
    fn a_zero_length_session_places_nothing() {
        let s = session(
            datetime!(2026-11-03 08:00:00 UTC),
            datetime!(2026-11-03 08:00:00 UTC),
            dec!(3),
        );
        let split = s.in_viertelstunden().unwrap();
        assert!(split.slots.is_empty());
        assert_eq!(split.nicht_zugeordnet_kwh, dec!(3));
    }

    /// A backend that reports the wrong century must not allocate until the
    /// process dies.
    #[test]
    fn an_implausibly_long_session_is_refused_before_any_allocation() {
        let s = session(
            datetime!(2026-11-03 08:00:00 UTC),
            datetime!(2126-11-03 08:00:00 UTC),
            dec!(8),
        );
        let e = s.in_viertelstunden().unwrap_err();
        assert!(matches!(e, EmobError::LadevorgangZuLang { .. }), "{e:?}");
        assert!(s.viertelstunden().is_err());
    }

    /// The bound is generous enough that a real long session is unaffected.
    #[test]
    fn a_week_long_session_is_still_allowed() {
        let s = session(
            datetime!(2026-11-03 08:00:00 UTC),
            datetime!(2026-11-10 08:00:00 UTC),
            dec!(500),
        );
        assert_eq!(s.viertelstunden().unwrap().len(), 7 * 96);
    }

    #[test]
    fn negative_energy_is_refused_rather_than_split() {
        let s = session(
            datetime!(2026-11-03 08:00:00 UTC),
            datetime!(2026-11-03 08:30:00 UTC),
            dec!(-5),
        );
        assert!(s.in_viertelstunden().is_err());
    }

    #[test]
    fn provenance_rides_on_every_slot() {
        let mut s = session(
            datetime!(2026-11-03 08:00:00 UTC),
            datetime!(2026-11-03 08:30:00 UTC),
            dec!(6),
        );
        s.provenance = Provenance::ClockAlignedMeterValues;
        let split = s.in_viertelstunden().unwrap();
        assert!(
            split
                .slots
                .iter()
                .all(|x| x.provenance == Provenance::ClockAlignedMeterValues)
        );
        assert!(Provenance::ClockAlignedMeterValues.ist_gemessen());
        assert!(!Provenance::CdrProRata.ist_gemessen());
    }
}

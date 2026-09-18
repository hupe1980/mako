//! # Example: MaKo deadlines — four shapes, one calendar, one clock
//!
//! Every answer a German market participant owes has a Frist, and getting one
//! wrong is not a rounding error: an answer sent after the window is a breached
//! process, and a window computed shorter than the AHB requires is work an
//! operator is never shown.
//!
//! Three facts do most of the damage, and this example shows each:
//!
//! 1. **The Frist has four shapes**, not one. „11:00 Uhr des 1. WT nach dem ÜT"
//!    is a wall-clock instant on a Werktag; „bis zum Ablauf des 4. Werktags" is
//!    day-granular; „spätester ÜT ist der 2. WT" resolves to the 17:00 MaKo
//!    cut-off; and „am ÜT" is the same day the message arrived. They are not
//!    interchangeable, and a `+ n days` shorthand is wrong for all four.
//! 2. **Werktag is BDEW's calendar**, not the Bundesland's. A holiday observed
//!    in *any* German state is a non-Werktag, so no Frist is ever computed
//!    shorter than the AHB requires for any participant.
//! 3. **The clock is Europe/Berlin**, always. A deadline read in UTC is the
//!    wrong instant for two hours of every day, and the wrong *day* around
//!    midnight.
//!
//! ## Run
//!
//! ```text
//! cargo run -p mako-fristen --example 01_antwortfristen
//! ```

use mako_fristen::{HolidayCalendar, antwort};
use time::macros::datetime;

fn main() {
    let mut bad = 0_usize;
    bad += the_four_shapes();
    bad += the_calendar_is_bdews();
    bad += the_clock_is_berlin();
    bad += a_window_an_operator_can_act_in();
    bad += every_obligation_cites_its_source();

    println!("\n─────────────────────────────────────────────────────────────");
    assert_eq!(bad, 0, "{bad} assertion(s) in this example did not hold");
    println!("✓ every Frist resolved as its Festlegung states it");
}

// ── 1. Four shapes ───────────────────────────────────────────────────────────

/// One arrival instant, four PIDs, four different answers.
///
/// A Tuesday afternoon, well inside the working week, so nothing here is a
/// weekend artefact — the differences are the *shapes*.
fn the_four_shapes() -> usize {
    section("1. Four shapes of Frist");
    let received = datetime!(2026-03-10 14:30:00 +01:00); // Tuesday
    let mut bad = 0;

    // One PID per *shape* the catalogue uses, so the differences are visible
    // rather than six samples of the same one.
    let mut seen: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    for o in antwort::all() {
        seen.entry(shape_name(o.frist)).or_insert(o.trigger_pid);
    }
    for (shape, pid) in &seen {
        let o = antwort::antwort_obligation(*pid).expect("just enumerated");
        let due = o
            .frist
            .due_at(received, HolidayCalendar::BdewMaKo)
            .to_offset(time::UtcOffset::from_hms(1, 0, 0).expect("CET"));
        println!("  {shape:<18}  PID {pid}  → {due}");
    }
    println!("  …one arrival instant, {} different answers.", seen.len());
    bad += check(
        "the catalogue really does carry more than one shape",
        seen.len() >= 2,
    );
    bad
}

/// The variant name alone — the payload differs per PID and would hide the
/// shape it is an instance of.
fn shape_name(shape: antwort::FristShape) -> String {
    use antwort::FristShape as F;
    match shape {
        F::WerktagAt { .. } => "WerktagAt",
        F::EndOfWerktag(_) => "EndOfWerktag",
        F::SameDayAt(_) => "SameDayAt",
        F::SameDay => "SameDay",
    }
    .to_owned()
}

// ── 2. The calendar ──────────────────────────────────────────────────────────

/// `Reformationstag`, 31 October, is a holiday in nine of sixteen states and a
/// working day in the other seven. BDEW's MaKo calendar treats it as a
/// non-Werktag everywhere — the conservative-inclusive rule, so a Frist is
/// never shorter than the AHB requires for *any* participant.
fn the_calendar_is_bdews() -> usize {
    section("2. Werktag is BDEW's calendar, not a Bundesland's");
    let mut bad = 0;
    let reformationstag = time::macros::date!(2026 - 10 - 31);
    println!(
        "  31.10.2026 (Reformationstag, 9 of 16 states) → Werktag? {}",
        mako_fristen::is_werktag(reformationstag, HolidayCalendar::BdewMaKo)
    );
    bad += check(
        "a holiday observed anywhere is a non-Werktag",
        !mako_fristen::is_werktag(reformationstag, HolidayCalendar::BdewMaKo),
    );

    // …so a one-Werktag Frist starting on the Friday before it lands on the
    // Monday: Saturday is a Saturday *and* a holiday, Sunday is a Sunday.
    let friday = time::macros::date!(2026 - 10 - 30);
    let plus_one = mako_fristen::add_werktage(friday, 1, HolidayCalendar::BdewMaKo);
    println!("  Fri 30.10.2026 + 1 Werktag                   → {plus_one}");
    bad += check(
        "31 Oct (Sat + holiday) and 1 Nov (Sun) are both skipped",
        plus_one == time::macros::date!(2026 - 11 - 02),
    );
    bad
}

// ── 3. The clock ─────────────────────────────────────────────────────────────

/// A business date is a **Berlin** date. Read in UTC it is the wrong day for
/// two hours of every day in summer and one in winter.
fn the_clock_is_berlin() -> usize {
    section("3. The clock is Europe/Berlin");
    let mut bad = 0;
    // 30 June 2026, 22:30 UTC — already 1 July in Berlin (CEST, +02:00).
    let instant = datetime!(2026-06-30 22:30:00 UTC);
    let berlin = mako_fristen::berlin_date(instant);
    println!("  2026-06-30T22:30:00Z → UTC date 2026-06-30, Berlin date {berlin}");
    bad += check(
        "the business date is the Berlin one",
        berlin == time::macros::date!(2026 - 07 - 01),
    );
    println!("  …so a billing period ending 30 June already closed two hours ago.");
    bad
}

// ── 4. The window an operator can act in ─────────────────────────────────────

/// A deadline is not a window. `operator_window` answers the question a queue
/// actually asks: **from when until when** can somebody do this work?
///
/// The interesting case is „am ÜT" — a same-day instant. Read literally, a
/// message arriving after that clock time yields a deadline already in the
/// past, which becomes a queue entry nobody is ever shown and a process
/// reported breached against a party that never had the chance to answer. The
/// window rolls to the same clock time on the next Werktag, which is the second
/// window GPKE Teil 2 publishes for exactly those Prüfidentifikatoren.
fn a_window_an_operator_can_act_in() -> usize {
    section("4. A window, not just an instant");
    let mut bad = 0;
    let Some(pid) = antwort::all()
        .find(|o| matches!(o.frist, antwort::FristShape::SameDayAt(_)))
        .map(|o| o.trigger_pid)
    else {
        println!("  (no same-day obligation in the catalogue)");
        return 0;
    };

    // Arrives at 23:00 Berlin, long after any same-day cut-off.
    let late = datetime!(2026-03-10 23:00:00 +01:00);
    let window = antwort::operator_window(pid, late);
    println!(
        "  PID {pid} arriving 23:00 → deadline {}, queue expires {}",
        window.deadline, window.expires_at
    );
    bad += check(
        "a same-day Frist that already passed rolls forward, it does not expire on arrival",
        window.deadline > late,
    );
    bad
}

// ── 5. Every obligation cites its source ─────────────────────────────────────

/// A Frist that cannot be traced to a published document is a number somebody
/// remembered. Every entry carries its citation, because the operator answering
/// a complaint has to quote the rule, not the code.
fn every_obligation_cites_its_source() -> usize {
    section("5. Every Frist cites the Festlegung that states it");
    let mut bad = 0;
    let total = antwort::all().count();
    let uncited = antwort::all()
        .filter(|o| o.source.trim().is_empty())
        .count();
    println!("  {total} answer obligations, {uncited} without a citation");
    bad += check("every obligation carries its source", uncited == 0);

    // Show a couple, so the shape of a citation is visible.
    for o in antwort::all().take(3) {
        println!("    {}  {:?}  — {}", o.trigger_pid, o.family, o.source);
    }
    bad
}

// ── Reporting ────────────────────────────────────────────────────────────────

fn section(title: &str) {
    println!("\n── {title} ──");
}

fn check(what: &str, ok: bool) -> usize {
    if ok {
        0
    } else {
        println!("  ✗ FAILED: {what}");
        1
    }
}

# mako-fristen

**Every Frist in German market communication, in one leaf crate.**

Deadlines are the regulatory asset the [mako](https://github.com/hupe1980/mako)
platform is built around, and this crate is the one answer to *when is this due*.
It depends on nothing but `time`, so any service can read it without pulling in
a workflow engine — which is what keeps the calendar, the per-Prüfidentifikator
answer tables and the alerting from each carrying a copy that disagrees with the
others.

```rust
use mako_fristen::{self as fristen, HolidayCalendar};
use time::{Date, Month};

// 5 Werktage after Monday 2025-01-06 → Monday 2025-01-13.
let start = Date::from_calendar_date(2025, Month::January, 6).unwrap();
let due   = fristen::add_werktage(start, 5, HolidayCalendar::BdewMaKo);
assert_eq!(due, Date::from_calendar_date(2025, Month::January, 13).unwrap());
```

## What it answers

| Question | Answer |
|---|---|
| What is today's date, in the market's terms? | `heute`, `berlin_date`, `berlin_midnight`, `berlin_now` |
| When is the 4th Werktag after this instant? | `add_werktage`, `deadline_at_werktage`, `end_of_werktag_after`, `next_werktag_at` |
| Which answer window does PID 55001 carry? | `antwort` |
| Which messages does receiving it oblige me to send? | `meldung` |
| How much Vorlauf does this Anmeldung need? | `vorlauf`, `abmeldung` |
| When must the CONTRL / APERAK go out? | `contrl_due_at`, `aperak_strom_due_at`, `aperak_gas_folgeprozess_due_at`, `aperak_gas_initialprozess_due_at` |

## Three clocks, never one number

The mistake this crate is shaped to prevent is treating any of these as the
others. They differ by orders of magnitude and they fail for different reasons.

| Clock | Window | Meaning |
|---|---|---|
| **CONTRL** | 6 wall-clock hours (CONTRL AHB 1.0 §1.2) | the interchange was syntactically readable |
| **APERAK** | 45 min Strom weekday; Gas: next Werktag 12:00 (Folgeprozess) or 3 Werktage (Initialprozess) | the message was accepted for processing |
| **Antwortfrist** | per PID — 11:00 of the 1. Werktag for a GPKE Anmeldung, 4 Werktage for a Gas Anmeldung, 3/5/7/1 WT for WiM Strom | the *business* answer is owed |

**There is no 24-hour GPKE window**, under BK6-22-024 or anything else. A flat
24 h is neither of the two real clocks, and it is wrong in the direction that
does not announce itself: it reports a lapsed Frist as still running.

The same trap has a Gas half. **„10 Werktage" is the GeLi Gas supplier's
*Vorlauffrist*** — how far ahead of Lieferbeginn the LF must send (GeLi Gas 3.0
Kap. 3.2.3) — and not the Netzbetreiber's answer window, which is 4 / 3 / 2
Werktage by Geschäftsvorfall. Both constants are spelled out in `antwort` so a
reader who reaches for the familiar number finds the correction:
`GPKE_IS_NOT_TWENTY_FOUR_HOURS` and
`TEN_WERKTAGE_IS_THE_SUPPLIERS_VORLAUFFRIST`.

## A business date is a Berlin date

`heute()` returns the current date in `Europe/Berlin`, never in UTC. Between
00:00 and 01:00 CEST the two disagree by a day, which is a day of Frist. Every
service reads the date through this crate; `cargo xtask check-business-dates`
refuses a business date read in UTC.

## Holiday calendar: BDEW-defined, Germany-wide

`HolidayCalendar::BdewMaKo` is the single holiday calendar used in all BNetzA
MaKo processes. BDEW EDI@Energy specifies a conservative-inclusive approach:
every public holiday observed in *any* German state is a non-Werktag, so no
Frist is ever shorter than the AHB requires. Per-state calendars are **not**
used in BDEW MaKo.

## Sourcing

Every window in `antwort`, `meldung` and `vorlauf` carries the document and
chapter it was read from. A window with no source is absent from the table
rather than filled with a plausible default: a fabricated Frist fails silently,
in whichever direction it was guessed.

## License

MIT OR Apache-2.0

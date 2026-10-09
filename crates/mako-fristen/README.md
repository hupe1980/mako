# mako-fristen

[![Crates.io](https://img.shields.io/crates/v/mako-fristen?label=mako-fristen&color=f59e0b&logo=rust)](https://crates.io/crates/mako-fristen)

Every Frist in German market communication, in one leaf crate: Werktage and the
BDEW MaKo holiday calendar, the Berlin business date, the CONTRL and APERAK
clocks, and the per-Prüfidentifikator Antwort-, Meldungs- and Vorlauffristen.
It depends only on `time` and `time-tz`, so any service reads its deadlines
here instead of carrying a copy.

## Example

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
| Is this deadline label a delivery window (retired on delivery, owned by no workflow)? | `is_delivery_window` |

## Three clocks, never one number

| Clock | Window | Meaning |
|---|---|---|
| **CONTRL** | 6 wall-clock hours; **15 min** for a Strom UTILMD/ORDERS Syntaxfehlermeldung (6 h on a Saturday), **45 min** for a GABi-Gas ALOCAT; deferred to the end of a Formatumstellung window (CONTRL AHB 1.0 §2.3.1, §2.4.1) | the syntax check of the Übertragungsdatei |
| **APERAK** | 45 min Strom on every day but Samstag (a Saturday arrival is due Sonntag 12:00); Gas: next Werktag 12:00 (Folgeprozess) or 3 Werktage (Initialprozess) | the message was accepted for processing |
| **Antwortfrist** | per PID — 11:00 of the 1. Werktag for a GPKE Anmeldung, 4 Werktage for a Gas Anmeldung, 3/5/7/1 WT for WiM Strom | the *business* answer is owed |

There is no 24-hour GPKE window (`antwort::GPKE_IS_NOT_TWENTY_FOUR_HOURS`), and
the GeLi Gas „10 Werktage" is the supplier's Vorlauffrist, not the
Netzbetreiber's answer window (`antwort::TEN_WERKTAGE_IS_THE_SUPPLIERS_VORLAUFFRIST`).

## Rules

- **A business date is a Berlin date.** `heute()` reads `Europe/Berlin`;
  `cargo xtask check-business-dates` refuses a business date read in UTC.
- **`HolidayCalendar::BdewMaKo`** counts every holiday observed in any
  Bundesland, plus 24.12. and 31.12.; Saturday is not a Werktag.
- **Every window carries its source** (document and chapter). A window with no
  source is absent — `antwort_obligation` answers `None`, which means unknown,
  never unbounded. `cargo xtask check-vorlauf-consulted` holds every `vorlauf`
  row to having a reader.

API documentation: <https://docs.rs/mako-fristen>.

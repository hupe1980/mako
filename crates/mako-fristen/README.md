# mako-fristen

[![Crates.io](https://img.shields.io/crates/v/mako-fristen?label=mako-fristen&color=f59e0b&logo=rust)](https://crates.io/crates/mako-fristen)

Every Frist in German market communication, in one leaf crate: Werktage and the
BDEW MaKo holiday calendar, the Berlin business date, the CONTRL and APERAK
clocks, and the per-Prüfidentifikator Antwort-, Meldungs- and Vorlauffristen.
It depends only on `time` and `time-tz`, so a service reads its deadlines here
instead of carrying a copy. A window no document states is absent, never
guessed: `antwort::antwort_obligation` answers `None`, which means unknown, not
unbounded.

The Werktag here is the market's: a holiday in any one Bundesland counts
nationwide, and 24.12. and 31.12. are holidays (GPKE Teil 1 Kap. 7). It is the
wrong calendar for an appointment in one place — for the holidays of a single
Bundesland use `metering::time::holiday::Bundesland` from the
[`metering`](https://crates.io/crates/metering) crate.

## Example

```rust
use mako_fristen::{self as fristen, HolidayCalendar};
use time::{Date, Month};

// 5 Werktage after Monday 2025-01-06 → Monday 2025-01-13.
let start = Date::from_calendar_date(2025, Month::January, 6).unwrap();
let due   = fristen::add_werktage(start, 5, HolidayCalendar::BdewMaKo);
assert_eq!(due, Date::from_calendar_date(2025, Month::January, 13).unwrap());
```

## What it covers

| Question | Answer |
|---|---|
| What is today's date, in the market's terms? | `heute`, `berlin_date`, `berlin_midnight`, `berlin_now` |
| Which UTC instant does a process date carry? | `tagesbeginn_utc`, `tag_des_beginns` — 00:00 in Strom, the Gastag's 06:00 in Gas |
| When is the n-th Werktag after this instant? | `add_werktage`, `deadline_at_werktage`, `end_of_werktag_after`, `next_werktag_at` |
| Which answer window does PID 55001 carry? | `antwort` |
| Which messages does receiving it oblige me to send? | `meldung` |
| How much Vorlauf does this Anmeldung need? | `vorlauf`, `abmeldung` |
| When does a consumer's Widerrufsfrist end? | `vorlauf::widerrufsfrist` — § 355 Abs. 2 BGB, § 356 Abs. 3 and 4 BGB, § 193 BGB |
| When must the CONTRL / APERAK go out? | `contrl_due_at`, `aperak_strom_due_at`, `aperak_gas_folgeprozess_due_at`, `aperak_gas_initialprozess_due_at` |
| Is this label a delivery window, owned by no workflow? | `is_delivery_window` |
| By when is this invoice owed, and its credit paid out? | `vorlauf::abrechnung_faellig`, `vorlauf::guthaben_faellig` — § 40c Abs. 2 and 3 EnWG |
| May this invoice go out other than as an E-Rechnung? | `e_rechnung` — § 27 Abs. 38 UStG |
| May mako tax this Rechnungsempfänger as domestic? | `empfaengerland` — `DE` only; another Member State or a third country is refused |

## Three clocks, never one number

| Clock | Window | Source |
|---|---|---|
| **CONTRL** | 6 hours; 15 min for a Strom UTILMD/ORDERS Syntaxfehlermeldung (6 h on a Saturday); 45 min for a GABi-Gas ALOCAT; deferred past a Formatumstellung | CONTRL AHB 1.0 Kap. 2.3.1, Kap. 2.4.1 |
| **APERAK** | Strom UTILMD/ORDERS 45 min, a Saturday arrival due Sonntag 12:00; other Strom next Werktag 12:00; Gas next Werktag 12:00 (Folgeprozess) or 3 Werktage (Initialprozess) | APERAK AHB 1.1 Kap. 2.4.1, Kap. 2.3.1 |
| **Antwortfrist** | per PID — 11:00 of the 1. Werktag for a GPKE Anmeldung, 4 Werktage for a Gas Anmeldung, 3/5/7/1 WT for WiM Strom | BK6-24-174, BK7-24-01-009 |

There is no 24-hour GPKE window (`antwort::GPKE_IS_NOT_TWENTY_FOUR_HOURS`), and
the GeLi Gas „10 Werktage" is the supplier's Vorlauffrist, not the
Netzbetreiber's answer window (`antwort::TEN_WERKTAGE_IS_THE_SUPPLIERS_VORLAUFFRIST`).

## Rules

- **A business date is a Berlin date.** `heute()` reads `Europe/Berlin`;
  `cargo xtask check-business-dates` refuses a business date read in UTC.
- **`HolidayCalendar::BdewMaKo`** counts every holiday observed in any
  Bundesland, plus 24.12. and 31.12.; Saturday is not a Werktag (BK6-24-174 GPKE Teil 1 Kap. 7).
- **Every window carries its source.** `cargo xtask check-vorlauf-consulted`
  holds every `vorlauf` row to having a reader.

## Sources

- BK6-24-174 (GPKE, WiM Strom, MaBiS) and BK7-24-01-009 (GeLi Gas 3.0)
- APERAK AHB 1.1, CONTRL AHB 1.0
- BGB §§ 355, 356, 193; UStG § 27 Abs. 38

Deadline reference: [Process Catalog](https://hupe1980.github.io/mako/docs/reference/processes/) ·
API: <https://docs.rs/mako-fristen>

License: MIT OR Apache-2.0

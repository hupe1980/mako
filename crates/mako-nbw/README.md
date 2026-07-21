# mako-nbw

**NBW — Netzbetreiberwechsel (DSO Concession Handover)**

> **This crate is a name reservation. Implementation is pending.**

Process engine workflows for the regulated transfer of all market locations
(MaLo/MeLo) from an outgoing distribution system operator (DSO) to an incoming
DSO when a local grid concession changes hands under **§ 46 EnWG**.

## Domain background

Every 20 years, municipalities competitively award local grid concessions. When
the concession for a grid area changes operator, the outgoing DSO must hand over
a complete, accurate registry of all market participants and location data to the
incoming DSO using standardised **PARTIN** (Party Information) EDIFACT messages.

This is fundamentally different from all other MaKo processes:

| Aspect | GPKE / WiM / GeLi Gas | NBW |
|---|---|---|
| Granularity | Single MeLo / MaLo per message | **Thousands of MaLo/MeLo in one batch** |
| Trigger | Inbound EDIFACT per transaction | **Grid concession transfer event** |
| Duration | Hours to days | **Months (preparation + execution)** |
| Counterparties | LF ↔ NB | **alter NB ↔ neuer NB + suppliers** |
| EDIFACT format | UTILMD, INVOIC, ORDERS | **PARTIN** |

## PID Inventory

All PIDs 37000–37014 are defined in the BDEW PARTIN AHB and covered by the
`edi-energy` crate's PARTIN profile. The PARTIN AHB defines these as
**Kommunikationsdaten** (party communication data) messages.

> **Dual use of PID numbers:** PIDs 37000–37014 serve two distinct purposes
> that share the same message format but differ in context:
>
> 1. **Day-to-day Kommunikationsdaten** — routine partner master data updates
>    (GLN, AS4 endpoint, email) sent between market participants during normal
>    operations. These are registered as simple-receipt workflows:
>    PIDs 37000–37006 in `mako-gpke` (`gpke-partin`) and
>    PIDs 37008–37014 in `mako-geli-gas` (`geli-gas-partin`).
>
> 2. **Netzbetreiberwechsel bulk handover** — the same PARTIN PID numbers are
>    used during a grid concession change (§ 46 EnWG) to transfer thousands of
>    market-location registrations. This is the scope of `mako-nbw` (planned).
>
> Both use cases co-exist. The NBW bulk context is distinguished from day-to-day
> updates by the presence of a bulk transfer header (`BGM` document code) and a
> large MaLo count. `mako-nbw` will handle the NBW-context PARTIN alongside the
> existing Kommunikationsdaten workflows.

Both **Strom** and **Gas** roles are covered within the same PID block.
There is no separate `mako-nbw-gas` crate — Gas NBW (see AWH V1.0 below)
uses the same PARTIN format and PIDs; Gas-specific roles are served by
PIDs 37008–37014.

| PID | Description (PARTIN AHB) | Sparte | Status |
|---|---|---|---|
| 37000 | Kommunikationsdaten des LF Strom | Strom | ⏳ Planned |
| 37001 | Kommunikationsdaten des NB Strom | Strom | ⏳ Planned |
| 37002 | Kommunikationsdaten des MSB Strom | Strom | ⏳ Planned |
| 37003 | Kommunikationsdaten des BKV Strom | Strom | ⏳ Planned |
| 37004 | Kommunikationsdaten des BIKO Strom | Strom | ⏳ Planned |
| 37005 | Kommunikationsdaten des ÜNB Strom | Strom | ⏳ Planned |
| 37006 | Kommunikationsdaten des ESA Strom | Strom | ⏳ Planned |
| 37007 | — (absent from all known AHB versions) | — | — |
| 37008 | Kommunikationsdaten des LF Gas | Gas | ✅ Day-to-day: `mako-geli-gas` · NBW bulk: planned |
| 37009 | Kommunikationsdaten des NB Gas | Gas | ✅ Day-to-day: `mako-geli-gas` · NBW bulk: planned |
| 37010 | Kommunikationsdaten des MSB Gas | Gas | ✅ Day-to-day: `mako-geli-gas` · NBW bulk: planned |
| 37011 | Kommunikationsdaten des MGV Gas | Gas | ✅ Day-to-day: `mako-geli-gas` · NBW bulk: planned |
| 37012 | Spartenübergreifende Kommunikationsdaten (NB an andere) | Both | ✅ Day-to-day: `mako-geli-gas` · NBW bulk: planned |
| 37013 | Spartenübergreifende Kommunikationsdaten (MSB Gas an andere) | Both | ✅ Day-to-day: `mako-geli-gas` · NBW bulk: planned |
| 37014 | Spartenübergreifende Kommunikationsdaten (MSB Strom an andere) | Both | ✅ Day-to-day: `mako-geli-gas` · NBW bulk: planned |

## Market roles

| Role | Abbrev. | Description |
|---|---|---|
| alter Netzbetreiber | alter NB | Outgoing DSO (concession ends) |
| neuer Netzbetreiber | neuer NB | Incoming DSO (concession begins) |
| Lieferant | LF | Affected supplier (notified of location transfer) |
| Bundesnetzagentur | BNetzA | Regulatory authority |

## Architecture (planned)

Because NBW deals with bulk data rather than individual messages, the planned
implementation will differ from other domain crates:

- A single `NbwWorkflow` per concession area (not per MaLo)
- Batch ingestion: the `ReceivePartin` command carries the full list of
  transferred MaLo IDs from a parsed PARTIN message
- Long-running: the workflow may span months with many intermediate state
  transitions before `Settled`

## Regulatory references

- **§ 46 EnWG** — statutory basis for distribution grid concession competition (Strom and Gas)
- **BDEW AWH Netzbetreiberwechselprozesse Strom V1.2** (2025-10-30) — Strom NBW process documentation
- **BDEW AWH Marktprozesse Netzbetreiberwechsel Sparte Gas V1.0** (2026-06-26) — Gas NBW process documentation
- **BNetzA GPKE Mitteilung Nr. 71** (01.07.2024) — Empfehlung Marktprozesse NBW Strom
- **BDEW PARTIN AHB** — Application Handbook for NBW PARTIN messages (PIDs 37000–37014)
- **BNetzA BK6 / BK7** — governing regulatory chambers (electricity / gas)

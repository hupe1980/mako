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

## PID inventory — dual use of 37000–37014

PIDs 37000–37014 are defined in the BDEW PARTIN AHB as **Kommunikationsdaten**
(party communication data) messages and covered by the `edi-energy` PARTIN
profile. The same PID numbers serve two distinct purposes that share the message
format but differ in context:

1. **Day-to-day Kommunikationsdaten** — routine partner master-data updates
   (GLN, AS4 endpoint, email) exchanged during normal operations. These run
   today as simple-receipt workflows: PIDs 37000–37006 in `mako-gpke`
   (`gpke-partin`) and 37008–37014 in `mako-geli-gas` (`geli-gas-partin`).
2. **Netzbetreiberwechsel bulk handover** — the same PARTIN PIDs carried during
   a grid-concession change (§ 46 EnWG) to transfer thousands of market-location
   registrations at once. This is the domain `mako-nbw` reserves. The bulk
   context is distinguished by a bulk-transfer header (`BGM` document code) and a
   large MaLo count.

Both **Strom** and **Gas** roles share the one PID block — there is no separate
`mako-nbw-gas` crate; Gas-specific roles use PIDs 37008–37014.

| PID | Description (PARTIN AHB) | Sparte | Day-to-day home |
|---|---|---|---|
| 37000 | Kommunikationsdaten des LF Strom | Strom | `mako-gpke` |
| 37001 | Kommunikationsdaten des NB Strom | Strom | `mako-gpke` |
| 37002 | Kommunikationsdaten des MSB Strom | Strom | `mako-gpke` |
| 37003 | Kommunikationsdaten des BKV Strom | Strom | `mako-gpke` |
| 37004 | Kommunikationsdaten des BIKO Strom | Strom | `mako-gpke` |
| 37005 | Kommunikationsdaten des ÜNB Strom | Strom | `mako-gpke` |
| 37006 | Kommunikationsdaten des ESA Strom | Strom | `mako-gpke` |
| 37007 | — (absent from all known AHB versions) | — | — |
| 37008 | Kommunikationsdaten des LF Gas | Gas | `mako-geli-gas` |
| 37009 | Kommunikationsdaten des NB Gas | Gas | `mako-geli-gas` |
| 37010 | Kommunikationsdaten des MSB Gas | Gas | `mako-geli-gas` |
| 37011 | Kommunikationsdaten des MGV Gas | Gas | `mako-geli-gas` |
| 37012 | Spartenübergreifende Kommunikationsdaten (NB an andere) | Both | `mako-geli-gas` |
| 37013 | Spartenübergreifende Kommunikationsdaten (MSB Gas an andere) | Both | `mako-geli-gas` |
| 37014 | Spartenübergreifende Kommunikationsdaten (MSB Strom an andere) | Both | `mako-geli-gas` |

## Market roles

| Role | Abbrev. | Description |
|---|---|---|
| alter Netzbetreiber | alter NB | Outgoing DSO (concession ends) |
| neuer Netzbetreiber | neuer NB | Incoming DSO (concession begins) |
| Lieferant | LF | Affected supplier (notified of location transfer) |
| Bundesnetzagentur | BNetzA | Regulatory authority |

## Architecture

NBW handles bulk data rather than individual messages, so its shape differs from
the other domain crates: one `NbwWorkflow` per concession area (not per MaLo),
batch ingestion (a single command carrying the full transferred-MaLo list from a
parsed PARTIN message), and a long-running lifecycle that may span months of
intermediate state transitions before settlement.

## Regulatory references

- **§ 46 EnWG** — statutory basis for distribution grid concession competition (Strom and Gas)
- **BDEW AWH Netzbetreiberwechselprozesse Strom V1.2** (2025-10-30) — Strom NBW process documentation
- **BDEW AWH Marktprozesse Netzbetreiberwechsel Sparte Gas V1.0** (2026-06-26) — Gas NBW process documentation
- **BNetzA GPKE Mitteilung Nr. 71** (01.07.2024) — Empfehlung Marktprozesse NBW Strom
- **BDEW PARTIN AHB** — Application Handbook for NBW PARTIN messages (PIDs 37000–37014)
- **BNetzA BK6 / BK7** — governing regulatory chambers (electricity / gas)

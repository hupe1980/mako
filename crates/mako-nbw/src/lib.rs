//! `mako-nbw` — Netzbetreiberwechsel (DSO concession change) process engine
//! for the German energy market.
//!
//! **This crate is a name reservation. Implementation is pending.**
//!
//! # Domain background
//!
//! **Netzbetreiberwechsel** (NBW) is the regulated process by which all market
//! locations (MaLo/MeLo) registered under one distribution system operator
//! (VNB/NB) are transferred to a new operator when a local distribution
//! concession changes hands. Concessions are competitively awarded every
//! 20 years under **§ 46 EnWG**; when a municipality's concession transfers,
//! the new DSO must receive a complete handover of all market participants and
//! location data from the outgoing operator.
//!
//! The BDEW has defined standardised **PARTIN** (Party Information) EDIFACT
//! messages for this data transfer. Unlike all other MaKo processes, NBW
//! operates as a **bulk data migration** rather than an event-driven
//! per-message workflow: thousands of MaLo records may be transferred in a
//! single NBW event.
//!
//! # PARTIN PID range
//!
//! All PIDs 37000–37014 are defined in the BDEW PARTIN AHB and covered by
//! the `edi-energy` crate's PARTIN profile. The PARTIN AHB defines these as
//! **Kommunikationsdaten** (party communication data) messages exchanged
//! during and after the NBW bulk handover — **not** process-coordination
//! EDIFACT messages. Both Strom and Gas roles are covered within the same
//! PID block; there is no separate "PARTIN Gas" profile or AHB.
//!
//! | PID | Description (PARTIN AHB) | Sparte |
//! |---|---|---|
//! | 37000 | Kommunikationsdaten des LF Strom | Strom |
//! | 37001 | Kommunikationsdaten des NB Strom | Strom |
//! | 37002 | Kommunikationsdaten des MSB Strom | Strom |
//! | 37003 | Kommunikationsdaten des BKV Strom | Strom |
//! | 37004 | Kommunikationsdaten des BIKO Strom | Strom |
//! | 37005 | Kommunikationsdaten des ÜNB Strom | Strom |
//! | 37006 | Kommunikationsdaten des ESA Strom | Strom |
//! | 37008 | Kommunikationsdaten des LF Gas | Gas |
//! | 37009 | Kommunikationsdaten des NB Gas | Gas |
//! | 37010 | Kommunikationsdaten des MSB Gas | Gas |
//! | 37011 | Kommunikationsdaten des MGV Gas | Gas |
//! | 37012 | Spartenübergreifende Kommunikationsdaten des NB Gas, MSB Gas und MSB Strom (NB an andere) | Both |
//! | 37013 | Spartenübergreifende Kommunikationsdaten des NB Gas, MSB Gas und MSB Strom (MSB Gas an andere) | Both |
//! | 37014 | Spartenübergreifende Kommunikationsdaten des NB Gas, MSB Gas und MSB Strom (MSB Strom an andere) | Both |
//!
//! Note: PID 37007 is absent from all known profile versions.
//!
//! # Gas NBW coverage
//!
//! The BDEW AWH **Marktprozesse Netzbetreiberwechsel Sparte Gas V1.0**
//! (published 2026-06-26, `docs/pdfs/bdew-mako/BDEW_VKU_GEODE_AWH_Marktprozesse
//! Netzbetreiberwechsel Sparte Gas_V1_0_20260626.pdf`) defines Gas NBW process
//! flows. Gas NBW uses the same PARTIN message format and the same PID block
//! (37000–37014) as Strom NBW — the Gas-specific roles are covered by PIDs
//! 37008–37014. There is no separate `mako-nbw-gas` crate; when this crate is
//! implemented it will handle all 37000–37014 PIDs and route by Sparte internally.
//!
//! # Market roles
//!
//! | Role | Abbrev. | Description |
//! |------|---------|-------------|
//! | alter Netzbetreiber | alter NB | Outgoing DSO (losing concession) |
//! | neuer Netzbetreiber | neuer NB | Incoming DSO (winning concession) |
//! | Lieferant | LF | Supplier (notified of location transfer) |
//! | Bundesnetzagentur | BNetzA | Regulatory authority |
//!
//! # Key characteristics
//!
//! Unlike GPKE/WiM/GeLi Gas which operate on single location/single message
//! granularity, NBW processes operate on:
//! - **Batch scope**: thousands of MaLo/MeLo records per concession area
//! - **Long duration**: the handover period spans months (preparation + execution)
//! - **Bilateral coordination**: tight sequencing between old NB, new NB, and
//!   affected suppliers
//!
//! # Regulatory references
//!
//! - **§ 46 EnWG** — statutory basis for distribution grid concession competition
//! - **BDEW PARTIN AHB** — Application Handbook for NBW PARTIN messages
//! - **BNetzA BK6** rulings governing concession handover procedures
//! - **BDEW MaKo documentation** at [bdew-mako.de](https://www.bdew-mako.de)
//!
//! # Dependencies (planned)
//!
//! When implemented this crate will depend on:
//! - `mako-engine` — event-sourced workflow runtime
//! - `edi-energy` — PARTIN message parsing and validation

#![deny(unsafe_code)]
#![deny(missing_docs)]

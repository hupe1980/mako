//! `mako-gabi-gas` — GaBi Gas process engine for the German gas market
//! (Gasbilanzierung Gas).
//!
//! **This crate is a name reservation. Implementation is pending until
//! `dvgw-edi` (the DVGW format layer) is complete.**
//!
//! # Two-crate architecture for GaBi Gas
//!
//! | Crate | Responsibility | Status |
//! |---|---|---|
//! | `dvgw-edi` | ALLOCAT, NOMINT, NOMRES parsing and validation | ⏳ Placeholder |
//! | `mako-gabi-gas` | Process engine — Workflow impls, PID routing, deadline handling | ⏳ **This crate** |
//!
//! # Domain background
//!
//! **GaBi Gas** (*Gasbilanzierung Gas*) is the German regulatory framework for
//! gas balancing, established by the Bundesnetzagentur (BNetzA) under the
//! Gasnetzzugangsverordnung (GasNZV). The current version, **GaBi Gas 2.0**,
//! entered into force with BNetzA order **BK7-14-020**.
//!
//! The framework governs the exchange of gas quantity data between balance
//! responsible parties (BKV), network operators (FNB/VNB), and market area
//! managers (MGV) via standardised EDIFACT messages.
//!
//! # Process families (planned)
//!
//! | Process | Primary messages | Governing document |
//! |---|---|---|
//! | Allokation (gas quantity allocation) | ALLOCAT | DVGW AHB ALLOCAT |
//! | Nominierung (gas nominations) | NOMINT / NOMRES | DVGW AHB NOMINT |
//! | Mehr-/Mindermengenabrechnung (reconciliation billing) | INVOIC / REMADV | BDEW/DVGW MIG |
//! | Tagesbilanz / Monatsbilanz (balance reporting) | ALLOCAT | DVGW AHB |
//!
//! # Market roles
//!
//! | Role | Abbrev. | Description |
//! |------|---------|-------------|
//! | Fernleitungsnetzbetreiber | FNB | Gas transmission system operator |
//! | Verteilnetzbetreiber | VNB | Gas distribution system operator |
//! | Bilanzkreisverantwortlicher | BKV | Balance responsible party |
//! | Marktgebietsverantwortlicher | MGV | Market area manager |
//! | Großhändler / Produzent | GH | Gas wholesaler / producer |
//!
//! # Regulatory references
//!
//! - **GasNZV** (Gasnetzzugangsverordnung) — statutory basis for gas network
//!   access and balancing
//! - **BNetzA BK7-14-020** — GaBi Gas 2.0 ruling (current)
//! - Note: BK7-06-067 is the original **GeLi Gas** ruling, not GaBi Gas
//! - **DVGW G 685** — technical rules for gas metering and allocation
//!
//! # Dependencies (planned)
//!
//! When implemented this crate will depend on:
//! - `mako-engine` — event-sourced workflow runtime
//! - `dvgw-edi` — DVGW EDIFACT format layer (ALLOCAT, NOMINT, NOMRES)

#![deny(unsafe_code)]
#![deny(missing_docs)]

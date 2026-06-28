//! `dvgw-edi` — DVGW EDIFACT format parser and validator for the German gas
//! market.
//!
//! **This crate is a name reservation. Implementation is pending.**
//!
//! # Scope
//!
//! [`edi-energy`] covers the BDEW EDI@Energy formats used in both electricity
//! and gas markets (UTILMD, MSCONS, INVOIC, REMADV, APERAK, CONTRL, …). This
//! crate covers the **DVGW-governed EDIFACT formats** that are specific to gas
//! network and balancing processes — primarily used by gas transmission system
//! operators (FNB), distribution system operators (VNB), balance responsible
//! parties (BKV), and market area managers (MGV).
//!
//! # Format family
//!
//! | Message | UN/EDIFACT version | Description |
//! |---|---|---|
//! | `ALLOCAT` | D03A | Gas allocation (Allokationsnachricht) |
//! | `NOMINT` | D01B | Nomination integration (Nominierungsintegration) |
//! | `NOMRES` | D01B | Nomination response (Nominierungsantwort) |
//! | `APERAK` | D01B | Application error and acknowledgement (shared) |
//! | `CONTRL` | D14A | Interchange control acknowledgement (shared) |
//!
//! Application Handbooks (AHBs) and Message Implementation Guides (MIGs) for
//! these formats are published by DVGW as part of the **GaBi Gas** regulatory
//! framework.
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
//! - DVGW AHBs and MIGs published at
//!   [dvgw.de](https://www.dvgw.de) and
//!   [bdew-mako.de](https://www.bdew-mako.de)
//!
//! # Relationship to other crates
//!
//! | Crate | Layer | Status |
//! |---|---|---|
//! | `dvgw-edi` | EDIFACT parsing/validation (ALLOCAT, NOMINT, NOMRES) | ⏳ **This crate** — name reservation |
//! | `mako-gabi-gas` | GaBi Gas process engine (Workflow impls, deadline handling) | ⏳ Placeholder |
//! | `mako-engine` | Event-sourced workflow runtime | ✅ In workspace |
//! | `edi-energy` | BDEW EDI@Energy formats (UTILMD, MSCONS, APERAK, …) | ✅ In workspace |

#![deny(unsafe_code)]
#![deny(missing_docs)]

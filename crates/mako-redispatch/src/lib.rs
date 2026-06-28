//! `mako-redispatch` — Redispatch 2.0 process engine for German grid
//! congestion management (§§ 13, 13a, 14 EnWG).
//!
//! **This crate is a name reservation. Implementation is pending until
//! `redispatch-xml` (the format/XSD layer) is complete.**
//!
//! # Three-crate architecture for Redispatch 2.0
//!
//! | Crate | Responsibility | Status |
//! |---|---|---|
//! | `edi-energy` | IFTSTA status messages (EDIFACT) | ✅ In workspace |
//! | `redispatch-xml` | XML/XSD format parsing and validation (ActivationDocument, PlannedResourceScheduleDocument, Stammdaten, …) | ⏳ Placeholder |
//! | `mako-redispatch` | Process engine — Workflow impls, PID routing, deadline handling | ⏳ **This crate** |
//!
//! # Domain background
//!
//! **Redispatch 2.0** entered into force on **1 October 2021** via the
//! Netzausbaubeschleunigungsgesetz (NABEG) and requires all German grid
//! operators to coordinate congestion management across transmission and
//! distribution networks.
//!
//! Redispatch 2.0 uses **CIM/IEC 62325-based XML documents** for the primary
//! data exchange (handled by `redispatch-xml`), and **IFTSTA (EDIFACT)** for
//! status messages (handled by `edi-energy`). Unlike GPKE/WiM/GeLi Gas, this
//! process family does not use UTILMD or APERAK.
//!
//! # Planned PID range
//!
//! PIDs for Redispatch 2.0 workflows will be registered in the `PidRouter`
//! once the process engine is implemented.
//!
//! | Process | Governing document |
//! |---|---|
//! | Abruffahrplan Redispatch | BDEW Redispatch 2.0 specification |
//! | Stammdatenübermittlung | BDEW Redispatch 2.0 specification |
//! | Verfügbarkeitsmeldung | BDEW Redispatch 2.0 specification |
//!
//! # Dependencies (planned)
//!
//! When implemented this crate will depend on:
//! - `mako-engine` — event-sourced workflow runtime
//! - `redispatch-xml` — Redispatch 2.0 XML format layer
//! - `edi-energy` — IFTSTA status message parsing

//! `redispatch-xml` — Redispatch 2.0 XML/XSD format parsing and validation
//! for the German electricity grid (§§ 13, 13a, 14 EnWG).
//!
//! # Domain background
//!
//! **Redispatch 2.0** entered into force on **1 October 2021** via the
//! Netzausbaubeschleunigungsgesetz (NABEG) and requires all German grid
//! operators to coordinate congestion management across transmission and
//! distribution networks. It covers:
//!
//! - All renewable-energy (EE) and combined heat-and-power (KWK) plants
//!   with ≥ 100 kW installed capacity
//! - All installations permanently remote-controllable by a grid operator
//!   (e.g. via Smart-Meter-Gateway)
//!
//! # Format family
//!
//! Redispatch 2.0 uses CIM/IEC 62325-based XML documents for the primary data
//! exchange between TSOs, DSOs, and balance responsible parties. Documents are
//! validated against BDEW-published XSD schemas (topicGroupId 25 in the BDEW
//! MaKo document API).
//!
//! | Document type | Current version | Valid from |
//! |---|---|---|
//! | `ActivationDocument` | XSD 1.1d | 2025-10-01 |
//! | `PlannedResourceScheduleDocument` | XSD 1.0f | 2025-10-01 |
//! | `AcknowledgementDocument` | 1.0f | 2025-10-01 |
//! | `Stammdaten` | XSD 1.4b | 2025-10-01 |
//! | `StatusRequest_MarketDocument` | XSD 1.1 | 2025-10-01 |
//! | `Unavailability_MarketDocument` | XSD 1.1b | 2025-10-01 |
//! | `Beschaffungsanforderung_energetischerAusgleich` | XSD | 2025-10-01 |
//! | `Beschaffungsvorbehalt` | XSD | 2025-10-01 |
//! | `Kostenblatt` | XSD | 2025-10-01 |
//!
//! These are **not** EDIFACT. IFTSTA status messages for the Redispatch 2.0
//! workflow are handled separately by the `edi-energy` crate.
//!
//! # Market roles
//!
//! | Role | Abbrev. | Description |
//! |------|---------|-------------|
//! | Übertragungsnetzbetreiber | ÜNB | Transmission system operator |
//! | Verteilnetzbetreiber | VNB | Distribution system operator |
//! | Anlagenbetreiber | ANB | Generation asset operator |
//! | Direktvermarkter | DV | Direct marketer of renewable energy |
//! | Bilanzkreisverantwortlicher | BKV | Balance responsible party |
//!
//! # Regulatory references
//!
//! - **§§ 13, 13a, 14 EnWG** — statutory basis for grid congestion management
//! - **NABEG 2019** — introduced Redispatch 2.0, effective 1 October 2021
//! - **BNetzA BK6-20-059/060/061** — three BNetzA rulings governing billing
//!   balance, grid operator coordination, and information provision
//! - **BDEW XML-Datenformate Redispatch 2.0** — XSD schemas and application
//!   guidelines published on [bdew-mako.de](https://www.bdew-mako.de)
//!
//! This crate is a name reservation. Implementation is coming soon.

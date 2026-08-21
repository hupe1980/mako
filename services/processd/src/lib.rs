//! `processd` — Process decision engine for German energy market automation.
//!
//! Consumes `de.mako.process.initiated` from `marktd`'s durable fan-out and
//! applies the policy of the role that owes the answer, inside the regulatory
//! Frist. What it cannot decide it puts in front of an operator with that Frist
//! attached.
//!
//! # Role
//!
//! Consumes `de.mako.process.initiated` CloudEvents from `marktd` and applies
//! role-specific policy to make decisions within regulatory deadlines.
//!
//! ## LF module (`role-lf-strom`, `role-lf-gas`)
//!
//! Answers the GPKE processes the NB initiates against the supplier, from
//! `VersorgungsStatus` and without ERP involvement, escalating to
//! `approval_queue` when the data does not decide it:
//! - **Ankündigung NB-seitiges Lieferende** — inbound **55007**, answered
//!   55008/55009 (EBD `E_0609`), due **05:00 Uhr des 1. WT nach dem ÜT**.
//! - **Anfrage zur Beendigung der Zuordnung** — inbound **55010**, answered
//!   55011/55012 (EBD `E_0624`), due **09:00 Uhr des 1. WT nach dem ÜT**.
//!
//! The 45-minute APERAK window on the same message is a separate clock and is
//! `makod`'s to answer.
//!
//! ## NB module (`role-nb-strom`, `role-nb-gas`)
//!
//! - **Anmeldung** — **55001** (verbrauchende MaLo), **55077** (erzeugende
//!   MaLo, § 10c EEG Monatserster rule) and **44001** (Gas). EBD `E_0622`.
//! - **Abmeldung** — **55004** and **44004**, the Lieferende a supplier
//!   initiates. EBD `E_0607`, whose ERC codes are a *different* space from the
//!   Anmeldung's.
//! - Evaluation via the `mako-pruefung` pure library.
//! - **EoG gap closure** (§ 36/§ 38 EnWG) and the daily 3-month timer.
//! - The MSB-Wechsel PIDs the NB answers: **55042** (Anmeldung MSB) and
//!   **55051** (Ende MSB).
//! - STP target ≥ 95 %, which needs `malo_grid` coverage in `marktd`.
//!
//! **55016 „Kündigung" is not here**: it is LFN → LFA (Anwendungsübersicht 4.0
//! lfd. Nr. 20030), answered by the Altlieferant under EBD `E_0614`.
//!
//! ## MSB module (`role-msb-strom`)
//!
//! The Messstellenbetreiber's own obligations, deliberately **not** compiled
//! into an NB binary:
//! - **REQOTE → auto QUOTES** from `PreisblattMessung`; anything it cannot
//!   quote automatically becomes an approval-queue entry with its 5-Werktage
//!   Frist, never a bare log line.
//! - **§ 14a Steuerungsauftrag** auto-ORDRSP against the contracted
//!   `konfigurationsprodukte`.
//! - The MSB-Wechsel PIDs the MSB answers: **55039** (Kündigung MSB, MSBN →
//!   MSBA — it never reaches the NB) and **55168** (Verpflichtungsanfrage).
//!
//! ## Fristen
//!
//! Every business answer window comes from [`fristen`], which reads the same
//! per-family tables `makod` registers the process deadline from. They are not
//! flat durations: the GPKE ones are wall-clock instants on the first Werktag
//! after the Übertragungstag, and the GeLi Gas ones run to the end of the
//! *n*-th Werktag after receipt.
//!
//! # Regulatory basis
//!
//! - GPKE: BK6-24-174 Teil 2 — per-process Antwortfristen, in
//!   [`mako_gpke::antwortfrist`]
//! - GeLi Gas: BK7-24-01-009 Kap. 2.6 / 3.2.2 / 3.2.3, in
//!   [`mako_geli_gas::antwortfrist`]
//! - WiM Strom Teil 1: per-PID Antwortfristen (3 / 5 / 7 / 1 WT)
//! - EBD 4.3: `E_0607`, `E_0609`, `E_0614`, `E_0622`, `E_0624`
//! - § 7 EnWG: the role features are separate binaries; § 20 EnWG:
//!   `initiator_is_affiliate` recorded for every decision
#![deny(unsafe_code)]
#![allow(clippy::doc_markdown)]

pub mod config;
pub mod fristen;
pub mod handler;
pub mod mcp_server;
pub mod metrics;
pub mod pg;
pub mod server;

#[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
pub mod nb_module;

#[cfg(any(feature = "role-lf-strom", feature = "role-lf-gas"))]
pub mod lf_module;

// The MSB-Wechsel machinery is shared: the NB answers 55042/55051 and the MSB
// answers 55039/55168, so the module compiles for either role.
#[cfg(any(
    feature = "role-nb-strom",
    feature = "role-nb-gas",
    feature = "role-msb-strom"
))]
pub mod msb_module;

#[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
pub mod eog_module;

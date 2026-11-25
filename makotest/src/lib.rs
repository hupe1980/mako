//! PyO3 bindings backing the `makotest` Python toolkit.
//!
//! The bindings reuse the production check-digit validators, calendar, decision
//! trees, AHB/MIG profiles and message builders. Python supplies fixture data,
//! controls simulated counterparties and compares the observed outcomes.
//!
//! Module map:
//!
//! | Module | Binds |
//! |---|---|
//! | [`identifiers`] | `rubo4e::identifiers` — MaLo, MeLo, MP-ID, EIC, Kap. 8.2 resource IDs |
//! | [`fristen`] | `mako-fristen` — Werktag calendar, ack clocks, the answer-Frist table |
//! | [`pids`] | `edi-energy` registry — PID introspection, releases, the AHB answer pairs |
//! | [`antwortcodes`] | `mako-pruefung` — the Entscheidungsbaum Codelisten a counterparty may answer with |
//! | [`edifact`] | `edi-energy` — build UTILMD/MSCONS/REMADV/ORDERS/ORDRSP/IFTSTA/QUOTES/APERAK/CONTRL, validate an interchange |
//! | [`events`] | `mako-events` — the CloudEvents type catalog and its glob matcher |

#![deny(unsafe_code)]

pub mod antwortcodes;
pub mod edifact;
pub mod events;
pub mod fristen;
pub mod identifiers;
pub mod pids;
mod rehearsal;

use pyo3::prelude::*;
use pyo3::types::PyModule;

/// The BO4E schema generation the platform's object model is generated from.
///
/// Asked of `rubo4e::current` rather than written down — naming a version alias
/// here would be the very drift this function exists to detect. Compare it
/// against whatever the system under test advertises before asserting over
/// business objects: assertions written for one generation against a platform on
/// another produce passes that mean nothing.
#[pyfunction]
fn bo4e_schema_version() -> String {
    use rubo4e::Bo4eTyped as _;
    rubo4e::current::Marktlokation::SCHEMA_VERSION.to_owned()
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add(
        "__doc__",
        "Rust core of makotest — BDEW identifiers, Fristen, EDIFACT.",
    )?;
    m.add_function(wrap_pyfunction!(bo4e_schema_version, m)?)?;
    m.add_function(wrap_pyfunction!(rehearsal::decommissioned_nb_refusal, m)?)?;
    m.add_function(wrap_pyfunction!(rehearsal::esa_invoice_verdict, m)?)?;
    identifiers::register(m)?;
    events::register(m)?;
    fristen::register(m)?;
    pids::register(m)?;
    antwortcodes::register(m)?;
    edifact::register(m)?;
    Ok(())
}

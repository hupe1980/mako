//! PyO3 bindings backing the `makotest` Python toolkit.
//!
//! Only the concerns a *regulator* defines in a table are bound here — BDEW
//! identifier check digits, the Werktag/Feiertag calendar, the published
//! answer-Frist table, AHB/MIG validation and the message builders. Everything
//! shaped by test ergonomics (price curves, counterparty behaviour, fixtures)
//! stays in Python.
//!
//! The reason for the split is drift. A second implementation of the AHB rule
//! tables, of the BDEW check digit, or of "how long does a Netzbetreiber have
//! to answer a 55001" would disagree with production at the first
//! Formatumstellung — and a harness that disagrees with the system under test
//! about what is valid, or about when a Frist expires, is worse than no
//! harness.
//!
//! Module map:
//!
//! | Module | Binds |
//! |---|---|
//! | [`identifiers`] | `rubo4e::identifiers` — MaLo, MeLo, MP-ID, EIC, §8.2 resource IDs |
//! | [`fristen`] | `mako-fristen` — Werktag calendar, ack clocks, the answer-Frist table |
//! | [`pids`] | `edi-energy` registry — PID introspection, releases, the AHB answer pairs |
//! | [`edifact`] | `edi-energy` — build UTILMD/MSCONS/APERAK/CONTRL, validate an interchange |
//! | [`events`] | `mako-events` — the CloudEvents type catalog and its glob matcher |

pub mod edifact;
pub mod events;
pub mod fristen;
pub mod identifiers;
pub mod pids;

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
    use rubo4e::Bo4eObject as _;
    rubo4e::current::Marktlokation::default()
        .schema_version()
        .to_owned()
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add(
        "__doc__",
        "Rust core of makotest — BDEW identifiers, Fristen, EDIFACT.",
    )?;
    m.add_function(wrap_pyfunction!(bo4e_schema_version, m)?)?;
    identifiers::register(m)?;
    events::register(m)?;
    fristen::register(m)?;
    pids::register(m)?;
    edifact::register(m)?;
    Ok(())
}

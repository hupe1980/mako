//! Integration tests of `mako-geli-gas`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod answer_pid_conformance;
mod cross_fv;
mod datenabruf;
mod invoic;
mod lieferbeginn;
mod no_gas_anerkennungsmeldung;
mod sperrung;
mod stornierung_routing;
mod utilmd;

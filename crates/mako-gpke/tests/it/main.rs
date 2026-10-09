//! Integration tests of `mako-gpke`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod answer_pid_conformance;
mod bilateral_lieferbeginn;
mod cross_fv;
mod deadline_wiring;
mod konfiguration;
mod lf_abmeldung;
mod lf_antwort_reaches_the_wire;
mod nb_antwort_reaches_the_wire;
mod sperrung;
mod stornierung;
mod wechselprozesse;

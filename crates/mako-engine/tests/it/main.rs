//! Integration tests of `mako-engine`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod append_conformance;
mod cross_fv_dispatch;
mod fristen_exact_utc;
mod slatedb_store;

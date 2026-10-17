//! Integration tests of `netzbilanzd`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod authorization_guard;
mod dev_config;
mod documented_examples;
mod invoice_drafts_pg;
mod mabis;
mod router_builds;
mod schema_enum_guard;

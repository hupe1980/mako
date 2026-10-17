//! Integration tests of `billingd`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod authorization;
mod demo_config;
mod demo_payloads;
mod dev_config;
mod einvoice_render;
mod print_config;
mod records_integration;
mod router_builds;
mod schema_code_guard;

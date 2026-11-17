//! Integration tests of `einsd`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod authorization_guard;
mod demo_config;
mod mcp_round_trips;
mod mutating_tools;
mod openapi_coverage;
mod router_builds;
mod schema_code_guard;
mod seed_payloads;
mod settlement_integration;
mod settlement_tests;
mod verguetungssaetze_seed_guard;

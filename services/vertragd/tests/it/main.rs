//! Integration tests of `vertragd`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod authorization_guard;
mod contract_tests;
mod demo_config;
mod demo_payloads;
mod dev_config;
mod dispatch_integration;
mod schema_enum_guard;

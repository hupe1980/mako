//! Integration tests of `productd`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod angebot_conformance;
mod angebot_pricing;
mod authorization_guard;
mod bo4e_angebot;
mod bo4e_conformance;
mod catalog_integration;
mod demo_config;
mod demo_payloads;
mod dev_config;
mod schema_enum_guard;
mod tariff_tests;

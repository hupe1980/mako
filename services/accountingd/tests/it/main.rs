//! Integration tests of `accountingd`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod authorization_guard;
mod db_scenarios;
mod demo_config;
mod demo_payloads;
mod dev_config;
mod integration_tests;
mod router_builds;
mod startup_posture;
mod unit_tests;

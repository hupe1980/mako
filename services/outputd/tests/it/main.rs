//! Integration tests of `outputd`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod anrede;
mod authorization;
mod demo_config;
mod relay_signing;
mod router_builds;
mod seed_payloads;
mod store_integration;
mod zugferd_carrier;

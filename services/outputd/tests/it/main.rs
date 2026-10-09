//! Integration tests of `outputd`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod authorization;
mod demo_config;
mod demo_payloads;
mod dev_config;
mod relay_signing;
mod store_integration;
mod zugferd_carrier;

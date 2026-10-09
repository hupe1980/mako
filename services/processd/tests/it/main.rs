//! Integration tests of `processd`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod authorization_guard;
mod demo_config;
mod dev_config;
mod esa_pid_contract;
mod mcp_route_contract;
mod pid_contract;
mod published_config;
mod role_separation;
mod schema_enum_guard;
mod sql_integration;
mod startup_posture;

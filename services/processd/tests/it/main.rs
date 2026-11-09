//! Integration tests of `processd`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod authorization_guard;
mod demo_config;
mod dev_config;
#[cfg(feature = "role-lf-gas")]
mod eog_answer_contract;
#[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
mod eog_command_contract;
mod esa_pid_contract;
mod former_actions;
mod mcp_round_trips;
mod mcp_route_contract;
mod obs;
mod pid_contract;
mod published_config;
mod request_bodies;
mod role_separation;
mod router_builds;
mod schema_enum_guard;
#[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
mod sperr;
mod sql_integration;
mod startup_posture;

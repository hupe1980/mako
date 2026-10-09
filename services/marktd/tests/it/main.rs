//! Integration tests of `marktd`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod authorization_guard;
mod demo_config;
mod demo_payloads;
mod dev_config;
mod esa_integration;
mod fanout_durable_integration;
mod mabis_zp_integration;
mod melo_graph_integration;
mod overlap_is_a_client_error;
mod published_examples;
mod registries_integration;
mod schema_enum_guard;
mod temporal_constraints_integration;
mod tenant_isolation_integration;
mod versorgung_integration;

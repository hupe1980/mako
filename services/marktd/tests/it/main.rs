//! Integration tests of `marktd`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod authorization_guard;
mod demo_config;
mod esa_integration;
mod grundzustaendiger_msb;
mod konfigurationsprodukte_body;
mod lokationsbuendel_integration;
mod mabis_zp_integration;
mod malo_write_calls_no_makod;
mod mcp_round_trips;
mod melo_graph_integration;
mod openapi_coverage;
mod overlap_is_a_client_error;
mod published_examples;
mod referenzreihen_integration;
mod registries_integration;
mod router_builds;
mod schema_enum_guard;
mod seed_payloads;
mod temporal_constraints_integration;
mod tenant_isolation_integration;
mod utilmd_stammdaten_integration;
mod versorgung_integration;
mod webhooks_integration;
mod write_and_event_commit_together;
mod zuordnungsermaechtigung_integration;

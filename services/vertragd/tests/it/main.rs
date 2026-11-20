//! Integration tests of `vertragd`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod authorization_guard;
mod bo4e_bodies;
mod contract_tests;
mod demo_config;
mod dispatch_integration;
mod identitaet_body;
mod inbound_marker;
mod mcp_round_trips;
mod online_vertragsschluss;
mod openapi_coverage;
mod processd_supply_contract;
mod router_builds;
mod sales_edge;
mod schema_enum_guard;
mod seed_payloads;
mod webhooks;

//! Integration tests of `accountingd`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod authorization_guard;
mod db_scenarios;
mod demo_config;
mod eeg_payout;
mod integration_tests;
mod mcp_round_trips;
mod receivables;
mod request_bodies;
mod router_builds;
mod seed_payloads;
mod sharing_booking;
mod startup_posture;
mod unit_tests;

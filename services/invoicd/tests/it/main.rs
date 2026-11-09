//! Integration tests of `invoicd`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod cedar_actions;
mod dev_config;
mod dtm_qualifiers;
mod mcp_round_trips;
mod receipts_pg;
mod receiving_has_no_grace;
mod startup;
mod startup_posture;

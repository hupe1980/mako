//! Integration tests of `agentd`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod deadline_triage;
mod dev_config;
mod durability;
mod evidence;
mod ingest;
mod oversight;
mod plane_golden_run;
mod procedure_contract;
mod regulatory;
mod router_builds;
mod specialist_smoke;

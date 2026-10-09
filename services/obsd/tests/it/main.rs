//! Integration tests of `obsd`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod dev_config;
mod startup_posture;
mod wire_shape;
mod worker_integration;

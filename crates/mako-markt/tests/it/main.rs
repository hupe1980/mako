//! Integration tests of `mako-markt`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod demo_fixtures;
mod regulatory_scenarios;

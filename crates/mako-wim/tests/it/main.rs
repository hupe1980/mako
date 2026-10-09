//! Integration tests of `mako-wim`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod esa_wertebestellung;
mod geraeteubernahme;
mod geraetewechsel;
mod utilmd;
mod wertebestellung;

//! Integration tests of `mako-gabi-gas`, linked as one test binary: one module per
//! file in this directory. A crate links its dependency graph once per test
//! binary, so a file per binary multiplies link time and disk by the file count.

mod allocation;
mod invoic;
mod nomination;

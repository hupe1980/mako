//! `#[allow]` only shrinks.
//!
//! An `#[allow]` stays silent when the lint it suppresses stops firing, so a
//! stale one is invisible; `#[expect]` fails the build instead. Every
//! suppression carries a `reason` (`allow_attributes_without_reason`, in the
//! workspace lints), and an `#[allow]` is kept only where the lint fires in
//! some feature or target combinations and not in others — where an
//! expectation would be unfulfilled in the builds that do not reach the code.
//!
//! [`CEILING`] is the count of those today. A new `#[allow]` fails here: write
//! `#[expect(…, reason = "…")]`, or, if it is one of the cfg-dependent cases,
//! raise the ceiling in the same change and say why in its reason.

use std::path::Path;

/// `#[allow(` / `#![allow(` attributes in `crates/`, `services/`, `xtask/` and
/// `makotest/src`. Only shrinks. Two of them are makod's AS4 test seams
/// (`As4Clients::build_with_http`, the `As4Evidence` readers): `pub` in the
/// library target the integration tests use, dead in the binary target that
/// compiles the same modules — an `#[expect]` is unfulfilled in one of the two.
const CEILING: usize = 81;

/// The `#[allow(` attributes in `src` — attribute lines, not prose that names
/// one.
fn count(src: &str) -> usize {
    src.lines()
        .map(str::trim_start)
        .filter(|l| l.starts_with("#[allow(") || l.starts_with("#![allow("))
        .count()
}

fn walk(dir: &Path, total: &mut usize) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name()
                .is_some_and(|n| n == "target" || n == "clippy-fixtures")
            {
                continue;
            }
            walk(&p, total);
        } else if p.extension().is_some_and(|x| x == "rs") {
            *total += count(&std::fs::read_to_string(&p).unwrap_or_default());
        }
    }
}

fn workspace_count(root: &Path) -> usize {
    let mut total = 0;
    for d in ["crates", "services", "xtask", "makotest/src"] {
        walk(&root.join(d), &mut total);
    }
    total
}

#[test]
fn the_allow_count_does_not_rise() {
    let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
    let n = workspace_count(root);
    assert!(
        n <= CEILING,
        "{n} `#[allow(` attributes, ceiling {CEILING} — write `#[expect(…, reason = \"…\")]` instead"
    );
    assert!(
        n >= CEILING,
        "{n} `#[allow(` attributes, ceiling {CEILING} — lower the ceiling to {n}"
    );
}

#[test]
fn an_added_allow_is_counted_and_prose_is_not() {
    let src = "#[allow(dead_code, reason = \"x\")]\nfn f() {}\n// a #[allow(foo)] in a comment\n    #![allow(clippy::x, reason = \"y\")]\n";
    assert_eq!(count(src), 2);
}

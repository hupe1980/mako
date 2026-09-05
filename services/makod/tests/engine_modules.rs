//! Guard: every domain module a crate implements is wired into `makod`.
//!
//! A domain crate becomes part of the platform in two places that never meet.
//! It implements `mako_engine::builder::EngineModule`, which declares the
//! Prüfidentifikatoren it routes and the workflows it owns; and
//! `makod::startup::production_modules()` pushes it into the engine. The impl
//! is what makes the module *possible*; the push is what makes it *run*.
//!
//! Every other module-level check in this suite enumerates
//! `production_modules()` — dispatch coverage, PID reference, published counts,
//! outbox types, meldepflicht coverage. All five therefore measure the list
//! against itself: a module that is implemented, tested by its own crate,
//! documented in the service pages and simply never pushed is invisible to
//! every one of them. Its PIDs route nowhere, its inbound messages dead-letter,
//! and each guard reports full coverage of the modules that *are* wired.
//!
//! So this one reads the crates instead. `impl … EngineModule for <Type>` in a
//! crate's `lib.rs` is the declaration; the `<Type>` must be named in
//! `services/makod/src/startup/mod.rs`.
//!
//! The `#[cfg(feature = "role-…")]` gate above each push is not read: a module
//! wired for one role is wired, and which builds select it is the roles'
//! question, not this one's.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("makod lives two levels below the workspace root")
        .to_path_buf()
}

/// The types `src` implements `EngineModule` for.
///
/// Split from the filesystem so the rule is testable against exact text.
/// Comments are skipped: `mako-engine`'s own documentation shows the impl.
fn engine_module_types(src: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in src.lines() {
        let line = line.trim_start();
        if line.starts_with("//") || !line.starts_with("impl") {
            continue;
        }
        let Some(rest) = line.split("EngineModule for ").nth(1) else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            out.insert(name);
        }
    }
    out
}

/// Every `crates/*/src/lib.rs`.
fn crate_lib_files(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root.join("crates")) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path().join("src/lib.rs"))
        .filter(|p| p.is_file())
        .collect();
    out.sort();
    out
}

/// The declared module types `startup` does not name.
fn unwired(declared: &BTreeSet<String>, startup: &str) -> Vec<String> {
    declared
        .iter()
        .filter(|name| !startup.contains(name.as_str()))
        .cloned()
        .collect()
}

#[test]
fn every_engine_module_is_pushed_by_production_modules() {
    let root = workspace_root();

    let mut declared = BTreeSet::new();
    for path in crate_lib_files(&root) {
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        declared.extend(engine_module_types(&src));
    }

    // A scanner that finds nothing passes vacuously, which is the failure mode
    // this whole file exists to prevent one level up.
    assert!(
        declared.len() >= 7,
        "found only {} `impl EngineModule` in crates/*/src/lib.rs — has the layout \
         changed? {declared:?}",
        declared.len()
    );

    let startup_path = root.join("services/makod/src/startup/mod.rs");
    let startup = std::fs::read_to_string(&startup_path)
        .unwrap_or_else(|e| panic!("{} is readable: {e}", startup_path.display()));
    assert!(
        startup.contains("pub fn production_modules()"),
        "{} no longer declares production_modules()",
        startup_path.display()
    );

    let missing = unwired(&declared, &startup);
    assert!(
        missing.is_empty(),
        "these crates implement EngineModule and production_modules() never pushes them, \
         so their Prüfidentifikatoren route nowhere and every inbound message for them \
         dead-letters — while the five guards that enumerate production_modules() report \
         full coverage:\n  {}",
        missing.join("\n  ")
    );
}

/// The mutation this guard exists for: one `m.push` deleted.
///
/// Nothing else notices. The crate still compiles, its own tests still pass,
/// the service page still documents it, and every guard that starts from
/// `production_modules()` simply stops counting it.
#[test]
fn a_module_left_out_of_the_push_list_is_caught() {
    let declared = engine_module_types(
        "impl mako_engine::builder::EngineModule for GpkeModule {\n\
         impl EngineModule for RedispatchModule {\n\
         //! impl EngineModule for DocumentedOnly {\n",
    );
    assert_eq!(
        declared.iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["GpkeModule", "RedispatchModule"],
        "a doc comment showing the impl is documentation"
    );

    let wired = "    m.push(Box::new(mako_gpke::GpkeModule));\n\
                 \x20   m.push(Box::new(mako_redispatch::RedispatchModule));\n";
    assert!(unwired(&declared, wired).is_empty());

    let one_removed = "    m.push(Box::new(mako_gpke::GpkeModule));\n";
    assert_eq!(
        unwired(&declared, one_removed),
        vec!["RedispatchModule".to_owned()]
    );
}

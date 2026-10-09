//! Every crate links its integration tests as one binary, `tests/it/main.rs`.
//!
//! Each `tests/*.rs` is its own test crate, and each links the crate's whole
//! dependency graph: a file per binary multiplies link time and `target/` size
//! by the file count. Two ways back to that layout are refused here:
//!
//! - a `tests/*.rs` beside a `tests/it/` binary — a new file, or one recreated
//!   at its pre-consolidation path;
//! - a `tests/it/*.rs` that `main.rs` does not declare — a module nothing
//!   compiles, whose tests therefore never run and never fail.
//!
//! A crate without `tests/it/` may keep one `tests/*.rs`: one file is one
//! binary already.

use std::path::Path;

fn rs_stems(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let p = e.ok()?.path();
            (p.is_file() && p.extension()? == "rs")
                .then(|| p.file_stem()?.to_str().map(str::to_owned))?
        })
        .collect();
    out.sort();
    out
}

fn declared(main_rs: &str) -> Vec<String> {
    main_rs
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("pub ").unwrap_or(l).strip_prefix("mod ")?;
            Some(rest.strip_suffix(';')?.trim().to_owned())
        })
        .collect()
}

fn problems(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for tree in ["crates", "services"] {
        let Ok(entries) = std::fs::read_dir(root.join(tree)) else {
            continue;
        };
        let mut crates: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        crates.sort();
        for krate in crates {
            let rel = format!(
                "{tree}/{}",
                krate.file_name().unwrap_or_default().to_string_lossy()
            );
            let tests = krate.join("tests");
            let loose = rs_stems(&tests);
            let it = tests.join("it");
            if it.join("main.rs").is_file() {
                for f in &loose {
                    out.push(format!("{rel}/tests/{f}.rs sits beside the tests/it binary — move it to tests/it/ and declare it in main.rs"));
                }
                let main = std::fs::read_to_string(it.join("main.rs")).unwrap_or_default();
                let mods = declared(&main);
                let mut files = rs_stems(&it);
                files.retain(|f| f != "main");
                for f in files {
                    if !mods.contains(&f) {
                        out.push(format!("{rel}/tests/it/{f}.rs is not declared in main.rs, so nothing compiles it"));
                    }
                }
            } else if loose.len() > 1 {
                out.push(format!(
                    "{rel} links {} test binaries — consolidate them into tests/it/main.rs",
                    loose.len()
                ));
            }
        }
    }
    out
}

#[test]
fn every_crate_links_one_integration_test_binary() {
    let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
    assert_eq!(problems(root), Vec::<String>::new());
}

#[test]
fn a_loose_file_and_an_undeclared_module_are_named() {
    let dir = std::env::temp_dir().join(format!("xtask-test-layout-{}", std::process::id()));
    let it = dir.join("crates/a/tests/it");
    std::fs::create_dir_all(&it).unwrap();
    std::fs::write(it.join("main.rs"), "mod declared;\n").unwrap();
    std::fs::write(it.join("declared.rs"), "").unwrap();
    std::fs::write(it.join("orphan.rs"), "").unwrap();
    std::fs::write(dir.join("crates/a/tests/recreated.rs"), "").unwrap();
    let b = dir.join("services/b/tests");
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(b.join("one.rs"), "").unwrap();
    std::fs::write(b.join("two.rs"), "").unwrap();
    let found = problems(&dir);
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(found[0].contains("recreated.rs"), "{found:?}");
    assert!(found[1].contains("orphan.rs"), "{found:?}");
    assert!(found[2].contains("services/b links 2"), "{found:?}");
}

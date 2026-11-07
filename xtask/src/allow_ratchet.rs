//! Every `#[allow]` is a named site.
//!
//! An `#[allow]` stays silent when the lint it suppresses stops firing, so a
//! stale one is invisible; `#[expect]` fails the build instead. Every
//! suppression carries a `reason` (`allow_attributes_without_reason`, in the
//! workspace lints), and an `#[allow]` is kept only where an `#[expect]` would
//! be unfulfilled in some build that compiles the code — a lint that fires in
//! some feature or target combinations and not in others, or a crate-level
//! opt-out from a lint group the crate turns on.
//!
//! [`SITES`] names each file that holds one, how many it holds and why an
//! `#[expect]` cannot stand there. The scan must match it exactly: a new
//! `#[allow(` in an unlisted file, or one more in a listed file, fails until
//! the list names it with its reason — so a raise names its site, and a review
//! reads the entry. A removed `#[allow(` fails until the entry shrinks or
//! goes, so the list never claims a suppression the tree no longer has. There
//! is no aggregate ceiling to raise.

use std::collections::BTreeMap;
use std::path::Path;

/// Why an `#[expect]` cannot replace the `#[allow]`s of a site.
#[derive(Clone, Copy, Debug)]
enum Why {
    /// The lint fires in some feature combination of the crate and not in
    /// another: `cargo hack check --each-feature` (`just check-features`)
    /// compiles the builds where it does not.
    Feature,
    /// The same source is compiled into two targets — a service's library and
    /// binary, or a test harness and the library — and the lint fires in only
    /// one of them.
    Target,
    /// A crate-level opt-out from a lint group the crate enables
    /// (`#![warn(clippy::pedantic)]`): it states which members of the group
    /// the crate does not adopt, whether or not one fires today.
    GroupPolicy,
}

/// One file holding `#[allow(` attributes.
struct Site {
    /// Path from the workspace root.
    path: &'static str,
    /// The `#[allow(` / `#![allow(` attribute lines in the file.
    count: usize,
    why: Why,
    /// What, in this file, makes the lint cfg- or target-dependent.
    reason: &'static str,
}

/// The files that hold an `#[allow(`, in path order.
const SITES: &[Site] = &[
    Site {
        path: "crates/edi-energy/src/any_message.rs",
        count: 3,
        why: Why::Feature,
        reason: "match arms and bindings over the message variants, each compiled by its own message-type feature",
    },
    Site {
        path: "crates/edi-energy/src/lib.rs",
        count: 1,
        why: Why::Feature,
        reason: "`return_self_not_must_use` fires on the builders, which only the message-type features compile",
    },
    Site {
        path: "crates/edi-energy/src/parse.rs",
        count: 1,
        why: Why::Feature,
        reason: "the parameters are read only by the message-type features' dispatch arms",
    },
    Site {
        path: "crates/edi-energy/src/report.rs",
        count: 3,
        why: Why::Feature,
        reason: "helpers called by the feature-gated message modules",
    },
    Site {
        path: "crates/edi-energy/tests/it/fuzz_compat.rs",
        count: 2,
        why: Why::Feature,
        reason: "exhaustiveness checks over the feature-gated message variants; which of the listed lints fires depends on how many variants are compiled",
    },
    Site {
        path: "crates/edi-energy/tests/it/parse.rs",
        count: 2,
        why: Why::Feature,
        reason: "an arm and a helper reached only from tests of some message-type features",
    },
    Site {
        path: "crates/edi-energy/tests/it/registry.rs",
        count: 1,
        why: Why::Feature,
        reason: "imports used only by the feature-gated test functions",
    },
    Site {
        path: "crates/edi-energy/tests/it/roundtrip.rs",
        count: 3,
        why: Why::Feature,
        reason: "imports and a helper used only by the feature-gated test modules",
    },
    Site {
        path: "crates/edi-energy/tests/it/semantic.rs",
        count: 3,
        why: Why::Feature,
        reason: "an import and helpers used only by the feature-gated test functions",
    },
    Site {
        path: "crates/mako-engine/src/builder.rs",
        count: 2,
        why: Why::Feature,
        reason: "the Noop stores are `#[deprecated]` only outside `any(test, feature = \"testing\")`",
    },
    Site {
        path: "crates/mako-engine/src/partner.rs",
        count: 1,
        why: Why::Feature,
        reason: "`NoopPartnerStore` is `#[deprecated]` only outside `any(test, feature = \"testing\")`",
    },
    Site {
        path: "crates/mako-engine/src/store_slatedb.rs",
        count: 1,
        why: Why::Feature,
        reason: "the test module's trait imports, which method syntax resolves without them in one feature combination and needs in another",
    },
    Site {
        path: "crates/mako-markt/src/lib.rs",
        count: 1,
        why: Why::GroupPolicy,
        reason: "`#![warn(clippy::pedantic)]` minus `missing_errors_doc`, which fires in some feature combinations of the crate and not in others",
    },
    Site {
        path: "crates/mako-service/src/lib.rs",
        count: 1,
        why: Why::GroupPolicy,
        reason: "`#![warn(clippy::pedantic)]` minus nine categories the crate does not adopt, not all of which fire today",
    },
    Site {
        path: "services/makod/src/orchestrator/ingest_dispatcher/mod.rs",
        count: 1,
        why: Why::Target,
        reason: "the outcome's fields are read in the library target and never in the binary target, which compiles the same module",
    },
    Site {
        path: "services/makod/src/startup/mod.rs",
        count: 1,
        why: Why::Target,
        reason: "a rustdoc lint: the private links resolve under `--document-private-items` and not under a plain `cargo doc`",
    },
    Site {
        path: "services/makod/src/transport/as4_clients.rs",
        count: 1,
        why: Why::Target,
        reason: "`build_with_http` is the integration tests' loopback seam: used through the library target, dead in the binary target",
    },
    Site {
        path: "services/makod/src/transport/as4_evidence.rs",
        count: 1,
        why: Why::Target,
        reason: "the evidence readers are used through the library target; the binary target only writes evidence",
    },
    Site {
        path: "services/processd/tests/it/role_separation.rs",
        count: 5,
        why: Why::Feature,
        reason: "each PID list is read only by the role features (`lf-only`, `nb-only`, `msb-only`, `integrated`) whose exclusions it tests",
    },
    Site {
        path: "xtask/src/check_frist_literals.rs",
        count: 1,
        why: Why::Target,
        reason: "an admission entry's `reason` is read by the `#[cfg(test)]` tests, not by the guard binary",
    },
    Site {
        path: "xtask/src/check_known_wrong.rs",
        count: 1,
        why: Why::Target,
        reason: "an admission entry's `reason` is read by the `#[cfg(test)]` tests, not by the guard binary",
    },
];

/// The trees scanned, relative to the workspace root.
const TREES: &[&str] = &["crates", "services", "xtask", "makotest/src"];

/// The `#[allow(` attributes in `src` — attribute lines, not prose that names
/// one.
fn count(src: &str) -> usize {
    src.lines()
        .map(str::trim_start)
        .filter(|l| l.starts_with("#[allow(") || l.starts_with("#![allow("))
        .count()
}

fn walk(root: &Path, dir: &Path, found: &mut BTreeMap<String, usize>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name()
                .is_some_and(|n| n == "target" || n == "clippy-fixtures")
            {
                continue;
            }
            walk(root, &p, found);
        } else if p.extension().is_some_and(|x| x == "rs") {
            let n = count(&std::fs::read_to_string(&p).unwrap_or_default());
            if n > 0 {
                let rel = p.strip_prefix(root).unwrap_or(&p);
                found.insert(rel.to_string_lossy().replace('\\', "/"), n);
            }
        }
    }
}

/// Per file, the `#[allow(` attributes under [`TREES`] of `root`.
fn scan(root: &Path) -> BTreeMap<String, usize> {
    let mut found = BTreeMap::new();
    for d in TREES {
        walk(root, &root.join(d), &mut found);
    }
    found
}

/// Every disagreement between the scan and `sites`, one line each.
fn findings(found: &BTreeMap<String, usize>, sites: &[Site]) -> Vec<String> {
    let mut out = Vec::new();
    let mut listed = BTreeMap::new();
    for s in sites {
        if s.reason.trim().is_empty() {
            out.push(format!("{}: the entry carries no reason", s.path));
        }
        if listed.insert(s.path, s).is_some() {
            out.push(format!("{}: listed twice", s.path));
        }
    }
    for (path, &n) in found {
        match listed.get(path.as_str()) {
            None => out.push(format!(
                "{path}: {n} `#[allow(` in a file `SITES` does not name — write \
                 `#[expect(…, reason = \"…\")]`, or add the site with why an expectation \
                 cannot stand there"
            )),
            Some(s) if s.count != n => out.push(format!(
                "{path}: {n} `#[allow(`, `SITES` names {} ({:?}: {})",
                s.count, s.why, s.reason
            )),
            Some(_) => {}
        }
    }
    for s in sites {
        if !found.contains_key(s.path) {
            out.push(format!(
                "{}: `SITES` names {} `#[allow(`, the file holds none — drop the entry",
                s.path, s.count
            ));
        }
    }
    out
}

#[test]
fn every_allow_is_a_named_site() {
    let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
    let f = findings(&scan(root), SITES);
    assert!(f.is_empty(), "{}", f.join("\n"));
}

#[test]
fn an_added_allow_is_counted_and_prose_is_not() {
    let src = "#[allow(dead_code, reason = \"x\")]\nfn f() {}\n// a #[allow(foo)] in a comment\n    #![allow(clippy::x, reason = \"y\")]\n";
    assert_eq!(count(src), 2);
}

/// A planted `#[allow(…, reason)]` is refused until its file's entry names it;
/// a lowered count and an entry without its file are refused too.
#[test]
fn a_planted_allow_is_refused_until_its_site_names_it() {
    let dir = std::env::temp_dir().join(format!("allow-ratchet-{}", std::process::id()));
    let src = dir.join("crates/demo/src");
    std::fs::create_dir_all(&src).unwrap();
    let file = src.join("lib.rs");
    let one = "#[allow(dead_code, reason = \"only in the `x` feature\")]\nfn f() {}\n";
    std::fs::write(&file, one).unwrap();
    let site = |count| Site {
        path: "crates/demo/src/lib.rs",
        count,
        why: Why::Feature,
        reason: "`f` is called only under the `x` feature",
    };
    assert!(findings(&scan(&dir), &[site(1)]).is_empty());

    // One more `#[allow]` in the same file: the entry still says one.
    std::fs::write(
        &file,
        format!("{one}#[allow(unused_imports, reason = \"y\")]\nuse std::fmt;\n"),
    )
    .unwrap();
    let f = findings(&scan(&dir), &[site(1)]);
    assert!(f.iter().any(|l| l.contains("2 `#[allow(`")), "{f:?}");
    assert!(findings(&scan(&dir), &[site(2)]).is_empty());

    // An unlisted file is refused, naming it.
    let other = src.join("other.rs");
    std::fs::write(&other, one).unwrap();
    let f = findings(&scan(&dir), &[site(2)]);
    assert!(
        f.iter().any(|l| l.starts_with("crates/demo/src/other.rs")),
        "{f:?}"
    );
    std::fs::remove_file(&other).unwrap();

    // The tree shrinks below the entry: the entry is stale.
    std::fs::write(&file, "fn f() {}\n").unwrap();
    let f = findings(&scan(&dir), &[site(1)]);
    assert!(f.iter().any(|l| l.contains("holds none")), "{f:?}");

    // An entry without a reason is refused.
    std::fs::write(&file, one).unwrap();
    let bare = Site {
        reason: " ",
        ..site(1)
    };
    assert!(!findings(&scan(&dir), &[bare]).is_empty());

    std::fs::remove_dir_all(&dir).unwrap();
}

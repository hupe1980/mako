//! Guard: every real-PostgreSQL test suite is `#[ignore]`d and run by a recipe.
//!
//! A suite that starts a `testcontainers` PostgreSQL has two ways to verify
//! nothing, and both report success:
//!
//! * **It is not `#[ignore]`d.** `just ci` then runs it, and on a machine with
//!   no Docker daemon the suite's own `Option`-returning pool helper hands back
//!   `None`, every test returns early, and the run prints `ok`. Nothing was
//!   exercised and nothing says so. `#[ignore]` makes the same situation print
//!   `N ignored`, which is the honest word for it — and keeps `just ci` free of
//!   a Docker dependency, which is the other half of why it matters.
//! * **No recipe names it.** `--include-ignored` is what runs an ignored test,
//!   and only a `test-*-db` recipe passes it. A suite outside every recipe is
//!   skipped by `just ci` and never reached by `just test-db`, so it is run by
//!   nothing at all.
//!
//! The two failures compose: a suite can be ignored *and* unnamed, at which
//! point it is dead code that reads as coverage.
//!
//! A test in such a file that needs no database is exempt and must not be
//! `#[ignore]`d — ignoring it removes real coverage from `just ci`. The scan
//! therefore keys on the pool helper each test actually calls, not on the file.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Crate directories holding test suites.
const TEST_TREES: &[&str] = &["crates", "services"];

/// How a suite reaches PostgreSQL. A file naming none of these is not a
/// database suite and is out of scope.
const CONTAINER_MARKERS: &[&str] = &["testcontainers"];

/// The helpers a database test calls to obtain a pool.
///
/// A test in a container suite that calls none of them needs no database — a
/// Cedar policy parse, a constant check — and is deliberately left runnable by
/// `just ci`.
const POOL_CALLS: &[&str] = &[
    "test_pool",
    "pg_pool",
    "pool_or_skip",
    "pg_container",
    "with_pool",
    "connect_lazy",
    "PgPool::connect",
];

/// One suite and what is wrong with it.
struct Finding {
    path: PathBuf,
    /// Tests that take a pool and are not `#[ignore]`d.
    unignored: Vec<String>,
    /// Whether any `test-*-db` recipe names the suite.
    unnamed: bool,
}

/// Scan the workspace.
///
/// Returns `true` when every container suite is ignored and named.
pub fn run(workspace_root: &Path) -> bool {
    let justfile = std::fs::read_to_string(workspace_root.join("justfile")).unwrap_or_default();
    if justfile.is_empty() {
        eprintln!("check-db-suites: the justfile is empty or unreadable — the scan cannot decide");
        return false;
    }

    let mut suites = Vec::new();
    for tree in TEST_TREES {
        collect(&workspace_root.join(tree), &mut suites);
    }
    suites.sort();

    // A guard that scanned nothing reports nothing. Refuse instead.
    if suites.is_empty() {
        eprintln!(
            "check-db-suites: the scan found no testcontainers suite under {} — \
             the layout has probably changed",
            TEST_TREES.join(", ")
        );
        return false;
    }

    let named = recipe_suites(&justfile);
    let mut findings = Vec::new();
    for path in &suites {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let finding = Finding {
            path: path.clone(),
            unignored: unignored_db_tests(&src),
            unnamed: !named.contains(stem.as_ref()),
        };
        if !finding.unignored.is_empty() || finding.unnamed {
            findings.push(finding);
        }
    }

    if findings.is_empty() {
        println!(
            "check-db-suites: all {} real-PostgreSQL suite(s) are `#[ignore]`d and named by a \
             `test-*-db` recipe",
            suites.len()
        );
        return true;
    }

    eprintln!(
        "check-db-suites: {} suite(s) would report success without verifying anything:",
        findings.len()
    );
    for f in &findings {
        let rel = f
            .path
            .strip_prefix(workspace_root)
            .unwrap_or(&f.path)
            .display();
        if f.unnamed {
            eprintln!(
                "  {rel}\n      run by no recipe — select it in its `test-*-db` recipe \
                 (`--test it -- {}::`, or `--test` for a crate without a `tests/it` binary)",
                f.path.file_stem().unwrap_or_default().to_string_lossy()
            );
        }
        for name in &f.unignored {
            eprintln!(
                "  {rel}\n      `{name}` takes a pool and is not `#[ignore]`d — without Docker it \
                 returns early and prints `ok`"
            );
        }
    }
    eprintln!(
        "\nMark a database test \
         `#[ignore = \"requires Docker (testcontainers PostgreSQL)\"]`, and name its suite in a \
         `test-*-db` recipe so `--include-ignored` reaches it. A test in the same file that \
         needs no database stays un-ignored on purpose."
    );
    false
}

/// Suite names the justfile's `cargo test` commands run.
///
/// A suite is named either as its own test target (`--test <name>`) or, in a
/// crate whose integration tests are one `tests/it/` binary, as a module filter
/// after `--` on a command selecting that binary (`--test it -- <name>::`).
fn recipe_suites(justfile: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    // Join `\`-continued lines so one command is one string.
    let joined = justfile.replace("\\\n", " ");
    let is_name = |n: &str| !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '_');
    for line in joined.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        // The justfile's private `suite <crate> <name>` recipe selects one suite
        // in either layout; position 0 is the recipe's own definition line.
        if let Some(at) = words
            .iter()
            .position(|w| *w == "suite")
            .filter(|at| *at > 0)
        {
            if let Some(name) = words.get(at + 2).filter(|n| is_name(n)) {
                out.insert((*name).to_owned());
            }
            continue;
        }
        if !line.contains("cargo test") {
            continue;
        }
        let mut selects_it = false;
        let mut after_separator = false;
        for pair in words.windows(2) {
            if pair[0] == "--test" {
                let name: String = pair[1]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if name == "it" {
                    selects_it = true;
                } else if is_name(&name) {
                    out.insert(name);
                }
            }
        }
        for word in &words {
            if *word == "--" {
                after_separator = true;
            } else if after_separator && selects_it {
                if let Some(module) = word.strip_suffix("::").filter(|m| is_name(m)) {
                    out.insert(module.to_owned());
                }
            }
        }
    }
    out
}

/// Tests in `src` that obtain a pool and carry no `#[ignore]`.
///
/// Split from the filesystem so the rule is testable against exact text.
#[must_use]
pub fn unignored_db_tests(src: &str) -> Vec<String> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t != "#[test]" && t != "#[tokio::test]" {
            continue;
        }
        // The attributes between `#[test]` and the signature.
        let mut j = i + 1;
        let mut ignored = false;
        while j < lines.len() && lines[j].trim().starts_with("#[") {
            if lines[j].trim().starts_with("#[ignore") {
                ignored = true;
            }
            j += 1;
        }
        if ignored || j >= lines.len() {
            continue;
        }
        let Some(name) = fn_name(lines[j]) else {
            continue;
        };
        // The body, to the next item at column zero.
        let mut body = String::new();
        for line in lines.iter().skip(j + 1) {
            let starts_item = line.starts_with("#[")
                || line.starts_with("fn ")
                || line.starts_with("async fn ")
                || line.starts_with("pub ");
            if starts_item && !body.is_empty() {
                break;
            }
            body.push_str(line);
            body.push('\n');
        }
        if POOL_CALLS.iter().any(|c| body.contains(c)) {
            out.push(name);
        }
    }
    out
}

/// The function name on a signature line, if it is one.
fn fn_name(line: &str) -> Option<String> {
    let t = line.trim();
    let after = t
        .strip_prefix("async fn ")
        .or_else(|| t.strip_prefix("fn "))?;
    let name: String = after
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Every `tests/*.rs` under `dir` that starts a container.
fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect(&path, out);
            continue;
        }
        // Only integration suites: a `#[cfg(test)]` module inside `src/` is a
        // unit test compiled with the crate, not something a recipe can select.
        // A suite is `tests/<name>.rs` (its own binary) or `tests/it/<name>.rs`
        // (a module of the crate's one test binary, whose `main.rs` is not a
        // suite).
        let named = |p: &Path, n: &str| p.file_name().is_some_and(|f| f == n);
        let is_integration = path.parent().is_some_and(|p| {
            named(p, "tests")
                || (named(p, "it")
                    && p.parent().is_some_and(|g| named(g, "tests"))
                    && !named(&path, "main.rs"))
        });
        if !is_integration || path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        if CONTAINER_MARKERS.iter().any(|m| src.contains(m)) {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IGN: &str = "#[ignore = \"requires Docker (testcontainers PostgreSQL)\"]";

    #[test]
    fn a_pool_taking_test_without_ignore_is_reported() {
        let src = format!(
            "#[tokio::test]\nasync fn needs_a_db() {{\n    let (pool, _g) = test_pool().await;\n}}\n\n\
             #[tokio::test]\n{IGN}\nasync fn already_marked() {{\n    let (pool, _g) = test_pool().await;\n}}\n"
        );
        assert_eq!(unignored_db_tests(&src), vec!["needs_a_db".to_owned()]);
    }

    /// A test in a container suite that needs no database stays runnable.
    #[test]
    fn a_test_that_takes_no_pool_is_left_alone() {
        let src = "#[test]\nfn parses_the_policy() {\n    let e = Enforcer::from_str(SRC);\n}\n";
        assert!(unignored_db_tests(src).is_empty());
    }

    #[test]
    fn recipe_suites_reads_every_test_flag() {
        let jf = "test-marktd-db:\n    cargo test -p marktd \\\n        --test a_one --test b_two \\\n        -- --include-ignored\n";
        let got = recipe_suites(jf);
        assert!(got.contains("a_one"), "{got:?}");
        assert!(got.contains("b_two"), "{got:?}");
    }

    /// In a crate with one `tests/it` binary, the module filter after `--` is
    /// what names the suite.
    #[test]
    fn recipe_suites_reads_module_filters_of_the_it_binary() {
        let jf = "test-edmd-db:\n    cargo test -p edmd --test it \\\n        -- meterstore_integration:: --include-ignored --test-threads=1\n";
        let got = recipe_suites(jf);
        assert!(got.contains("meterstore_integration"), "{got:?}");
        assert!(!got.contains("it"), "the binary is not a suite: {got:?}");
    }

    /// The `suite` helper recipe names the suite whichever layout its crate has.
    #[test]
    fn recipe_suites_reads_the_suite_helper() {
        let jf = "test-einsd-db:\n    {{ just_executable() }} suite einsd settlement_integration --include-ignored\n";
        assert!(recipe_suites(jf).contains("settlement_integration"));
    }

    /// A `::` word on a command that does not select the `it` binary names
    /// nothing — it is a test filter on some other target.
    #[test]
    fn a_module_filter_without_the_it_binary_names_nothing() {
        let got = recipe_suites("x:\n    cargo test -p a --lib -- some_mod:: --include-ignored\n");
        assert!(got.is_empty(), "{got:?}");
    }

    /// The scan is over a tree; an empty one certifies nothing.
    #[test]
    fn refuses_a_tree_it_found_nothing_in() {
        assert!(!run(Path::new("/nonexistent-mako-root")));
    }
}

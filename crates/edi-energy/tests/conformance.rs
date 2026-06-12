//! Conformance tests — drive the full validation pipeline against file-based
//! fixtures stored in `tests/fixtures/`.
//!
//! # Layout
//!
//! ```text
//! tests/fixtures/
//!   <message_type>/
//!     valid/
//!       <name>.edi           — must parse + validate without errors
//!     invalid/
//!       <name>.edi           — must parse; validation must produce errors
//!       <name>.expected.json — lists the rule-ID prefixes that must fire
//! ```
//!
//! # Expected JSON schema
//!
//! ```json
//! { "expected_rule_prefixes": ["SEM-UTILMD-MALO-FORMAT"] }
#![allow(dead_code)]
use std::path::{Path, PathBuf};

#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "contrl",
    feature = "invoic",
    feature = "remadv",
    feature = "orders",
    feature = "iftsta",
    feature = "insrpt",
    feature = "reqote",
    feature = "partin",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "pricat",
    feature = "utilts",
))]
use edi_energy::{EdiEnergyMessage, parse};

/// Deterministic reference date used by all conformance tests.
///
/// Derived dynamically from the latest `valid_from` across all registered
/// profiles, plus a 365-day margin.  This means the date automatically
/// advances when new profiles (with later `valid_from` dates) are added via
/// `cargo xtask codegen`, without any manual constant update (F-016 fix).
///
/// If no profile has a `valid_from` date, falls back to 2027-01-01.
#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "contrl",
    feature = "invoic",
    feature = "remadv",
    feature = "orders",
    feature = "iftsta",
    feature = "insrpt",
    feature = "reqote",
    feature = "partin",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "pricat",
    feature = "utilts",
))]
fn conformance_reference_date() -> time::Date {
    use edi_energy::registry::ReleaseRegistry;
    let registry = ReleaseRegistry::global();
    let latest = registry
        .all_profiles()
        .iter()
        .filter_map(|p| p.valid_from())
        .max();
    match latest {
        Some(d) => d.saturating_add(time::Duration::days(365)),
        None => time::Date::from_calendar_date(2027, time::Month::January, 1)
            .expect("hard-coded fallback date is valid"),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Load every `*.edi` file directly under `dir`, returning `(name, bytes)`.
fn load_edi_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    if !dir.exists() {
        return Vec::new();
    }
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("cannot read fixture directory")
        .filter_map(std::result::Result::ok)
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("edi"))
        .map(|e| {
            let path = e.path();
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("?")
                .to_owned();
            let bytes = std::fs::read(&path)
                .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
            (name, bytes)
        })
        .collect();
    files.sort_by(|(a, _), (b, _)| a.cmp(b));
    files
}

/// Parse `<name>.expected.json` next to an invalid `.edi` file.
fn load_expected(dir: &Path, name: &str) -> Vec<String> {
    let json_path = dir.join(format!("{name}.expected.json"));
    let raw = std::fs::read_to_string(&json_path)
        .unwrap_or_else(|_| panic!("missing {}", json_path.display()));
    let value: serde_json::Value = serde_json::from_str(&raw)
        .unwrap_or_else(|_| panic!("invalid JSON in {}", json_path.display()));
    value["expected_rule_prefixes"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "expected_rule_prefixes must be an array in {}",
                json_path.display()
            )
        })
        .iter()
        .map(|v| {
            v.as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "expected_rule_prefixes must be strings in {}",
                        json_path.display()
                    )
                })
                .to_owned()
        })
        .collect()
}

// ── Valid fixture runner ──────────────────────────────────────────────────────
#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "contrl",
    feature = "invoic",
    feature = "remadv",
    feature = "orders",
    feature = "iftsta",
    feature = "insrpt",
    feature = "reqote",
    feature = "partin",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "pricat",
    feature = "utilts",
))]
fn run_valid_fixtures(message_type: &str) {
    let dir = fixtures_dir().join(message_type).join("valid");
    let files = load_edi_files(&dir);
    assert!(
        !files.is_empty(),
        "no valid fixtures found under {}",
        dir.display()
    );
    for (name, bytes) in files {
        let msg = parse(&bytes).unwrap_or_else(|e| {
            panic!("[{message_type}/valid/{name}] parse error: {e}");
        });
        let report = msg
            .validate_on_date(conformance_reference_date())
            .unwrap_or_else(|e| {
                panic!("[{message_type}/valid/{name}] validate() error: {e}");
            });
        assert!(
            report.is_valid(),
            "[{message_type}/valid/{name}] expected valid report, but got errors: {:#?}",
            report.errors()
        );
    }
}

// ── Invalid fixture runner ────────────────────────────────────────────────────
#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "contrl",
    feature = "invoic",
    feature = "remadv",
    feature = "orders",
    feature = "iftsta",
    feature = "insrpt",
    feature = "reqote",
    feature = "partin",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "pricat",
    feature = "utilts",
))]
fn run_invalid_fixtures(message_type: &str) {
    let dir = fixtures_dir().join(message_type).join("invalid");
    let files = load_edi_files(&dir);
    assert!(
        !files.is_empty(),
        "no invalid fixtures found under {}",
        dir.display()
    );
    for (name, bytes) in files {
        let expected_prefixes = load_expected(&dir, &name);
        let msg = parse(&bytes).unwrap_or_else(|e| {
            panic!("[{message_type}/invalid/{name}] parse error: {e}");
        });
        let report = msg
            .validate_on_date(conformance_reference_date())
            .unwrap_or_else(|e| {
                panic!("[{message_type}/invalid/{name}] validate() error: {e}");
            });
        assert!(
            report.has_errors(),
            "[{message_type}/invalid/{name}] expected errors in report, but report is valid"
        );
        for prefix in &expected_prefixes {
            let filtered = report.filter_by_rule_prefix(prefix);
            assert!(
                filtered.has_errors(),
                "[{message_type}/invalid/{name}] expected rule '{prefix}' to fire, \
                 but it was not found in errors: {:#?}",
                report.errors()
            );
        }
    }
}

// ── Test cases ────────────────────────────────────────────────────────────────

#[cfg(feature = "utilmd")]
#[test]
fn conformance_utilmd_valid() {
    run_valid_fixtures("utilmd");
}

#[cfg(feature = "utilmd")]
#[test]
fn conformance_utilmd_invalid() {
    run_invalid_fixtures("utilmd");
}

#[cfg(feature = "mscons")]
#[test]
fn conformance_mscons_valid() {
    run_valid_fixtures("mscons");
}

#[cfg(feature = "mscons")]
#[test]
fn conformance_mscons_invalid() {
    run_invalid_fixtures("mscons");
}

#[cfg(feature = "aperak")]
#[test]
fn conformance_aperak_valid() {
    run_valid_fixtures("aperak");
}

#[cfg(feature = "aperak")]
#[test]
fn conformance_aperak_invalid() {
    run_invalid_fixtures("aperak");
}

#[cfg(feature = "contrl")]
#[test]
fn conformance_contrl_valid() {
    run_valid_fixtures("contrl");
}

#[cfg(feature = "contrl")]
#[test]
fn conformance_contrl_invalid() {
    run_invalid_fixtures("contrl");
}

#[cfg(feature = "iftsta")]
#[test]
fn conformance_iftsta_valid() {
    run_valid_fixtures("iftsta");
}

#[cfg(feature = "insrpt")]
#[test]
fn conformance_insrpt_valid() {
    run_valid_fixtures("insrpt");
}

#[cfg(feature = "invoic")]
#[test]
fn conformance_invoic_valid() {
    run_valid_fixtures("invoic");
}

#[cfg(feature = "remadv")]
#[test]
fn conformance_remadv_valid() {
    run_valid_fixtures("remadv");
}

#[cfg(feature = "reqote")]
#[test]
fn conformance_reqote_valid() {
    run_valid_fixtures("reqote");
}

#[cfg(feature = "orders")]
#[test]
fn conformance_orders_valid() {
    run_valid_fixtures("orders");
}

#[cfg(feature = "ordchg")]
#[test]
fn conformance_ordchg_valid() {
    run_valid_fixtures("ordchg");
}

#[cfg(feature = "ordrsp")]
#[test]
fn conformance_ordrsp_valid() {
    run_valid_fixtures("ordrsp");
}

#[cfg(feature = "partin")]
#[test]
fn conformance_partin_valid() {
    run_valid_fixtures("partin");
}

#[cfg(feature = "pricat")]
#[test]
fn conformance_pricat_valid() {
    run_valid_fixtures("pricat");
}

#[cfg(feature = "quotes")]
#[test]
fn conformance_quotes_valid() {
    run_valid_fixtures("quotes");
}

#[cfg(feature = "comdis")]
#[test]
fn conformance_comdis_valid() {
    run_valid_fixtures("comdis");
}

#[cfg(feature = "utilts")]
#[test]
fn conformance_utilts_valid() {
    run_valid_fixtures("utilts");
}

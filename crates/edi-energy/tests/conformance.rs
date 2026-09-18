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
//! { "expected_rule_prefixes": ["SEM-UTILMD-LOKATIONS-ID"] }
// Helper functions and imports are gated by the same #[cfg(any(feature = …))]
// blocks as the test fns that call them — no blanket suppression needed.
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
use edi_energy::{EdiEnergyMessage, Platform};

/// Deterministic reference date used by all conformance tests.
///
/// Derived dynamically from the latest `valid_from` across all registered
/// profiles, plus a 365-day margin.  This means the date automatically
/// advances when new profiles (with later `valid_from` dates) are imported,
/// without any manual constant update.
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
fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Load every `*.edi` file directly under `dir`, returning `(name, bytes)`.
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
        let msg = Platform::with_all_profiles()
            .parse(&bytes)
            .unwrap_or_else(|e| {
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
        let msg = Platform::with_all_profiles()
            .parse(&bytes)
            .unwrap_or_else(|e| {
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

/// The Meldepunkt's own ID decides which Prüfschablone branch applies, and a
/// rule that says so has to change a verdict.
///
/// QUOTES 15005 lists three `SG27` „Erforderliches Produkt" branches —
/// `[56]` Messlokation, `[57]` Netzlokation, `[58]` Steuerbare Ressource — and
/// names no branch for a Marktlokation. Every one of those Bedingungen is a
/// statement about the *value* in `LOC+172` DE 3225, which is what
/// [`Voraussetzung::ElementShape`] reads. Until it did, all three evaluated to
/// `Unknown`, the `SG27` was never demanded, and a 15005 answering about a
/// Messlokation without its Angebotsposition validated clean.
///
/// So this drives the same message twice and changes one value. The floor in
/// `extraction_fidelity` proves the rule still *parses*; this proves it still
/// *decides*.
#[cfg(feature = "quotes")]
#[test]
fn the_meldepunkt_id_selects_the_sg27_branch() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/quotes/valid/pid_15005_1_3b.edi");
    let melo = std::fs::read_to_string(&path).expect("the 15005 fixture");
    assert!(
        melo.contains("LOC+172+DE00056266802AO6G56M11SN51G21M24S'"),
        "the fixture names a Messlokation by its Zählpunktbezeichnung"
    );
    // The same message about a Marktlokation: `41373559241` is the worked
    // example in the BDEW Anwendungshilfe „Identifikatoren".
    let malo = melo.replace("DE00056266802AO6G56M11SN51G21M24S", "41373559241");

    let profile = edi_energy::ReleaseRegistry::global()
        .profiles_for(edi_energy::MessageType::Quotes)
        .find(|p| p.release().as_str() == "1.3b")
        .expect("QUOTES 1.3b is shipped");
    let pid = edi_energy::Pruefidentifikator::new(15005).ok();

    let judge = |wire: &str| -> Vec<String> {
        // The Prüfschablone is about `UNH…UNT`; the interchange envelope is
        // the transport's.
        let segs: Vec<edifact_rs::OwnedSegment> = edifact_rs::from_bytes(wire.as_bytes())
            .map(|s| s.map(edifact_rs::Segment::into_owned))
            .collect::<Result<Vec<_>, _>>()
            .expect("the fixture parses")
            .into_iter()
            .filter(|s| !matches!(s.tag.as_ref(), "UNB" | "UNZ"))
            .collect();
        profile
            .validate(&segs, pid)
            .iter()
            .filter_map(|i| i.rule_id().map(str::to_owned))
            .collect()
    };

    // With the Messlokation named, `[56]` holds and its SG27 is Muss — the
    // fixture carries it, so nothing is reported.
    assert!(
        judge(&melo).is_empty(),
        "the Messlokation branch is satisfied: {:?}",
        judge(&melo)
    );

    // With a Marktlokations-ID, none of the three Bedingungen holds, so the
    // SG27 the fixture still carries is not part of this column.
    let with_malo = judge(&malo);
    assert!(
        with_malo
            .iter()
            .any(|r| r.starts_with("AHB-15005-SG27-") && r.ends_with("-NOT-PERMITTED")),
        "a Marktlokations-ID selects no SG27 branch, so the group is not \
         permitted — got {with_malo:?}"
    );

    // Deleting the SG27 with the Messlokation named brings the demand back.
    let without = melo
        .lines()
        .filter(|l| {
            !l.starts_with("LIN+")
                && !l.starts_with("PIA+")
                && !l.starts_with("PRI+")
                && !l.starts_with("RNG+")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let missing = judge(&without);
    assert!(
        missing.iter().any(|r| r == "AHB-15005-SG27-00075-MISSING"),
        "[56] demands the Messlokation SG27 — got {missing:?}"
    );
}

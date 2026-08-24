//! Guard: the BO4E schema release mako advertises must be the one it generates.
//!
//! Four places name a BO4E schema release, and they are not interchangeable:
//!
//! | Place | Spelling | Example |
//! |---|---|---|
//! | a payload's `_version` field | **no `v`** | `202607.1.0` |
//! | the SQL `bo4e_version` column `DEFAULT` | same as the payload | `202607.1.0` |
//! | a `raw.githubusercontent.com` schema URL | the git **tag**, with `v` | `v202607.1.0` |
//! | version dispatch | the **series** only | `202607` |
//!
//! rubo4e wrote the tag spelling into `_version` through 0.9, so every BO and
//! COM mako produced carried a version string that no BO4E schema accepts and
//! no other implementation writes — and the SQL `DEFAULT`s, copied from it, said
//! the same thing. The mistake was invisible because nothing compared the four.
//!
//! This test compares them. It reads the SQL migrations and the compiled
//! constants and checks each against `rubo4e`'s own `schema_version()`, so a
//! rubo4e upgrade that advances the schema fails here rather than silently
//! leaving mako stamping a release it no longer generates.

use std::path::{Path, PathBuf};

use mako_markt::bo4e::{SCHEMA_SERIES, SCHEMA_VERSION, version_is_readable};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("makod lives two levels below the workspace root")
        .to_path_buf()
}

/// The payload spelling carries no `v`, and the series is its `YYYYMM` prefix.
#[test]
fn the_wire_spelling_has_no_v_prefix() {
    let v = *SCHEMA_VERSION;
    assert!(
        !v.starts_with('v'),
        "`_version` carries the payload spelling, not the git tag: {v:?}. \
         BO4E prefixes only its tags with a `v`; no schema accepts one in the field."
    );
    assert_eq!(
        v.split('.').next(),
        Some(*SCHEMA_SERIES),
        "the series must be the release's own YYYYMM prefix"
    );
    assert_eq!(
        SCHEMA_SERIES.len(),
        6,
        "a series is YYYYMM: {:?}",
        *SCHEMA_SERIES
    );
}

/// Every `bo4e_version` column defaults to exactly what rubo4e stamps.
///
/// A `DEFAULT` that drifts from the generated types is worse than no default:
/// rows written without an explicit version claim a release the payload beside
/// them does not carry, and a later reader dispatching on the column reaches for
/// the wrong types.
#[test]
fn every_sql_default_matches_the_generated_schema_version() {
    let root = workspace_root();
    let expected = format!("DEFAULT '{}'", *SCHEMA_VERSION);

    let mut offenders = Vec::new();
    let mut checked = 0usize;
    for entry in glob_migrations(&root) {
        let Ok(sql) = std::fs::read_to_string(&entry) else {
            continue;
        };
        for (n, line) in sql.lines().enumerate() {
            if !line.contains("bo4e_version") || !line.contains("DEFAULT") {
                continue;
            }
            checked += 1;
            if !line.contains(&expected) {
                offenders.push(format!(
                    "{}:{}  {}",
                    entry.strip_prefix(&root).unwrap_or(&entry).display(),
                    n + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        checked > 10,
        "the scanner found only {checked} bo4e_version defaults — it stopped matching the migrations"
    );
    assert!(
        offenders.is_empty(),
        "these `bo4e_version` defaults do not match what rubo4e stamps ({expected:?}):\n  {}",
        offenders.join("\n  ")
    );
}

/// The schema **URL** tag is the payload version with a `v` in front.
///
/// Both spellings are correct in their own place; what must not happen is one
/// of them advancing without the other, which leaves mako publishing a
/// CloudEvent that points at a schema release its payloads do not match.
#[test]
fn the_schema_url_tag_is_the_payload_version_with_a_v() {
    let expected = format!("BO4E-Schemas/v{}/", *SCHEMA_VERSION);
    assert!(
        mako_engine::erp::BO4E_V202607_BASE.contains(&expected),
        "the schema URL base points at a different release than rubo4e generates.\n  \
         base:     {}\n  expected: …{expected}…",
        mako_engine::erp::BO4E_V202607_BASE
    );
}

/// A payload from anywhere in the series is readable; one from another is not.
///
/// BO4E ships patch releases *inside* a series and every one of them
/// deserializes into the same Rust types, so dispatching on the full triple
/// rejects a producer one patch ahead that mako reads perfectly. A stored value
/// carrying the git tag's `v` is read rather than refused.
#[test]
fn readability_is_decided_by_the_series() {
    let series = *SCHEMA_SERIES;

    assert!(version_is_readable(*SCHEMA_VERSION));
    assert!(
        version_is_readable(&format!("{series}.9.9")),
        "a later patch inside the series is readable"
    );
    assert!(
        version_is_readable(&format!("v{series}.0.0")),
        "a stored value carrying the git tag's `v` is still readable"
    );
    assert!(
        !version_is_readable("202501.0.0"),
        "another series is not readable"
    );
    assert!(!version_is_readable(""));
}

fn glob_migrations(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let services = root.join("services");
    let Ok(entries) = std::fs::read_dir(&services) else {
        return out;
    };
    for svc in entries.flatten() {
        let dir = svc.path().join("migrations");
        let Ok(files) = std::fs::read_dir(&dir) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().is_some_and(|e| e == "sql") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

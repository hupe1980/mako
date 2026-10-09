//! Every INVOIC `DTM` qualifier this service names is one the INVOIC MIG
//! publishes.
//!
//! # Why this is a test
//!
//! `invoicd` never parses EDIFACT — it reads the BO4E `Rechnung` that `makod`
//! translated — so a wrong qualifier in a comment breaks no code and no
//! fixture. It misleads the next reader instead, and the reader it misleads is
//! the one changing how `pay_by` is filled: the schema comment, the row
//! documentation and the handler comment were the three places that said where
//! the Zahlungsziel comes from, and two of them named `DTM+92`.
//!
//! **`DTM+92` is not an INVOIC qualifier at all.** The INVOIC MIG publishes
//! 15 `DTM` segments and the Zahlungsziel is `SG8 DTM+265` (Nr. 00033,
//! „Fälligkeitsdatum"), whose DE 2379 admits only `303`; `92` names the
//! Vertragsbeginn in UTILMD, a different message. The only INVOIC `DTM` that
//! admits `102` is `DTM+203`.
//!
//! The check is a literal string scan over this crate's own sources, which is
//! all it needs to be: the claim is prose, so prose is what goes wrong.

use std::path::{Path, PathBuf};

/// Qualifiers that must never appear in this crate, with the reason.
const FORBIDDEN: &[(&str, &str)] = &[(
    "DTM+92",
    "not an INVOIC qualifier — the Zahlungsziel is SG8 DTM+265 (MIG Nr. 00033)",
)];

/// Files worth scanning: sources, migrations and the service README.
fn scanned_files() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {dir:?}: {e}"));
        for entry in entries {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                // `target/` is build output, not a claim anyone reads.
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                stack.push(path);
            } else if path
                .extension()
                .is_some_and(|e| e == "rs" || e == "sql" || e == "md" || e == "toml")
            {
                out.push(path);
            }
        }
    }
    assert!(
        out.len() > 5,
        "the scan found {} files — it is not reading the crate",
        out.len()
    );
    out
}

#[test]
fn no_source_names_a_dtm_qualifier_invoic_does_not_have() {
    let mut offenders = Vec::new();
    for path in scanned_files() {
        // This file names the forbidden qualifiers to forbid them.
        if path.file_name().is_some_and(|n| n == "dtm_qualifiers.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
        for (line_no, line) in text.lines().enumerate() {
            for (qualifier, why) in FORBIDDEN {
                if line.contains(qualifier) {
                    offenders.push(format!(
                        "{}:{}: {qualifier} — {why}\n    {}",
                        path.display(),
                        line_no + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "INVOIC DTM qualifiers that do not exist:\n{}",
        offenders.join("\n")
    );
}

/// The Zahlungsziel is named somewhere, and where it is named it is `DTM+265`.
///
/// Without this half, deleting every mention would pass the scan above.
#[test]
fn the_zahlungsziel_qualifier_is_stated_and_is_265() {
    let named: Vec<PathBuf> = scanned_files()
        .into_iter()
        .filter(|p| p.file_name().is_some_and(|n| n != "dtm_qualifiers.rs"))
        .filter(|p| {
            std::fs::read_to_string(p).is_ok_and(|t| {
                t.contains("DTM+265") && t.contains("pay_by")
                    || t.contains("DTM+265") && t.contains("faelligkeitsdatum")
            })
        })
        .collect();
    assert!(
        !named.is_empty(),
        "no file states where the Zahlungsziel comes from — it is SG8 DTM+265"
    );
}

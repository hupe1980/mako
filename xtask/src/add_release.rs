//! `cargo xtask add-release` — scaffold a new BDEW format-version profile.
//!
//! Creates the profile directory skeleton for a new BDEW format version,
//! copying the base structure from the most-recently-published profile for
//! each message type.  The generated files are drafts that should be
//! populated by `cargo xtask extract-pdf` and then reviewed before running
//! `cargo xtask codegen`.
//!
//! ## Usage
//!
//! ```text
//! cargo xtask add-release \
//!   --fv          FV2027-10-01    # BDEW format-version string
//!   --date        2027-10-01      # valid_from date (ISO 8601)
//!   --message-type UTILMD         # optional: only scaffold one message type
//!   --dry-run                     # print what would be created without touching the FS
//! ```
//!
//! ## What it creates
//!
//! For each message type under `crates/edi-energy/profiles/`:
//!
//! ```text
//! crates/edi-energy/profiles/<type>/fv<YYYYMMDD>/
//!   mig.json        — profile skeleton (segments: [], segment_groups: [])
//!   ahb.json        — AHB skeleton (pruefidentifikatoren: {})
//!   codelists.json  — codelist skeleton (lists: {})
//! ```
//!
//! Each skeleton carries the correct `schema_version`, `message_type`,
//! `release` (copied from the predecessor's `mig.json`), and `valid_from` /
//! `valid_until` (predecessor's `valid_from` moved to `valid_until`).
//!
//! ## Manual steps after scaffolding
//!
//! 1. Place the new BDEW AHB / MIG PDFs in `docs/pdfs/`.
//! 2. `cargo xtask extract-pdf --message-type <TYPE>` for each updated type.
//! 3. Review the extracted draft files for completeness.
//! 4. `cargo xtask codegen --message-type <TYPE>` to regenerate Rust profiles.
//! 5. `cargo xtask validate-profiles` to catch structural errors.
//! 6. `cargo xtask validate-pruefids` to verify PID coverage.
//! 7. `cargo test --all-features` to confirm CI gate passes.

use std::path::{Path, PathBuf};

use serde_json::Value;

// ── Public entry point ────────────────────────────────────────────────────────

/// Run `cargo xtask add-release` with the supplied CLI arguments.
///
/// Returns `true` on success, `false` on any error (prints diagnostics).
pub fn run(workspace_root: &str, args: &[String]) -> bool {
    let fv_str = match parse_named_arg(args, "--fv") {
        Some(v) => v,
        None => {
            eprintln!("error: --fv <FORMAT_VERSION> is required (e.g. FV2027-10-01)");
            return false;
        }
    };

    let date_str = match parse_named_arg(args, "--date") {
        Some(v) => v,
        None => {
            // Infer date from FV string: FV2027-10-01 → "2027-10-01"
            if let Some(suffix) = fv_str.strip_prefix("FV") {
                suffix.to_owned()
            } else {
                eprintln!(
                    "error: --date <YYYY-MM-DD> is required when --fv does not start with 'FV'"
                );
                return false;
            }
        }
    };

    // Validate date format (YYYY-MM-DD).
    if !is_valid_date(&date_str) {
        eprintln!("error: --date '{date_str}' is not a valid ISO 8601 date (YYYY-MM-DD)");
        return false;
    }

    let type_filter = parse_named_arg(args, "--message-type").map(|t| t.to_lowercase());
    let dry_run = args.iter().any(|a| a == "--dry-run");

    // Compute the directory name: FV2027-10-01 → fv20271001
    let dir_name = fv_to_dir_name(&fv_str);

    let profiles_dir = format!("{workspace_root}/crates/edi-energy/profiles");

    let message_types = collect_message_types(&profiles_dir, type_filter.as_deref());
    if message_types.is_empty() {
        if let Some(ref t) = type_filter {
            eprintln!("error: no profile directory found for message type '{t}'");
        } else {
            eprintln!("error: no profile directories found under {profiles_dir}");
        }
        return false;
    }

    let mut ok = true;
    let mut created_count = 0usize;
    let mut skipped_count = 0usize;

    for msg_type in &message_types {
        let type_dir = format!("{profiles_dir}/{msg_type}");
        let target_dir = PathBuf::from(format!("{type_dir}/{dir_name}"));

        if target_dir.exists() {
            println!("skip  {msg_type}/{dir_name}  (directory already exists)");
            skipped_count += 1;
            continue;
        }

        // Find the most-recently-published profile for this message type
        // to use as the predecessor template.
        let predecessor = match find_latest_predecessor(&type_dir, &dir_name) {
            Some(p) => p,
            None => {
                eprintln!(
                    "warn  {msg_type}: no existing profile to use as predecessor template; skipping"
                );
                skipped_count += 1;
                continue;
            }
        };

        match scaffold_profile_dir(
            &type_dir,
            msg_type,
            &dir_name,
            &date_str,
            &predecessor,
            dry_run,
        ) {
            Ok(()) => {
                let prefix = if dry_run { "dry   " } else { "create" };
                println!("{prefix} {msg_type}/{dir_name}  (from {predecessor})");
                created_count += 1;
            }
            Err(e) => {
                eprintln!("error {msg_type}/{dir_name}: {e}");
                ok = false;
            }
        }
    }

    println!();
    if dry_run {
        println!("dry-run: {created_count} directories would be created, {skipped_count} skipped");
        println!();
        println!("next steps:");
        println!("  1. cargo xtask add-release --fv {fv_str}   (without --dry-run)");
        println!("  2. place new BDEW PDFs in docs/pdfs/");
        println!("  3. cargo xtask extract-pdf --message-type <TYPE>  for each updated type");
        println!("  4. cargo xtask codegen");
        println!("  5. cargo xtask validate-profiles");
    } else {
        println!("created {created_count} skeleton directories, {skipped_count} skipped");
        if created_count > 0 {
            println!();
            println!("next steps:");
            println!("  1. place new BDEW PDFs in docs/pdfs/");
            println!("  2. cargo xtask extract-pdf --message-type <TYPE>  for each updated type");
            println!("  3. cargo xtask codegen");
            println!("  4. cargo xtask validate-profiles");
            println!("  5. cargo xtask validate-pruefids");
            println!("  6. cargo test --all-features");
        }
    }

    ok
}

// ── Scaffold ──────────────────────────────────────────────────────────────────

/// Create `mig.json`, `ahb.json`, and `codelists.json` in `target_dir`.
fn scaffold_profile_dir(
    type_dir: &str,
    msg_type: &str,
    dir_name: &str,
    valid_from: &str,
    predecessor: &str,
    dry_run: bool,
) -> Result<(), String> {
    let pred_dir = format!("{type_dir}/{predecessor}");

    let mig_json = scaffold_mig(msg_type, valid_from, predecessor, &pred_dir)?;
    let ahb_json = scaffold_ahb(msg_type, predecessor, &pred_dir)?;
    let codelists = scaffold_codelists(predecessor, &pred_dir)?;

    if dry_run {
        return Ok(());
    }

    let target = PathBuf::from(format!("{type_dir}/{dir_name}"));
    std::fs::create_dir_all(&target).map_err(|e| format!("create_dir_all({target:?}): {e}"))?;

    write_json(&target.join("mig.json"), &mig_json)?;
    write_json(&target.join("ahb.json"), &ahb_json)?;
    write_json(&target.join("codelists.json"), &codelists)?;

    Ok(())
}

/// Build a `mig.json` skeleton for the new release.
///
/// - Copies `schema_version` from the predecessor.
/// - Copies `release` from the predecessor (annotated TODO for the operator).
/// - Sets `valid_from` to the new date.
/// - Sets `valid_until` to the predecessor's `valid_from` minus one day
///   (or omits it when the predecessor has no `valid_from`).
/// - Clears `segments` and `segment_groups` — to be populated by `extract-pdf`.
fn scaffold_mig(
    msg_type: &str,
    valid_from: &str,
    predecessor_name: &str,
    pred_dir: &str,
) -> Result<Value, String> {
    let pred_mig = load_json(&format!("{pred_dir}/mig.json"))?;

    let schema_version = pred_mig
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(1);

    // Predecessor release becomes a hint; the operator updates it after PDF extraction.
    let release_hint = pred_mig
        .get("release")
        .and_then(|v| v.as_str())
        .unwrap_or("TODO")
        .to_owned();

    // Predecessor `valid_from` becomes the new `valid_until` for the predecessor
    // (not written here, but used for the new profile's predecessor_valid_until).
    // Set `valid_until` for the predecessor by convention: new profile's `valid_from`
    // minus one day. We encode this as a JSON string for the operator to verify.
    let pred_valid_until = pred_mig
        .get("valid_from")
        .and_then(|v| v.as_str())
        .and_then(date_minus_one_day)
        .unwrap_or_else(|| format!("{predecessor_name}_valid_until"));

    Ok(serde_json::json!({
        "schema_version": schema_version,
        "message_type":   msg_type.to_uppercase(),
        "release":        release_hint,
        "valid_from":     valid_from,
        "valid_until":    Value::Null,
        "source_document": "TODO: BDEW AHB/MIG document title including revision date",
        "segments":       [],
        "segment_groups": [],
        // Informational: update the predecessor's mig.json with this value.
        "_predecessor_valid_until": pred_valid_until,
    }))
}

/// Build an `ahb.json` skeleton for the new release.
fn scaffold_ahb(msg_type: &str, _predecessor: &str, pred_dir: &str) -> Result<Value, String> {
    let pred_ahb = load_json(&format!("{pred_dir}/ahb.json"))?;

    let schema_version = pred_ahb
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(1);

    let release_hint = pred_ahb
        .get("release")
        .and_then(|v| v.as_str())
        .unwrap_or("TODO")
        .to_owned();

    Ok(serde_json::json!({
        "schema_version": schema_version,
        "message_type":   msg_type.to_uppercase(),
        "release":        release_hint,
        "source_document": "TODO: BDEW AHB document title including revision date",
        "pruefidentifikatoren": {},
    }))
}

/// Build a `codelists.json` skeleton for the new release.
fn scaffold_codelists(_predecessor: &str, pred_dir: &str) -> Result<Value, String> {
    let pred_cl = load_json(&format!("{pred_dir}/codelists.json"))?;

    let schema_version = pred_cl
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(1);

    let release_hint = pred_cl
        .get("release")
        .and_then(|v| v.as_str())
        .unwrap_or("TODO")
        .to_owned();

    // Copy the predecessor's code lists as the starting point.
    // The operator removes lists that changed and runs `import-codelists` for
    // any updated BDEW codelists.
    let lists = pred_cl
        .get("lists")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));

    Ok(serde_json::json!({
        "schema_version": schema_version,
        "release":        release_hint,
        "lists":          lists,
    }))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Return all message-type subdirectories of `profiles_dir`, optionally
/// filtered by `type_filter` (case-insensitive prefix match).
fn collect_message_types(profiles_dir: &str, type_filter: Option<&str>) -> Vec<String> {
    let mut types = Vec::new();
    let Ok(rd) = std::fs::read_dir(profiles_dir) else {
        return types;
    };
    for entry in rd.filter_map(std::result::Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.file_type().is_ok_and(|ft| ft.is_dir()) {
            continue;
        }
        // Skip meta-directories that are not message types.
        if name == "schemas" {
            continue;
        }
        if let Some(filter) = type_filter {
            if name != filter {
                continue;
            }
        }
        types.push(name);
    }
    types.sort();
    types
}

/// Find the most-recently-published profile directory for `type_dir` that
/// sorts before `exclude_dir` (the new directory we're about to create).
///
/// Profile directories are named `fv<YYYYMMDD>` or `fv<YYYYMMDD>_<suffix>`;
/// we sort lexicographically (works because the date is ISO 8601 formatted).
/// Returns the directory name (e.g. `"fv20251001"`), not the full path.
fn find_latest_predecessor(type_dir: &str, exclude_dir: &str) -> Option<String> {
    let rd = std::fs::read_dir(type_dir).ok()?;
    let mut candidates: Vec<String> = rd
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_ok_and(|ft| ft.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("fv") && name != exclude_dir)
        .collect();
    candidates.sort();
    candidates.into_iter().next_back()
}

/// Parse a named `--flag value` argument from a slice.
fn parse_named_arg(args: &[String], flag: &str) -> Option<String> {
    let pos = args.iter().position(|a| a == flag)?;
    args.get(pos + 1).cloned()
}

/// Convert a BDEW format-version string to a profile directory name.
///
/// `FV2027-10-01` → `fv20271001`
fn fv_to_dir_name(fv: &str) -> String {
    let digits: String = fv.chars().filter(|c| c.is_ascii_digit()).collect();
    format!("fv{digits}")
}

/// Rudimentary ISO 8601 date validation (YYYY-MM-DD).
fn is_valid_date(s: &str) -> bool {
    let parts: Vec<&str> = s.splitn(3, '-').collect();
    if parts.len() != 3 {
        return false;
    }
    let (y, m, d) = (parts[0], parts[1], parts[2]);
    y.len() == 4
        && m.len() == 2
        && d.len() == 2
        && y.chars().all(|c| c.is_ascii_digit())
        && m.chars().all(|c| c.is_ascii_digit())
        && d.chars().all(|c| c.is_ascii_digit())
        && matches!(m.parse::<u8>(), Ok(1..=12))
        && matches!(d.parse::<u8>(), Ok(1..=31))
}

/// Subtract one calendar day from an ISO 8601 date string.
///
/// Handles month and year boundaries.  Returns `None` when the date cannot
/// be parsed or is 0001-01-01.
fn date_minus_one_day(date: &str) -> Option<String> {
    let parts: Vec<&str> = date.splitn(3, '-').collect();
    if parts.len() != 3 {
        return None;
    }
    let year: u32 = parts[0].parse().ok()?;
    let month: u32 = parts[1].parse().ok()?;
    let day: u32 = parts[2].parse().ok()?;

    let (ny, nm, nd) = if day > 1 {
        (year, month, day - 1)
    } else if month > 1 {
        let prev_m = month - 1;
        let last_day = days_in_month(year, prev_m);
        (year, prev_m, last_day)
    } else if year > 1 {
        (year - 1, 12, 31)
    } else {
        return None;
    };

    Some(format!("{ny:04}-{nm:02}-{nd:02}"))
}

/// Return the number of days in `month` of `year`, handling leap years.
fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 30,
    }
}

fn is_leap(year: u32) -> bool {
    year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100))
}

/// Load and parse a JSON file, returning an error string on failure.
fn load_json(path: &str) -> Result<Value, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    serde_json::from_str(&src).map_err(|e| format!("parse {path}: {e}"))
}

/// Write a JSON value to `path` with pretty formatting and a trailing newline.
fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let json =
        serde_json::to_string_pretty(value).map_err(|e| format!("serialize {path:?}: {e}"))?;
    let content = format!("{json}\n");
    std::fs::write(path, content).map_err(|e| format!("write {path:?}: {e}"))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fv_to_dir_name_strips_hyphens_and_lowercases() {
        assert_eq!(fv_to_dir_name("FV2027-10-01"), "fv20271001");
        assert_eq!(fv_to_dir_name("FV2026-10-01"), "fv20261001");
    }

    #[test]
    fn is_valid_date_accepts_valid() {
        assert!(is_valid_date("2027-10-01"));
        assert!(is_valid_date("2024-02-29")); // syntactically valid
        assert!(is_valid_date("2026-12-31"));
    }

    #[test]
    fn is_valid_date_rejects_invalid() {
        assert!(!is_valid_date("2027-13-01")); // month out of range
        assert!(!is_valid_date("2027-00-01")); // month zero
        assert!(!is_valid_date("2027-10")); // missing day
        assert!(!is_valid_date("not-a-date")); // non-numeric
    }

    #[test]
    fn date_minus_one_day_basic_cases() {
        assert_eq!(date_minus_one_day("2027-10-01"), Some("2027-09-30".into()));
        assert_eq!(date_minus_one_day("2027-01-01"), Some("2026-12-31".into()));
        assert_eq!(date_minus_one_day("2024-03-01"), Some("2024-02-29".into())); // leap year
        assert_eq!(date_minus_one_day("2025-03-01"), Some("2025-02-28".into())); // non-leap
        assert_eq!(date_minus_one_day("2027-10-15"), Some("2027-10-14".into()));
    }

    #[test]
    fn fv_to_dir_name_works_for_gas_suffix() {
        // We don't include the _gas suffix; the caller provides the full dir name.
        assert_eq!(fv_to_dir_name("FV2027-10-01"), "fv20271001");
    }

    #[test]
    fn parse_named_arg_finds_value() {
        let args: Vec<String> = vec!["--fv".into(), "FV2027-10-01".into()];
        assert_eq!(parse_named_arg(&args, "--fv"), Some("FV2027-10-01".into()));
        assert_eq!(parse_named_arg(&args, "--date"), None);
    }
}

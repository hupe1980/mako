//! `cargo xtask validate-pruefids`
//!
//! For every PID declared in a committed `ahb.json` profile, this task verifies
//! that at least one `.edi` fixture file contains a BGM segment with that PID in
//! DE 1004 (the second `+`-delimited field).
//!
//! Coverage evidence is intentionally restricted to `.edi` fixture files:
//! source-code string literals and comments are excluded to prevent false
//! positives (a comment like `// TODO: PID 13001` or an unrelated numeric
//! constant would otherwise satisfy the substring check).
//!
//! Rationale: without coverage evidence, a PID can regress silently.  This gate
//! prevents that by failing loudly when a PID is declared but has no associated
//! test fixture.
//!
//! # Output
//!
//! ```text
//! COVERED   utilmd  11001  (tests/fixtures/utilmd/valid/pid_11001.edi)
//! COVERED   utilmd  11004  (tests/fixtures/utilmd/valid/pid_11004.edi)
//! MISSING   utilmd  11043  — no .edi fixture BGM segment found
//! ```
//!
//! # Exit codes
//!
//! - Exits 1 when any fixture references a PID not declared in any `ahb.json`
//!   (ORPHANED — always a correctness bug: either a stale fixture or an ahb.json
//!   that is missing a PID declaration).
//! - MISSING PIDs (declared in ahb.json but lacking a fixture) cause a non-zero
//!   exit only when `--strict` mode is requested; otherwise they are
//!   informational coverage warnings unless the `--min-coverage` floor is hit.
//! - `--min-coverage` defaults to 100 so the workspace can enforce full fixture
//!   coverage by default while still allowing opt-out for ad-hoc local scans.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

// ── JSON models (minimal — only fields we need) ───────────────────────────────

#[derive(Deserialize)]
struct AhbProfile {
    pruefidentifikatoren: Vec<PruefidentifikatorEntry>,
}

#[derive(Deserialize)]
struct PruefidentifikatorEntry {
    code: u32,
}

// ── Public entry-point ────────────────────────────────────────────────────────

/// Validate Prüfidentifikator coverage.
///
/// Returns `true` (exit 0) when:
/// - No ORPHANED fixtures exist (fixtures referencing PIDs not in any `ahb.json`).
/// - `strict == false` (the default CI mode) — MISSING PIDs are informational only.
/// - `strict == true`  — MISSING PIDs also trigger exit 1.
/// - `min_coverage_pct` (ratchet gate) — fails if `covered / total < min_coverage_pct / 100`.
///   Set to 0 to disable the ratchet gate. The CLI default is 100.
///
/// `message_type_filter` — when `Some("INVOIC")` (case-insensitive), only
/// checks PIDs for that message type and ignores all others.
pub fn run(
    workspace_root: &str,
    message_type_filter: Option<&str>,
    strict: bool,
    min_coverage_pct: u32,
) -> bool {
    let profiles_dir = format!("{workspace_root}/crates/edi-energy/profiles");
    let tests_dir = format!("{workspace_root}/crates/edi-energy/tests");
    let examples_dir = format!("{workspace_root}/crates/edi-energy/examples");

    // Collect all PIDs across every ahb.json, grouped by message type.
    let pids = collect_pids(&profiles_dir);
    if pids.is_empty() {
        println!("validate-pruefids: no ahb.json profiles found under {profiles_dir}");
        return true;
    }

    let all_known_pids: std::collections::HashSet<u32> = pids.values().flatten().copied().collect();

    // Collect the set of covered PIDs by extracting them from BGM segments in
    // `.edi` fixture files only.  Substring matching in `.rs` source files is
    // intentionally excluded: a comment, string literal, or numeric constant that
    // coincidentally matches a PID code is not valid coverage evidence.
    let fixtures_dir = format!("{tests_dir}/fixtures");
    let mut covered_pids: std::collections::HashSet<u32> = std::collections::HashSet::new();
    collect_covered_pids(Path::new(&fixtures_dir), &mut covered_pids);
    collect_covered_pids(Path::new(&examples_dir), &mut covered_pids);

    let mut missing_count: usize = 0;
    let mut covered_count: usize = 0;
    let filter_lower = message_type_filter.map(str::to_lowercase);

    for (message_type, pid_list) in &pids {
        // Skip types not matching the filter (if one was given).
        if let Some(ref f) = filter_lower
            && message_type != f
        {
            continue;
        }
        for &pid in pid_list {
            if covered_pids.contains(&pid) {
                println!("COVERED   {message_type:<8}  {pid}");
                covered_count += 1;
            } else {
                println!("MISSING   {message_type:<8}  {pid}  — no .edi fixture BGM segment found");
                missing_count += 1;
            }
        }
    }

    let total = covered_count + missing_count;
    let coverage_pct = covered_count
        .checked_mul(100)
        .and_then(|n| n.checked_div(total))
        .unwrap_or(100) as u32;
    eprintln!();
    eprintln!("coverage: {covered_count}/{total} Pruefidentifikatoren covered ({coverage_pct}%)");

    // Reverse check: every PID appearing in fixture BGM segments must be declared
    // in an AHB profile.  This catches stale or mislabelled fixtures.
    // ORPHANED is always a hard error regardless of --strict.
    let mut orphaned =
        collect_orphaned_pids(&fixtures_dir, &all_known_pids, filter_lower.as_deref());
    // Also scan examples/ for stale PID references.
    orphaned.extend(collect_orphaned_pids(
        &examples_dir,
        &all_known_pids,
        filter_lower.as_deref(),
    ));
    for (fixture_path, pid) in &orphaned {
        println!("ORPHANED  fixture {fixture_path}  PID {pid}  — not declared in any ahb.json");
    }
    if !orphaned.is_empty() {
        eprintln!();
        eprintln!(
            "error: {} fixture(s) reference Pruefidentifikatoren not declared in any ahb.json.",
            orphaned.len()
        );
        eprintln!("  Either add the PID to the relevant ahb.json or remove the stale fixture.");
    }

    if missing_count > 0 {
        eprintln!();
        if strict {
            eprintln!(
                "error (--strict): {missing_count} Pruefidentifikator(en) declared in ahb.json \
                 profiles have no .edi fixture."
            );
            eprintln!(
                "  Add a `.edi` fixture under crates/edi-energy/tests/fixtures/<type>/valid/ \
                 with a BGM segment carrying the PID in element 1004 (field 2)."
            );
        } else {
            eprintln!(
                "warning: {missing_count} Pruefidentifikator(en) have no .edi fixture \
                 (run with --strict to treat this as an error)."
            );
        }
    }

    // ── Ratchet coverage gate (--min-coverage) ────────────────────────────────
    // Fail if coverage drops below the specified floor, even when --strict is not set.
    // This prevents silent fixture regressions when PIDs are added without fixtures.
    let coverage_ok = if min_coverage_pct > 0 && coverage_pct < min_coverage_pct {
        eprintln!(
            "error (--min-coverage {min_coverage_pct}): coverage {coverage_pct}% is below the \
             required minimum of {min_coverage_pct}%."
        );
        eprintln!(
            "  Add .edi fixtures for MISSING PIDs to raise coverage, or lower --min-coverage \
             if the baseline has genuinely regressed."
        );
        false
    } else {
        true
    };

    // Pass when: no ORPHANED fixtures AND (not strict OR no MISSING) AND coverage >= min.
    orphaned.is_empty() && (!strict || missing_count == 0) && coverage_ok
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Walk `<profiles_dir>/<message_type>/<release>/ahb.json` and collect every
/// declared PID, grouped by lower-case message type.
fn collect_pids(profiles_dir: &str) -> BTreeMap<String, Vec<u32>> {
    let mut map: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let base = Path::new(profiles_dir);

    let msg_type_dirs = match std::fs::read_dir(base) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("validate-pruefids: cannot read {profiles_dir}: {e}");
            return map;
        }
    };

    for msg_entry in msg_type_dirs.flatten() {
        let msg_path = msg_entry.path();
        if !msg_path.is_dir() {
            continue;
        }
        let msg_type = msg_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();

        let release_dirs = match std::fs::read_dir(&msg_path) {
            Ok(d) => d,
            Err(_) => continue,
        };

        for rel_entry in release_dirs.flatten() {
            let rel_path = rel_entry.path();
            if !rel_path.is_dir() {
                continue;
            }
            let ahb_path = rel_path.join("ahb.json");
            if !ahb_path.exists() {
                continue;
            }
            let content = match std::fs::read_to_string(&ahb_path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("validate-pruefids: cannot read {}: {e}", ahb_path.display());
                    continue;
                }
            };
            let profile: AhbProfile = match serde_json::from_str(&content) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!(
                        "validate-pruefids: cannot parse {}: {e}",
                        ahb_path.display()
                    );
                    continue;
                }
            };
            let entry = map.entry(msg_type.clone()).or_default();
            for pid in profile.pruefidentifikatoren {
                entry.push(pid.code);
            }
        }
    }

    // Sort each list for deterministic output.
    for v in map.values_mut() {
        v.sort_unstable();
        v.dedup();
    }

    map
}

/// Recursively collect the text content of every `*.rs` and `*.edi` file under
/// `tests_dir`.  This lets fixtures stored as `.edi` files count as coverage
/// evidence — a BGM line such as `BGM+380+00031001+9'` satisfies the zero-padded
/// PID check just as well as an inline byte literal in a Rust test.
/// Walk every `.edi` file under `dir` recursively and insert every PID found
/// in BGM segment field 2 (DE 1004, first component) or in `RFF+Z13:<pid>`
/// into `covered`.
///
/// This is the authoritative coverage signal: only actual EDI fixture content
/// counts as evidence, not source comments or string literals in `.rs` files.
fn collect_covered_pids(dir: &Path, covered: &mut std::collections::HashSet<u32>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(d) => d,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_covered_pids(&path, covered);
        } else if path.extension().and_then(|e| e.to_str()) == Some("edi")
            && let Ok(content) = std::fs::read_to_string(&path)
        {
            for line in content.lines() {
                let trimmed = line.trim_start();
                // BgmDe1004 types: PID is BGM DE 1004 (field index 2).
                if trimmed.starts_with("BGM") {
                    let fields: Vec<&str> = trimmed.splitn(4, '+').collect();
                    if fields.len() >= 3 {
                        let pid_str = fields[2]
                            .split(':')
                            .next()
                            .unwrap_or("")
                            .trim_end_matches('\'')
                            .trim();
                        if let Ok(pid) = pid_str.parse::<u32>()
                            && (10_000..=99_999).contains(&pid)
                        {
                            covered.insert(pid);
                        }
                    }
                }
                // RffZ13 types (COMDIS, PRICAT, UTILTS): PID is in RFF+Z13:<pid>
                if trimmed.starts_with("RFF") {
                    let fields: Vec<&str> = trimmed.splitn(3, '+').collect();
                    if fields.len() >= 2 {
                        let composite = fields[1].trim_end_matches('\'');
                        let parts: Vec<&str> = composite.splitn(2, ':').collect();
                        if parts.len() == 2 && parts[0] == "Z13" {
                            let pid_str = parts[1]
                                .split(':')
                                .next()
                                .unwrap_or("")
                                .trim_end_matches('\'')
                                .trim();
                            if let Ok(pid) = pid_str.parse::<u32>()
                                && (10_000..=99_999).contains(&pid)
                            {
                                covered.insert(pid);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Walk every `.edi` fixture under `fixtures_dir` and return `(path_str, pid)` pairs
/// where `pid` does not appear in `known_pids`.
///
/// BGM element DE 1004 is the second `+`-delimited field; the PID is the first
/// component of that element (before any `:`).  We match it as an 8-digit decimal
/// string.
fn collect_orphaned_pids(
    fixtures_dir: &str,
    known_pids: &std::collections::HashSet<u32>,
    filter_type: Option<&str>,
) -> Vec<(String, u32)> {
    let mut orphaned = Vec::new();
    collect_orphaned_in_dir(
        Path::new(fixtures_dir),
        known_pids,
        filter_type,
        &mut orphaned,
    );
    orphaned.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    orphaned
}

fn collect_orphaned_in_dir(
    dir: &Path,
    known_pids: &std::collections::HashSet<u32>,
    filter_type: Option<&str>,
    out: &mut Vec<(String, u32)>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(d) => d,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_orphaned_in_dir(&path, known_pids, filter_type, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("edi") {
            // Apply optional type filter by checking the fixture path components.
            if let Some(ft) = filter_type {
                let path_str = path.to_string_lossy().to_lowercase();
                if !path_str.contains(&format!("/{ft}/"))
                    && !path_str.contains(&format!("\\{ft}\\"))
                {
                    continue;
                }
            }
            if let Ok(content) = std::fs::read_to_string(&path) {
                for line in content.lines() {
                    let trimmed = line.trim_start();
                    if !trimmed.starts_with("BGM") {
                        continue;
                    }
                    // Split on '+' — BGM fields are '+'-delimited.
                    let fields: Vec<&str> = trimmed.splitn(4, '+').collect();
                    if fields.len() < 3 {
                        continue;
                    }
                    // DE 1004 is in element 2 (0-indexed); take the first component.
                    let pid_str = fields[2].split(':').next().unwrap_or("").trim();
                    if let Ok(pid) = pid_str.parse::<u32>()
                        && (10_000..=99_999).contains(&pid)
                        && !known_pids.contains(&pid)
                    {
                        out.push((path.to_string_lossy().into_owned(), pid));
                    }
                }
            }
        }
    }
}

//! `cargo xtask validate-release-codes`
//!
//! Cross-checks that every non-archived `mig.json` profile's `"release"` field
//! appears in at least one UNH segment (data element 0057) in the `.edi` fixture
//! files under `crates/edi-energy/tests/fixtures/`.
//!
//! # Motivation
//!
//! If BDEW revises a format version and bumps the association-assigned code
//! (UNH 0057) — e.g. `S2.1` → `S2.2` after publishing a corrected AHB — but
//! the `mig.json` `release` field is not updated, the profile dispatcher will
//! reject or misroute inbound messages whose UNH carries the new code.
//!
//! This task makes the mismatch visible before it reaches production.
//!
//! # UNH segment layout
//!
//! ```text
//! UNH+<ref>+<msg_type>:<version>:<release>:<org>:<0057>'
//!                                                  ^^^^
//!                                           association-assigned code
//! ```
//!
//! Example:  `UNH+00001+UTILMD:D:11A:UN:S2.2'`
//! Extracted code: `S2.2`
//!
//! # Exit codes
//!
//! - 0 — every active (non-archived) profile's release code appears in at least
//!   one fixture UNH 0057 value for the same message type.
//! - 1 — one or more profiles have no matching fixture (wire-value mismatch risk).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;

// ── JSON models ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct MigProfile {
    message_type: Option<String>,
    release: String,
    #[serde(default)]
    archived: bool,
}

// ── Public entry-point ────────────────────────────────────────────────────────

/// Returns `true` (exit 0) when every active profile's `release` field is
/// witnessed by at least one fixture UNH 0057 value.
pub fn run(workspace_root: &str, _args: &[String]) -> bool {
    let profiles_dir = format!("{workspace_root}/crates/edi-energy/profiles");
    let fixtures_dir = format!("{workspace_root}/crates/edi-energy/tests");

    // ── Step 1: collect declared release codes per message type ──────────────
    // Key: (message_type_lowercase, release_code)
    let mut declared: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let profiles_path = Path::new(&profiles_dir);
    if !profiles_path.exists() {
        eprintln!("validate-release-codes: profiles directory not found: {profiles_dir}");
        return false;
    }
    collect_profiles(profiles_path, &mut declared);

    if declared.is_empty() {
        println!("validate-release-codes: no non-archived mig.json profiles found");
        return true;
    }

    // ── Step 2: collect observed UNH 0057 values per message type ────────────
    // Key: message_type_lowercase → set of observed 0057 codes
    let mut observed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let tests_path = Path::new(&fixtures_dir);
    if tests_path.exists() {
        collect_unh_codes(tests_path, &mut observed);
    }

    // Also check examples directory for additional UNH evidence
    let examples_dir = format!("{workspace_root}/crates/edi-energy/examples");
    let examples_path = Path::new(&examples_dir);
    if examples_path.exists() {
        collect_unh_codes(examples_path, &mut observed);
    }

    // ── Step 3: cross-check ───────────────────────────────────────────────────
    let mut ok = true;

    for ((msg_type, release_code), profile_paths) in &declared {
        let witnessed = observed
            .get(msg_type)
            .map(|codes| codes.contains(release_code))
            .unwrap_or(false);

        let status = if witnessed { "OK     " } else { "MISSING" };
        let path_display = profile_paths.first().map(String::as_str).unwrap_or("?");

        if witnessed {
            println!("{status}  {msg_type:<10}  release={release_code:<12}  {path_display}");
        } else {
            eprintln!("{status}  {msg_type:<10}  release={release_code:<12}  {path_display}");
            eprintln!(
                "         → no .edi fixture has UNH 0057={release_code:?} for message type {msg_type:?}"
            );
            if let Some(codes) = observed.get(msg_type) {
                let mut sorted: Vec<&str> = codes.iter().map(String::as_str).collect();
                sorted.sort();
                eprintln!("         → observed UNH 0057 codes for {msg_type:?}: {sorted:?}");
            } else {
                eprintln!("         → no .edi fixtures found at all for message type {msg_type:?}");
            }
            ok = false;
        }
    }

    if ok {
        println!(
            "\nvalidate-release-codes: all active profiles have matching fixture UNH 0057 values ✓"
        );
    } else {
        eprintln!("\nvalidate-release-codes: FAILED — see MISSING entries above");
        eprintln!("  Action: add a fixture file with UNH+x+<TYPE>:D:11A:UN:<release_code>' or");
        eprintln!("  update the mig.json release field to match the actual BDEW wire value.");
    }

    ok
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Walk `dir` recursively and collect every `mig.json` profile that is not
/// archived. The key is `(message_type_lowercase, release_code)`.
fn collect_profiles(dir: &Path, out: &mut BTreeMap<(String, String), Vec<String>>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_profiles(&path, out);
        } else if path.file_name().is_some_and(|n| n == "mig.json") {
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(profile) = serde_json::from_str::<MigProfile>(&content) else {
                eprintln!("validate-release-codes: failed to parse {}", path.display());
                continue;
            };
            if profile.archived {
                continue;
            }
            // Derive message type from directory structure when not in JSON.
            // Path: …/profiles/<msg_type>/<release>/mig.json
            let msg_type = profile.message_type.unwrap_or_else(|| {
                path.ancestors()
                    .nth(2)
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            let key = (msg_type.to_lowercase(), profile.release.clone());
            out.entry(key).or_default().push(path.display().to_string());
        }
    }
}

/// Walk `dir` recursively and extract the UNH 0057 (association-assigned code)
/// from every `.edi` file. The key is the message type (lowercase).
///
/// UNH format: `UNH+<ref>+<type>:<ver>:<release>:<org>:<code>'`
/// The code is the 5th (0-indexed: 4th) colon-split field of UNH element 1.
fn collect_unh_codes(dir: &Path, out: &mut BTreeMap<String, BTreeSet<String>>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_unh_codes(&path, out);
        } else if path.extension().is_some_and(|e| e == "edi") {
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            // Segments may be separated by "'" (compact) or newlines.
            // Split on both to cover all release characters.
            for segment in content.split(['\'', '\n']) {
                let seg = segment.trim();
                if !seg.starts_with("UNH") {
                    continue;
                }
                // UNH+<ref>+<composite>
                let parts: Vec<&str> = seg.splitn(3, '+').collect();
                if parts.len() < 3 {
                    continue;
                }
                let composite = parts[2];
                let fields: Vec<&str> = composite.split(':').collect();
                // fields[0] = msg_type (e.g. "UTILMD")
                // fields[4] = association-assigned code (UNH 0057, e.g. "S2.2")
                if fields.len() >= 5 {
                    let msg_type = fields[0].to_lowercase();
                    let code = fields[4].trim_end_matches('\'').trim().to_owned();
                    if !code.is_empty() {
                        out.entry(msg_type).or_default().insert(code);
                    }
                }
            }
        }
    }
}

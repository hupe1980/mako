//! `cargo xtask validate-release-codes`
//!
//! Cross-checks that every `mig.json` profile a counterparty can still send
//! declares a `"release"` that appears in at least one UNH segment (data
//! element 0057) in the `.edi` fixtures under `crates/edi-energy/tests/`.
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
//! # Which profiles must be witnessed
//!
//! The ones that can still carry an inbound message: in force today, or not yet
//! in force (their Anwendungszeitpunkt is the day mako starts receiving them, so
//! the fixture has to exist *before* the cutover, not after).
//!
//! A **superseded** profile — one past its `valid_until` — is reported but does
//! not fail. `ReleaseRegistry::is_acceptable_on` refuses it at the BDEW default
//! receive tolerance of zero days, so no fixture can witness a live wire value
//! that no longer exists, and retiring the fixtures alongside the format version
//! is correct rather than a regression.
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
//! - 0 — every receivable profile's release code appears in at least one fixture
//!   UNH 0057 value for the same message type.
//! - 1 — a receivable profile has no matching fixture (wire-value mismatch risk).

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
    /// Last day this format version is on the wire; `None` for the current one.
    #[serde(default)]
    valid_until: Option<String>,
}

// ── Public entry-point ────────────────────────────────────────────────────────

/// Returns `true` (exit 0) when every active profile's `release` field is
/// witnessed by at least one fixture UNH 0057 value.
pub fn run(workspace_root: &str, _args: &[String]) -> bool {
    let profiles_dir = format!("{workspace_root}/crates/edi-energy/profiles");
    let fixtures_dir = format!("{workspace_root}/crates/edi-energy/tests");

    // ── Step 1: collect declared release codes per message type ──────────────
    // Key: (message_type_lowercase, release_code)
    let mut declared: BTreeMap<(String, String), Declared> = BTreeMap::new();
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
    let today = mako_fristen::heute();
    let mut ok = true;
    let mut superseded = 0usize;

    for ((msg_type, release_code), declared) in &declared {
        let witnessed = observed
            .get(msg_type)
            .is_some_and(|codes| codes.contains(release_code));
        let receivable = declared.receivable_on(today);
        let path_display = declared.paths.first().map(String::as_str).unwrap_or("?");

        if witnessed {
            println!("OK        {msg_type:<10}  release={release_code:<12}  {path_display}");
        } else if receivable {
            eprintln!("MISSING   {msg_type:<10}  release={release_code:<12}  {path_display}");
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
        } else {
            let expired_on = declared.last_day.map(|d| d.to_string()).unwrap_or_default();
            println!(
                "SUPERSEDED {msg_type:<9}  release={release_code:<12}  {path_display}\n\
                 {:11}→ superseded on {expired_on}; no fixture needed",
                ""
            );
            superseded += 1;
        }
    }

    if ok {
        println!(
            "\nvalidate-release-codes: every receivable profile has a matching fixture UNH 0057 value ✓\
             \n  ({superseded} superseded profile(s) skipped — see SUPERSEDED above)"
        );
    } else {
        eprintln!("\nvalidate-release-codes: FAILED — see MISSING entries above");
        eprintln!("  Action: add a fixture file with UNH+x+<TYPE>:D:11A:UN:<release_code>' or");
        eprintln!("  update the mig.json release field to match the actual BDEW wire value.");
    }

    ok
}

/// Every profile declaring one `(message type, release)` pair.
///
/// BDEW reuses a release code across format versions — `2.0b` covers both
/// CONTRL `fv20251001` and `fv20260101` — so a code is on the wire while *any*
/// profile carrying it is unexpired.
#[derive(Default)]
struct Declared {
    paths: Vec<String>,
    /// Set when one of those profiles is open-ended (`valid_until: null`).
    open_ended: bool,
    /// Otherwise the last day any of them is on the wire.
    last_day: Option<time::Date>,
}

impl Declared {
    /// Whether a counterparty can still send this release code on `today`.
    fn receivable_on(&self, today: time::Date) -> bool {
        self.open_ended || self.last_day.is_none_or(|last| today <= last)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Walk `dir` recursively and collect every `mig.json` profile that is not
/// archived. The key is `(message_type_lowercase, release_code)`.
fn collect_profiles(dir: &Path, out: &mut BTreeMap<(String, String), Declared>) {
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
            let valid_until = profile.valid_until.as_deref().and_then(|raw| {
                time::Date::parse(raw, &time::format_description::well_known::Iso8601::DATE).ok()
            });
            let key = (msg_type.to_lowercase(), profile.release.clone());
            let slot = out.entry(key).or_default();
            slot.paths.push(path.display().to_string());
            match valid_until {
                None => slot.open_ended = true,
                Some(day) => slot.last_day = Some(slot.last_day.map_or(day, |cur| cur.max(day))),
            }
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

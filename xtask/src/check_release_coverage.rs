/// `cargo xtask check-release-coverage` — fail when the current (or a given)
/// date falls within a release window for which no profile exists.
///
/// For every message type under `crates/edi-energy/profiles/` the command reads
/// the `valid_from` and `valid_until` fields of every `mig.json` and checks that
/// at least one profile covers `date`.
///
/// A profile that lacks a `valid_until` field is treated as open-ended (valid
/// from `valid_from` until the next profile takes over, or indefinitely).
///
/// Profiles with `"archived": true` are excluded from coverage checks (they
/// have been intentionally retired and are expected to have gaps).
///
/// Exit codes:
///   0 — all checked message types have at least one profile covering `date`
///   1 — one or more message types have no profile covering `date`
use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct MigJson {
    message_type: String,
    valid_from: Option<String>,
    valid_until: Option<String>,
    release: Option<String>,
    #[serde(default)]
    archived: bool,
    /// `pid_exempt` profiles (CONTRL) can be exempt from coverage warnings
    /// because they are protocol-level messages, not domain FV-tracked ones.
    #[serde(default)]
    pid_exempt: bool,
}

#[derive(Debug)]
struct ProfileSpan {
    dir: String,
    release: String,
    valid_from: Option<time::Date>,
    valid_until: Option<time::Date>,
    archived: bool,
    pid_exempt: bool,
}

// No hardcoded EXEMPT_TYPES: exemption is handled through the `pid_exempt` field
// in each profile's mig.json (e.g. CONTRL, which is a protocol-level acknowledgement
// format whose profiles all carry `"pid_exempt": true`).
//
// Extraordinary publications (e.g. INSRPT 1.1a fv20260101, CONTRL 2.0b fv20260101)
// are handled by their `valid_from` / `valid_until` dates exactly like annual profiles —
// they are NOT exempt from date-coverage verification.

pub fn check_release_coverage() {
    let args: Vec<String> = std::env::args().skip(2).collect();

    // Parse --date YYYY-MM-DD (defaults to today)
    let date_str = args
        .iter()
        .position(|a| a == "--date")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let check_date: time::Date = if let Some(s) = date_str {
        match time::Date::parse(
            &s,
            time::macros::format_description!("[year]-[month]-[day]"),
        ) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("error: invalid --date value `{s}`: {e}");
                std::process::exit(1);
            }
        }
    } else {
        time::OffsetDateTime::now_utc().date()
    };

    let profiles_root = Path::new("crates/edi-energy/profiles");
    if !profiles_root.exists() {
        eprintln!(
            "error: profiles directory not found: {}",
            profiles_root.display()
        );
        std::process::exit(1);
    }

    // Collect all (message_type -> [ProfileSpan]) entries.
    let mut by_type: BTreeMap<String, Vec<ProfileSpan>> = BTreeMap::new();

    for entry in std::fs::read_dir(profiles_root).expect("read profiles dir") {
        let entry = entry.expect("read dir entry");
        let path = entry.path();
        if !path.is_dir() {
            continue; // skip `schemas/`
        }
        let type_name = path.file_name().unwrap().to_string_lossy().to_uppercase();
        if type_name == "SCHEMAS" {
            continue;
        }

        for fv_entry in std::fs::read_dir(&path).expect("read fv dir") {
            let fv_entry = fv_entry.expect("read fv entry");
            let fv_path = fv_entry.path();
            if !fv_path.is_dir() {
                continue;
            }
            let mig_path = fv_path.join("mig.json");
            if !mig_path.exists() {
                continue;
            }

            let mig_str = std::fs::read_to_string(&mig_path)
                .unwrap_or_else(|e| panic!("read {}: {e}", mig_path.display()));
            let mig: MigJson = serde_json::from_str(&mig_str)
                .unwrap_or_else(|e| panic!("parse {}: {e}", mig_path.display()));

            let valid_from = mig.valid_from.as_deref().and_then(parse_date);
            let valid_until = mig.valid_until.as_deref().and_then(parse_date);

            let dir_name = fv_path.file_name().unwrap().to_string_lossy().to_string();

            by_type
                .entry(mig.message_type.to_uppercase())
                .or_default()
                .push(ProfileSpan {
                    dir: dir_name,
                    release: mig.release.unwrap_or_default(),
                    valid_from,
                    valid_until,
                    archived: mig.archived,
                    pid_exempt: mig.pid_exempt,
                });
        }
    }

    println!(
        "check-release-coverage: checking {} message types for date {}",
        by_type.len(),
        check_date
    );
    println!();

    let mut errors: Vec<String> = Vec::new();
    let mut ok_count = 0u32;
    let mut skip_count = 0u32;

    for (msg_type, spans) in &by_type {
        // Skip if all profiles for this type are pid_exempt
        // (e.g. CONTRL — protocol-level acknowledgement, not domain-FV-tracked).
        if spans.iter().all(|s| s.pid_exempt) {
            println!("  -  {msg_type}: skipped (all profiles pid_exempt — protocol-level format)");
            skip_count += 1;
            continue;
        }

        // Find profiles that cover `check_date` (non-archived).
        let covering: Vec<&ProfileSpan> = spans
            .iter()
            .filter(|s| !s.archived && covers(s, check_date))
            .collect();

        if covering.is_empty() {
            // Report the nearest future profile as context.
            let future: Vec<&ProfileSpan> = spans
                .iter()
                .filter(|s| !s.archived && s.valid_from.is_some_and(|vf| vf > check_date))
                .collect();

            let hint = if let Some(next) = future.iter().min_by_key(|s| s.valid_from) {
                format!(
                    " (next: {} valid_from {})",
                    next.dir,
                    next.valid_from.map_or("?".to_owned(), |d| format!("{d}"))
                )
            } else {
                String::new()
            };

            let past: Vec<&ProfileSpan> = spans.iter().filter(|s| !s.archived).collect();
            let available = past
                .iter()
                .map(|s| format!("{} ({})", s.dir, s.release))
                .collect::<Vec<_>>()
                .join(", ");

            errors.push(format!(
                "  {msg_type}: NO profile covers {check_date}{hint}\n    available: [{available}]"
            ));
        } else {
            let active = covering
                .iter()
                .map(|s| format!("{} ({})", s.dir, s.release))
                .collect::<Vec<_>>()
                .join(", ");
            println!("  ✓  {msg_type}: {active}");
            ok_count += 1;
        }
    }

    println!();
    println!(
        "Summary: {ok_count} covered, {} gap(s), {skip_count} skipped (pid_exempt)",
        errors.len()
    );

    if !errors.is_empty() {
        println!();
        eprintln!("COVERAGE GAPS for {check_date}:");
        for e in &errors {
            eprintln!("{e}");
        }
        eprintln!();
        eprintln!("Run `cargo xtask extract-pdf` + `cargo xtask codegen` to add missing profiles.");
        eprintln!("Check docs/annual-release-workflow.md for the standard remediation procedure.");
        std::process::exit(1);
    } else {
        println!("All message types are covered for {check_date}. ✓");
    }
}

fn parse_date(s: &str) -> Option<time::Date> {
    time::Date::parse(s, time::macros::format_description!("[year]-[month]-[day]")).ok()
}

/// Returns `true` when `span` is active on `date`.
///
/// A span is active when:
/// - `valid_from` is absent (undated legacy profile), OR `valid_from <= date`
/// - AND (`valid_until` is absent) OR `valid_until >= date`
fn covers(span: &ProfileSpan, date: time::Date) -> bool {
    let from_ok = span.valid_from.is_none_or(|vf| vf <= date);
    let until_ok = span.valid_until.is_none_or(|vu| vu >= date);
    from_ok && until_ok
}

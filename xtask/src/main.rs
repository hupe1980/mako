// Suppress lints that are impractical to fix in build-tool code.
#![allow(clippy::collapsible_if)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::redundant_closure_for_method_calls)]
#![allow(clippy::unnecessary_map_or)]

const HELP: &str = "\
Usage: cargo xtask <COMMAND>

Commands:
  add-release         Scaffold a new BDEW format-version profile directory skeleton
  bump-version        Bump workspace version in root Cargo.toml (usage: bump-version X.Y.Z)
  codegen             Generate Rust profile code from EDI@Energy specifications
  validate-profiles   Validate all committed profiles for consistency errors
  validate-pruefids   Check that every AHB Pruefidentifikator has a test fixture
  validate-release-codes  Verify that every profile's release code appears in a UNH 0057 fixture
  audit-ahb           Comprehensive AHB rule-coverage analysis for all profiles
  check-bo4e-coverage  Count distinct rubo4e::current types used across crates/ and services/ and
                        verify the count matches the claim in README.md. Exits 1 if the count
                        deviates by more than 2.
  check-release-coverage  Fail when no profile covers the current (or --date) date

Options for `validate-pruefids`:
  --message-type <TYPE> Filter coverage check to the given message type (e.g. INVOIC)
  --strict              Treat MISSING PIDs as errors (exit 1). Enable once fixture
                        coverage is sufficient (e.g. ≥80%).
  --min-coverage <PCT>  Fail if covered / total PIDs < PCT%. Ratchet gate: set this
                        to the current coverage floor to prevent silent regressions
                        when new PIDs are added without fixtures. Default: 0 (disabled).
  --json                Emit a machine-readable JSON report to stdout in addition to
                        the human-readable output (keys: covered, missing, orphaned,
                        coverage_pct, ok). Each entry in missing has pid and message_type.
  release-diff        Compare two releases of a message-type profile

Options for `generate-fixtures`:
  --dry-run              Print what would be generated without touching the FS.
  --message-type <TYPE>  Only generate for one message type (e.g. UTILMD).

  --date <YYYY-MM-DD>   Date to check against (default: today)

Options for `audit-ahb`:
  --message-type <TYPE>   Limit audit to one message type (e.g. UTILMD)
  --output-json  <PATH>   Write machine-readable JSON report to a file
  --min-density  <N>      Fail if avg (seg+grp) rules/PID < N for any active profile
  --min-cond-rules <N>    Fail if total conditional_rules < N for any active profile
  extract-pdf         Extract MIG/AHB table data from a PDF (best-effort draft)
  extract-docx        Extract MIG/AHB table data from a DOCX (exact column parser)
  import-xml-ahb      Import AHB from official BDEW XML (requires BDEW subscription)
  import-xml-mig      Import MIG from official BDEW XML (requires BDEW subscription)
  import-codelists    Import code values from CSV into a codelists.json profile
  generate-fixtures   Generate minimal synthetic .edi fixtures for uncovered PIDs
  help                Print this help message

Options for `codegen`:
  --dry-run             Print what would be generated without writing files
  --check               Verify generated files are up-to-date; exit 1 if stale (CI drift guard)
  --message-type <TYPE> Regenerate only profiles for the given message type (e.g. UTILMD)
                        (skips pre-codegen schema validation for speed)
  --prune-expired       Mark profiles whose valid_until + GRACE_DAYS is in the past as
                        archived=true in their mig.json, then regenerate mod.rs.
                        Archived profiles require the `{type}-archive` or `archive` Cargo
                        feature to compile and are excluded from the default build.
                        Run this annually after the BDEW format update cycle.
  --grace-days <N>      Grace period in days after valid_until before archiving (default: 90).

Options for `add-release`:
  --fv           <FV>       BDEW format-version string (e.g. FV2027-10-01)
  --date         <DATE>     valid_from date ISO 8601; inferred from --fv when omitted
  --message-type <TYPE>     Only scaffold one message type (e.g. UTILMD)
  --dry-run                 Print what would be created without touching the FS

Options for `release-diff`:
  --message-type <TYPE> Message type to diff (e.g. UTILMD)
  --from <RELEASE>      Starting release folder name (e.g. fv20251001)
  --to   <RELEASE>      Target release folder name   (e.g. fv20261001)
  --output-file <PATH>  Write diff output to a file instead of stdout

Options for `import-codelists`:
  --file         <PATH>     CSV file with columns DE_ID,Code,Description
  --message-type <TYPE>     Target message type (e.g. INVOIC)
  --release      <RELEASE>  Target release (e.g. 2.8e)
  --dry-run                 Print proposed changes without writing

Options for `extract-pdf`:
  --file         <PATH>     PDF file to extract from
  --message-type <TYPE>     Message type (utilmd, mscons, aperak, contrl, …)
  --release      <RELEASE>  EDI@Energy release (inferred from path if omitted)

Options for `extract-docx`:
  --file         <PATH>     DOCX file to extract from
  --message-type <TYPE>     Message type (utilmd, mscons, aperak, contrl, …)
  --release      <RELEASE>  EDI@Energy release (inferred from path if omitted)
  --mode         <MODE>     What to extract: mig | ahb | both (default: both)

Options for `import-xml-ahb`:
  --file         <PATH>     BDEW AHB XML file (<AHB> root)
  --message-type <TYPE>     Message type (inferred from XML root when possible)
  --release      <RELEASE>  Format version (e.g. FV2026-10-01; inferred from path)
  --valid-from   <DATE>     ISO 8601 profile activation date (e.g. 2026-10-01)

Options for `import-xml-mig`:
  --file         <PATH>     BDEW MIG XML file (<M_MSGTYPE> root)
  --message-type <TYPE>     Message type (inferred from XML root when possible)
  --release      <RELEASE>  Format version (e.g. FV2026-10-01; inferred from path)
  --valid-from   <DATE>     ISO 8601 profile activation date (e.g. 2026-10-01)

Exit codes:
  0  All checks passed / codegen succeeded
  1  One or more errors were found
";

mod add_release;
mod audit_ahb;
mod bump_version;
mod check_release_coverage;
mod codegen;
mod extract_docx;
mod extract_pdf;
mod generate_fixtures;
mod import_codelists;
mod import_xml_profiles;
mod release_diff;
mod validate_profiles;
mod validate_pruefids;
mod validate_release_codes;

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("add-release") => add_release(),
        Some("bump-version") => bump_version(),
        Some("audit-ahb") => audit_ahb(),
        Some("check-bo4e-coverage") => check_bo4e_coverage(),
        Some("check-release-coverage") => check_release_coverage::check_release_coverage(),
        Some("codegen") => codegen(),
        Some("validate-profiles") => validate_profiles(),
        Some("validate-pruefids") => validate_pruefids(),
        Some("validate-release-codes") => validate_release_codes(),
        Some("release-diff") => release_diff(),
        Some("extract-pdf") => extract_pdf(),
        Some("extract-docx") => extract_docx(),
        Some("import-xml-ahb") => import_xml_ahb(),
        Some("import-xml-mig") => import_xml_mig(),
        Some("import-codelists") => import_codelists(),
        Some("generate-fixtures") => generate_fixtures(),
        Some("help" | "--help" | "-h") | None => print!("{HELP}"),
        Some(other) => {
            eprintln!("error: unknown task `{other}`");
            eprintln!();
            eprint!("{HELP}");
            std::process::exit(1);
        }
    }
}

// ── Tasks ─────────────────────────────────────────────────────────────────────

fn add_release() {
    let (workspace_root, _) = workspace_info();
    let args: Vec<String> = std::env::args().skip(2).collect();
    let ok = add_release::run(&workspace_root, &args);
    if !ok {
        std::process::exit(1);
    }
}

fn bump_version() {
    let (workspace_root, _) = workspace_info();
    let args: Vec<String> = std::env::args().skip(2).collect();
    let ok = bump_version::run(&workspace_root, &args);
    if !ok {
        std::process::exit(1);
    }
}

fn audit_ahb() {
    let (workspace_root, _) = workspace_info();
    let args: Vec<String> = std::env::args().skip(2).collect();
    let ok = audit_ahb::run(&workspace_root, &args);
    if !ok {
        std::process::exit(1);
    }
}

fn check_bo4e_coverage() {
    // Count distinct rubo4e::current types used across crates/ and services/.
    //
    // The grep for single-line imports misses multi-line brace imports such as
    //   use rubo4e::current::{
    //       Energiemenge, Lastgang, ...
    //   };
    // This implementation scans all Rust source files and accumulates type names
    // from both single-line and multi-line import forms.
    //
    // Exit 1 if the count deviates by more than 2 from the claimed value in README.md.
    use std::collections::BTreeSet;

    let (root_str, _) = workspace_info();
    let root = std::path::Path::new(&root_str);
    let mut types: BTreeSet<String> = BTreeSet::new();

    // Walk crates/ and services/ for *.rs files.
    let search_dirs = [root.join("crates"), root.join("services")];
    let mut rs_files: Vec<std::path::PathBuf> = Vec::new();
    for dir in &search_dirs {
        if let Ok(walker) = std::fs::read_dir(dir) {
            collect_rs_files(walker, &mut rs_files);
        }
    }

    // For each file, scan for `use rubo4e::current::<ident>` or `use rubo4e::current::{...}`
    for path in &rs_files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let mut in_import = false;
        for line in src.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("use rubo4e::current::") {
                in_import = true;
            }
            if in_import {
                // Extract PascalCase identifiers, skipping `as Alias` rename targets.
                // e.g. `Sparte as Bo4eSparte` → adds Sparte, skips Bo4eSparte.
                let mut word = String::new();
                let mut prev_word_was_as = false;
                for ch in trimmed.chars() {
                    if ch.is_alphanumeric() || ch == '_' {
                        word.push(ch);
                    } else if !word.is_empty() {
                        if word == "as" {
                            prev_word_was_as = true;
                        } else {
                            let is_type = word.chars().next().is_some_and(|c| c.is_uppercase());
                            if is_type && !prev_word_was_as && word != "Bool" && word != "String" {
                                types.insert(word.clone());
                            }
                            prev_word_was_as = false;
                        }
                        word.clear();
                    }
                }
                if !word.is_empty() && word != "as" {
                    let is_type = word.chars().next().is_some_and(|c| c.is_uppercase());
                    if is_type && !prev_word_was_as {
                        types.insert(word);
                    }
                }
                // End of import: no trailing comma or open brace means single-line form done.
                if !trimmed.ends_with('{') && !trimmed.ends_with(',') {
                    in_import = false;
                }
            }
        }
    }

    println!("rubo4e::current types found:");
    for t in &types {
        println!("  {t}");
    }
    let found = types.len();
    println!("\nTotal: {found} distinct rubo4e::current types");

    // Check against the claim in README.md.
    // The claim looks like: **24 active `rubo4e::current` types**
    let readme = std::fs::read_to_string(root.join("README.md")).unwrap_or_default();
    let claimed: usize = readme
        .lines()
        .find(|l| l.contains("rubo4e::current") && l.contains("types"))
        .and_then(|l| {
            // Strip markdown formatting and find the first integer.
            let stripped: String = l
                .chars()
                .filter(|c| c.is_ascii_digit() || c.is_whitespace())
                .collect();
            stripped
                .split_whitespace()
                .find_map(|w| w.parse::<usize>().ok())
        })
        .unwrap_or(0);

    if claimed == 0 {
        println!("⚠ Could not parse claimed count from README.md.");
    } else {
        let delta = (found as i64 - claimed as i64).unsigned_abs() as usize;
        if delta > 2 {
            eprintln!(
                "ERROR: found {found} types but README.md claims {claimed} (delta={delta} > 2)."
            );
            eprintln!("Update the count in README.md and docs/index.md.");
            std::process::exit(1);
        } else {
            println!("✓ Count {found} matches README.md claim {claimed} (delta={delta} ≤ 2).");
        }
    }
}

fn collect_rs_files(walker: std::fs::ReadDir, out: &mut Vec<std::path::PathBuf>) {
    for entry in walker.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Ok(sub) = std::fs::read_dir(&path) {
                collect_rs_files(sub, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn codegen() {
    let (workspace_root, _) = workspace_info();
    let args: Vec<String> = std::env::args().skip(2).collect();
    codegen::run(&workspace_root, &args);
}

fn validate_profiles() {
    let (workspace_root, _) = workspace_info();
    let ok = validate_profiles::run(&workspace_root);
    if !ok {
        std::process::exit(1);
    }
}

fn validate_pruefids() {
    let (workspace_root, _) = workspace_info();
    let args: Vec<String> = std::env::args().skip(2).collect();
    let message_type_filter = parse_named_arg(&args, "--message-type");
    let strict = args.iter().any(|a| a == "--strict");
    let json_output = args.iter().any(|a| a == "--json");
    let min_coverage_pct: u32 = args
        .windows(2)
        .find(|w| w[0] == "--min-coverage")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(100);
    let ok = validate_pruefids::run(
        &workspace_root,
        message_type_filter.as_deref(),
        strict,
        min_coverage_pct,
        json_output,
    );
    if !ok {
        std::process::exit(1);
    }
}

fn validate_release_codes() {
    let (workspace_root, _) = workspace_info();
    let args: Vec<String> = std::env::args().skip(2).collect();
    let ok = validate_release_codes::run(&workspace_root, &args);
    if !ok {
        std::process::exit(1);
    }
}

fn release_diff() {
    let (workspace_root, _) = workspace_info();
    let args: Vec<String> = std::env::args().skip(2).collect();
    let no_diff = release_diff::run(&workspace_root, &args);
    if !no_diff {
        // exit 1 = either differences found OR an error occurred
        std::process::exit(1);
    }
}

fn extract_pdf() {
    let (workspace_root, _) = workspace_info();
    let args: Vec<String> = std::env::args().skip(2).collect();
    let ok = extract_pdf::run(&workspace_root, &args);
    if !ok {
        std::process::exit(1);
    }
}

fn extract_docx() {
    let (workspace_root, _) = workspace_info();
    let args: Vec<String> = std::env::args().skip(2).collect();
    let ok = extract_docx::run(&workspace_root, &args);
    if !ok {
        std::process::exit(1);
    }
}

fn import_xml_ahb() {
    let (workspace_root, _) = workspace_info();
    let args: Vec<String> = std::env::args().skip(2).collect();
    let ok = import_xml_profiles::run_import_ahb(&workspace_root, &args);
    if !ok {
        std::process::exit(1);
    }
}

fn import_xml_mig() {
    let (workspace_root, _) = workspace_info();
    let args: Vec<String> = std::env::args().skip(2).collect();
    let ok = import_xml_profiles::run_import_mig(&workspace_root, &args);
    if !ok {
        std::process::exit(1);
    }
}

fn import_codelists() {
    let (workspace_root, _) = workspace_info();
    let args: Vec<String> = std::env::args().skip(2).collect();
    let ok = import_codelists::run(&workspace_root, &args);
    if !ok {
        std::process::exit(1);
    }
}

fn generate_fixtures() {
    let (workspace_root, _) = workspace_info();
    let args: Vec<String> = std::env::args().skip(2).collect();
    let ok = generate_fixtures::run(&workspace_root, &args);
    if !ok {
        std::process::exit(1);
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns the value of a named argument like `--flag value` from a slice.
fn parse_named_arg(args: &[String], flag: &str) -> Option<String> {
    let pos = args.iter().position(|a| a == flag)?;
    args.get(pos + 1).cloned()
}

/// Returns `(workspace_root, path_to_root_Cargo.toml)`.
fn workspace_info() -> (String, String) {
    // CARGO_MANIFEST_DIR for the xtask crate itself is `<workspace>/xtask`.
    // Walk one level up to reach the workspace root.
    let xtask_dir = std::env::var("CARGO_MANIFEST_DIR")
        .unwrap_or_else(|_| std::env::current_dir().unwrap().display().to_string());

    let root = std::path::Path::new(&xtask_dir)
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| xtask_dir.clone());

    let manifest = format!("{root}/Cargo.toml");
    (root, manifest)
}

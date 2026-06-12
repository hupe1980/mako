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
  codegen             Generate Rust profile code from EDI@Energy specifications
  validate-profiles   Validate all committed profiles for consistency errors
  validate-pruefids   Check that every AHB Pruefidentifikator has a test fixture
  audit-ahb           Comprehensive AHB rule-coverage analysis for all profiles

Options for `validate-pruefids`:
  --message-type <TYPE> Filter coverage check to the given message type (e.g. INVOIC)
  --strict              Treat MISSING PIDs as errors (exit 1). Enable once fixture
                        coverage is sufficient (e.g. ≥80%).
  --min-coverage <PCT>  Fail if covered / total PIDs < PCT%. Ratchet gate: set this
                        to the current coverage floor to prevent silent regressions
                        when new PIDs are added without fixtures. Default: 0 (disabled).
  release-diff        Compare two releases of a message-type profile

Options for `generate-fixtures`:
  --dry-run              Print what would be generated without touching the FS.
  --message-type <TYPE>  Only generate for one message type (e.g. UTILMD).

Options for `audit-ahb`:
  --message-type <TYPE>   Limit audit to one message type (e.g. UTILMD)
  --output-json  <PATH>   Write machine-readable JSON report to a file
  --min-density  <N>      Fail if avg (seg+grp) rules/PID < N for any active profile
  --min-cond-rules <N>    Fail if total conditional_rules < N for any active profile
  extract-pdf         Extract MIG/AHB table data from a PDF (best-effort draft)
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

Exit codes:
  0  All checks passed / codegen succeeded
  1  One or more errors were found
";

mod audit_ahb;
mod codegen;
mod extract_pdf;
mod generate_fixtures;
mod import_codelists;
mod release_diff;
mod validate_profiles;
mod validate_pruefids;

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("audit-ahb") => audit_ahb(),
        Some("codegen") => codegen(),
        Some("validate-profiles") => validate_profiles(),
        Some("validate-pruefids") => validate_pruefids(),
        Some("release-diff") => release_diff(),
        Some("extract-pdf") => extract_pdf(),
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

fn audit_ahb() {
    let (workspace_root, _) = workspace_info();
    let args: Vec<String> = std::env::args().skip(2).collect();
    let ok = audit_ahb::run(&workspace_root, &args);
    if !ok {
        std::process::exit(1);
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
    let min_coverage_pct: u32 = args
        .windows(2)
        .find(|w| w[0] == "--min-coverage")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(0);
    let ok = validate_pruefids::run(
        &workspace_root,
        message_type_filter.as_deref(),
        strict,
        min_coverage_pct,
    );
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

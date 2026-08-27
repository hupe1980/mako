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
  validate-extraction Measure how well extract-pdf drafts reproduce the curated profiles
  validate-pruefids   Check that every AHB Pruefidentifikator has a test fixture
  validate-release-codes  Verify that every profile's release code appears in a UNH 0057 fixture
  audit-ahb           Comprehensive AHB rule-coverage analysis for all profiles
  check-bo4e-coverage  Count distinct rubo4e::current types used across crates/ and services/ and
                        verify the count matches the claim in README.md exactly. A tolerance
                        band would let types appear or vanish unnoticed.
  check-release-coverage  Fail when no profile covers the current (or --date) date
  check-prompt-tools  Refuse a procedure step naming a tool the agent cannot reach
  check-routes        Refuse axum 0.7 `/:param` route literals, which panic at startup
  check-bo4e-attributes Refuse a ZusatzAttribut that is not `mako:`-namespaced and registered
  check-bo4e-discriminants  Refuse a hand-written BO4E `_typ` — the discriminant is the type's
  check-bo4e-examples  Refuse a documented BO4E example using a field BO4E does not define
  check-malo-ids       Refuse a MaLo-ID literal whose BDEW check digit is wrong
  check-wire-timestamps  Refuse raw `time` values in JSON output (they serialise as component arrays)
                        under axum 0.8 (the fix is `/{param}`)
  check-answer-commands  Refuse an invoicd PID route naming a makod command that does not exist
  check-tool-grants   Verify every agentd manifest tool grant names a real MCP tool and
                        agrees with that server's own `read_only_hint`

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
mod check_answer_commands;
mod check_bo4e_attributes;
mod check_bo4e_discriminants;
mod check_bo4e_examples;
mod check_malo_ids;
mod check_prompt_tools;
mod check_release_coverage;
mod check_routes;
mod check_tool_grants;
mod check_wire_timestamps;
mod codegen;
mod extract_docx;
mod extract_pdf;
mod generate_fixtures;
mod import_codelists;
mod import_xml_profiles;
mod release_diff;
mod validate_extraction;
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
        Some("check-prompt-tools") => check_prompt_tools(),
        Some("check-routes") => check_routes(),
        Some("check-bo4e-attributes") => check_bo4e_attributes(),
        Some("check-bo4e-discriminants") => check_bo4e_discriminants(),
        Some("check-bo4e-examples") => check_bo4e_examples(),
        Some("check-malo-ids") => check_malo_ids(),
        Some("check-wire-timestamps") => check_wire_timestamps(),
        Some("check-answer-commands") => check_answer_commands(),
        Some("check-tool-grants") => check_tool_grants(),
        Some("codegen") => codegen(),
        Some("validate-extraction") => validate_extraction::validate_extraction(),
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

fn check_routes() {
    let (workspace_root, _) = workspace_info();
    if !check_routes::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_prompt_tools() {
    let (workspace_root, _) = workspace_info();
    if !check_prompt_tools::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_answer_commands() {
    let (workspace_root, _) = workspace_info();
    if !check_answer_commands::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_bo4e_discriminants() {
    let (workspace_root, _) = workspace_info();
    if !check_bo4e_discriminants::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_bo4e_examples() {
    let (workspace_root, _) = workspace_info();
    if !check_bo4e_examples::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_bo4e_attributes() {
    let (workspace_root, _) = workspace_info();
    if !check_bo4e_attributes::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_malo_ids() {
    let workspace_root =
        std::env::var("CARGO_MANIFEST_DIR").map_or_else(|_| ".".to_owned(), |d| format!("{d}/.."));
    if !check_malo_ids::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_wire_timestamps() {
    let (workspace_root, _) = workspace_info();
    if !check_wire_timestamps::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_tool_grants() {
    let (workspace_root, _) = workspace_info();
    if !check_tool_grants::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_bo4e_coverage() {
    // Count the distinct `rubo4e::current` types mako actually uses.
    //
    // Two forms count, because both are real usage:
    //
    //   use rubo4e::current::{Energiemenge, Lastgang};   // imported
    //   let v: Vec<rubo4e::current::Lastgang> = ...;     // fully-qualified inline
    //
    // Counting only the import form under-reports every type a service names
    // inline — `netzbilanzd` deserialises `Vec<rubo4e::current::Lastgang>`
    // without importing it.
    //
    // Comments are stripped first. A type named only in a doc comment (the
    // sibling line above is exactly that) is documentation, not usage, and
    // counting it would inflate the figure the README publishes.
    use std::collections::BTreeSet;

    let (root_str, _) = workspace_info();
    let root = std::path::Path::new(&root_str);
    let mut types: BTreeSet<String> = BTreeSet::new();

    let search_dirs = [root.join("crates"), root.join("services")];
    let mut rs_files: Vec<std::path::PathBuf> = Vec::new();
    for dir in &search_dirs {
        if let Ok(walker) = std::fs::read_dir(dir) {
            collect_rs_files(walker, &mut rs_files);
        }
    }

    for path in &rs_files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let code = strip_line_comments(&src);
        collect_rubo4e_types(&code, &mut types);
    }

    println!("rubo4e::current types found:");
    for t in &types {
        println!("  {t}");
    }
    let found = types.len();
    println!("\nTotal: {found} distinct rubo4e::current types");

    // The claim reads: **80 active `rubo4e::current` types — ...**
    let readme = std::fs::read_to_string(root.join("README.md")).unwrap_or_default();
    let claimed: usize = readme
        .lines()
        .find(|l| l.contains("rubo4e::current") && l.contains("types"))
        .and_then(|l| {
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
        eprintln!("ERROR: could not parse the claimed count from README.md.");
        std::process::exit(1);
    }
    // Exact, not a tolerance. A published count is either right or it is not,
    // and a ±2 band silently permits three types to appear or vanish.
    if found == claimed {
        println!("✓ README.md claim {claimed} matches.");
    } else {
        eprintln!("ERROR: found {found} types but README.md claims {claimed}.");
        eprintln!("Update the count in README.md and concepts/BO4E_COVERAGE.md.");
        std::process::exit(1);
    }
}

/// Remove `//` line comments, preserving anything inside a string literal.
fn strip_line_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let bytes = line.as_bytes();
        let mut in_str = false;
        let mut cut = line.len();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' if in_str => i += 1,
                b'"' => in_str = !in_str,
                b'/' if !in_str && i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                    cut = i;
                    break;
                }
                _ => {}
            }
            i += 1;
        }
        out.push_str(&line[..cut]);
        out.push('\n');
    }
    out
}

/// Accumulate every `rubo4e::current::…` type named in `code`, covering both the
/// braced-import form and fully-qualified inline paths.
fn collect_rubo4e_types(code: &str, types: &mut std::collections::BTreeSet<String>) {
    const PREFIX: &str = "rubo4e::current::";
    let mut rest = code;
    while let Some(idx) = rest.find(PREFIX) {
        rest = &rest[idx + PREFIX.len()..];
        let tail = rest.trim_start();
        if let Some(inner) = tail.strip_prefix('{') {
            // Braced group, possibly spanning lines: take everything to the `}`.
            let Some(close) = inner.find('}') else {
                continue;
            };
            for part in inner[..close].split(',') {
                let name = part.trim().split(" as ").next().unwrap_or("").trim();
                if is_type_ident(name) {
                    types.insert(name.to_owned());
                }
            }
        } else {
            let name: String = tail
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if is_type_ident(&name) {
                types.insert(name);
            }
        }
    }
}

fn is_type_ident(s: &str) -> bool {
    !s.is_empty() && s.chars().next().is_some_and(char::is_uppercase)
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

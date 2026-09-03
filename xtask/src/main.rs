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
  bump-version        Bump workspace version in root Cargo.toml (usage: bump-version X.Y.Z)
  validate-profiles   Validate the committed profiles: sources, dates, Prüfidentifikatoren
  sync-regulatories   Mirror and audit the BDEW document set behind the profiles
  validate-ebd-codes  Hold the mako-pruefung Antwortcode catalogue against the published EBD
  check-bo4e-coverage  Count distinct rubo4e::current types used across crates/ and services/ and
                        verify the count matches the claim in README.md exactly. A tolerance
                        band would let types appear or vanish unnoticed.
  check-release-coverage  Fail when no profile covers the current (or --date) date
  check-prompt-tools  Refuse a procedure step naming a tool the agent cannot reach
  check-routes        Refuse axum 0.7 `/:param` route literals, which panic at startup
  check-publish-order Refuse a crates.io publish order that precedes its own dependencies
  check-bo4e-attributes Refuse a ZusatzAttribut that is not `mako:`-namespaced and registered
  check-bo4e-discriminants  Refuse a hand-written BO4E `_typ` — the discriminant is the type's
  check-bo4e-examples  Refuse a documented BO4E example using a field BO4E does not define
  check-malo-ids       Refuse a MaLo-ID literal whose BDEW check digit is wrong
  check-business-dates   Refuse a business date read in UTC rather than in Europe/Berlin
  check-rounding         Refuse banker's rounding — money rounds kaufmännisch (DIN 1333)
  check-pid-coverage     Compare the shipped AHB profiles against the published
                        Prüfidentifikator inventory (no source documents needed)
  import-pid-overview    Extract that inventory from the BDEW Anwendungsübersicht .xlsx
  check-dep-versions     Documented dependency versions must match the manifests
  check-wire-timestamps  Refuse raw `time` values in JSON output (they serialise as component arrays)
                        under axum 0.8 (the fix is `/{param}`)
  check-answer-commands  Refuse an invoicd PID route naming a makod command that does not exist
  check-tool-grants   Verify every agentd manifest tool grant names a real MCP tool and
                        agrees with that server's own `read_only_hint`

  --date <YYYY-MM-DD>   Date to check against (default: today)

Exit codes:
  0  All checks passed / codegen succeeded
  1  One or more errors were found
";

mod bdew;
mod bump_version;
mod check_answer_commands;
mod check_bo4e_attributes;
mod check_bo4e_discriminants;
mod check_bo4e_examples;
mod check_business_dates;
mod check_dep_versions;
mod check_malo_ids;
mod check_prompt_tools;
mod check_publish_order;
mod check_release_coverage;
mod check_rounding;
mod check_routes;
mod check_tool_grants;
mod check_wire_timestamps;
mod import_profiles;
mod pid_overview;
mod sync_regulatories;
mod validate_ebd_codes;
mod validate_profiles;

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("bump-version") => bump_version(),
        Some("check-bo4e-coverage") => check_bo4e_coverage(),
        Some("check-release-coverage") => check_release_coverage::check_release_coverage(),
        Some("check-prompt-tools") => check_prompt_tools(),
        Some("check-routes") => check_routes(),
        Some("check-publish-order") => check_publish_order::check_publish_order(),
        Some("check-bo4e-attributes") => check_bo4e_attributes(),
        Some("check-bo4e-discriminants") => check_bo4e_discriminants(),
        Some("check-bo4e-examples") => check_bo4e_examples(),
        Some("check-malo-ids") => check_malo_ids(),
        Some("sync-regulatories") => sync_regulatories(),
        Some("check-business-dates") => check_business_dates(),
        Some("check-rounding") => check_rounding(),
        Some("check-pid-coverage") => check_pid_coverage(),
        Some("import-pid-overview") => import_pid_overview(),
        Some("check-dep-versions") => check_dep_versions(),
        Some("check-wire-timestamps") => check_wire_timestamps(),
        Some("check-answer-commands") => check_answer_commands(),
        Some("check-tool-grants") => check_tool_grants(),
        Some("validate-ebd-codes") => validate_ebd_codes(),
        Some("validate-profiles") => validate_profiles(),
        Some("import-profiles") => import_profiles(),
        Some("pdf-grid") => pdf_grid(),
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

/// `cargo xtask pdf-grid <pdf>` — print a BDEW PDF on the character grid the
/// profile importer reads, for inspecting an extraction problem.
fn pdf_grid() {
    let Some(path) = std::env::args().nth(2) else {
        eprintln!("usage: cargo xtask pdf-grid <pdf>");
        std::process::exit(1);
    };
    match bdew::pdf_lines(std::path::Path::new(&path)) {
        Ok(lines) => {
            for l in lines {
                println!("{l}");
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

fn import_profiles() {
    let (workspace_root, _) = workspace_info();
    let args: Vec<String> = std::env::args().skip(2).collect();
    if !import_profiles::run(&workspace_root, &args) {
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

fn check_business_dates() {
    let (workspace_root, _) = workspace_info();
    if !check_business_dates::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_rounding() {
    let (workspace_root, _) = workspace_info();
    if !check_rounding::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_pid_coverage() {
    let (workspace_root, _) = workspace_info();
    if !pid_overview::check(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn import_pid_overview() {
    let (workspace_root, _) = workspace_info();
    let Some(xlsx) = std::env::args().nth(2) else {
        eprintln!("usage: cargo xtask import-pid-overview <Anwendungsuebersicht.xlsx>");
        std::process::exit(2);
    };
    if let Err(e) = pid_overview::import(
        std::path::Path::new(&workspace_root),
        std::path::Path::new(&xlsx),
    ) {
        eprintln!("import-pid-overview: {e}");
        std::process::exit(1);
    }
}

fn check_dep_versions() {
    let (workspace_root, _) = workspace_info();
    if !check_dep_versions::run(std::path::Path::new(&workspace_root)) {
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

fn sync_regulatories() {
    let (workspace_root, _) = workspace_info();
    let args: Vec<String> = std::env::args().skip(2).collect();
    if !sync_regulatories::run(std::path::Path::new(&workspace_root), &args) {
        std::process::exit(1);
    }
}

fn validate_profiles() {
    let (workspace_root, _) = workspace_info();
    let ok = validate_profiles::run(&workspace_root);
    if !ok {
        std::process::exit(1);
    }
}

fn validate_ebd_codes() {
    let (workspace_root, _) = workspace_info();
    if !validate_ebd_codes::run(&workspace_root) {
        std::process::exit(1);
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

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

// Suppress lints that are impractical to fix in build-tool code.
#![deny(unsafe_code)]
#![expect(
    clippy::collapsible_if,
    reason = "a pedantic style lint this crate does not adopt"
)]
#![expect(
    clippy::too_many_lines,
    reason = "a pedantic style lint this crate does not adopt"
)]
#![expect(
    clippy::redundant_closure_for_method_calls,
    reason = "a pedantic style lint this crate does not adopt"
)]

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
  check-crate-lints   Every crate root denies `unsafe_code`
  check-db-suites     Every real-PostgreSQL suite is ignored and run by a recipe
  check-expected-tenant  A service reading `Claims` pins the tenant it expects
  check-vorlauf-consulted  Every catalogued Mindestvorlaufzeit is read by something
  check-behg-prices   Refuse a BEHG price table row for a year no enacted statute prices
  check-specs         Local: every spec's Status matches its tasks, and specs/ and
                        concepts/ROADMAP.md name each other (skips when they are absent)
  check-runner-disk   Refuse a workflow job that compiles the workspace without reclaiming
                        runner disk — ~14 GB does not fit, and it fails as a linker SIGBUS
  check-runner-routes No daemon claims a route `mako_service::run` already mounts
                              (`Router::merge` panics on those at startup)
  check-sql           Every service's SQL literals prepare against its own schema
                              (needs a Docker daemon)
  check-routes        Refuse axum 0.7 `/:param` route literals, which panic at startup
  check-publish-order Refuse a crates.io publish order that precedes its own dependencies
  check-bo4e-attributes Refuse a ZusatzAttribut that is not `mako:`-namespaced and registered
  check-request-bodies  Every `Json<T>` body denies unknown fields, and a BO4E document in
                        one is a `Bo4e<T>` rather than an ungated `serde_json::Value`
  check-bo4e-discriminants  Refuse a hand-written BO4E `_typ` — the discriminant is the type's
  check-bo4e-examples  Refuse a documented BO4E example using a field BO4E does not define
  check-malo-ids       Refuse a MaLo-ID literal whose BDEW check digit is wrong
  check-business-dates   Refuse a business date read in UTC rather than in Europe/Berlin
  check-outbox-notify    Hold each durable outbox's commit-coupled wake-up together: the
                        AFTER INSERT trigger, the channel it raises on, the worker's LISTEN
                        — and refuse an in-process notify on a producer path
  check-approval-commands  Refuse a processd approval-queue command with no answer contract —
                        a decision the queue cannot fill fails in makod and expires
  check-occupancy        Hold the count of ingest arms deciding resume-vs-spawn on index
                        presence alone (no occupancy verdict) — it may only fall
  check-atomic-spawn     Refuse a correlation key written outside its events' batch —
                        the duplicate guard finds a process only through that key
  check-command-constructors  Refuse a workflow command variant only a test constructs —
                        the states it leads to are unreachable in production
  check-workflow-interpolation  Refuse a `${{ }}` value interpolated into a `run:` script —
                        substituted as text before bash parses, so one quote executes the rest
  check-workflow-purity  Refuse a clock, random, environment or filesystem read inside an
                        `impl … Workflow for …` block — a workflow is a pure function of
                        (state, command), re-run on retry and re-folded on every replay
  check-event-payloads   Refuse `deny_unknown_fields` on a workflow event payload — an
                        event is read back years later, so a removed field must not make
                        a § 147 AO record unfoldable
  check-as4-controls     Refuse an AS4 control that is unreachable — the declared sign-and-encrypt
                        floor with no call site, or the unpinned base session reaching the
                        inbound pipeline (which refuses every signed message)
  check-tenant-predicate Refuse a statement against a tenant-scoped table that names neither
                        the tenant nor the table's own primary key — `tenant` is the isolation
                        key and Postgres does not supply it
  check-cedar-roles      Refuse a Cedar role literal no token can carry — `contains` is an
                        exact match on `mako_roles`, so a misspelled role is a disjunct
                        that is false for every caller and shows up only as a 403
  check-citations        Refuse a Festlegung cited in a form it does not publish
  check-controls-consulted  Every declared control — Frist constants, Meldepflicht rows,
                        credential fields, role columns, feature-gated security modules —
                        names where it is consulted, or why nothing does
  check-frist-literals   Refuse a Werktage or clock-time literal in a string that also names
                        a Festlegung, an AHB or an AWH — the Frist renders from mako_fristen
  check-known-wrong      Refuse a claim AGENTS.md lists as known-wrong, and a tracked file
                        pointing into the untracked roadmap or specifications
  check-rounding         Refuse banker's rounding — money rounds kaufmännisch (DIN 1333)
  check-pid-coverage     Compare the shipped AHB profiles against the published
                        Prüfidentifikator inventory (no source documents needed)
  import-pid-overview    Extract that inventory from the BDEW Anwendungsübersicht .xlsx
  check-dep-versions     Documented dependency versions must match the manifests
  check-action-pins      Every `uses:` pinned to a commit SHA with its tag in a comment, every
                        Dockerfile `FROM` to a digest, every workflow defaulting to `contents: read`,
                        every pip install hash-pinned and every npm install an `npm ci`
  check-vex              release/vex.json carries one OpenVEX statement per advisory deny.toml ignores,
                        each versioned subcomponent locked in Cargo.lock or fuzz/Cargo.lock
  check-ci-guards        Every `just ci` recipe that is one xtask call is run by a ci.yml job
  evidence-pack          Assemble (--inputs <dir> --out <file>) or verify (--check <file>) the
                        release evidence pack; a section without its input reads `not measured`
  check-licenses         The licence governance page must state deny.toml's allow list
  check-wire-timestamps  Refuse raw `time` values in JSON output (they serialise as component arrays)
                        under axum 0.8 (the fix is `/{param}`)
  check-answer-commands  Refuse an invoicd PID route naming a makod command that does not exist
  check-tool-grants   Verify every agentd manifest tool grant names a real MCP tool and
                        agrees with that server's own `read_only_hint`
  check-clippy-bans   Every clippy.toml ban fires on its planted use in xtask/clippy-fixtures
  dep-age             Report each direct dependency's pin against crates.io (network; a
                        report, never a failure; --recorded <file> reads a saved response)
  import-profiles     Regenerate the profiles from the mirrored BDEW documents
                        (--profile <type>/<fv>, --check to prove the committed files)
  profile-diff        Per Prüfidentifikator, what changed between two Formatversionen
                        (usage: profile-diff <type> <from-fv> <to-fv> [--pid <n>])
  pdf-grid            Print a BDEW PDF on the character grid the importer reads

  --date <YYYY-MM-DD>   Date to check against (default: today)

Exit codes:
  0  All checks passed
  1  One or more errors were found
";

#[cfg(test)]
mod allow_ratchet;
mod bdew;
mod bump_version;
mod check_action_pins;
mod check_answer_commands;
mod check_approval_commands;
mod check_as4_controls;
mod check_atomic_spawn;
mod check_behg_prices;
mod check_bo4e_attributes;
mod check_bo4e_discriminants;
mod check_bo4e_examples;
mod check_business_dates;
mod check_cedar_roles;
mod check_ci_guards;
mod check_citations;
mod check_clippy_bans;
mod check_command_constructors;
mod check_controls_consulted;
mod check_crate_lints;
mod check_db_suites;
mod check_dep_versions;
mod check_event_payloads;
mod check_expected_tenant;
mod check_frist_literals;
mod check_known_wrong;
mod check_licenses;
mod check_malo_ids;
mod check_occupancy;
mod check_outbox_notify;
mod check_prompt_tools;
mod check_publish_order;
mod check_release_coverage;
mod check_request_bodies;
mod check_rounding;
mod check_routes;
mod check_runner_disk;
mod check_runner_routes;
mod check_specs;
mod check_sql;
mod check_tenant_predicate;
mod check_tool_grants;
mod check_vex;
mod check_vorlauf_consulted;
mod check_wire_timestamps;
mod check_workflow_interpolation;
mod check_workflow_purity;
mod dep_age;
#[cfg(test)]
mod dev_configs;
mod evidence_pack;
mod import_profiles;
mod pid_overview;
mod profile_diff;
mod profile_expressions;
mod sync_regulatories;
#[cfg(test)]
mod test_layout;
mod validate_ebd_codes;
mod validate_profiles;

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("bump-version") => bump_version(),
        Some("check-bo4e-coverage") => check_bo4e_coverage(),
        Some("check-release-coverage") => check_release_coverage::check_release_coverage(),
        Some("check-prompt-tools") => check_prompt_tools(),
        Some("check-crate-lints") => check_crate_lints(),
        Some("check-db-suites") => check_db_suites(),
        Some("check-expected-tenant") => check_expected_tenant(),
        Some("check-vorlauf-consulted") => check_vorlauf_consulted(),
        Some("check-behg-prices") => check_behg_prices(),
        Some("check-specs") => check_specs(),
        Some("check-routes") => check_routes(),
        Some("check-runner-disk") => check_runner_disk(),
        Some("check-runner-routes") => check_runner_routes(),
        Some("check-sql") => check_sql(),
        Some("check-publish-order") => check_publish_order::check_publish_order(),
        Some("check-bo4e-attributes") => check_bo4e_attributes(),
        Some("check-request-bodies") => check_request_bodies(),
        Some("check-bo4e-discriminants") => check_bo4e_discriminants(),
        Some("check-bo4e-examples") => check_bo4e_examples(),
        Some("check-malo-ids") => check_malo_ids(),
        Some("sync-regulatories") => sync_regulatories(),
        Some("check-business-dates") => check_business_dates(),
        Some("check-citations") => check_citations(),
        Some("check-known-wrong") => check_known_wrong(),
        Some("check-frist-literals") => check_frist_literals(),
        Some("check-controls-consulted") => check_controls_consulted(),
        Some("check-rounding") => check_rounding(),
        Some("check-pid-coverage") => check_pid_coverage(),
        Some("import-pid-overview") => import_pid_overview(),
        Some("check-dep-versions") => check_dep_versions(),
        Some("dep-age") => {
            let (root, _) = workspace_info();
            let args: Vec<String> = std::env::args().skip(2).collect();
            if !dep_age::run(std::path::Path::new(&root), &args) {
                std::process::exit(1);
            }
        }
        Some("check-action-pins") => root_task(check_action_pins::run),
        Some("check-ci-guards") => root_task(check_ci_guards::run),
        Some("check-vex") => root_task(check_vex::run),
        Some("evidence-pack") => evidence_pack(),
        Some("check-licenses") => check_licenses(),
        Some("check-wire-timestamps") => check_wire_timestamps(),
        Some("check-outbox-notify") => check_outbox_notify(),
        Some("check-atomic-spawn") => check_atomic_spawn(),
        Some("check-command-constructors") => root_task(check_command_constructors::run),
        Some("check-approval-commands") => check_approval_commands(),
        Some("check-occupancy") => check_occupancy(),
        Some("check-workflow-interpolation") => check_workflow_interpolation(),
        Some("check-workflow-purity") => check_workflow_purity(),
        Some("check-event-payloads") => check_event_payloads(),
        Some("check-cedar-roles") => check_cedar_roles(),
        Some("check-tenant-predicate") => check_tenant_predicate(),
        Some("check-as4-controls") => check_as4_controls(),
        Some("check-answer-commands") => check_answer_commands(),
        Some("check-tool-grants") => check_tool_grants(),
        Some("check-clippy-bans") => root_task(check_clippy_bans::run),
        Some("validate-ebd-codes") => validate_ebd_codes(),
        Some("validate-profiles") => validate_profiles(),
        Some("import-profiles") => import_profiles(),
        Some("profile-diff") => profile_diff(),
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
                println!("{}", l.text);
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

fn profile_diff() {
    let (workspace_root, _) = workspace_info();
    let args: Vec<String> = std::env::args().skip(2).collect();
    if !profile_diff::run(&workspace_root, &args) {
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

fn check_crate_lints() {
    let (workspace_root, _) = workspace_info();
    if !check_crate_lints::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_db_suites() {
    let (workspace_root, _) = workspace_info();
    if !check_db_suites::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_expected_tenant() {
    let (workspace_root, _) = workspace_info();
    if !check_expected_tenant::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_vorlauf_consulted() {
    let (workspace_root, _) = workspace_info();
    if !check_vorlauf_consulted::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_behg_prices() {
    let (workspace_root, _) = workspace_info();
    if !check_behg_prices::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_specs() {
    let (workspace_root, _) = workspace_info();
    if !check_specs::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_occupancy() {
    let (workspace_root, _) = workspace_info();
    if !check_occupancy::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_approval_commands() {
    let (workspace_root, _) = workspace_info();
    if !check_approval_commands::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_atomic_spawn() {
    let (workspace_root, _) = workspace_info();
    if !check_atomic_spawn::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_outbox_notify() {
    let (workspace_root, _) = workspace_info();
    if !check_outbox_notify::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_workflow_interpolation() {
    let (workspace_root, _) = workspace_info();
    if !check_workflow_interpolation::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_workflow_purity() {
    let (workspace_root, _) = workspace_info();
    if !check_workflow_purity::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_event_payloads() {
    let (workspace_root, _) = workspace_info();
    if !check_event_payloads::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_cedar_roles() {
    let (workspace_root, _) = workspace_info();
    if !check_cedar_roles::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_tenant_predicate() {
    let (workspace_root, _) = workspace_info();
    if !check_tenant_predicate::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_as4_controls() {
    let (workspace_root, _) = workspace_info();
    if !check_as4_controls::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_routes() {
    let (workspace_root, _) = workspace_info();
    if !check_routes::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_runner_disk() {
    let (workspace_root, _) = workspace_info();
    if !check_runner_disk::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_runner_routes() {
    let (workspace_root, _) = workspace_info();
    if !check_runner_routes::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_sql() {
    let (workspace_root, _) = workspace_info();
    if !check_sql::run(std::path::Path::new(&workspace_root)) {
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

fn check_request_bodies() {
    let (workspace_root, _) = workspace_info();
    if !check_request_bodies::run(std::path::Path::new(&workspace_root)) {
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

fn check_citations() {
    let (workspace_root, _) = workspace_info();
    if !check_citations::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_controls_consulted() {
    let (workspace_root, _) = workspace_info();
    if !check_controls_consulted::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_frist_literals() {
    let (workspace_root, _) = workspace_info();
    if !check_frist_literals::run(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn check_known_wrong() {
    let (workspace_root, _) = workspace_info();
    if !check_known_wrong::run(std::path::Path::new(&workspace_root)) {
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

/// Run a guard that takes only the workspace root, exiting 1 when it refuses.
fn root_task(guard: fn(&std::path::Path) -> bool) {
    let (workspace_root, _) = workspace_info();
    if !guard(std::path::Path::new(&workspace_root)) {
        std::process::exit(1);
    }
}

fn evidence_pack() {
    let (workspace_root, _) = workspace_info();
    let args: Vec<String> = std::env::args().skip(2).collect();
    if !evidence_pack::run(std::path::Path::new(&workspace_root), &args) {
        std::process::exit(1);
    }
}

fn check_licenses() {
    let (workspace_root, _) = workspace_info();
    if !check_licenses::run(std::path::Path::new(&workspace_root)) {
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

    // The claim reads: **94 active `rubo4e::current` types — ...**
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
        eprintln!("Update the count in README.md.");
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

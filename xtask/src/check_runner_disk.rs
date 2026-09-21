//! Guard: a workflow job that compiles this workspace reclaims runner disk.
//!
//! A GitHub-hosted runner has roughly 14 GB free, and a debug build of this
//! workspace with `--all-features` does not fit beside Docker's layer store.
//! What makes it a guard rather than a note is how it fails: `No space left on
//! device` arrives mid-compile, and sometimes as a **linker SIGBUS** instead,
//! because `rust-lld` mmaps its output and a write to a full disk is not
//! reported as `ENOSPC`. Neither reads as "out of disk", so the job is debugged
//! as a code failure.
//!
//! The mitigation is one step that deletes the ~25 GB of CodeQL, Android,
//! dotnet, ghc and PowerShell a mako build never touches. It is three lines, it
//! is easy to add to the job in front of you, and that is exactly the problem:
//! it has twice been added to the jobs someone remembered and not to the rest.
//! `docker-builder` and `docker-bake` — the two that compile the whole
//! workspace *inside Docker* — were the pair left out.
//!
//! ## What counts as compiling the workspace
//!
//! A step that runs `cargo build`/`test`/`clippy`, or that drives a Docker build
//! of this repo (`docker/build-push-action`, `docker/bake-action`, a bare
//! `docker build`/`docker buildx bake`). A job whose Docker build only *pulls*
//! an image does not compile anything and is not in scope.
//!
//! ## Linux runners only
//!
//! The step is `sudo rm -rf /opt/hostedtoolcache/...`, which exists on the
//! hosted Ubuntu images. A job whose runner matrix includes macOS or Windows is
//! skipped rather than failed: `makotest-wheels` builds one extension module
//! across three operating systems, and a Linux-only step there would break the
//! other two for a footprint that has never come close to the limit.

use std::path::Path;

/// The step every in-scope job must carry.
const STEP: &str = "Reclaim runner disk";

/// Actions that build this repo's Dockerfile, whose builder stage compiles
/// every service in one `cargo build`.
const DOCKER_BUILDS: &[&str] = &["docker/build-push-action", "docker/bake-action"];

/// Commands that compile the **whole** workspace.
///
/// `cargo publish` walks 25 crates; `just ci` is the whole gate. Everything
/// else is recognised by scope rather than by name — see [`builds_workspace`] —
/// because what decides the footprint is `--workspace` / `--all-features`, not
/// which cargo subcommand carries it.
const WHOLE_WORKSPACE: &[&str] = &["cargo publish", "just ci", "maturin build"];

/// Does this line compile the whole workspace?
///
/// A `-p <crate>` command is scoped to one crate and does not approach the
/// limit — `cargo test -p edi-energy` across a feature matrix is the shape
/// ci.yml uses most, and demanding the step of it would be noise nobody reads.
/// `cargo xtask <cmd>` compiles one small binary, and `docker buildx imagetools
/// create` only rewrites a manifest, which is why a bare `docker build` marker
/// is absent: it is a prefix of `docker buildx`.
fn builds_workspace(line: &str) -> bool {
    if DOCKER_BUILDS.iter().any(|m| line.contains(m)) {
        return true;
    }
    if WHOLE_WORKSPACE.iter().any(|m| line.contains(m)) {
        return true;
    }
    let cargo = line.contains("cargo ") && !line.contains("cargo xtask");
    cargo
        && (line.contains("--workspace") || line.contains("--all-features"))
        && !line.contains("-p ")
}

/// Runner images the reclaim step does not apply to.
const NON_LINUX: &[&str] = &["macos", "windows"];

/// The lowest number of jobs a healthy scan may find.
///
/// The measured total across the workflow files, so a parser that stops seeing
/// jobs is refused rather than reporting a clean line for a file it never read.
const MIN_JOBS: usize = 20;

/// Run the check, returning `true` when every compiling job reclaims disk.
pub fn run(workspace_root: &Path) -> bool {
    let dir = workspace_root.join(".github/workflows");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("check-runner-disk: no workflows at {}", dir.display());
        return false;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "yml" || e == "yaml"))
        .collect();
    files.sort();

    let mut jobs = 0usize;
    let mut missing: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        for (name, body) in job_blocks(&src) {
            jobs += 1;
            // Comments carry commands too — ci.yml's `makotest` job explains
            // what `cargo test --workspace` does *not* cover — so a line whose
            // content starts a comment is not a step.
            let runs = body
                .lines()
                .filter(|l| !l.trim_start().starts_with('#'))
                .any(builds_workspace);
            if !runs {
                continue;
            }
            if NON_LINUX.iter().any(|os| body.contains(os)) {
                skipped.push(format!("{rel}  {name}"));
                continue;
            }
            if !body.contains(STEP) {
                missing.push(format!("{rel}  {name}"));
            }
        }
    }

    if jobs < MIN_JOBS {
        eprintln!(
            "check-runner-disk: found only {jobs} job(s) in {} workflow file(s) — at least \
             {MIN_JOBS} are expected. The parser has stopped matching the job shape; fix it \
             before trusting a green run.",
            files.len()
        );
        return false;
    }

    if missing.is_empty() {
        println!(
            "check-runner-disk: every workflow job that compiles the workspace reclaims \
             runner disk first ({jobs} job(s) scanned, {} skipped as non-Linux)",
            skipped.len()
        );
        return true;
    }

    eprintln!(
        "check-runner-disk: {} job(s) compile this workspace without a `{STEP}` step:",
        missing.len()
    );
    for m in &missing {
        eprintln!("  {m}");
    }
    eprintln!(
        "\nA hosted runner has ~14 GB free and this workspace does not fit beside Docker's \
         layer store. The failure arrives as `No space left on device` mid-compile, or as a \
         linker SIGBUS — neither of which reads as a disk problem. Add the step that deletes \
         the CodeQL, Android, dotnet, ghc and PowerShell trees, as the other jobs do."
    );
    false
}

/// Every `  <name>:` job block in a workflow's `jobs:` mapping, with its body.
///
/// Jobs sit at exactly two spaces of indentation under `jobs:`; a block ends at
/// the next line with that indentation or less. Line-based rather than a YAML
/// parse so `xtask` keeps no YAML dependency, and the shape it reads is fixed by
/// the formatter these files are kept in.
fn job_blocks(src: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut in_jobs = false;
    let mut i = 0usize;
    while i < lines.len() {
        let l = lines[i];
        if l.starts_with("jobs:") {
            in_jobs = true;
            i += 1;
            continue;
        }
        if in_jobs && !l.starts_with(' ') && !l.trim().is_empty() {
            in_jobs = false;
        }
        let is_job = in_jobs
            && l.starts_with("  ")
            && !l.starts_with("   ")
            && l.trim_end().ends_with(':')
            && !l.trim_start().starts_with('#');
        if !is_job {
            i += 1;
            continue;
        }
        let name = l.trim().trim_end_matches(':').to_owned();
        let start = i + 1;
        let mut end = start;
        while end < lines.len() {
            let b = lines[end];
            if !b.trim().is_empty() && !b.starts_with("    ") {
                break;
            }
            end += 1;
        }
        out.push((name, lines[start..end].join("\n")));
        i = end;
    }
    out
}

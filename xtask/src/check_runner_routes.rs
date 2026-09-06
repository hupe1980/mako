//! Guard: no daemon claims a route `mako_service::run` already mounts.
//!
//! The runner assembles the served router as
//! `ServiceBuilder::new().with_health(…).with_trace_layer().with_metrics().merge(D::build(…))`.
//! `Router::merge` **panics** when both sides carry the same method and path:
//!
//! ```text
//! Overlapping method route. Handler for `GET /metrics` already exists
//! ```
//!
//! That panic happens while the router is built — which for these daemons is
//! startup, before the listener binds. `accountingd` shipped it: its own router
//! registered `GET /metrics` for the ledger gauges, the runner had already
//! mounted `/metrics`, and the daemon could not boot. Nothing caught it,
//! because the handler tests build handlers rather than the assembled router.
//!
//! A service that wants its own metrics gives them their own path — `invoicd`
//! serves `/invoicd/metrics` and `obsd` `/obs/metrics`. This holds every
//! `run::<D>()` daemon to that.
//!
//! `makod` is exempt and named as such: it predates the runner, drives its own
//! `main`, and mounts `/metrics` and `/health/*` itself.

use std::path::{Path, PathBuf};

/// The paths `ServiceBuilder`'s infrastructure layers mount for every daemon.
///
/// Read from `mako-service` rather than listed here, so a route added to the
/// builder is covered without editing this file.
fn runner_routes(workspace_root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for file in ["builder.rs", "health.rs"] {
        let path = workspace_root.join("crates/mako-service/src").join(file);
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Comment lines out first, then scan the whole file: a `.route(` call
        // wraps onto the next line as soon as its handler is a closure, and a
        // line-at-a-time reader sees two of the four routes. Only the real
        // calls matter — the module docs show `/webhook` as an example of what
        // a *service* router carries.
        let code: String = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut rest = code.as_str();
        while let Some(at) = rest.find(".route(") {
            rest = &rest[at + ".route(".len()..];
            let arg = rest.trim_start();
            let Some(body) = arg.strip_prefix('"') else {
                continue;
            };
            let Some(end) = body.find('"') else {
                break;
            };
            out.push(body[..end].to_owned());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Daemons that drive their own `main` and so merge nothing from the runner.
const EXEMPT: &[&str] = &["makod"];

/// Check every runner-hosted daemon.
///
/// Returns `true` when no daemon router claims a runner route.
pub fn run(workspace_root: &Path) -> bool {
    let owned = runner_routes(workspace_root);
    if owned.is_empty() {
        eprintln!("check-runner-routes: read no routes out of mako-service — the guard is blind");
        return false;
    }

    let mut findings: Vec<String> = Vec::new();
    let mut checked = 0usize;
    let Ok(entries) = std::fs::read_dir(workspace_root.join("services")) else {
        eprintln!("check-runner-routes: no services directory");
        return false;
    };
    let mut services: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    services.sort();

    for dir in services {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if EXEMPT.contains(&name.as_str()) {
            continue;
        }
        let mut files = Vec::new();
        collect(&dir.join("src"), &mut files);
        files.sort();
        // Only daemons the runner assembles a router for.
        if !files.iter().any(|f| {
            std::fs::read_to_string(f)
                .map(|s| s.contains("mako_service::run::<"))
                .unwrap_or(false)
        }) {
            continue;
        }
        checked += 1;
        for file in &files {
            let Ok(src) = std::fs::read_to_string(file) else {
                continue;
            };
            for (n, line) in src.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                for route in &owned {
                    if line.contains(&format!(".route(\"{route}\"")) {
                        findings.push(format!(
                            "{}:{} — {name} registers `{route}`, which the runner mounts",
                            file.strip_prefix(workspace_root).unwrap_or(file).display(),
                            n + 1
                        ));
                    }
                }
            }
        }
    }

    if findings.is_empty() {
        println!(
            "check-runner-routes: {checked} runner-hosted daemon(s) leave the {} runner route(s) alone",
            owned.len()
        );
        return true;
    }
    eprintln!(
        "check-runner-routes: {} route collision(s) — `Router::merge` panics on these at startup:",
        findings.len()
    );
    for f in &findings {
        eprintln!("  {f}");
    }
    eprintln!(
        "\nGive the service's own handler its own path — `invoicd` serves\n\
         `/invoicd/metrics`, `obsd` serves `/obs/metrics`."
    );
    false
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

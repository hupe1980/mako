//! Guard: a service that reads `Claims` also pins the tenant it expects.
//!
//! `mako_service::oidc::Claims` refuses a token carrying no `mako_tenant`, but
//! whether that tenant is *this deployment's* is decided by an
//! `ExpectedTenant` extension the router installs. Without it, a token signed
//! by the same realm for a different operator extracts cleanly, and the only
//! thing standing between it and the data is that every Cedar rule remembered
//! to carry `context.principal_tenant == context.resource_tenant`.
//!
//! That is a condition repeated in hundreds of policy rules standing in for one
//! layer, and the failure mode is silent in both directions: a rule written
//! without the comparison authorises across tenants, and nothing about the
//! request looks wrong.
//!
//! The check is deliberately shallow — it asks whether the service names
//! `ExpectedTenant` at all, not where. Proving the layer reaches every route
//! needs the assembled router, which is each service's own
//! `authorization_guard.rs`. What this closes is the case of a service that
//! never installs it, which is the one that has actually happened.

use std::path::{Path, PathBuf};

/// Where the services live.
const SERVICES: &str = "services";

/// The extractor whose presence makes the pin necessary.
const CLAIMS: &str = "oidc::Claims";

/// The extension that supplies it.
const EXPECTED: &str = "ExpectedTenant";

/// Services that read `Claims` through a different mechanism and pin the tenant
/// their own way, with the reason each is exempt.
///
/// This list may only shrink: a service that starts naming `ExpectedTenant`
/// fails the check below until its entry goes, so an exemption cannot outlive
/// the reason for it.
const EXEMPT: &[(&str, &str)] = &[(
    "makod",
    "predates the runner and authenticates through `cedar_schema::BearerAuthenticator`, \
     which takes the expected tenant directly (`with_expected_tenant`, core/cedar_authz.rs)",
)];

/// Scan the services.
///
/// Returns `true` when every service reading `Claims` also pins a tenant.
pub fn run(workspace_root: &Path) -> bool {
    let root = workspace_root.join(SERVICES);
    let Ok(entries) = std::fs::read_dir(&root) else {
        eprintln!(
            "check-expected-tenant: {} is unreadable — the scan cannot decide",
            root.display()
        );
        return false;
    };

    let mut checked = 0usize;
    let mut missing: Vec<String> = Vec::new();
    let mut stale_exemptions: Vec<&str> = Vec::new();

    let mut services: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    services.sort();

    for dir in &services {
        let name = dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let src = concat_sources(&dir.join("src"));
        if !src.contains(CLAIMS) {
            continue;
        }
        checked += 1;
        let pins = src.contains(EXPECTED);
        let exempt = EXEMPT.iter().find(|(n, _)| *n == name);
        match (pins, exempt) {
            (false, None) => missing.push(name),
            (true, Some((n, _))) => stale_exemptions.push(n),
            _ => {}
        }
    }

    // A guard that scanned nothing reports nothing, and "all 0 service(s)"
    // reads exactly like a pass. Refuse instead.
    if checked == 0 {
        eprintln!(
            "check-expected-tenant: no service under {SERVICES}/ reads `{CLAIMS}` — \
             the layout has probably changed"
        );
        return false;
    }

    if missing.is_empty() && stale_exemptions.is_empty() {
        println!(
            "check-expected-tenant: all {checked} service(s) reading `{CLAIMS}` pin the tenant \
             they expect ({} documented exemption(s))",
            EXEMPT.len()
        );
        return true;
    }

    for name in &missing {
        eprintln!(
            "check-expected-tenant: `{name}` reads `{CLAIMS}` and never installs \
             `{EXPECTED}` — a token signed by the same realm for another operator \
             extracts cleanly there"
        );
    }
    for name in &stale_exemptions {
        eprintln!(
            "check-expected-tenant: `{name}` is listed as exempt and now names `{EXPECTED}` — \
             remove its entry from EXEMPT"
        );
    }
    if !missing.is_empty() {
        eprintln!(
            "\nAdd `.layer(Extension(mako_service::oidc::ExpectedTenant(<tenant>.clone())))` \
             beside the OIDC verifier's own layer."
        );
    }
    false
}

/// Every `.rs` file under `dir`, concatenated.
fn concat_sources(dir: &Path) -> String {
    let mut out = String::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.push_str(&concat_sources(&path));
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
            && let Ok(src) = std::fs::read_to_string(&path)
        {
            out.push_str(&src);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scan is over a tree; an empty one certifies nothing.
    #[test]
    fn refuses_a_tree_it_found_nothing_in() {
        assert!(!run(Path::new("/nonexistent-mako-root")));
    }

    /// Every exemption names a reason, so the list cannot grow silently.
    #[test]
    fn every_exemption_states_why() {
        for (name, reason) in EXEMPT {
            assert!(!name.is_empty());
            assert!(
                reason.len() > 30,
                "{name}'s exemption must say why, not merely that it is one"
            );
        }
    }
}

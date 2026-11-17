//! Guard: an `/api/v1` route is mounted through `utoipa-axum`'s
//! `OpenApiRouter`, never as a bare `.route("/api/v1…")`.
//!
//! A route registered with `routes!(handler)` carries its document entry; a
//! bare `.route(…)` serves a path the service's OpenAPI document does not
//! list, so an integrator's generated client cannot call it and nothing says
//! it exists. The served document is `mako_service::openapi::serve`'s, and the
//! committed `services/<svc>/openapi.json` is held to it by
//! `mako_service::openapi::assert_committed`.
//!
//! [`SEED`] is today's count of bare `/api/v1` routes per service, a
//! shrink-only ratchet: a service with more fails (a new route outside the
//! document), and one with fewer fails until its entry is lowered, so the list
//! never claims a route the tree no longer has. It is empty when every service
//! mounts its `/api/v1` routes through `OpenApiRouter`.
//!
//! Scanned: the production text (`#[cfg(test)]` items and comments blanked)
//! of `services/*/src/**/*.rs`, for `.route(` followed by a string literal
//! starting `"/api/v1`.

use std::collections::BTreeMap;
use std::path::Path;

/// Bare `/api/v1` routes per service, as the tree holds them.
const SEED: &[(&str, usize)] = &[("outputd", 15)];

/// The lowest number of bare routes a healthy scan of today's tree finds —
/// below it the pattern has stopped matching.
const MIN_ROUTES: usize = 100;

/// How many bare `/api/v1` routes `src` mounts.
pub fn bare_routes(src: &str) -> usize {
    let text = crate::check_error_tuples::production_text(src);
    let mut n = 0;
    let mut rest = text.as_str();
    while let Some(at) = rest.find(".route(") {
        rest = &rest[at + ".route(".len()..];
        if rest.trim_start().starts_with("\"/api/v1") {
            n += 1;
        }
    }
    n
}

/// Bare routes per service under `root/services`.
fn scan(root: &Path) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(root.join("services")) else {
        return out;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let mut files = Vec::new();
        crate::check_workflow_purity::walk_rs(&dir.join("src"), &mut files);
        // A `*_tests.rs` file is a `#[cfg(test)]` module its parent declares,
        // so none of its stand-in routes is served.
        let n: usize = files
            .iter()
            .filter(|f| !f.to_string_lossy().ends_with("_tests.rs"))
            .filter_map(|f| std::fs::read_to_string(f).ok())
            .map(|src| bare_routes(&src))
            .sum();
        if n > 0 {
            out.insert(name, n);
        }
    }
    out
}

/// What the scan says against `seed`.
fn findings(found: &BTreeMap<String, usize>, seed: &[(&str, usize)]) -> Vec<String> {
    let mut out = Vec::new();
    let seeded: BTreeMap<&str, usize> = seed.iter().copied().collect();
    for (svc, &n) in found {
        let allowed = seeded.get(svc.as_str()).copied().unwrap_or(0);
        if n > allowed {
            out.push(format!(
                "services/{svc} mounts {n} bare `.route(\"/api/v1…\")` (ratchet: {allowed}) — \
                 mount the new route through `OpenApiRouter::routes(routes!(handler))` with a \
                 `#[utoipa::path]`, so the OpenAPI document lists it"
            ));
        }
    }
    for (svc, allowed) in seed {
        let n = found.get(*svc).copied().unwrap_or(0);
        if n < *allowed {
            out.push(format!(
                "services/{svc} mounts {n} bare `/api/v1` route(s), the ratchet allows {allowed} \
                 — lower its entry in xtask/src/check_openapi_routes.rs to {n}"
            ));
        }
    }
    out
}

pub fn run(root: &Path) -> bool {
    let found = scan(root);
    let total: usize = found.values().sum();
    if total < MIN_ROUTES && SEED.iter().map(|(_, n)| n).sum::<usize>() >= MIN_ROUTES {
        eprintln!(
            "check-openapi-routes: found {total} bare /api/v1 route(s), at least {MIN_ROUTES} \
             expected while the ratchet holds more — the pattern has stopped matching"
        );
        return false;
    }
    let out = findings(&found, SEED);
    if out.is_empty() {
        println!(
            "check-openapi-routes: {total} bare /api/v1 route(s) in {} service(s), none beyond \
             the ratchet",
            found.len()
        );
        return true;
    }
    for f in &out {
        eprintln!("check-openapi-routes: {f}");
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_route_is_counted_and_a_documented_one_is_not() {
        let src = r#"
fn router() -> Router {
    OpenApiRouter::new()
        .routes(routes!(get_thing))
        .split_for_parts().0
        .route("/api/v1/things", post(create))
        .route(
            "/api/v1/things/{id}",
            delete(remove),
        )
        .route("/health/ready", get(ready))
}
#[cfg(test)]
mod tests {
    fn t() { Router::new().route("/api/v1/test-only", get(x)); }
}
"#;
        assert_eq!(bare_routes(src), 2);
    }

    /// A planted route beyond the ratchet fails, naming the service; a
    /// removed one fails until the entry is lowered.
    #[test]
    fn a_planted_route_fails_and_a_removed_one_lowers_the_seed() {
        let seed = [("vertragd", 2)];
        let mut found = BTreeMap::from([("vertragd".to_owned(), 2)]);
        assert!(findings(&found, &seed).is_empty());
        found.insert("vertragd".into(), 3);
        let planted = findings(&found, &seed);
        assert_eq!(planted.len(), 1);
        assert!(
            planted[0].contains("services/vertragd mounts 3"),
            "{planted:?}"
        );
        found.insert("newd".into(), 1);
        assert_eq!(
            findings(&found, &seed).len(),
            2,
            "an unseeded service has 0"
        );
        let lowered = findings(&BTreeMap::from([("vertragd".to_owned(), 1)]), &seed);
        assert!(lowered[0].contains("lower its entry"), "{lowered:?}");
    }

    /// Today's tree matches the ratchet exactly.
    #[test]
    fn the_real_tree_matches_the_seed() {
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
        let found = scan(root);
        assert!(
            findings(&found, SEED).is_empty(),
            "{:?}",
            findings(&found, SEED)
        );
    }
}

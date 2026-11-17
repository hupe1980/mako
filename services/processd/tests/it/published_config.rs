//! Guard: the „Full `processd.toml` reference" the operator guide prints is a
//! config `processd` accepts, and the demo's marktd delivers processd every
//! event type its modules read.
//!
//! marktd delivers to processd through processd's endpoint in its
//! `[[subscriptions.endpoints]]`. An `event_types` there that leaves out a type
//! processd reads means `de.markt.versorgung.gap-detected` is never delivered
//! and the §§ 36/38 EnWG Ersatz-/Grundversorgung module never runs, or a
//! `de.mako.process.completed` never closes a row of the process read model —
//! silently, because both services run exactly as configured.

use std::path::Path;

const GUIDE: &str = "site/content/docs/services/processd.md";
const HEADING: &str = "### Full `processd.toml` reference";
const MARKTD_DEMO: &str = "deploy/demo/marktd.toml";

fn repo(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is not readable: {e}", path.display()))
}

/// The first `toml` block after `heading` in `md`.
fn reference_block(md: &str, heading: &str) -> Option<String> {
    let after = &md[md.find(heading)? + heading.len()..];
    let open = after.find("```toml")?;
    let body = &after[open + "```toml".len()..];
    let close = body.find("\n```")?;
    Some(body[..close].to_owned())
}

/// The types processd reads that marktd's config, `toml`, does not deliver to
/// its endpoint named `processd`.
fn undelivered(toml: &str) -> Vec<String> {
    let cfg: toml::Value = toml::from_str(toml).expect("marktd's config parses");
    let endpoint = cfg
        .get("subscriptions")
        .and_then(|s| s.get("endpoints"))
        .and_then(toml::Value::as_array)
        .and_then(|es| {
            es.iter()
                .find(|e| e.get("name").and_then(toml::Value::as_str) == Some("processd"))
        });
    let Some(endpoint) = endpoint else {
        return vec!["marktd names no endpoint `processd`".to_owned()];
    };
    let types: Vec<&str> = endpoint
        .get("event_types")
        .and_then(toml::Value::as_array)
        .map(|ts| ts.iter().filter_map(toml::Value::as_str).collect())
        .unwrap_or_default();
    if types.is_empty() {
        return Vec::new();
    }
    processd::config::read_event_types()
        .into_iter()
        .filter(|t| !types.iter().any(|p| mako_events::matches(p, t)))
        .map(|t| format!("processd's endpoint leaves out {t}"))
        .collect()
}

#[test]
fn the_published_reference_config_parses() {
    let md = repo(GUIDE);
    let block = reference_block(&md, HEADING)
        .unwrap_or_else(|| panic!("{GUIDE} has no toml block under „{HEADING}“"));
    if let Err(e) = toml::from_str::<processd::config::Config>(&block) {
        panic!("{GUIDE}, „{HEADING}“: processd refuses the block: {e}");
    }
}

#[test]
fn the_demo_marktd_delivers_every_type_processd_reads() {
    let missing = undelivered(&repo(MARKTD_DEMO));
    assert!(missing.is_empty(), "{MARKTD_DEMO}: {missing:#?}");
}

#[test]
fn an_endpoint_that_narrows_the_types_is_refused() {
    let planted = "[[subscriptions.endpoints]]\nname = \"processd\"\n\
                   url = \"http://processd:8580/webhook\"\n\
                   event_types = [\"de.mako.process.initiated\"]\n";
    let missing = undelivered(planted);
    assert!(
        missing
            .iter()
            .any(|d| d.contains(mako_events::markt::VERSORGUNG_GAP_DETECTED)),
        "{missing:?}"
    );
}

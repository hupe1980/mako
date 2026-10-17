//! Guard: the „Full `processd.toml` reference" the operator guide prints is a
//! config `processd` accepts, and it keeps the subscription default it states.
//!
//! An operator copies the reference block whole. A line in it that narrows
//! `[subscription] event_types` to one type registers `processd` with `marktd`
//! for that type alone, so `de.markt.versorgung.gap-detected` is never
//! delivered and the §§ 36/38 EnWG Ersatz-/Grundversorgung module the same
//! page documents never runs, and a `de.mako.process.completed` never closes a
//! row of the process read model — silently, because the service is running
//! exactly as configured.

use std::path::Path;

const GUIDE: &str = "site/content/docs/services/processd.md";
const HEADING: &str = "### Full `processd.toml` reference";

/// The first `toml` block after `heading` in `md`.
fn reference_block(md: &str, heading: &str) -> Option<String> {
    let after = &md[md.find(heading)? + heading.len()..];
    let open = after.find("```toml")?;
    let body = &after[open + "```toml".len()..];
    let close = body.find("\n```")?;
    Some(body[..close].to_owned())
}

/// What is wrong with a reference block, if anything.
fn reference_defects(block: &str) -> Vec<String> {
    let cfg = match toml::from_str::<processd::config::Config>(block) {
        Ok(cfg) => cfg,
        Err(e) => return vec![format!("processd refuses the block: {e}")],
    };
    let subscribed = &cfg.subscription.event_types;
    processd::config::default_event_types()
        .into_iter()
        .filter(|t| !subscribed.contains(t))
        .map(|t| format!("`[subscription] event_types` leaves out {t}"))
        .collect()
}

#[test]
fn the_published_reference_config_parses_and_subscribes_to_every_default_type() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(GUIDE);
    let md = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is not readable: {e}", path.display()));
    let block = reference_block(&md, HEADING)
        .unwrap_or_else(|| panic!("{GUIDE} has no toml block under „{HEADING}“"));
    let defects = reference_defects(&block);
    assert!(
        defects.is_empty(),
        "{GUIDE}, „{HEADING}“:\n{}",
        defects.join("\n")
    );
}

#[test]
fn a_reference_that_narrows_the_subscription_is_refused() {
    let planted = "[http]\naddr = \"0.0.0.0:8580\"\n\
                   [database]\nurl = \"env:DATABASE_URL\"\n\
                   [identity]\nown_mp_id = \"9900357000004\"\n\
                   [makod]\nurl = \"http://makod:8080\"\napi_key = \"k\"\n\
                   [marktd]\nurl = \"http://marktd:8180\"\napi_key = \"k\"\n\
                   [subscription]\nevent_types = [\"de.mako.process.initiated\"]\n";
    let defects = reference_defects(planted);
    assert!(
        defects
            .iter()
            .any(|d| d.contains(mako_events::markt::VERSORGUNG_GAP_DETECTED)),
        "{defects:?}"
    );
}

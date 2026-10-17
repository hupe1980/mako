//! Guard: `dev/config/makod.toml` loads through `makod`'s own config loader.
//!
//! The dev config is what `just dev makod` starts the daemon with, so a key the
//! schema renamed or a value that no longer parses is a fresh clone that does
//! not start. `ConfigFile` denies unknown fields, so a stray key fails here
//! rather than at boot.

#[test]
fn the_dev_config_loads() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dev/config/makod.toml");
    let cfg = makod::config::load(&path).unwrap_or_else(|e| panic!("{e:#}"));
    assert!(
        cfg.party.as_ref().is_some_and(|p| !p.is_empty()),
        "makod refuses to start without a [[party]]"
    );
    let rollen = cfg
        .deployment
        .as_ref()
        .map(|d| d.rollen.clone())
        .unwrap_or_default();
    makod::deployment::validate_rollen(&rollen, cfg.party.as_deref().unwrap_or_default())
        .unwrap_or_else(|e| panic!("dev/config/makod.toml [deployment] rollen: {e:#}"));
}

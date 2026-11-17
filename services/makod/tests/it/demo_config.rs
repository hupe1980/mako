//! Guard: `deploy/demo/makod.toml` is a configuration `makod` starts with.
//!
//! The demo configuration is what `just dev` runs on the host and `just demo`
//! runs in Docker, and what an operator copies when writing their own.
//! `ConfigFile` denies unknown fields, so a key it does not declare is a
//! startup failure rather than a silent drop.

/// The demo configuration, from this crate's own location.
fn demo_config() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/demo/makod.toml")
}

/// The file loads, and its roles agree with its `[[party]]` table — startup
/// refuses a deployment that does not.
#[test]
fn the_demo_config_loads_with_roles_its_parties_hold() {
    let cfg = makod::config::load(&demo_config()).unwrap_or_else(|e| panic!("{e:#}"));
    assert!(
        cfg.party.as_ref().is_some_and(|p| !p.is_empty()),
        "makod refuses to start without a [[party]]"
    );
    let rollen = cfg
        .deployment
        .as_ref()
        .map(|d| d.rollen.clone())
        .unwrap_or_default();
    let parties = cfg.party.as_deref().unwrap_or_default();
    let (_, mut befunde) = makod::deployment::pruefe_rollen(&rollen, parties);
    befunde.extend(makod::party_registry::MpIdRegistry::pruefe(parties));
    befunde.extend(makod::einrichtung::codenummer_befunde(parties));
    assert!(
        befunde.is_empty(),
        "deploy/demo/makod.toml identities: {befunde:#?}"
    );
}

/// The file alone: a key `makod` does not declare is refused by name.
#[test]
fn the_demo_config_names_no_key_the_service_ignores() {
    let path = demo_config();
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is not readable: {e}", path.display()));
    if let Err(e) = toml::from_str::<makod::config::ConfigFile>(&src) {
        let msg = e.to_string();
        assert!(
            !msg.contains("unknown field"),
            "deploy/demo/makod.toml names a key `makod` does not have — fix the config, or the type:\n{msg}"
        );
    }
}

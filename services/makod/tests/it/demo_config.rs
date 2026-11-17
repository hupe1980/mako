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

/// Every service that sends makod commands in the demo presents its own key,
/// listed in `act_on_behalf`, so its commands count as the system's and not
/// as a person's — and every name `act_on_behalf` grants is such a key, which
/// the boot refuses otherwise.
#[test]
fn every_commanding_service_has_its_own_key_granted_act_on_behalf() {
    let read = |name: &str| -> toml::Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../deploy/demo/{name}"));
        toml::from_str(&std::fs::read_to_string(&path).expect("readable")).expect("toml")
    };
    let makod = read("makod.toml");
    let keys: Vec<(String, String)> = makod["http"]["auth_keys"]
        .as_array()
        .expect("auth_keys")
        .iter()
        .filter_map(|k| k.as_str()?.split_once('='))
        .map(|(n, t)| (n.to_owned(), t.to_owned()))
        .collect();
    let granted: Vec<&str> = makod["authz"]["act_on_behalf"]
        .as_array()
        .expect("act_on_behalf")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect();
    for name in &granted {
        assert!(keys.iter().any(|(n, _)| n == name), "{name} has no key");
    }
    for (svc, token) in [
        (
            "processd",
            read("processd.toml")["makod"]["api_key"].clone(),
        ),
        (
            "netzbilanzd",
            read("netzbilanzd.toml")["makod_api_key"].clone(),
        ),
        ("invoicd", read("invoicd.toml")["makod"]["api_key"].clone()),
        ("marktd", read("marktd.toml")["makod"]["api_key"].clone()),
    ] {
        let token = token
            .as_str()
            .unwrap_or_else(|| panic!("{svc}'s makod key"));
        let owner = keys
            .iter()
            .find(|(_, t)| t == token)
            .map(|(n, _)| n.as_str());
        assert_eq!(owner, Some(svc), "{svc} presents another's makod key");
        assert!(granted.contains(&svc), "{svc} is not granted act_on_behalf");
    }
}

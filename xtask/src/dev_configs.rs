//! Every service has a dev config and a test that loads it.
//!
//! `just dev <service>` starts a service with `dev/config/<service>.toml`, and
//! each service's `dev_config` test loads that file through its own config
//! type. A service added without either is one a fresh clone cannot start, and
//! nothing else notices: the recipe reads the directory, and the per-service
//! test only exists where someone wrote it.

use std::collections::BTreeSet;
use std::path::Path;

fn root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
}

fn names(dir: &Path, keep: impl Fn(&Path) -> Option<String>) -> BTreeSet<String> {
    std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| keep(&e.ok()?.path()))
        .collect()
}

/// Problems in a tree: a service without a dev config or without its test, and
/// a dev config naming no service.
fn problems(root: &Path) -> Vec<String> {
    let services = names(&root.join("services"), |p| {
        p.join("Cargo.toml")
            .is_file()
            .then(|| p.file_name()?.to_str().map(str::to_owned))?
    });
    let configs = names(&root.join("dev/config"), |p| {
        (p.extension()? == "toml").then(|| p.file_stem()?.to_str().map(str::to_owned))?
    });
    let mut out = Vec::new();
    for s in &services {
        if !configs.contains(s) {
            out.push(format!("services/{s} has no dev/config/{s}.toml"));
        }
        let tests = root.join("services").join(s).join("tests");
        if !tests.join("dev_config.rs").is_file() && !tests.join("it/dev_config.rs").is_file() {
            out.push(format!(
                "services/{s} has no dev_config test loading its dev config"
            ));
        }
    }
    for c in configs.difference(&services) {
        out.push(format!("dev/config/{c}.toml names no service"));
    }
    out
}

#[test]
fn every_service_has_a_dev_config_and_a_test_that_loads_it() {
    assert_eq!(problems(root()), Vec::<String>::new());
}

#[test]
fn a_service_without_a_dev_config_is_named() {
    let dir = std::env::temp_dir().join(format!("xtask-dev-configs-{}", std::process::id()));
    let svc = dir.join("services/newd");
    std::fs::create_dir_all(svc.join("tests")).unwrap();
    std::fs::create_dir_all(dir.join("dev/config")).unwrap();
    std::fs::write(svc.join("Cargo.toml"), "").unwrap();
    std::fs::write(dir.join("dev/config/oldd.toml"), "").unwrap();
    let found = problems(&dir);
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(
        found,
        vec![
            "services/newd has no dev/config/newd.toml".to_owned(),
            "services/newd has no dev_config test loading its dev config".to_owned(),
            "dev/config/oldd.toml names no service".to_owned(),
        ]
    );
}

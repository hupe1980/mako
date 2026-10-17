//! Every service has a dev config and a test that loads it, and every dev
//! config agrees with the one dev port table.
//!
//! `just dev <service>` starts a service with `dev/config/<service>.toml`, and
//! each service's `dev_config` test loads that file through its own config
//! type. A service added without either is one a fresh clone cannot start, and
//! nothing else notices: the recipe reads the directory, and the per-service
//! test only exists where someone wrote it.
//!
//! `dev/ports.toml` names each service's dev port. A config listens on
//! `[http] addr = "127.0.0.1:<port>"` — the one listen shape; a top-level
//! `port` is refused — and every `http://127.0.0.1:<port>` URL in it reaches
//! the service its key names (`marktd_url`, `[marktd] url`, `marktd = …`), or,
//! for a key that names none (`webhook_url`), some port the table holds. A port
//! copied by hand into the nine files that call a service is the drift this
//! stops: a dev stack that starts and does not talk.

use std::collections::{BTreeMap, BTreeSet};
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

/// `dev/ports.toml`: service → port, and the external processes' ports.
fn ports(root: &Path) -> (BTreeMap<String, u16>, BTreeSet<u16>) {
    let text = std::fs::read_to_string(root.join("dev/ports.toml")).expect("dev/ports.toml");
    let table: toml::Table = toml::from_str(&text).expect("dev/ports.toml parses");
    let read = |key: &str| -> BTreeMap<String, u16> {
        table
            .get(key)
            .and_then(toml::Value::as_table)
            .map(|t| {
                t.iter()
                    .map(|(k, v)| {
                        let port = v
                            .as_integer()
                            .and_then(|p| u16::try_from(p).ok())
                            .unwrap_or_else(|| panic!("dev/ports.toml: {key}.{k} is no port"));
                        (k.clone(), port)
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let services = read("services");
    let external = read("external").into_values().collect();
    (services, external)
}

/// The port of a `http(s)://127.0.0.1:<port>` or `localhost:<port>` URL.
fn local_port(url: &str) -> Option<u16> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let rest = rest
        .strip_prefix("127.0.0.1:")
        .or_else(|| rest.strip_prefix("localhost:"))?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Every string value in `v` with the key path that leads to it.
fn strings<'a>(v: &'a toml::Value, path: &mut Vec<String>, out: &mut Vec<(Vec<String>, &'a str)>) {
    match v {
        toml::Value::String(s) => out.push((path.clone(), s)),
        toml::Value::Table(t) => {
            for (k, v) in t {
                path.push(k.clone());
                strings(v, path, out);
                path.pop();
            }
        }
        toml::Value::Array(a) => {
            for v in a {
                strings(v, path, out);
            }
        }
        _ => {}
    }
}

/// What `config` (the text of `dev/config/<service>.toml`) says that disagrees
/// with the port table.
fn port_problems(
    service: &str,
    config: &str,
    services: &BTreeMap<String, u16>,
    external: &BTreeSet<u16>,
) -> Vec<String> {
    let file = format!("dev/config/{service}.toml");
    let mut out = Vec::new();
    let table: toml::Table = match toml::from_str(config) {
        Ok(t) => t,
        Err(e) => return vec![format!("{file}: does not parse: {e}")],
    };
    let Some(&own) = services.get(service) else {
        return vec![format!(
            "{file}: dev/ports.toml names no port for {service}"
        )];
    };
    if table.contains_key("port") {
        out.push(format!(
            "{file}: a top-level `port` — the listen address is `[http] addr`"
        ));
    }
    let addr = table
        .get("http")
        .and_then(|h| h.get("addr"))
        .and_then(toml::Value::as_str);
    let want = format!("127.0.0.1:{own}");
    if addr != Some(want.as_str()) {
        out.push(format!(
            "{file}: [http] addr is {addr:?}, dev/ports.toml says \"{want}\""
        ));
    }
    let mut found = Vec::new();
    let whole = toml::Value::Table(table);
    strings(&whole, &mut Vec::new(), &mut found);
    for (path, value) in found {
        let Some(port) = local_port(value) else {
            continue;
        };
        // The service a key names: `marktd_url`, `marktd` under a table of
        // servers, or the `[marktd]` table a `url` sits in.
        let named = path.iter().rev().take(2).find_map(|k| {
            let k = k.strip_suffix("_url").unwrap_or(k);
            services.get_key_value(k)
        });
        let key = path.join(".");
        match named {
            Some((name, &p)) if p != port => out.push(format!(
                "{file}: {key} = {value:?} names {name}, which dev/ports.toml puts on {p}"
            )),
            Some(_) => {}
            None if !services.values().any(|&p| p == port) && !external.contains(&port) => out
                .push(format!(
                    "{file}: {key} = {value:?} reaches port {port}, which dev/ports.toml does \
                     not hold"
                )),
            None => {}
        }
    }
    out
}

/// Every dev config against `dev/ports.toml`, and the table against the
/// services that exist.
fn all_port_problems(root: &Path) -> Vec<String> {
    let (services, external) = ports(root);
    let mut out = Vec::new();
    let dirs = names(&root.join("services"), |p| {
        p.join("Cargo.toml")
            .is_file()
            .then(|| p.file_name()?.to_str().map(str::to_owned))?
    });
    for s in &dirs {
        let path = root.join(format!("dev/config/{s}.toml"));
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue; // the missing config is `problems`' finding
        };
        out.extend(port_problems(s, &text, &services, &external));
    }
    for s in services.keys() {
        if !dirs.contains(s) {
            out.push(format!("dev/ports.toml names {s}, which is no service"));
        }
    }
    out
}

#[test]
fn every_dev_config_agrees_with_the_port_table() {
    assert_eq!(all_port_problems(root()), Vec::<String>::new());
}

/// Moving marktd in the table alone names every file that still says the old
/// port — its own listen address and each caller's URL.
#[test]
fn a_port_moved_in_the_table_names_every_file_that_disagrees() {
    let (mut services, external) = ports(root());
    services.insert("marktd".into(), 18180);
    let mut out = Vec::new();
    for s in services.keys() {
        if let Ok(text) = std::fs::read_to_string(root().join(format!("dev/config/{s}.toml"))) {
            out.extend(port_problems(s, &text, &services, &external));
        }
    }
    let files: BTreeSet<&str> = out.iter().filter_map(|f| f.split(':').next()).collect();
    assert!(files.contains("dev/config/marktd.toml"), "{out:#?}");
    for caller in [
        "edmd",
        "invoicd",
        "processd",
        "billingd",
        "netzbilanzd",
        "portald",
        "agentd",
        "makod",
    ] {
        assert!(
            files.contains(format!("dev/config/{caller}.toml").as_str()),
            "{caller} calls marktd and is not named: {out:#?}"
        );
    }
}

#[test]
fn the_other_listen_shape_is_refused() {
    let services = BTreeMap::from([("svc".to_owned(), 9000)]);
    let out = port_problems(
        "svc",
        "port = 9000\n[upstream]\nurl = \"http://127.0.0.1:9999\"\n",
        &services,
        &BTreeSet::new(),
    );
    assert_eq!(out.len(), 3, "{out:#?}");
    assert!(out[0].contains("top-level `port`"), "{out:#?}");
    assert!(out[1].contains("[http] addr is None"), "{out:#?}");
    assert!(out[2].contains("port 9999"), "{out:#?}");
    assert!(
        port_problems(
            "svc",
            "[http]\naddr = \"127.0.0.1:9000\"\n",
            &services,
            &BTreeSet::new()
        )
        .is_empty()
    );
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

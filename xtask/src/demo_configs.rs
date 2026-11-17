//! Every service has a demo configuration and a test that loads it, and every
//! demo configuration agrees with the one port table.
//!
//! `just dev` and `just demo` start each service with
//! `deploy/demo/<service>.toml`, and each service's `demo_config` test loads
//! that file through its own config type. A service added without either is
//! one the estate cannot start, and nothing else notices: the scripts read the
//! directory, and the per-service test only exists where someone wrote it.
//!
//! `deploy/ports.toml` names each service's port. A configuration listens on
//! `[http] addr = "0.0.0.0:<port>"` — the one listen shape; a top-level `port`
//! is refused — and reaches a peer as `http://<service>:<port>`, its compose
//! host name on that service's port (`just dev` moves the host to loopback).
//! A URL whose host is a service must use that service's port, and a key
//! that names a service (`marktd_url`, `[marktd] url`, `marktd = …`, or
//! `[[einrichtung.dienst]] name = "marktd"` beside `url`) must reach that
//! service. A loopback URL is a process outside the estate and must use a
//! port `[external]` holds; any other host is unknown. A port copied by hand
//! into the nine files that call a service is the drift this stops: an
//! estate that starts and does not talk.

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

/// Problems in a tree: a service without a demo configuration or without its
/// test, and a demo configuration naming no service.
fn problems(root: &Path) -> Vec<String> {
    let services = names(&root.join("services"), |p| {
        p.join("Cargo.toml")
            .is_file()
            .then(|| p.file_name()?.to_str().map(str::to_owned))?
    });
    let configs = names(&root.join("deploy/demo"), |p| {
        (p.extension()? == "toml").then(|| p.file_stem()?.to_str().map(str::to_owned))?
    });
    let mut out = Vec::new();
    for s in &services {
        if !configs.contains(s) {
            out.push(format!("services/{s} has no deploy/demo/{s}.toml"));
        }
        let test = root
            .join("services")
            .join(s)
            .join("tests/it/demo_config.rs");
        if !test.is_file() {
            out.push(format!(
                "services/{s} has no tests/it/demo_config.rs loading its demo configuration"
            ));
        }
    }
    for c in configs.difference(&services) {
        out.push(format!("deploy/demo/{c}.toml names no service"));
    }
    out
}

/// `deploy/ports.toml`: service → port, and the external processes' ports.
fn ports(root: &Path) -> (BTreeMap<String, u16>, BTreeSet<u16>) {
    let text = std::fs::read_to_string(root.join("deploy/ports.toml")).expect("deploy/ports.toml");
    let table: toml::Table = toml::from_str(&text).expect("deploy/ports.toml parses");
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
                            .unwrap_or_else(|| panic!("deploy/ports.toml: {key}.{k} is no port"));
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

/// The host and port of an `http(s)://<host>:<port>` URL.
fn host_port(url: &str) -> Option<(&str, u16)> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let (host, rest) = rest.split_once(':')?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some((host, digits.parse().ok()?))
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
                // A table in an array that names a service (`[[einrichtung.dienst]]
                // name = "marktd"`) names it for each URL beside the name.
                let named = v.get("name").and_then(toml::Value::as_str);
                if let Some(n) = named {
                    path.push(n.to_owned());
                }
                strings(v, path, out);
                if named.is_some() {
                    path.pop();
                }
            }
        }
        _ => {}
    }
}

/// What `config` (the text of `deploy/demo/<service>.toml`) says that
/// disagrees with the port table.
fn port_problems(
    service: &str,
    config: &str,
    services: &BTreeMap<String, u16>,
    external: &BTreeSet<u16>,
) -> Vec<String> {
    let file = format!("deploy/demo/{service}.toml");
    let mut out = Vec::new();
    let table: toml::Table = match toml::from_str(config) {
        Ok(t) => t,
        Err(e) => return vec![format!("{file}: does not parse: {e}")],
    };
    let Some(&own) = services.get(service) else {
        return vec![format!(
            "{file}: deploy/ports.toml names no port for {service}"
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
    let want = format!("0.0.0.0:{own}");
    if addr != Some(want.as_str()) {
        out.push(format!(
            "{file}: [http] addr is {addr:?}, deploy/ports.toml says \"{want}\""
        ));
    }
    let mut found = Vec::new();
    let whole = toml::Value::Table(table);
    strings(&whole, &mut Vec::new(), &mut found);
    for (path, value) in found {
        let Some((host, port)) = host_port(value) else {
            continue;
        };
        // The service a key names: `marktd_url`, `marktd` under a table of
        // servers, or the `[marktd]` table a `url`/`base_url` sits in. Any
        // other key in that table (`[marktd] webhook_url`, the caller's own
        // callback) names nothing.
        let mut rev = path.iter().rev();
        let last = rev.next().map_or("", String::as_str);
        let named = services
            .get_key_value(last.strip_suffix("_url").unwrap_or(last))
            .or_else(|| {
                matches!(last, "url" | "base_url")
                    .then(|| rev.next().and_then(|p| services.get_key_value(p.as_str())))
                    .flatten()
            });
        let key = path.join(".");
        if let Some((name, _)) = named
            && host != name
        {
            out.push(format!(
                "{file}: {key} = {value:?} names {name} and reaches host {host}"
            ));
            continue;
        }
        match services.get(host) {
            Some(&p) if p != port => out.push(format!(
                "{file}: {key} = {value:?} reaches {host} on {port}, which deploy/ports.toml \
                 puts on {p}"
            )),
            Some(_) => {}
            None if matches!(host, "127.0.0.1" | "localhost") => {
                if !external.contains(&port) {
                    out.push(format!(
                        "{file}: {key} = {value:?} reaches local port {port}, which \
                         deploy/ports.toml holds under no [external] process"
                    ));
                }
            }
            None => out.push(format!(
                "{file}: {key} = {value:?} reaches host {host}, which is no service \
                 deploy/ports.toml names"
            )),
        }
    }
    out
}

/// Every demo configuration against `deploy/ports.toml`, and the table
/// against the services that exist.
fn all_port_problems(root: &Path) -> Vec<String> {
    let (services, external) = ports(root);
    let mut out = Vec::new();
    let dirs = names(&root.join("services"), |p| {
        p.join("Cargo.toml")
            .is_file()
            .then(|| p.file_name()?.to_str().map(str::to_owned))?
    });
    for s in &dirs {
        let path = root.join(format!("deploy/demo/{s}.toml"));
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue; // the missing config is `problems`' finding
        };
        out.extend(port_problems(s, &text, &services, &external));
    }
    for s in services.keys() {
        if !dirs.contains(s) {
            out.push(format!("deploy/ports.toml names {s}, which is no service"));
        }
    }
    out
}

#[test]
fn every_demo_config_agrees_with_the_port_table() {
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
        if let Ok(text) = std::fs::read_to_string(root().join(format!("deploy/demo/{s}.toml"))) {
            out.extend(port_problems(s, &text, &services, &external));
        }
    }
    let files: BTreeSet<&str> = out.iter().filter_map(|f| f.split(':').next()).collect();
    assert!(files.contains("deploy/demo/marktd.toml"), "{out:#?}");
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
            files.contains(format!("deploy/demo/{caller}.toml").as_str()),
            "{caller} calls marktd and is not named: {out:#?}"
        );
    }
}

/// A daemon the checklist asks by `name` is reached on that daemon's host and
/// port.
#[test]
fn a_named_table_reaches_the_service_it_names() {
    let services = BTreeMap::from([("svc".to_owned(), 9000), ("marktd".to_owned(), 8180)]);
    let config = |host: &str, port: u16| {
        format!(
            "[http]\naddr = \"0.0.0.0:9000\"\n[[einrichtung.dienst]]\nname = \"marktd\"\n\
             url = \"http://{host}:{port}\"\n"
        )
    };
    assert!(port_problems("svc", &config("marktd", 8180), &services, &BTreeSet::new()).is_empty());
    let out = port_problems("svc", &config("marktd", 9000), &services, &BTreeSet::new());
    assert_eq!(out.len(), 1, "{out:#?}");
    assert!(out[0].contains("puts on 8180"), "{out:#?}");
    let out = port_problems("svc", &config("svc", 9000), &services, &BTreeSet::new());
    assert_eq!(out.len(), 1, "{out:#?}");
    assert!(
        out[0].contains("names marktd and reaches host svc"),
        "{out:#?}"
    );
}

/// Loopback is for a process outside the estate, and an unknown host is a typo.
#[test]
fn loopback_and_unknown_hosts_are_held_to_the_table() {
    let services = BTreeMap::from([("svc".to_owned(), 9000)]);
    let external = BTreeSet::from([11434]);
    let config = |url: &str| format!("[http]\naddr = \"0.0.0.0:9000\"\n[up]\nurl = \"{url}\"\n");
    assert!(
        port_problems(
            "svc",
            &config("http://127.0.0.1:11434"),
            &services,
            &external
        )
        .is_empty()
    );
    let out = port_problems(
        "svc",
        &config("http://127.0.0.1:9000"),
        &services,
        &external,
    );
    assert!(out.len() == 1 && out[0].contains("[external]"), "{out:#?}");
    let out = port_problems("svc", &config("http://marktdd:8180"), &services, &external);
    assert!(
        out.len() == 1 && out[0].contains("host marktdd"),
        "{out:#?}"
    );
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
    assert!(out[2].contains("local port 9999"), "{out:#?}");
    assert!(
        port_problems(
            "svc",
            "[http]\naddr = \"0.0.0.0:9000\"\n",
            &services,
            &BTreeSet::new()
        )
        .is_empty()
    );
    // The loopback listen address `just dev` writes is not the file's shape.
    let out = port_problems(
        "svc",
        "[http]\naddr = \"127.0.0.1:9000\"\n",
        &services,
        &BTreeSet::new(),
    );
    assert_eq!(out.len(), 1, "{out:#?}");
}

#[test]
fn every_service_has_a_demo_config_and_a_test_that_loads_it() {
    assert_eq!(problems(root()), Vec::<String>::new());
}

#[test]
fn a_service_without_a_demo_config_is_named() {
    let dir = std::env::temp_dir().join(format!("xtask-demo-configs-{}", std::process::id()));
    let svc = dir.join("services/newd");
    std::fs::create_dir_all(svc.join("tests/it")).unwrap();
    std::fs::create_dir_all(dir.join("deploy/demo")).unwrap();
    std::fs::write(svc.join("Cargo.toml"), "").unwrap();
    std::fs::write(dir.join("deploy/demo/oldd.toml"), "").unwrap();
    let found = problems(&dir);
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(
        found,
        vec![
            "services/newd has no deploy/demo/newd.toml".to_owned(),
            "services/newd has no tests/it/demo_config.rs loading its demo configuration"
                .to_owned(),
            "deploy/demo/oldd.toml names no service".to_owned(),
        ]
    );
}

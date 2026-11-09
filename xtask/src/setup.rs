//! `cargo xtask setup` — the deployment checklist in the terminal.
//!
//! Two sources, one fold (`mako_service::einrichtung::checkliste`, the one
//! makod serves at `GET /admin/einrichtung/deployment`):
//!
//! - `--berichte <dir>`: one `<daemon>.json` per daemon, each the stdout of
//!   `<daemon> --check-config --json`, with its stderr beside it as
//!   `<daemon>.err`. `just setup` writes them for every `dev/config/*.toml`.
//! - `--url <makod> [--token-env VAR]`: the checklist a running makod
//!   assembles from its `[[einrichtung.dienst]]`, with the bearer token read
//!   from the environment variable `VAR`.
//!
//! Exits non-zero while the deployment is not ready: an item is red, or a
//! daemon could not be asked.

use std::path::Path;

use mako_service::einrichtung::{Antwort, Checkliste, DienstBericht, checkliste, lies};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

/// The reports in `dir`, one per `<daemon>.json`, sorted by name.
fn aus_verzeichnis(dir: &Path) -> Result<Vec<DienstBericht>, String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension().is_some_and(|x| x == "json"))
                .then(|| p.file_stem()?.to_str().map(str::to_owned))
                .flatten()
        })
        .collect();
    names.sort();
    Ok(names
        .into_iter()
        .map(|name| {
            let roh = std::fs::read_to_string(dir.join(format!("{name}.json"))).unwrap_or_default();
            let err = std::fs::read_to_string(dir.join(format!("{name}.err"))).unwrap_or_default();
            let antwort = if roh.trim().is_empty() {
                Antwort::Unlesbar {
                    grund: "--check-config printed no report".to_owned(),
                    roh: err.trim().to_owned(),
                }
            } else {
                lies(&roh)
            };
            DienstBericht {
                dienst: name,
                url: String::new(),
                antwort,
            }
        })
        .collect())
}

fn von_makod(url: &str, token: Option<String>) -> Result<Checkliste, String> {
    let ziel = format!("{}/admin/einrichtung/deployment", url.trim_end_matches('/'));
    let mut req = reqwest::blocking::Client::new().get(&ziel);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = req.send().map_err(|e| format!("{ziel}: {e}"))?;
    let status = resp.status();
    let body = resp.text().map_err(|e| format!("{ziel}: {e}"))?;
    if !status.is_success() {
        return Err(format!("{ziel}: HTTP {status}: {body}"));
    }
    serde_json::from_str(&body).map_err(|e| format!("{ziel}: not a checklist ({e}): {body}"))
}

pub fn run(_root: &Path) -> bool {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let liste = if let Some(url) = arg(&args, "--url") {
        let token = arg(&args, "--token-env").and_then(|v| std::env::var(v).ok());
        von_makod(&url, token)
    } else if let Some(dir) = arg(&args, "--berichte") {
        aus_verzeichnis(Path::new(&dir)).map(checkliste)
    } else {
        Err(
            "usage: cargo xtask setup --berichte <dir> | --url <makod> [--token-env VAR]"
                .to_owned(),
        )
    };
    match liste {
        Ok(c) => {
            if args.iter().any(|a| a == "--json") {
                println!("{}", serde_json::to_string_pretty(&c).unwrap_or_default());
            } else {
                print!("{}", c.text());
            }
            c.bereit
        }
        Err(e) => {
            eprintln!("setup: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_service::befund::{Befund, Bereich};
    use mako_service::einrichtung::{Ampel, Einrichtung};
    use mako_service::policy::PolicyReport;

    /// The reports a daemon printed, folded: a blocking finding is red, a
    /// daemon that printed nothing is kept with its stderr, not dropped.
    #[test]
    fn the_reports_in_a_directory_fold_into_the_checklist() {
        let dir = std::env::temp_dir().join(format!("xtask-setup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rot = Einrichtung::neu(
            "processd",
            vec![
                Befund::blockiert("lf.bilanzkreise[0]", "missing field `sparte`")
                    .bereich(Bereich::Bilanzierung),
            ],
            PolicyReport::new(),
            vec![],
        );
        std::fs::write(
            dir.join("processd.json"),
            serde_json::to_string(&rot).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join("edmd.json"), "").unwrap();
        std::fs::write(dir.join("edmd.err"), "edmd config is invalid").unwrap();
        let berichte = aus_verzeichnis(&dir).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(berichte.len(), 2);
        let c = checkliste(berichte);
        assert!(!c.bereit);
        let bilanz = c
            .punkte
            .iter()
            .find(|p| p.bereich == Bereich::Bilanzierung)
            .unwrap();
        assert_eq!(bilanz.ampel, Ampel::Rot);
        assert!(c.text().contains("edmd config is invalid"), "{}", c.text());
    }
}

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
//!
//! `--tabelle` prints one line per daemon — its state and how many findings
//! block or warn — instead of the checklist (`just doctor`).
//!
//! `--vorschlag <answers>` prints instead the configuration the setup
//! interview's answers propose (`mako_service::einrichtung::vorschlag`, the
//! function behind makod's `POST /admin/einrichtung/vorschlag` and the Desk),
//! marked against the reports of the other source; `<answers>` is the
//! interview as TOML or JSON. With `--schreiben <dir>` each proposed file is
//! written there too. It exits non-zero while the proposal has a gap or a
//! refused answer. Nothing is written to a server.

use std::path::Path;

use mako_service::einrichtung::{
    Antwort, Antworten, Checkliste, DienstBericht, DienstStatus, checkliste, lies, vorschlag,
};

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

/// The interview's answers, as TOML or JSON by the file's extension.
fn antworten_aus(pfad: &Path) -> Result<Antworten, String> {
    let text = std::fs::read_to_string(pfad).map_err(|e| format!("{}: {e}", pfad.display()))?;
    if pfad.extension().is_some_and(|x| x == "json") {
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", pfad.display()))
    } else {
        toml::from_str(&text).map_err(|e| format!("{}: {e}", pfad.display()))
    }
}

/// One line per daemon: its state, version and how many findings block or warn.
fn tabelle(c: &Checkliste) -> String {
    use std::fmt::Write as _;
    let mut out = format!(
        "{:<14} {:<13} {:>9} {:>8}  {}\n",
        "Dienst", "Zustand", "blockiert", "Warnung", "Version"
    );
    for (zeile, bericht) in c.dienste.iter().zip(&c.berichte) {
        let (b, w) = match &bericht.antwort {
            Antwort::Erreicht { einrichtung } => {
                let b = einrichtung
                    .befunde
                    .iter()
                    .filter(|f| f.ist_blockiert())
                    .count();
                (b.to_string(), (einrichtung.befunde.len() - b).to_string())
            }
            _ => ("–".to_owned(), "–".to_owned()),
        };
        let zustand = match zeile.status {
            DienstStatus::Bereit => "bereit",
            DienstStatus::Blockiert => "blockiert",
            DienstStatus::Unerreichbar => "unerreichbar",
            DienstStatus::Unlesbar => "unlesbar",
        };
        let _ = writeln!(
            out,
            "{:<14} {:<13} {:>9} {:>8}  {}",
            zeile.dienst,
            zustand,
            b,
            w,
            zeile.version.as_deref().unwrap_or("")
        );
    }
    let _ = writeln!(
        out,
        "\n{}",
        if c.bereit {
            "Einrichtung bereit."
        } else {
            "Einrichtung nicht bereit — `just setup` zeigt jeden Befund."
        }
    );
    out
}

pub fn run(_root: &Path) -> bool {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let berichte: Result<Vec<DienstBericht>, String> = if let Some(url) = arg(&args, "--url") {
        let token = arg(&args, "--token-env").and_then(|v| std::env::var(v).ok());
        von_makod(&url, token).map(|c| c.berichte)
    } else if let Some(dir) = arg(&args, "--berichte") {
        aus_verzeichnis(Path::new(&dir))
    } else if args.iter().any(|a| a == "--vorschlag") {
        Ok(Vec::new())
    } else {
        Err(
            "usage: cargo xtask setup (--berichte <dir> | --url <makod> [--token-env VAR]) \
             [--tabelle | --json | --vorschlag <answers.toml> [--schreiben <dir>]]"
                .to_owned(),
        )
    };
    let berichte = match berichte {
        Ok(b) => b,
        Err(e) => {
            eprintln!("setup: {e}");
            return false;
        }
    };
    if let Some(datei) = arg(&args, "--vorschlag") {
        return match antworten_aus(Path::new(&datei)) {
            Ok(a) => {
                let v = vorschlag(&a, &berichte);
                vorschlag_zeigen(&v, arg(&args, "--schreiben").as_deref().map(Path::new))
            }
            Err(e) => {
                eprintln!("setup: {e}");
                false
            }
        };
    }
    let c = checkliste(berichte);
    if args.iter().any(|a| a == "--json") {
        println!("{}", serde_json::to_string_pretty(&c).unwrap_or_default());
    } else if args.iter().any(|a| a == "--tabelle") {
        print!("{}", tabelle(&c));
    } else {
        print!("{}", c.text());
    }
    c.bereit
}

/// Print each proposed file as a diff, then every refused answer; write the
/// files to `ziel` when given.
fn vorschlag_zeigen(v: &mako_service::einrichtung::Vorschlag, ziel: Option<&Path>) -> bool {
    for d in &v.dateien {
        println!("{}", d.diff);
        if let Some(dir) = ziel {
            let pfad = dir.join(&d.datei);
            if let Err(e) =
                std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&pfad, &d.toml))
            {
                eprintln!("setup: {}: {e}", pfad.display());
                return false;
            }
            println!("  → {}\n", pfad.display());
        }
    }
    for b in &v.abgelehnt {
        println!("abgelehnt  {b}");
    }
    println!(
        "{}",
        if v.vollstaendig {
            "Vorschlag vollständig.".to_owned()
        } else {
            format!(
                "Vorschlag unvollständig: {} Lücken, {} abgelehnte Antworten.",
                v.luecken,
                v.abgelehnt.len()
            )
        }
    );
    v.vollstaendig
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

    /// The interview's answers as an operator writes them in TOML read into
    /// the same answers the Desk sends, and the table names each daemon.
    #[test]
    fn answers_in_toml_propose_and_the_table_names_each_daemon() {
        let dir = std::env::temp_dir().join(format!("xtask-vorschlag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pfad = dir.join("antworten.toml");
        std::fs::write(
            &pfad,
            "verbundene_lieferanten = []\n\n[[rollen]]\nrolle = \"lf-strom\"\nmp_id = \"9900357000004\"\n\n\
             [[bilanzkreise]]\nsparte = \"strom\"\nstandard = [\"11XBK-STD------Y\"]\n",
        )
        .unwrap();
        let a = antworten_aus(&pfad).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        let v = vorschlag(&a, &[]);
        assert!(v.abgelehnt.is_empty(), "{:?}", v.abgelehnt);
        assert!(
            v.dateien[0].diff.contains("+mp_id = \"9900357000004\""),
            "{}",
            v.dateien[0].diff
        );
        // AS4 is unanswered: a gap, so the proposal is not complete.
        assert!(!vorschlag_zeigen(&v, None));

        let c = checkliste(vec![DienstBericht {
            dienst: "processd".into(),
            url: String::new(),
            antwort: Antwort::Erreicht {
                einrichtung: Einrichtung::neu(
                    "processd",
                    vec![Befund::blockiert("lf.bilanzkreise[0]", "x")],
                    PolicyReport::new(),
                    vec![],
                ),
            },
        }]);
        let t = tabelle(&c);
        assert!(
            t.lines()
                .nth(1)
                .is_some_and(|l| l.starts_with("processd") && l.contains("blockiert")),
            "{t}"
        );
    }
}

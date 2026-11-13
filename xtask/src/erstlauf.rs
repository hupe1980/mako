//! `cargo xtask erstlauf` — the first run, timed: from an empty deployment to a
//! green checklist and a passing Probelauf, scripted.
//!
//! The steps an operator takes, each timed and each checked:
//!
//! 1. **Leer** — makod's operating frame alone (port, key, volatile store, no
//!    AS4): `makod --check-config --json` must refuse it — no Rolle, no
//!    identity — so the run demonstrably starts from nothing.
//! 2. **Vorschlag** — the fixture interview „Lieferant Strom und Gas“
//!    answered, `mako_service::einrichtung::vorschlag` proposes makod.toml
//!    against the empty deployment's report, and the proposal is applied as
//!    the operator applies it: appended to the file.
//! 3. **Prüfung** — `makod --check-config --json` must report the deployment
//!    ready.
//! 4. **Start** — makod boots on that file until `/health/ready`.
//! 5. **Stammdaten** — the sample Marktlokation is put into makod's MaLo
//!    cache (`PUT /admin/malo/{id}`), as marktd's fan-out would in operation.
//! 6. **Probelauf** — `POST /api/v1/probe/gpke.lieferbeginn.anmelden` with
//!    the Desk's sample facts must pass the pre-send check.
//!
//! It prints each step's time and the total, and fails when a step fails or
//! the whole run exceeds the 30 minutes the setup guide promises a new
//! operator. Needs only a built makod (`cargo build -p makod`, which the
//! recipe runs) — no Docker, no database.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use mako_service::einrichtung::befragung::{
    Antworten, BilanzkreisAntwort, Rolle, RolleAntwort, Sparte,
};
use mako_service::einrichtung::{Antwort, DienstBericht, Einrichtung, vorschlag};

/// What a new operator is promised: a green checklist and a passing
/// Probelauf within half an hour.
const ZIEL: Duration = Duration::from_secs(30 * 60);

const SCHLUESSEL: &str = "erstlauf-schluessel";

/// The fixture interview: a Lieferant Strom und Gas, the published sample
/// codes the tests use.
fn antworten() -> Antworten {
    Antworten {
        rollen: vec![
            RolleAntwort {
                rolle: Rolle::LfStrom,
                mp_id: Some("9900357000004".to_owned()),
            },
            RolleAntwort {
                rolle: Rolle::LfGas,
                mp_id: Some("9800357000005".to_owned()),
            },
        ],
        primaer: Some("9900357000004".to_owned()),
        bilanzkreise: vec![BilanzkreisAntwort {
            sparte: Sparte::Strom,
            standard: vec!["11XBK-LF-------9".to_owned()],
        }],
        verbundene_lieferanten: Some(Vec::new()),
        as4: None,
        dienste: Vec::new(),
    }
}

/// makod's operating frame — what infrastructure provides, not what the
/// interview asks: the port, a key, a store that forgets, no AS4 transport.
fn rahmen(port: u16) -> String {
    format!(
        "[authz]\npermit_all = true\n\n[logging]\nlevel = \"warn\"\nformat = \"pretty\"\n\n\
         [storage]\nallow_volatile = true\n\n[http]\naddr = \"127.0.0.1:{port}\"\n\
         auth_keys = [\"erstlauf={SCHLUESSEL}\"]\n\n[as4]\nallow_no_signing = true\n"
    )
}

/// The canonical sample MaLo-ID.
const MALO: &str = "51238696012";

/// The sample Marktlokation as the MaLo-Ident answer carries it, under a
/// synthetic Netzbetreiber.
fn malo() -> serde_json::Value {
    serde_json::json!({
        "source": "erstlauf",
        "result": {
            "dataMarketLocation": {
                "maloId": MALO,
                "energyDirection": "consumption",
                "measurementTechnologyClassification": "conventionalMeasuringSystem",
                "optionalChangeForecastBasis": "notPossible",
                "dataMarketLocationProperties": [],
                "dataMarketLocationNetworkOperators": [
                    { "marketPartnerId": "9900001000001", "executionTimeFrom": "2000-01-01T00:00:00Z" }
                ],
                "dataMarketLocationTransmissionSystemOperators": []
            }
        }
    })
}

/// The Desk's sample Anmeldung (`desk/src/app/proben.ts`).
fn anmeldung() -> serde_json::Value {
    serde_json::json!({
        "marktrolle": "LF",
        "payload": {
            "malo_id": MALO,
            "lieferbeginn_datum": "2027-01-01",
            "transaktionsgrund": "E03",
            "bilanzkreis": "11XBK-LF-------9",
            "haushaltskunde": true,
            "enfg_grundlage": "ZG1",
            "kunde": {
                "vorname": "Erika",
                "nachname": "Mustermann",
                "anschrift": {
                    "strasse": "Hauptstraße",
                    "hausnummer": "1",
                    "postleitzahl": "50667",
                    "ort": "Köln",
                    "landescode": "DE"
                }
            }
        }
    })
}

fn makod(root: &Path) -> PathBuf {
    let target =
        std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    target.join("debug").join("makod")
}

/// `makod --check-config --json` on `config`: its report.
fn pruefen(bin: &Path, config: &Path) -> Result<Einrichtung, String> {
    let out = Command::new(bin)
        .arg("--config")
        .arg(config)
        .args(["--check-config", "--json"])
        .env_remove("MAKOD_CONFIG")
        .output()
        .map_err(|e| format!("{}: {e}", bin.display()))?;
    serde_json::from_slice(&out.stdout).map_err(|e| {
        format!(
            "--check-config printed no report ({e}): {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn freier_port() -> Result<u16, String> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| e.to_string())
}

struct Lauf {
    schritte: Vec<(&'static str, Duration, String)>,
    start: Instant,
}

impl Lauf {
    fn schritt<T>(
        &mut self,
        name: &'static str,
        f: impl FnOnce() -> Result<(T, String), String>,
    ) -> Result<T, String> {
        let t = Instant::now();
        let (wert, was) = f().map_err(|e| format!("{name}: {e}"))?;
        self.schritte.push((name, t.elapsed(), was));
        Ok(wert)
    }
}

fn erstlauf(root: &Path) -> Result<Lauf, String> {
    let bin = makod(root);
    if !bin.exists() {
        return Err(format!(
            "{} is not built — `cargo build -p makod`",
            bin.display()
        ));
    }
    let dir = std::env::temp_dir().join(format!("mako-erstlauf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let config = dir.join("makod.toml");
    let port = freier_port()?;
    let mut lauf = Lauf {
        schritte: Vec::new(),
        start: Instant::now(),
    };

    let leer = lauf.schritt("Leer", || {
        std::fs::write(&config, rahmen(port)).map_err(|e| e.to_string())?;
        let e = pruefen(&bin, &config)?;
        if e.bereit {
            return Err("an empty deployment reports ready — the run starts from nothing".into());
        }
        let n = e.befunde.iter().filter(|b| b.ist_blockiert()).count();
        Ok((e, format!("{n} blockierende Befunde")))
    })?;

    lauf.schritt("Vorschlag", || {
        let geladen = [DienstBericht {
            dienst: "makod".to_owned(),
            url: String::new(),
            antwort: Antwort::Erreicht { einrichtung: leer },
        }];
        let v = vorschlag(&antworten(), &geladen);
        if !v.abgelehnt.is_empty() {
            return Err(format!("answers refused: {:?}", v.abgelehnt));
        }
        let datei = v
            .dateien
            .iter()
            .find(|d| d.dienst == "makod")
            .ok_or("no makod.toml proposed")?;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&config)
            .map_err(|e| e.to_string())?;
        writeln!(f, "\n{}", datei.toml).map_err(|e| e.to_string())?;
        Ok((
            (),
            format!("{} Zeilen neu, {} Lücken", datei.aenderungen, datei.luecken),
        ))
    })?;

    lauf.schritt("Prüfung", || {
        let e = pruefen(&bin, &config)?;
        if !e.bereit {
            return Err(format!("not ready:\n{}", e.text()));
        }
        Ok(((), format!("bereit, {} Warnungen", e.befunde.len())))
    })?;

    let mut kind = Command::new(&bin)
        .arg("--config")
        .arg(&config)
        .env_remove("MAKOD_CONFIG")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let basis = format!("http://127.0.0.1:{port}");
    let http = reqwest::blocking::Client::new();
    let ergebnis = (|| {
        lauf.schritt("Start", || {
            let bis = Instant::now() + Duration::from_secs(120);
            while Instant::now() < bis {
                if let Ok(r) = http.get(format!("{basis}/health/ready")).send()
                    && r.status().is_success()
                {
                    return Ok(((), "bereit".to_owned()));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err("makod was not ready within 2 minutes".to_owned())
        })?;
        lauf.schritt("Stammdaten", || {
            let r = http
                .put(format!("{basis}/admin/malo/{MALO}"))
                .bearer_auth(SCHLUESSEL)
                .json(&malo())
                .send()
                .map_err(|e| e.to_string())?;
            if !r.status().is_success() {
                return Err(format!(
                    "HTTP {}: {}",
                    r.status(),
                    r.text().unwrap_or_default()
                ));
            }
            Ok(((), format!("MaLo {MALO} im Cache")))
        })?;
        lauf.schritt("Probelauf", || {
            let r = http
                .post(format!("{basis}/api/v1/probe/gpke.lieferbeginn.anmelden"))
                .bearer_auth(SCHLUESSEL)
                .json(&anmeldung())
                .send()
                .map_err(|e| e.to_string())?;
            let status = r.status();
            let body: serde_json::Value = r.json().map_err(|e| e.to_string())?;
            if !status.is_success() || body["urteil"] != "besteht" {
                return Err(format!("HTTP {status}: {body:#}"));
            }
            Ok(((), "besteht — nichts gesendet oder gespeichert".to_owned()))
        })
    })();
    let _ = kind.kill();
    let _ = kind.wait();
    std::fs::remove_dir_all(&dir).ok();
    ergebnis.map(|()| lauf)
}

pub fn run(root: &Path) -> bool {
    match erstlauf(root) {
        Ok(lauf) => {
            println!("Erstlauf: Lieferant Strom und Gas, von leer bis Probelauf\n");
            for (name, dauer, was) in &lauf.schritte {
                println!("  {name:<10} {:>8.2} s  {was}", dauer.as_secs_f64());
            }
            let gesamt = lauf.start.elapsed();
            println!(
                "\n  {:<10} {:>8.2} s  (Ziel: unter {} Minuten)",
                "Gesamt",
                gesamt.as_secs_f64(),
                ZIEL.as_secs() / 60
            );
            gesamt <= ZIEL
        }
        Err(e) => {
            eprintln!("erstlauf: {e}");
            false
        }
    }
}

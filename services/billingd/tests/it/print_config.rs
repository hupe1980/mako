//! `billingd --print-config` runs the checks boot runs.
//!
//! The runner calls `Daemon::validate` from `--print-config` and before
//! `build`, so a config `--print-config` accepts is one the daemon starts on.
//! These run the real binary against a temporary config file.

use std::process::Command;

fn print_config(toml: &str) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!(
        "billingd-print-config-{}-{}",
        std::process::id(),
        toml.len()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("billingd.toml");
    std::fs::write(&file, toml).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_billingd"))
        .arg("--print-config")
        .env("BILLINGD_CONFIG", &file)
        .env_remove("BILLINGD_DATABASE__URL")
        .output()
        .expect("run billingd --print-config");
    std::fs::remove_dir_all(&dir).ok();
    out
}

const BASE: &str = r#"
tenant = "9900357000004"
productd_url = "http://localhost:9080"
edmd_url = "http://localhost:8380"
marktd_url = "http://localhost:8080"
seller_vat_id = "DE123456789"
allow_insecure_no_auth = true

[database]
url = "postgres://localhost/billingd"
"#;

/// An overlapping supplied `[rates]` row is refused by `--print-config`, as
/// at boot, naming the overlap.
#[test]
fn print_config_refuses_an_overlapping_rates_row() {
    let toml = format!(
        "{BASE}
[rates]
stromsteuer_ct_per_kwh = [
  {{ von = \"2031-01-01\", bis = \"2031-06-30\", wert = 2.05, quelle = \"StromStG § 3 (Fixture)\" }},
  {{ von = \"2031-06-01\", bis = \"2031-12-31\", wert = 2.05, quelle = \"StromStG § 3 (Fixture)\" }},
]
"
    );
    let out = print_config(&toml);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "accepted: {stderr}");
    assert!(stderr.contains("overlaps"), "{stderr}");
}

/// A valid config prints the policy it runs under, each value with its origin
/// and source.
#[test]
fn print_config_prints_the_policy_report() {
    let out = print_config(BASE);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout
            .contains("# billingd.behg_regime=rvo_bezugspreis origin=default class=policy quelle="),
        "{stdout}"
    );
    assert!(
        stdout.contains("# billingd.settlement_form=endrechnung origin=default"),
        "{stdout}"
    );
}

/// The removed `[rates] behg_regime` is refused naming `[policy] behg_regime`.
#[test]
fn the_removed_rates_key_is_refused_naming_the_policy_key() {
    let out = print_config(&format!("{BASE}\n[rates]\nbehg_regime = \"korridor\"\n"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "accepted: {stderr}");
    assert!(stderr.contains("[policy] behg_regime"), "{stderr}");
}

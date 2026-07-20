//! `makod --check` exit-code contract.
//!
//! Deployment pipelines gate rollouts on this exit code: 0 must mean "all
//! startup validations passed", non-zero must mean "any failure" — a pipeline
//! that misreads it deploys a broken configuration.

use std::process::Command;

fn makod() -> Command {
    Command::new(env!("CARGO_BIN_EXE_makod"))
}

fn write_config(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("makod.toml");
    std::fs::write(&path, body).expect("write config");
    path
}

/// A valid volatile config passes `--check` with exit code 0.
#[test]
fn check_exits_zero_on_valid_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = write_config(
        dir.path(),
        r#"
[[party]]
mp_id = "9900001000001"
roles = ["NB"]
primary = true
"#,
    );
    let out = makod()
        .args(["--config"])
        .arg(&cfg)
        .args(["--allow-volatile", "--check"])
        .output()
        .expect("spawn makod");
    assert!(
        out.status.success(),
        "--check must exit 0 on a valid config.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// A §2.13 violation (mixed Strom+Gas roles in one [[party]] entry) fails
/// `--check` with a non-zero exit code and names the violation.
#[test]
fn check_exits_nonzero_on_mixed_sparte_party() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = write_config(
        dir.path(),
        r#"
[[party]]
mp_id = "9900001000001"
roles = ["NB", "GNB"]
primary = true
"#,
    );
    let out = makod()
        .args(["--config"])
        .arg(&cfg)
        .args(["--allow-volatile", "--check"])
        .output()
        .expect("spawn makod");
    assert!(
        !out.status.success(),
        "--check must exit non-zero on a mixed Strom+Gas party entry"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("2.13"), "error names §2.13: {stderr}");
}

/// Without `--allow-volatile` and without `--data-dir`, the daemon refuses to
/// start — volatile storage cannot meet §22 MessZV durability.
#[test]
fn refuses_volatile_storage_without_explicit_flag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = write_config(
        dir.path(),
        r#"
[[party]]
mp_id = "9900001000001"
roles = ["NB"]
primary = true
"#,
    );
    let out = makod()
        .args(["--config"])
        .arg(&cfg)
        .args(["--check"])
        .output()
        .expect("spawn makod");
    assert!(!out.status.success(), "volatile mode must be refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("volatile") || stderr.contains("--data-dir"),
        "error explains the volatile refusal: {stderr}"
    );
}

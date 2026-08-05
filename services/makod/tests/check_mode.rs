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
/// start — volatile storage cannot meet § 147 AO / GoBD durability.
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

/// `--check` must fail a config that enables an authenticated port without
/// credentials — the same way the real boot fails it.
///
/// This ordering was wrong once. The credential guard sat *below* the `--check`
/// early exit, so `--check` reported "all startup validations passed" (exit 0)
/// for a config whose real start then bailed with "--auth-key … is required".
/// A pipeline gating rollout on the exit code promoted it.
#[test]
fn check_rejects_an_authenticated_port_without_credentials() {
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
        .args([
            "--allow-volatile",
            "--api-webdienste-addr",
            "127.0.0.1:18090",
            "--check",
        ])
        .output()
        .expect("spawn makod");
    assert!(
        !out.status.success(),
        "--check must reject :8090 without --auth-key or --oidc-issuer.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--auth-key") || stderr.contains("--oidc-issuer"),
        "the error must name the flag that fixes it, got: {stderr}"
    );
}

/// The same port *with* credentials passes, so the test above is about the
/// missing credential and not merely about the flag being present.
#[test]
fn check_accepts_an_authenticated_port_with_credentials() {
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
        .args([
            "--allow-volatile",
            "--api-webdienste-addr",
            "127.0.0.1:18090",
            "--auth-key",
            "erp-prod=0123456789abcdef0123456789abcdef",
            "--check",
        ])
        .output()
        .expect("spawn makod");
    assert!(
        out.status.success(),
        "--check must accept :8090 once a named key is configured.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

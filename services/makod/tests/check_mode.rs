//! `makod --check` exit-code contract.
//!
//! Deployment pipelines gate rollouts on this exit code: 0 must mean "all
//! startup validations passed", non-zero must mean "any failure" — a pipeline
//! that misreads it deploys a broken configuration.

use std::process::Command;

fn makod() -> Command {
    Command::new(env!("CARGO_BIN_EXE_makod"))
}

/// The flags a startable minimal configuration needs beyond `[[party]]`.
///
/// An ingest transport, a credential for it, and an acknowledgement that
/// outbound EDIFACT has nowhere to go. Each is a real requirement of the
/// running daemon, so `--check` demands them too.
const STARTABLE: &[&str] = &[
    "--allow-volatile",
    "--http-addr",
    "127.0.0.1:18080",
    "--auth-key",
    "erp-prod=0123456789abcdef0123456789abcdef",
    "--allow-no-as4-signing",
];

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
        .args(STARTABLE)
        .arg("--check")
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
        .args(STARTABLE)
        .arg("--check")
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
        .args([
            "--http-addr",
            "127.0.0.1:18080",
            "--auth-key",
            "erp-prod=0123456789abcdef0123456789abcdef",
            "--allow-no-as4-signing",
            "--check",
        ])
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
            "--http-addr",
            "127.0.0.1:18080",
            "--allow-no-as4-signing",
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
        .args(STARTABLE)
        .args(["--api-webdienste-addr", "127.0.0.1:18090", "--check"])
        .output()
        .expect("spawn makod");
    assert!(
        out.status.success(),
        "--check must accept :8090 once a named key is configured.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

// ── Preflight: configuration errors the check used to miss ───────────────────
//
// Each case below exited 0 under `--check` and then killed the real boot. The
// unit tests in `core::preflight` cover the rules; these cover the contract the
// pipeline actually reads — the process exit code.

/// A registered AS4 partner with no encryption certificate cannot deliver a
/// single message (BDEW AS4-Profil v1.2 §2.2.6.2.2).
#[test]
fn check_rejects_an_as4_partner_without_an_encryption_certificate() {
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
        .args(STARTABLE)
        .args([
            "--as4-partner",
            "9900001000002=https://partner.example/as4/inbox",
            "--check",
        ])
        .output()
        .expect("spawn makod");
    assert!(
        !out.status.success(),
        "missing partner cert must be refused"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("encryption certificate"),
        "the error must name the missing material: {stderr}"
    );
}

/// A partner endpoint on plain HTTP would carry regulated market
/// communication in the clear.
#[test]
fn check_rejects_a_plaintext_as4_partner_endpoint() {
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
        .args(STARTABLE)
        .args([
            "--as4-partner",
            "9900001000002=http://partner.example/as4/inbox",
            "--allow-unencrypted-as4",
            "--check",
        ])
        .output()
        .expect("spawn makod");
    assert!(!out.status.success(), "plaintext endpoint must be refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("HTTPS"),
        "the error must name HTTPS: {stderr}"
    );
}

/// An unparseable Cedar policy leaves every endpoint's authorization undefined.
#[test]
fn check_rejects_an_invalid_cedar_policy() {
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
    let policy_dir = dir.path().join("cedar");
    std::fs::create_dir_all(&policy_dir).expect("create policy dir");
    std::fs::write(policy_dir.join("broken.cedar"), "this is not a policy\n")
        .expect("write policy");

    let out = makod()
        .args(["--config"])
        .arg(&cfg)
        .args(STARTABLE)
        .arg("--cedar-policy-dir")
        .arg(&policy_dir)
        .arg("--check")
        .output()
        .expect("spawn makod");
    assert!(!out.status.success(), "an invalid policy must be refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Cedar policy"),
        "the error must name the policy set: {stderr}"
    );
}

/// A daemon with no EDIFACT ingest transport can receive nothing. The real boot
/// exited 1 for this; `--check` now agrees instead of reporting success.
#[test]
fn check_rejects_a_config_with_no_ingest_transport() {
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
    assert!(!out.status.success(), "no ingest transport must be refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("No ingest transport"),
        "the error must explain the missing transport: {stderr}"
    );
}

/// Without signing material *and* without the webhook fallback, outbound
/// EDIFACT is logged and rescheduled forever. It must be an explicit choice.
#[test]
fn check_rejects_a_config_with_no_outbound_delivery_path() {
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
            "--http-addr",
            "127.0.0.1:18080",
            "--auth-key",
            "erp-prod=0123456789abcdef0123456789abcdef",
            "--check",
        ])
        .output()
        .expect("spawn makod");
    assert!(!out.status.success(), "no delivery path must be refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Outbound EDIFACT delivery"),
        "the error must explain the missing delivery path: {stderr}"
    );
}

// ── Config-file surface ──────────────────────────────────────────────────────

/// Key material must be expressible as file references, not only as flags and
/// environment variables — a secret passed by flag is visible in `ps` output.
#[test]
fn as4_key_material_can_be_supplied_from_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cert = dir.path().join("partner.pem");
    std::fs::write(
        &cert,
        "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n",
    )
    .expect("write cert");
    let cfg = write_config(
        dir.path(),
        &format!(
            r#"
[[party]]
mp_id = "9900001000001"
roles = ["NB"]
primary = true

[http]
addr      = "127.0.0.1:18080"
auth_keys = ["erp-prod=0123456789abcdef0123456789abcdef"]

[storage]
allow_volatile = true

[as4]
allow_no_signing   = true
partners           = ["9900001000002=https://partner.example/as4/inbox"]
partner_cert_files = ["9900001000002={}"]
"#,
            cert.display()
        ),
    );
    let out = makod()
        .args(["--config"])
        .arg(&cfg)
        .arg("--check")
        .output()
        .expect("spawn makod");
    assert!(
        out.status.success(),
        "a config whose partner certificate comes from a file must pass.\n\
         stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// An unknown TOML key is a typo, and a typo in a security-relevant field is a
/// silently weakened deployment. `deny_unknown_fields` must stay on.
#[test]
fn an_unknown_config_key_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = write_config(
        dir.path(),
        r#"
[[party]]
mp_id = "9900001000001"
roles = ["NB"]
primary = true

[as4]
allow_unencrypted_as4 = true   # not a field — the real name is allow_unencrypted
"#,
    );
    let out = makod()
        .args(["--config"])
        .arg(&cfg)
        .args(STARTABLE)
        .arg("--check")
        .output()
        .expect("spawn makod");
    assert!(!out.status.success(), "an unknown key must be refused");
}

/// `--check` must not run the boot's write step.
///
/// # Why this is a test
///
/// Pipelines run `--check` against the live configuration, which names the live
/// `--data-dir`. The check used to run the process-registry reconciliation —
/// the only startup step that writes domain state — *before* its exit, so
/// validating a configuration mutated the store it was validating. That step is
/// now sequenced after the check exit.
///
/// The reconciliation logs unconditionally, including when it finds nothing, so
/// the absence of that line is what proves it did not run. Diffing the data
/// directory would not: SlateDB writes its own manifest and WAL bookkeeping
/// whenever a store is opened, whether or not anything is stored in it.
#[test]
fn check_does_not_run_the_process_registry_reconciliation() {
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
        .args(STARTABLE)
        .args(["--log-level", "debug", "--check"])
        .output()
        .expect("spawn makod");
    assert!(
        out.status.success(),
        "--check must exit 0 on a valid config.\nstderr: {}",
        String::from_utf8_lossy(&out.stderr),
    );

    let logs = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        !logs.contains("ProcessRegistry reconciliation"),
        "--check reached the process-registry reconciliation, which writes. \
         It must be sequenced after the check-mode exit so a pipeline can \
         validate a live configuration without mutating the store.\nlogs: {logs}",
    );
    // The check did get far enough to be meaningful — otherwise the assertion
    // above would pass for a binary that exited before doing anything.
    assert!(
        logs.contains("check mode: all startup validations passed"),
        "--check must still run every validation.\nlogs: {logs}",
    );
}

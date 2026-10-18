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
/// An ingest transport, a credential for it, an acknowledgement that outbound
/// EDIFACT has nowhere to go, and an authorization posture. Each is a real
/// requirement of the running daemon, so `--check` demands them too.
///
/// `--cedar-permit-all` is here because authorization is default-deny: a
/// deployment that configures no policies would refuse every request, so
/// startup asks for one of the two answers rather than picking the open one.
/// These tests are about the exit-code contract, not about authorization, so
/// they give the development answer.
const STARTABLE: &[&str] = &[
    "--allow-volatile",
    "--http-addr",
    "127.0.0.1:18080",
    "--auth-key",
    "erp-prod=0123456789abcdef0123456789abcdef",
    "--allow-no-as4-signing",
    "--cedar-permit-all",
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
[deployment]
rollen = ["nb-strom"]

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
[deployment]
rollen = ["nb-strom", "nb-gas"]

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
[deployment]
rollen = ["nb-strom"]

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
            "--cedar-permit-all",
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
/// The credential guard has to sit *above* the `--check` early exit. Below it,
/// `--check` reports "all startup validations passed" (exit 0) for a config
/// whose real start bails with "--auth-key … is required", and a pipeline
/// gating rollout on that exit code promotes it.
#[test]
fn check_rejects_an_authenticated_port_without_credentials() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = write_config(
        dir.path(),
        r#"
[deployment]
rollen = ["nb-strom"]

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
            "--cedar-permit-all",
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
[deployment]
rollen = ["nb-strom"]

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
        // The content signing key is a separate refusal with its own test;
        // this one is about the port's credentials.
        .args([
            "--api-webdienste-addr",
            "127.0.0.1:18090",
            "--allow-unauthenticated-webdienste-client",
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

// ── Preflight: configuration errors that must fail the check ─────────────────
//
// Each case below is a configuration that boots and then dies. `--check` has to
// catch it, and catching it means a non-zero exit: the unit tests in
// `core::preflight` cover the rules, these cover the contract the pipeline
// actually reads.

/// An unparseable Cedar policy leaves every endpoint's authorization undefined.
#[test]
fn check_rejects_an_invalid_cedar_policy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = write_config(
        dir.path(),
        r#"
[deployment]
rollen = ["nb-strom"]

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
[deployment]
rollen = ["nb-strom"]

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
[deployment]
rollen = ["nb-strom"]

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
            "--cedar-permit-all",
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

/// An unknown TOML key is a typo, and a typo in a security-relevant field is a
/// silently weakened deployment. `deny_unknown_fields` must stay on.
#[test]
fn an_unknown_config_key_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = write_config(
        dir.path(),
        r#"
[deployment]
rollen = ["nb-strom"]

[[party]]
mp_id = "9900001000001"
roles = ["NB"]
primary = true

[as4]
allow_no_signng = true   # not a field — the real name is allow_no_signing
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
/// `--data-dir`. The process-registry reconciliation is the one startup step
/// that writes domain state, so it has to be sequenced *after* the check exit —
/// otherwise validating a configuration mutates the store it is validating.
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
[deployment]
rollen = ["nb-strom"]

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

/// The `--check` stderr and status for `body`.
fn check(body: &str, extra: &[&str]) -> (bool, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = write_config(dir.path(), body);
    let out = makod()
        .args(["--config"])
        .arg(&cfg)
        .args(STARTABLE)
        .args(extra)
        .arg("--check")
        .output()
        .expect("spawn makod");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A configuration that declares no deployment role refuses to start: it would
/// register no engine module and dead-letter every inbound message.
#[test]
fn check_refuses_a_config_without_rollen() {
    let (ok, stderr) = check(
        r#"
[[party]]
mp_id = "9900001000001"
roles = ["NB"]
primary = true
"#,
        &[],
    );
    assert!(!ok, "a config without [deployment] rollen must be refused");
    assert!(stderr.contains("no deployment role"), "{stderr}");
}

/// An MP-ID for a role the deployment does not declare is refused, naming it.
#[test]
fn check_refuses_an_mp_id_for_an_undeclared_role() {
    let (ok, stderr) = check(
        r#"
[deployment]
rollen = ["lf-strom"]

[[party]]
mp_id = "9900001000001"
roles = ["LF"]
primary = true

[[party]]
mp_id = "9900001000002"
roles = ["NB"]
"#,
        &[],
    );
    assert!(
        !ok,
        "an NB MP-ID under rollen = [\"lf-strom\"] must be refused"
    );
    assert!(stderr.contains("nb-strom"), "{stderr}");
}

/// A declared role without an MP-ID is refused, naming it.
#[test]
fn check_refuses_a_declared_role_without_an_mp_id() {
    let (ok, stderr) = check(
        r#"
[deployment]
rollen = ["nb-strom", "msb-strom"]

[[party]]
mp_id = "9900001000001"
roles = ["NB"]
primary = true
"#,
        &[],
    );
    assert!(!ok);
    assert!(stderr.contains("msb-strom"), "{stderr}");
}

/// An unknown role token is a parse error, not an ignored entry.
#[test]
fn check_refuses_an_unknown_role_token() {
    let (ok, stderr) = check(
        r#"
[deployment]
rollen = ["lf"]

[[party]]
mp_id = "9900001000001"
roles = ["LF"]
primary = true
"#,
        &[],
    );
    assert!(!ok);
    assert!(stderr.contains("unknown variant"), "{stderr}");
}

/// makod has no multi-instance mode: the flag is an unknown argument and the
/// TOML key an unknown field. The data-directory lock refuses a second writer,
/// so a switch acknowledging one would describe a topology that cannot run.
#[test]
fn allow_multi_instance_is_refused_in_both_forms() {
    let body = r#"
[deployment]
rollen = ["nb-strom"]

[[party]]
mp_id = "9900001000001"
roles = ["NB"]
primary = true
"#;
    let (ok, stderr) = check(body, &["--allow-multi-instance"]);
    assert!(!ok, "--allow-multi-instance must be an unknown argument");
    assert!(stderr.contains("--allow-multi-instance"), "{stderr}");

    let (ok, stderr) = check(
        &format!("{body}\n[storage]\nallow_multi_instance = true\n"),
        &[],
    );
    assert!(!ok, "storage.allow_multi_instance must be an unknown field");
    assert!(stderr.contains("allow_multi_instance"), "{stderr}");
}

// ── AS4 ──────────────────────────────────────────────────────────────────────

/// AS4 key material from a test SM-PKI, written to `dir`: makod's party
/// `9900001000001`, a partner `9900001000002`, the root and the CCRL-S.
fn write_as4_material(dir: &std::path::Path) {
    let ca = asx::testkit::pki::TestCa::sm_pki();
    let crl = ca.crl_signer("CCRL-S(Root)", asx::testkit::pki::KeyAlg::EcBrainpoolP256r1);
    let own = ca.bdew_party("9900001000001");
    let partner = ca.bdew_party("9900001000002");
    let w = |name: &str, bytes: &[u8]| std::fs::write(dir.join(name), bytes).expect("write");
    w("root.pem", ca.cert_pem());
    w("ccrl-s.pem", crl.cert_pem());
    for (name, pair) in [
        ("sig", &own.signing),
        ("enc", &own.encryption),
        ("tls", &own.tls),
    ] {
        w(&format!("{name}.cert.pem"), &pair.cert_pem);
        w(&format!("{name}.key.pem"), &pair.key_pem);
    }
    w("partner.enc.pem", &partner.encryption.cert_pem);
    w("partner.sig.pem", &partner.signing.cert_pem);
}

/// A makod.toml with AS4 configured from the files of [`write_as4_material`].
/// `partner` is the `[[as4.partner]]` body, `as4_extra` extra `[as4]` lines.
fn as4_config(dir: &std::path::Path, as4_extra: &str, partner: &str) -> String {
    let d = dir.display();
    format!(
        r#"
[deployment]
rollen = ["nb-strom"]

[[party]]
mp_id = "9900001000001"
roles = ["NB"]
primary = true

[party.as4]
signing_cert_pem_file    = "{d}/sig.cert.pem"
signing_key_pem_file     = "{d}/sig.key.pem"
encryption_cert_pem_file = "{d}/enc.cert.pem"
encryption_key_pem_file  = "{d}/enc.key.pem"
tls_cert_pem_file        = "{d}/tls.cert.pem"
tls_key_pem_file         = "{d}/tls.key.pem"

[http]
addr      = "127.0.0.1:18080"
auth_keys = ["erp-prod=0123456789abcdef0123456789abcdef"]

[storage]
allow_volatile = true

[authz]
permit_all = true

[as4]
addr = "127.0.0.1:14080"
{as4_extra}

[[as4.partner]]
mp_id = "9900001000002"
{partner}
"#
    )
}

const AS4_TRUST: &str = r#"message_id_domain = "as4.check.test"
trust_anchor_pem_file = "DIR/root.pem"
crl_signer_pem_file = "DIR/ccrl-s.pem""#;

fn as4_check(dir: &std::path::Path, body: &str, env: Option<(&str, &str)>) -> (bool, String) {
    let cfg = write_config(dir, &body.replace("DIR", &dir.display().to_string()));
    let mut cmd = makod();
    cmd.args(["--config"]).arg(&cfg).arg("--check");
    if let Some((k, v)) = env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn makod");
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

/// The production shape — every MP-ID's key pairs, the SM-PKI root, the
/// CRL signer and a partner, all from files — passes `--check`, which ran
/// `As4::builder(..).build()` for the party.
#[test]
fn a_complete_as4_configuration_passes_the_check() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_as4_material(dir.path());
    let partner = r#"endpoint = "https://partner.example/as4/inbox/9900001000002"
encryption_cert_pem_file = "DIR/partner.enc.pem"
signing_cert_pem_files = ["DIR/partner.sig.pem"]
quirks = ["receipt-without-non-repudiation"]"#;
    let (ok, out) = as4_check(
        dir.path(),
        &as4_config(dir.path(), AS4_TRUST, partner),
        None,
    );
    assert!(ok, "a complete AS4 configuration must pass --check:\n{out}");
}

/// A partner with an endpoint and no encryption certificate cannot be
/// delivered to (BDEW AS4-Profil 1.2 §2.2.6.2.2).
#[test]
fn check_rejects_an_as4_partner_without_an_encryption_certificate() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_as4_material(dir.path());
    let partner = r#"endpoint = "https://partner.example/as4/inbox/9900001000002""#;
    let (ok, out) = as4_check(
        dir.path(),
        &as4_config(dir.path(), AS4_TRUST, partner),
        None,
    );
    assert!(
        !ok,
        "a partner without an encryption certificate must be refused"
    );
    assert!(out.contains("encryption certificate"), "{out}");
}

/// A partner endpoint on plain HTTP would carry market communication in the
/// clear.
#[test]
fn check_rejects_a_plaintext_as4_partner_endpoint() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_as4_material(dir.path());
    let partner = r#"endpoint = "http://partner.example/as4/inbox"
encryption_cert_pem_file = "DIR/partner.enc.pem""#;
    let (ok, out) = as4_check(
        dir.path(),
        &as4_config(dir.path(), AS4_TRUST, partner),
        None,
    );
    assert!(!ok, "a plaintext endpoint must be refused");
    assert!(out.contains("HTTPS"), "{out}");
}

/// The SM-PKI root is the one trust anchor; without it nothing verifies, so
/// the AS4 listener is refused and the key is named.
#[test]
fn check_rejects_an_as4_listener_without_a_trust_anchor() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_as4_material(dir.path());
    let trust = r#"message_id_domain = "as4.check.test"
crl_signer_pem_file = "DIR/ccrl-s.pem""#;
    let (ok, out) = as4_check(dir.path(), &as4_config(dir.path(), trust, ""), None);
    assert!(
        !ok,
        "an AS4 listener without the SM-PKI root must be refused"
    );
    assert!(out.contains("as4.trust_anchor_pem_file"), "{out}");
}

/// The CCRL-S(Root) is required: without it no certificate below the root
/// passes the revocation check.
#[test]
fn check_rejects_an_as4_listener_without_a_crl_signer() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_as4_material(dir.path());
    let trust = r#"message_id_domain = "as4.check.test"
trust_anchor_pem_file = "DIR/root.pem""#;
    let (ok, out) = as4_check(dir.path(), &as4_config(dir.path(), trust, ""), None);
    assert!(!ok);
    assert!(out.contains("as4.crl_signer_pem_file"), "{out}");
}

/// The removed lenient-receipts switch is refused — from the config file and
/// from the environment — naming the per-partner setting that replaces it.
#[test]
fn the_removed_lenient_receipts_switch_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_as4_material(dir.path());
    let trust = format!("{AS4_TRUST}\nlenient_receipts = true");
    let (ok, out) = as4_check(dir.path(), &as4_config(dir.path(), &trust, ""), None);
    assert!(!ok);
    assert!(
        out.contains("quirks = [\"receipt-without-non-repudiation\"]"),
        "{out}"
    );

    let (ok, out) = as4_check(
        dir.path(),
        &as4_config(dir.path(), AS4_TRUST, ""),
        Some(("MAKOD_AS4_LENIENT_RECEIPTS", "true")),
    );
    assert!(!ok);
    assert!(out.contains("receipt-without-non-repudiation"), "{out}");
}

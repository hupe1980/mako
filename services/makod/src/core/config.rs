//! `makod.toml` — TOML configuration file support.
//!
//! Every setting that can be passed via a CLI flag or environment variable can
//! also be placed in a TOML file and supplied with `--config <FILE>` (or
//! `MAKOD_CONFIG=<FILE>`). The `cli_fields_are_reachable_from_toml` guard test
//! in `main.rs` keeps that promise honest: a new CLI flag that `apply_config_file`
//! never reads fails the build.
//!
//! **Precedence (highest → lowest)**
//!
//! 1. CLI flags (e.g. `--log-level debug`)
//! 2. Environment variables (e.g. `MAKOD_LOG_LEVEL=debug`)
//! 3. Config file (e.g. `makod.toml` `[logging] level = "debug"`)
//! 4. Built-in defaults
//!
//! ## Secrets
//!
//! Every field carrying key material or a shared secret has a `*_file`
//! companion that reads the value from disk at startup. Prefer the file form in
//! production: a value passed as a CLI flag is visible in `ps` output, and a
//! value passed by environment variable is visible to anything that can read
//! `/proc/<pid>/environ` or a container inspection API.
//!
//! | Inline | File companion |
//! |---|---|
//! | `as4.signing_key_pem` | `as4.signing_key_pem_file` |
//! | `as4.signing_cert_pem` | `as4.signing_cert_pem_file` |
//! | `as4.decryption_key_pem` | `as4.decryption_key_pem_file` |
//! | `as4.trust_anchor_pem` | `as4.trust_anchor_pem_file` |
//! | `as4.partner_certs` | `as4.partner_cert_files` |
//! | `http.auth_keys` | `http.auth_keys_file` |
//! | `erp.webhook_secret` | `erp.webhook_secret_file` |
//! | `marktd.api_key` | `marktd.api_key_file` |
//!
//! ## Minimal example
//!
//! ```toml
//! [logging]
//! level  = "info"
//! format = "json"
//!
//! [storage]
//! backend = "s3"
//!
//! [storage.s3]
//! bucket = "my-makod-bucket"
//! prefix = "makod"
//!
//! # Single Marktpartner-ID covering all roles (most common):
//! [[party]]
//! mp_id = "9900000000001"
//! roles = ["NB", "LF", "MSB"]
//!
//! [http]
//! addr      = "0.0.0.0:8080"
//! auth_keys = ["erp-prod=<token>"]
//!
//! [as4]
//! addr                  = "0.0.0.0:4080"
//! signing_key_pem_file  = "/etc/makod/signing.key.pem"
//! signing_cert_pem_file = "/etc/makod/signing.cert.pem"
//! decryption_key_pem_file = "/etc/makod/decryption.key.pem"
//! trust_anchor_pem_file   = "/etc/makod/bdew-ca.pem"
//! partners = ["9900000000002=https://partner-a.example/as4/inbox"]
//! partner_cert_files = ["9900000000002=/etc/makod/partners/9900000000002.pem"]
//! ```
//!
//! ## Notes
//!
//! - Unknown keys are **rejected** — a typo in a field name is an error, not
//!   silently ignored.
//! - Supplying both an inline field and its `*_file` companion is an error;
//!   the two would otherwise disagree silently.

use std::path::{Path, PathBuf};

use serde::Deserialize;

// ── Top-level ─────────────────────────────────────────────────────────────────

/// Root of `makod.toml`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    pub logging: Option<LoggingConfig>,
    pub otel: Option<OtelConfig>,
    pub storage: Option<StorageConfig>,
    pub http: Option<HttpConfig>,
    pub authz: Option<AuthzConfig>,
    pub oidc: Option<OidcConfig>,
    pub webdienste: Option<WebdiensteConfig>,
    pub engine: Option<EngineConfig>,
    pub as4: Option<As4Config>,
    pub erp: Option<ErpConfig>,
    pub marktd: Option<MarktdConfig>,
    pub maloid: Option<MaloIdConfig>,
    /// `[[party]]` — one entry per BDEW market-participant identity.
    ///
    /// The single source of truth for operator identity — at least one entry is
    /// required. An operator holding **multiple Marktpartner-IDs** (e.g. separate
    /// BDEW registrations for NB, LF, and MSB roles) lists one entry per identity.
    /// The first entry marked `primary = true` (or the first entry in document
    /// order when none is marked) becomes the storage partition key and the
    /// default EDIFACT sender MP-ID fallback.
    ///
    /// Example:
    /// ```toml
    /// [[party]]
    /// mp_id   = "9900001000001"
    /// roles   = ["NB"]
    /// primary = true
    ///
    /// [[party]]
    /// mp_id = "9900001000002"
    /// roles = ["LF"]
    /// ```
    pub party: Option<Vec<PartyConfig>>,
}

/// One `[[party]]` entry — a single BDEW market-participant identity.
///
/// Multiple `[[party]]` entries on the same `makod` instance describe an
/// operator who has registered separate Marktpartner-IDs for different roles
/// (e.g. a utility with distinct NB, LF, and MSB registrations). Allgemeine
/// Festlegungen §2.13 requires a separate code per Energieart *and* Marktrolle
/// for a market participant active in both Sparten, so a Strom NB and a Gas GNB
/// are always two entries.
///
/// For the common case — a single company covering all Strom roles — one entry
/// with all relevant roles is sufficient:
///
/// ```toml
/// [[party]]
/// mp_id = "9900001000001"
/// roles = ["NB", "LF", "MSB"]
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartyConfig {
    /// 13-digit BDEW-Codenummer, DVGW-Codenummer, or 16-char EIC (Marktpartner-ID).
    /// Must be globally unique per entry.
    /// BDEW-Codenummern start with `99`, DVGW-Codenummern with `98`.
    /// Only GS1-issued 13-digit codes are true GLNs (Allgemeine Festlegungen §2.13).
    pub mp_id: String,
    /// BDEW Marktrollen this Marktpartner-ID is authorised for.
    ///
    /// Valid values: `NB`, `LF`, `MSB`, `GNB`, `LFG`, `gMSB`, `MGV`, `BKV`,
    /// `UNB`, `ANB`, `VNB`, `NMSB`, `AMSB`, `ESA`.
    pub roles: Vec<String>,
    /// Marks this entry as the **storage partition key** for the engine.
    ///
    /// When `true`, this Marktpartner-ID is used to derive the `TenantId` UUID
    /// that scopes all event streams, outbox entries, and MaLo cache keys.
    /// Exactly one entry should have `primary = true`; when none does, the
    /// first entry in document order is used.
    #[serde(default)]
    pub primary: bool,
    /// NAD agency code for EDIFACT sender segments.
    ///
    /// Derived from the Marktpartner-ID prefix when omitted (`99…` → `"293"`,
    /// `98…` → `"332"`, 16-char EIC → `"ZEW"`, otherwise `"9"`). Set explicitly
    /// only for a GS1-issued GLN, which needs `"305"`.
    pub agency: Option<String>,
}

// ── Sections ─────────────────────────────────────────────────────────────────

/// `[logging]` — controls log verbosity and format.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingConfig {
    /// Minimum log level. One of: `trace`, `debug`, `info`, `warn`, `error`.
    /// Default: `info`.
    pub level: Option<String>,
    /// Log output format. One of: `pretty`, `compact`, `json`.
    /// Default: `pretty`.
    pub format: Option<String>,
}

/// `[otel]` — OpenTelemetry span export.
///
/// Setting `endpoint` enables OTLP export and switches the subscriber to the
/// structured JSON layer (`[logging] format` no longer applies). The
/// `OTEL_EXPORTER_OTLP_ENDPOINT` / `OTEL_SERVICE_NAME` environment variables
/// take precedence, so a deployment can keep telemetry entirely in the
/// orchestrator's environment.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OtelConfig {
    /// OTLP collector endpoint, e.g. `"http://otel-collector:4317"`.
    pub endpoint: Option<String>,
    /// `service.name` resource attribute. Default: `"makod"`.
    pub service_name: Option<String>,
}

/// `[storage]` — selects and configures the event-store backend.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    /// Object store backend. One of: `local`, `s3`, `gcs`, `azure`.
    /// Default: `local`.
    pub backend: Option<String>,

    /// Local filesystem path (only used when `backend = "local"`).
    /// When omitted, a volatile in-memory store is used — requires
    /// `allow_volatile = true` or `MAKOD_ALLOW_VOLATILE=1`.
    pub data_dir: Option<PathBuf>,

    /// Explicitly permit volatile (in-memory) mode.
    ///
    /// Set to `true` only in development, testing, or CI environments.
    /// **Never set this in production.**
    #[serde(default)]
    pub allow_volatile: bool,

    /// Per-stream event quota — a circuit breaker against runaway streams
    /// whose replay cost would grow without bound. `0` disables it.
    /// Default: 100 000.
    pub max_stream_events: Option<u64>,

    /// Acknowledge that an external distributed lock protects inbox
    /// deduplication, suppressing the multi-instance startup warning.
    #[serde(default)]
    pub allow_multi_instance: bool,

    /// `[storage.s3]` — AWS S3 / S3-compatible settings.
    pub s3: Option<S3Config>,

    /// `[storage.gcs]` — Google Cloud Storage settings.
    pub gcs: Option<GcsConfig>,

    /// `[storage.azure]` — Azure Blob Storage settings.
    pub azure: Option<AzureConfig>,
}

/// `[storage.s3]`
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct S3Config {
    /// S3 bucket name (required when `backend = "s3"`).
    pub bucket: Option<String>,
    /// Key prefix within the bucket. Default: `"makod"`.
    pub prefix: Option<String>,
    /// Custom endpoint for MinIO or other S3-compatible stores.
    /// When the URL starts with `http://`, plain HTTP is permitted (dev only).
    pub endpoint: Option<String>,
}

/// `[storage.gcs]`
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GcsConfig {
    /// GCS bucket name (required when `backend = "gcs"`).
    pub bucket: Option<String>,
    /// Key prefix within the bucket. Default: `"makod"`.
    pub prefix: Option<String>,
}

/// `[storage.azure]`
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AzureConfig {
    /// Blob container name (required when `backend = "azure"`).
    pub container: Option<String>,
    /// Storage account name (required when `backend = "azure"`).
    pub account: Option<String>,
    /// Key prefix within the container. Default: `"makod"`.
    pub prefix: Option<String>,
}

/// `[http]` — REST API, MCP endpoint, and admin surfaces.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    /// TCP listen address, e.g. `"0.0.0.0:8080"`.
    pub addr: Option<std::net::SocketAddr>,
    /// Maximum request body in bytes for `POST /edifact` and the command API.
    /// Default: 10 MiB.
    pub max_body_bytes: Option<usize>,
    /// Named API keys as `"NAME=TOKEN"` pairs. Each maps a bearer token to a
    /// Cedar principal; the name appears in every audit log line.
    pub auth_keys: Option<Vec<String>>,
    /// File containing one `NAME=TOKEN` pair per line (`#` comments allowed).
    /// Preferred over `auth_keys` — tokens then never appear in the config file.
    pub auth_keys_file: Option<PathBuf>,
}

/// `[authz]` — Cedar ABAC policy loading.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthzConfig {
    /// Directory of additional `*.cedar` policy files, loaded in name order.
    pub cedar_policy_dir: Option<PathBuf>,
    /// Drop the built-in permit-all baseline so only `cedar_policy_dir` grants
    /// access. Required for a least-privilege deployment and for § 6a EnWG role
    /// separation in a combined-role (VIU) deployment.
    #[serde(default)]
    pub no_default_policy: bool,
}

/// `[oidc]` — OIDC/JWT bearer token authentication.
///
/// When configured, `makod` validates JWT bearer tokens issued by the given
/// OIDC provider. The `sub` claim becomes the Cedar principal name. API-key
/// authentication and OIDC can be enabled simultaneously.
///
/// ## Example
///
/// ```toml
/// [oidc]
/// issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
/// audience = "api://makod"
/// jwks_refresh_secs = 300
/// ```
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OidcConfig {
    /// OIDC issuer URL (e.g. `https://login.microsoftonline.com/{tenant}/v2.0`).
    pub issuer: Option<String>,

    /// Expected JWT `aud` claim.
    pub audience: Option<String>,

    /// JWKS background refresh interval in seconds. Default: 300.
    pub jwks_refresh_secs: Option<u64>,
}

/// `[webdienste]` — BDEW API-Webdienste Strom server.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebdiensteConfig {
    /// TCP listen address, e.g. `"0.0.0.0:8090"`.
    pub addr: Option<std::net::SocketAddr>,
    /// Serve the port without the bearer/OIDC + Cedar layer. Only acceptable
    /// behind a proxy that terminates mTLS against the BDEW PKI CA.
    #[serde(default)]
    pub allow_unauthenticated: bool,
}

/// `[engine]` — engine-level and worker settings.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineConfig {
    /// Maximum seconds to wait for the store to close after the shutdown
    /// signal. Default: 30.
    pub shutdown_timeout_secs: Option<u64>,
    /// Events between automatic workflow snapshots. Default: 100.
    pub snapshot_interval: Option<u64>,
    /// Seconds between projection checkpoint writes; `0` disables the workers.
    /// Default: 60.
    pub projection_checkpoint_interval: Option<u64>,
    /// Deadline scheduler poll interval in seconds. Default: 30.
    pub deadline_poll_interval_secs: Option<u64>,
    /// Tokio worker threads. Defaults to the CPU count.
    pub worker_threads: Option<usize>,
    /// Marktrollen allow-list for the ERP command API. Defaults to the union of
    /// all `[[party]]` roles.
    pub marktrollen: Option<Vec<String>>,
    /// Engine deployment roles gating PID registration. Defaults to the union
    /// of all `[[party]]` roles.
    pub deployment_roles: Option<Vec<String>>,
}

/// `[as4]` — AS4 / ebMS3 inbound and outbound transport.
///
/// BDEW AS4-Profil v1.2 §2.2.6.2.2 requires every message to be signed **and**
/// encrypted, which means three distinct pieces of key material: the operator's
/// signing key pair, the operator's own decryption key, and one encryption
/// certificate per trading partner.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct As4Config {
    /// AS4 inbound listen address, e.g. `"0.0.0.0:4080"`.
    pub addr: Option<std::net::SocketAddr>,

    /// BDEW party ID for this MSH. Defaults to the primary `[[party]]` MP-ID.
    /// Must match the subject of the signing certificate (AS4-Profil §2.3.2).
    pub party_id: Option<String>,

    /// PEM-encoded EC (BrainpoolP256r1) private key for WS-Security XML-DSig
    /// signing, inline.
    ///
    /// Provide either this field **or** `signing_key_pem_file`, not both.
    pub signing_key_pem: Option<String>,

    /// Path to the PEM file holding the XML-DSig signing private key.
    ///
    /// The recommended production form — combine with Kubernetes Secrets,
    /// a Secrets Store CSI driver, a vault-agent tmpfs sink, or systemd
    /// `LoadCredential=`.
    pub signing_key_pem_file: Option<PathBuf>,

    /// PEM-encoded X.509 certificate matching the signing key, inline.
    pub signing_cert_pem: Option<String>,

    /// Path to the PEM file holding the signing certificate.
    pub signing_cert_pem_file: Option<PathBuf>,

    /// PEM-encoded EC private key used to decrypt inbound AS4 messages, inline.
    ///
    /// Without it the daemon cannot prove an inbound message was encrypted and
    /// refuses to start unless `allow_unencrypted = true`.
    pub decryption_key_pem: Option<String>,

    /// Path to the PEM file holding the inbound decryption private key.
    pub decryption_key_pem_file: Option<PathBuf>,

    /// PEM-encoded trust anchor for verifying counterparty signatures, inline.
    ///
    /// In production this is the BDEW/BNetzA PKI CA certificate. Leaving it
    /// unset falls back to the operator's own signing certificate, which
    /// rejects every counterparty.
    pub trust_anchor_pem: Option<String>,

    /// Path to the PEM file holding the trust anchor certificate.
    pub trust_anchor_pem_file: Option<PathBuf>,

    /// Trading-partner AS4 endpoints as `"MP-ID=HTTPS-URL"` pairs.
    ///
    /// These entries are **bootstrapped** into the durable `PartnerStore` at
    /// startup and survive restarts. Once seeded, individual records can be
    /// updated at runtime via `PUT /admin/partners/{mp_id}` or by an inbound
    /// PARTIN interchange (`POST /admin/partners/import`).
    pub partners: Option<Vec<String>>,

    /// Trading-partner encryption certificates as `"MP-ID=<PEM>"` pairs.
    ///
    /// Required for every entry in `partners`: the send path refuses to encrypt
    /// without the recipient's certificate, so a missing one dead-letters every
    /// delivery to that partner.
    pub partner_certs: Option<Vec<String>>,

    /// Trading-partner encryption certificates as `"MP-ID=/path/to/cert.pem"`
    /// pairs. Preferred over `partner_certs` — keeps PEM blobs out of the
    /// config file.
    pub partner_cert_files: Option<Vec<String>>,

    /// DEV/TEST ONLY. Downgrade the missing-encryption-material refusals
    /// (inbound decryption key, per-partner certificates) to warnings.
    #[serde(default)]
    pub allow_unencrypted: bool,

    /// DEV/TEST ONLY. Start without AS4 signing credentials and without an
    /// EDIFACT outbox webhook, so outbound EDIFACT is logged rather than sent.
    #[serde(default)]
    pub allow_no_signing: bool,

    /// DEV/TEST ONLY. Start the AS4 listener without a counterparty trust
    /// anchor, accepting that every partner's signature will be rejected.
    #[serde(default)]
    pub allow_no_trust_anchor: bool,

    /// Treat a missing or unverifiable synchronous `eb:Receipt` as a warning
    /// instead of a delivery failure. Interop debugging only.
    #[serde(default)]
    pub lenient_receipts: bool,
}

/// `[erp]` — ERP / backend integration settings (BO4E contract).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErpConfig {
    /// HTTP(S) URL to which ERP events are POSTed as CloudEvents.
    ///
    /// When set, `makod` starts an `OutboxErpWorker` that POSTs every outbox
    /// entry carrying a BO4E payload to this URL. When absent, ERP events are
    /// only logged.
    pub webhook_url: Option<String>,

    /// Shared secret for Standard Webhooks request signing.
    pub webhook_secret: Option<String>,

    /// Path to a file holding the webhook signing secret. Preferred over
    /// `webhook_secret`.
    pub webhook_secret_file: Option<PathBuf>,

    /// URL that receives rendered outbound EDIFACT as CloudEvents when no AS4
    /// signing material is configured (development / ERP-side transport).
    pub edifact_outbox_webhook_url: Option<String>,

    /// §20b EnWG Netzzugangsplattform endpoint. Absent until a platform
    /// interface exists; requests then fall back to the ERP webhook.
    pub netzzugang_endpoint_url: Option<String>,
}

/// `[marktd]` — master-data service used for consent and Konfigurationsprodukt
/// gates.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarktdConfig {
    /// Base URL of the `marktd` instance, e.g. `"http://marktd:8180"`.
    pub url: Option<String>,
    /// API key for `marktd`.
    pub api_key: Option<String>,
    /// Path to a file holding the `marktd` API key. Preferred over `api_key`.
    pub api_key_file: Option<PathBuf>,
}

/// `[maloid]` — BDEW MaLo-ID Identifikationsverfahren (API-Webdienste Strom).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaloIdConfig {
    /// Counterparty MaLo-ID callback endpoints as `"MP-ID=URL"` pairs.
    pub partners: Option<Vec<String>>,
    /// Base URL of the BDEW Verzeichnisdienst used to resolve unknown
    /// counterparty endpoints.
    pub verzeichnisdienst_url: Option<String>,
}

// ── Loading ───────────────────────────────────────────────────────────────────

/// Read and parse the TOML config file at `path`.
///
/// # Errors
///
/// Returns an error if the file cannot be read or if the TOML is malformed /
/// contains unknown keys.
pub fn load(path: &Path) -> anyhow::Result<ConfigFile> {
    use anyhow::Context as _;
    let src = std::fs::read_to_string(path)
        .with_context(|| format!("reading config file: {}", path.display()))?;
    toml::from_str(&src).with_context(|| format!("parsing config file: {}", path.display()))
}

/// Resolve an inline value and its `*_file` companion into one value.
///
/// Supplying both is an error: the two would otherwise disagree silently and
/// the winner would be an implementation detail of field ordering.
///
/// # Errors
///
/// Returns an error when both forms are present, or when the file cannot be read.
pub fn either_inline_or_file(
    field: &str,
    inline: Option<String>,
    file: Option<&PathBuf>,
) -> anyhow::Result<Option<String>> {
    use anyhow::Context as _;
    match (inline, file) {
        (Some(_), Some(path)) => Err(anyhow::anyhow!(
            "config: {field} and {field}_file are both set ({}); provide exactly one",
            path.display()
        )),
        (Some(v), None) => Ok(Some(v)),
        (None, Some(path)) => {
            Ok(Some(std::fs::read_to_string(path).with_context(|| {
                format!("config: reading {field}_file {}", path.display())
            })?))
        }
        (None, None) => Ok(None),
    }
}

/// Parse a file of `NAME=TOKEN` lines into the CLI's repeatable-flag form.
///
/// Blank lines and `#` comments are skipped so the file can be annotated.
///
/// # Errors
///
/// Returns an error when the file cannot be read or a line has no `=`.
pub fn read_pairs_file(field: &str, path: &Path) -> anyhow::Result<Vec<String>> {
    use anyhow::Context as _;
    let src = std::fs::read_to_string(path)
        .with_context(|| format!("config: reading {field} {}", path.display()))?;
    let mut out = Vec::new();
    for (n, line) in src.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        anyhow::ensure!(
            line.contains('='),
            "config: {field} {}:{}: expected NAME=VALUE, got {line:?}",
            path.display(),
            n + 1,
        );
        out.push(line.to_owned());
    }
    Ok(out)
}

/// Read `"KEY=/path/to/file"` pairs into `"KEY=<file contents>"` pairs.
///
/// Used for `as4.partner_cert_files`, which references one PEM file per
/// trading partner.
///
/// # Errors
///
/// Returns an error when an entry has no `=` or a referenced file is unreadable.
pub fn read_keyed_files(field: &str, entries: &[String]) -> anyhow::Result<Vec<String>> {
    use anyhow::Context as _;
    entries
        .iter()
        .map(|entry| {
            let (key, path) = entry.split_once('=').ok_or_else(|| {
                anyhow::anyhow!("config: {field}: expected MP-ID=PATH, got {entry:?}")
            })?;
            let content = std::fs::read_to_string(path.trim())
                .with_context(|| format!("config: {field}: reading {path}"))?;
            Ok(format!("{}={content}", key.trim()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The documented minimal example must actually parse — `deny_unknown_fields`
    /// turns a stale doc example into a config that refuses to start.
    #[test]
    fn documented_example_parses() {
        let src = r#"
[logging]
level  = "info"
format = "json"

[otel]
endpoint = "http://otel-collector:4317"

[storage]
backend = "s3"

[storage.s3]
bucket = "my-makod-bucket"
prefix = "makod"

[[party]]
mp_id = "9900000000001"
roles = ["NB", "LF", "MSB"]

[http]
addr      = "0.0.0.0:8080"
auth_keys = ["erp-prod=token"]

[as4]
addr                    = "0.0.0.0:4080"
signing_key_pem_file    = "/etc/makod/signing.key.pem"
signing_cert_pem_file   = "/etc/makod/signing.cert.pem"
decryption_key_pem_file = "/etc/makod/decryption.key.pem"
trust_anchor_pem_file   = "/etc/makod/bdew-ca.pem"
partners = ["9900000000002=https://partner-a.example/as4/inbox"]
partner_cert_files = ["9900000000002=/etc/makod/partners/9900000000002.pem"]
"#;
        let cfg: ConfigFile = toml::from_str(src).expect("documented example parses");
        assert_eq!(cfg.party.expect("party").len(), 1);
        assert!(cfg.otel.expect("otel").endpoint.is_some());
    }

    /// Setting an inline secret and its file companion is a hard error rather
    /// than a silent precedence rule.
    #[test]
    fn inline_and_file_conflict_is_rejected() {
        let err = either_inline_or_file(
            "as4.signing_key_pem",
            Some("inline".to_owned()),
            Some(&PathBuf::from("/dev/null")),
        )
        .expect_err("both forms must be rejected");
        assert!(err.to_string().contains("provide exactly one"), "{err}");
    }

    #[test]
    fn pairs_file_skips_comments_and_blanks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("keys");
        std::fs::write(&path, "# comment\n\nerp=token1\nci=token2\n").expect("write");
        let pairs = read_pairs_file("http.auth_keys_file", &path).expect("parse");
        assert_eq!(pairs, vec!["erp=token1", "ci=token2"]);
    }

    #[test]
    fn pairs_file_rejects_a_line_without_a_separator() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("keys");
        std::fs::write(&path, "no-separator-here\n").expect("write");
        let err = read_pairs_file("http.auth_keys_file", &path).expect_err("must reject");
        assert!(err.to_string().contains("expected NAME=VALUE"), "{err}");
    }
}

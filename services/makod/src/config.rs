//! `makod.toml` — TOML configuration file support.
//!
//! Every setting that can be passed via a CLI flag or environment variable can
//! also be placed in a TOML file and supplied with `--config <FILE>` (or
//! `MAKOD_CONFIG=<FILE>`).
//!
//! **Precedence (highest → lowest)**
//!
//! 1. CLI flags (e.g. `--log-level debug`)
//! 2. Environment variables (e.g. `MAKOD_LOG_LEVEL=debug`)
//! 3. Config file (e.g. `makod.toml` `[logging] level = "debug"`)
//! 4. Built-in defaults
//!
//! ## Minimal example
//!
//! ```toml
//! [logging]
//! level  = "info"
//! format = "json"
//!
//! [storage]
//! backend  = "s3"
//!
//! [storage.s3]
//! bucket = "my-makod-bucket"
//! prefix = "makod"
//!
//! # Single GLN covering all roles (most common):
//! [[party]]
//! gln   = "9900000000001"
//! roles = ["NB", "LF", "MSB"]
//!
//! [http]
//! addr = "0.0.0.0:8080"
//!
//! [oidc]
//! issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
//! audience = "api://makod"
//!
//! [as4]
//! addr     = "0.0.0.0:4080"
//! signing_key_pem_file  = "/etc/makod/signing.key.pem"
//! signing_cert_pem_file = "/etc/makod/signing.cert.pem"
//! partners = [
//!   "9900000000002=https://partner-a.example/as4/inbox",
//!   "9900000000003=https://partner-b.example/as4/inbox",
//! ]
//! ```
//!
//! ## Notes
//!
//! - Unknown keys are **rejected** — a typo in a field name is an error, not
//!   silently ignored.
//! - Sensitive values (signing keys, API tokens) can be placed inline or
//!   referenced via `*_file` path fields that point to PEM / secret files on
//!   disk.

use std::path::Path;

use serde::Deserialize;

// ── Top-level ─────────────────────────────────────────────────────────────────

/// Root of `makod.toml`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    pub logging: Option<LoggingConfig>,
    pub storage: Option<StorageConfig>,
    pub http: Option<HttpConfig>,
    pub oidc: Option<OidcConfig>,
    pub webdienste: Option<WebdiensteConfig>,
    pub engine: Option<EngineConfig>,
    pub as4: Option<As4Config>,
    pub erp: Option<ErpConfig>,
    /// `[[party]]` — one entry per BDEW market-participant identity.
    ///
    /// Use this instead of `[engine] tenant_id` + `--marktrollen` when the
    /// operator holds **multiple GLNs** (e.g. separate BDEW registrations for
    /// NB, LF, and MSB roles).  The first entry marked `primary = true` (or
    /// the first entry in document order when none is marked) becomes the
    /// storage partition key and the default EDIFACT sender GLN fallback.
    ///
    /// Example:
    /// ```toml
    /// [[party]]
    /// gln     = "9900001000001"
    /// roles   = ["NB"]
    /// primary = true
    ///
    /// [[party]]
    /// gln   = "9900001000002"
    /// roles = ["LF"]
    ///
    /// [[party]]
    /// gln   = "9900001000003"
    /// roles = ["LFG", "MSB"]
    /// ```
    pub party: Option<Vec<PartyConfig>>,
}

/// One `[[party]]` entry — a single BDEW market-participant identity.
///
/// Multiple `[[party]]` entries on the same `makod` instance describe an
/// operator who has registered separate BDEW GLNs for different roles
/// (e.g. a large utility with distinct NB, LF, and MSB subsidiaries).
///
/// For the common case — a single company GLN covering all roles — a single
/// entry with all relevant roles is sufficient:
///
/// ```toml
/// [[party]]
/// gln   = "9900001000001"
/// roles = ["NB", "LF", "MSB", "GNB", "LFG"]
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartyConfig {
    /// 13-digit BDEW GLN or 16-char EIC.  Must be globally unique per entry.
    pub gln: String,
    /// BDEW Marktrollen this GLN is authorised for.
    ///
    /// Valid values: `NB`, `LF`, `MSB`, `GNB`, `LFG`, `gMSB`, `MGV`, `BKV`,
    /// `UNB`, `ANB`, `VNB`, `NMSB`, `AMSB`.
    pub roles: Vec<String>,
    /// Marks this entry as the **storage partition key** for the engine.
    ///
    /// When `true`, this GLN is used to derive the `TenantId` UUID that scopes
    /// all event streams, outbox entries, and MaLo cache keys.
    /// Exactly one entry should have `primary = true`; when none does, the
    /// first entry in document order is used.
    #[serde(default)]
    pub primary: bool,
    /// NAD agency code for EDIFACT sender segments.
    ///
    /// Defaults to `"293"` (BDEW).  Set to `"305"` for GS1 GLNs or `"ZEW"`
    /// for EIC identifiers.
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
    pub data_dir: Option<std::path::PathBuf>,

    /// Explicitly permit volatile (in-memory) mode.
    ///
    /// Set to `true` only in development, testing, or CI environments.
    /// **Never set this in production.**
    #[serde(default)]
    pub allow_volatile: bool,

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

/// `[http]` — REST API for direct EDIFACT ingest and MaLo admin.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    /// TCP listen address, e.g. `"0.0.0.0:8080"`.
    pub addr: Option<std::net::SocketAddr>,
    /// Maximum `POST /edifact` request body in bytes. Default: 10 MiB.
    pub max_body_bytes: Option<usize>,
}

/// `[oidc]` — OIDC/JWT bearer token authentication.
///
/// When configured, `makod` validates JWT bearer tokens issued by the given
/// OIDC provider.  The `sub` claim becomes the Cedar principal name.
/// API-key authentication (`--auth-key`) and OIDC can be enabled simultaneously.
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
}

/// `[engine]` — engine-level settings.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineConfig {
    /// Maximum seconds to wait for the store to close after shutdown signal.
    /// Mirrors `--shutdown-timeout-secs`. Default: 30.
    pub shutdown_timeout_secs: Option<u64>,
}

/// `[as4]` — AS4 / ebMS3 inbound and outbound transport.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct As4Config {
    /// AS4 inbound listen address, e.g. `"0.0.0.0:4080"`.
    pub addr: Option<std::net::SocketAddr>,

    /// BDEW party ID (GLN) for this MSH. Defaults to `engine.tenant_id`.
    pub party_id: Option<String>,

    /// PEM-encoded RSA private key for WS-Security XML-DSig signing (inline).
    ///
    /// The value is immediately wrapped in `SecretString` after config parsing
    /// and is never logged.  **Prefer `signing_key_pem_file`** for production
    /// deployments so the key material does not appear in the config file or
    /// process listings.  For vault / secret-manager integration, use the
    /// init-container / CSI secrets-store pattern to write the key to a tmpfs
    /// mount and point `signing_key_pem_file` at that path.
    ///
    /// Provide either this field **or** `signing_key_pem_file`, not both.
    pub signing_key_pem: Option<String>,

    /// Path to a PEM file containing the RSA private key for XML-DSig signing.
    ///
    /// The file is read at startup and immediately wrapped in `SecretString`.
    /// This is the recommended production approach — combine with:
    ///
    /// - **Kubernetes Secrets**: `secretKeyRef` + volume mount
    /// - **AWS Secrets Manager**: Secrets Store CSI driver
    /// - **HashiCorp Vault**: vault-agent sidecar with tmpfs sink
    /// - **systemd**: `LoadCredential=signing.key.pem:/path/to/key`
    ///
    /// Provide either this field **or** `signing_key_pem`, not both.
    pub signing_key_pem_file: Option<std::path::PathBuf>,

    /// PEM-encoded X.509 certificate matching `signing_key_pem` (inline).
    ///
    /// Provide either this field **or** `signing_cert_pem_file`, not both.
    pub signing_cert_pem: Option<String>,

    /// Path to a PEM file containing the X.509 certificate.
    ///
    /// The file is read at startup. Provide either this field **or**
    /// `signing_cert_pem`, not both.
    pub signing_cert_pem_file: Option<std::path::PathBuf>,

    /// Trading-partner AS4 endpoints in `"GLN=HTTPS-URL"` format.
    ///
    /// These entries are **bootstrapped** into the durable `PartnerStore` at
    /// startup and survive restarts without requiring a redeploy. Once seeded,
    /// individual records can be updated at runtime via:
    ///
    /// - `PUT /admin/partners/{gln}` — manual JSON upsert
    /// - `POST /admin/partners/import` — ingest a raw PARTIN EDIFACT interchange
    ///
    /// When a partner sends an inbound PARTIN message (PIDs 37000–37014), the
    /// engine calls `PartnerStore::upsert` with the richer PARTIN-derived
    /// record; the bootstrapped AS4 URL is preserved until overwritten by a
    /// PARTIN `COM` segment with qualifier `AK`.
    ///
    /// Example:
    /// ```toml
    /// partners = [
    ///   "9900000000002=https://partner.example/as4/inbox",
    /// ]
    /// ```
    pub partners: Option<Vec<String>>,
}

/// `[erp]` — ERP / backend integration settings (BO4E contract).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErpConfig {
    /// HTTP(S) URL to which ERP events are POSTed as BO4E JSON.
    ///
    /// When set, `makod` starts an `OutboxErpWorker` that POSTs every
    /// outbox entry carrying a BO4E payload to this URL.
    /// When absent, ERP events are only logged.
    pub webhook_url: Option<String>,

    /// Shared secret for `X-Mako-Signature` HMAC-SHA256 request signing.
    ///
    /// When set, every webhook POST includes an
    /// `X-Mako-Signature: <hex>` header for authenticity verification.
    pub webhook_secret: Option<String>,
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

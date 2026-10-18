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
//! | `webdienste.client_cert_pem` | `webdienste.client_cert_pem_file` |
//! | `webdienste.client_key_pem` | `webdienste.client_key_pem_file` |
//! | `webdienste.signing_key_pem` | `webdienste.signing_key_pem_file` |
//! | `http.auth_keys` | `http.auth_keys_file` |
//! | `erp.webhook_secret` | `erp.webhook_secret_file` |
//! | `marktd.api_key` | `marktd.api_key_file` |
//! | `marktd.webhook_secret` | `marktd.webhook_secret_file` |
//!
//! AS4 key material and certificates are files only: `[party.as4]` names each
//! MP-ID's signing, encryption and TLS pairs, `[as4]` the SM-PKI root and the
//! CRL signer, `[[as4.partner]]` each partner's certificates.
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
//! # The roles this instance plays, and one Marktpartner-ID covering them:
//! [deployment]
//! rollen = ["nb-strom", "lf-strom", "msb-strom"]
//!
//! [[party]]
//! mp_id = "9900000000001"
//! roles = ["NB", "LF", "MSB"]
//!
//! [party.as4]
//! signing_cert_pem_file    = "/etc/makod/9900000000001/signing.cert.pem"
//! signing_key_pem_file     = "/etc/makod/9900000000001/signing.key.pem"
//! encryption_cert_pem_file = "/etc/makod/9900000000001/encryption.cert.pem"
//! encryption_key_pem_file  = "/etc/makod/9900000000001/encryption.key.pem"
//! tls_cert_pem_file        = "/etc/makod/9900000000001/tls.cert.pem"
//! tls_key_pem_file         = "/etc/makod/9900000000001/tls.key.pem"
//!
//! [http]
//! addr      = "0.0.0.0:8080"
//! auth_keys = ["erp-prod=<token>"]
//!
//! [as4]
//! addr                  = "0.0.0.0:4080"
//! message_id_domain     = "as4.stadtwerke-musterstadt.de"
//! trust_anchor_pem_file = "/etc/makod/sm-pki-root.pem"
//! crl_signer_pem_file   = "/etc/makod/sm-pki-ccrl-s-root.pem"
//!
//! [[as4.partner]]
//! mp_id                    = "9900000000002"
//! endpoint                 = "https://partner-a.example/as4/inbox"
//! encryption_cert_pem_file = "/etc/makod/partners/9900000000002.enc.pem"
//! signing_cert_pem_files   = ["/etc/makod/partners/9900000000002.sig.pem"]
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
    /// `[deployment]` — the roles this instance plays. Required.
    pub deployment: Option<DeploymentConfig>,
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
    /// `[policy]` — the operator readings this deployment runs under.
    #[serde(default)]
    pub policy: MakodPolicy,
}

/// `[policy]` — makod's operator settings.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakodPolicy {
    /// `[policy.as4]`.
    #[serde(default)]
    pub as4: As4Policy,
}

/// `[policy.as4]` — the AS4 inbox's operator settings.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct As4Policy {
    /// Whether a sender that is not a configured `[[as4.partner]]` is served.
    #[serde(default)]
    pub unbekannte_absender: mako_service::policy::Policy<UnbekannteAbsender>,
}

impl MakodPolicy {
    /// The policy makod runs under — logged at startup and by `--check`.
    #[must_use]
    pub fn report(&self) -> mako_service::policy::PolicyReport {
        mako_service::policy::PolicyReport::new().with(&self.as4.unbekannte_absender)
    }
}

mako_service::policy_choice! {
    /// Whether the AS4 inbox serves a sender that is not a configured partner.
    ///
    /// BDEW AS4-Profil 1.2 accepts any sender whose SM-PKI certificate carries
    /// the claimed MP-ID (§2.3.2) and lets a receiver reject certificates by
    /// that MP-ID as a whitelist (§2.3.7). Which of the two a deployment runs
    /// is the operator's choice.
    pub enum UnbekannteAbsender {
        key: "as4.unbekannte_absender",
        doc: "@/docs/services/makod.md#policy-as4-unbekannte-absender",
        default: tightening(NurKonfigurierte, "a sender the operator has not configured is never acted on: a forged or misrouted Abmeldung from an SM-PKI certificate holder is refused rather than processed, and the operator admits a new partner by configuring it"),
        options: {
            /// Only senders listed in `[[as4.partner]]`: any other is refused
            /// before dispatch and answered `EBMS:0202`.
            NurKonfigurierte = "nur_konfigurierte" => "BDEW AS4-Profil 1.2 Kap. 2.3.7 (S. 19): Zertifikate KÖNNEN auf der Grundlage der MP-ID im OU-Feld abgelehnt werden (Whitelist); die Whitelist sind die Partnerstammdaten des Betreibers",
            /// Any sender whose verified SM-PKI certificate carries the claimed
            /// MP-ID in its OU.
            BdewOuZulassen = "bdew_ou_zulassen" => "BDEW AS4-Profil 1.2 Kap. 2.3.2 (S. 16): Nachrichten mit beliebigen Werten in eb:From/eb:PartyId werden akzeptiert, sofern sie mit den Betreff-Feldern im vorgelegten Zertifikat übereinstimmen; Regelungen zum Übertragungsweg für AS4 2.6 Kap. 5.2 (S. 6): das OU des Subject enthält die MP-ID",
        }
    }
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
    /// `[party.as4]` — this MP-ID's AS4 key material. Every `[[party]]` needs
    /// one as soon as any does: a receiving MSH checks the signing
    /// certificate's OU against `eb:From`, so each MP-ID signs, decrypts and
    /// connects with its own key pairs.
    pub as4: Option<PartyAs4Config>,
}

/// `[party.as4]` — one MP-ID's three AS4 key pairs, as PEM files.
///
/// BDEW AS4-Profil 1.2 §2.3.4.4 (S. 17): different EC key pairs for signing,
/// encryption and TLS, issued by an SM-PKI CA, with the MP-ID in the subject —
/// in its OU (Regelungen zum Übertragungsweg für AS4 2.6 Kap. 5.2, S. 6). The
/// signing file holds the EMT.MAK leaf first, then the intermediates.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartyAs4Config {
    /// Signing certificate chain (leaf first).
    pub signing_cert_pem_file: PathBuf,
    /// Signing private key.
    pub signing_key_pem_file: PathBuf,
    /// Encryption certificate; partners encrypt to it.
    pub encryption_cert_pem_file: PathBuf,
    /// Decryption private key.
    pub encryption_key_pem_file: PathBuf,
    /// TLS client certificate chain, presented to a partner MSH.
    pub tls_cert_pem_file: PathBuf,
    /// TLS private key.
    pub tls_key_pem_file: PathBuf,
}

// ── Sections ─────────────────────────────────────────────────────────────────

/// `[deployment]` — the Marktrollen this instance plays.
///
/// ```toml
/// [deployment]
/// rollen = ["nb-strom", "msb-strom"]
/// ```
///
/// Selects the engine modules and is checked against `[[party]]`: each declared
/// role has exactly one MP-ID, and no MP-ID names an undeclared role
/// ([`crate::deployment::validate_rollen`]). Startup refuses without it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentConfig {
    /// Role tokens: `lf-strom`, `lf-gas`, `nb-strom`, `nb-gas`, `msb-strom`,
    /// `msb-gas`, `esa-strom`.
    pub rollen: Vec<crate::deployment::Rolle>,
}

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
    /// Add the built-in permit-all baseline, making every authenticated
    /// principal full admin. **Development only** — authorization is
    /// default-deny, and a baseline `permit` cannot be narrowed by an
    /// operator's own `permit` statements, only by a `forbid`.
    #[serde(default)]
    pub permit_all: bool,
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

/// `[webdienste]` — the BDEW API-Webdienste Strom boundary, both directions.
///
/// The `addr` and the two trust declarations describe the **inbound** port this
/// daemon serves. The `client_*` fields describe the **outbound** identity it
/// presents when it calls somebody else's API-Webdienste — the Verzeichnisdienst
/// and the MaLo-ID service. They are one section because they are one boundary,
/// and a deployment may need either half alone: a daemon that only calls out
/// sets no `addr`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebdiensteConfig {
    /// TCP listen address, e.g. `"0.0.0.0:8090"`.
    pub addr: Option<std::net::SocketAddr>,
    /// Serve the port without the bearer/OIDC + Cedar layer. Only acceptable
    /// behind a proxy that terminates mTLS against the BDEW PKI CA.
    #[serde(default)]
    pub allow_unauthenticated: bool,
    /// Take the calling Marktpartner's identity from the
    /// `x-mako-client-mp-id` request header.
    ///
    /// The BDEW API-Webdienste identify the caller by their mTLS client
    /// certificate, which a fronting proxy terminates and forwards in that
    /// header. The value decides whose name a § 14a Steuerungsauftrag or a WiM
    /// Anmeldung is placed in, so it is only evidence when a proxy sets it and
    /// **strips any copy the client sent**. `makod` cannot tell the two apart,
    /// which is why trusting it is a deployment declaration rather than a
    /// default: without this, the header is ignored and a handler that needs a
    /// caller refuses.
    ///
    /// Implied by [`Self::allow_unauthenticated`], which already declares that
    /// a proxy authenticates this port.
    #[serde(default)]
    pub trust_client_mp_id_header: bool,

    /// PEM X.509 certificate chain presented when calling an API-Webdienst.
    ///
    /// The EDI-Energy API-Webdienste authenticate their callers by mutual TLS
    /// with an EMT.API certificate from the BSI SM-PKI. Without an identity the
    /// TLS handshake completes only if the far side does not ask for one, so a
    /// deployment missing this does not fail locally — it is refused at the
    /// counterparty, which is the harder place to read it from.
    pub client_cert_pem: Option<String>,
    /// Path form of [`Self::client_cert_pem`] — keeps PEM out of the config.
    pub client_cert_pem_file: Option<PathBuf>,
    /// PEM PKCS#8 private key for [`Self::client_cert_pem`].
    pub client_key_pem: Option<String>,
    /// Path form of [`Self::client_key_pem`].
    pub client_key_pem_file: Option<PathBuf>,
    /// PEM PKCS#8 private key of the **EMT.API** certificate, for
    /// content-layer signing.
    ///
    /// Distinct from [`Self::client_key_pem`], which is the mTLS identity.
    /// Regelungen zum Übertragungsweg für API-Webdienste 1.2, Kap. 3.3 signs
    /// the URI, the RFC 8785 canonical body, the `creationDateTime`, the
    /// `transactionId` and any `initialTransactionId` (algorithms per
    /// TR-03116-3 Kap. 8 and 9), and carries the result in `X-BDEW-DIGEST` /
    /// `X-BDEW-SIGNATURE` — a layer above TLS, so the two are not substitutes.
    /// It applies to the Control Measures and MaLo Identification APIs; the
    /// Verzeichnisdienst uses JWS instead.
    pub signing_key_pem: Option<String>,
    /// Path form of [`Self::signing_key_pem`].
    pub signing_key_pem_file: Option<PathBuf>,
    /// PEM X.509 certificate of [`Self::signing_key_pem`], sent as
    /// `X-BDEW-CERT` with every signed request (same chapter). Required with
    /// the key.
    pub signing_cert_pem: Option<String>,
    /// Path form of [`Self::signing_cert_pem`].
    pub signing_cert_pem_file: Option<PathBuf>,

    /// DEV/TEST ONLY. Call the API-Webdienste without a client identity.
    ///
    /// The counterparty refuses the call rather than this daemon, which is the
    /// harder place to read the failure from — hence the boot refusal this
    /// downgrades.
    #[serde(default)]
    pub allow_unauthenticated_client: bool,

    /// Paths to additional root CA certificates to trust when calling out.
    ///
    /// The SM-PKI roots. Needed when they are not in the host trust store,
    /// which they usually are not.
    pub client_root_ca_pem_files: Option<Vec<PathBuf>>,
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
/// `asx::Profile::bdew()` fixes every security setting; what remains to
/// configure is who the partners are, the SM-PKI trust and where to listen.
/// Each local MP-ID's key pairs are its `[party.as4]`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct As4Config {
    /// AS4 inbound listen address, e.g. `"0.0.0.0:4080"`. Each local MP-ID is
    /// served at `/as4/inbox/<MP-ID>`.
    pub addr: Option<std::net::SocketAddr>,

    /// The `id-right` of every `eb:MessageId` this deployment sends
    /// (`<outbox id>@<domain>`, RFC 2822 §3.6.4). A domain the operator
    /// controls.
    pub message_id_domain: Option<String>,

    /// The SM-PKI root certificate — the one trust anchor every partner and
    /// every local signer chains to.
    pub trust_anchor_pem_file: Option<PathBuf>,

    /// The SM-PKI CCRL-S(Root) certificate, which signs the indirect CRL of
    /// the Root-CA (BSI TR-03109-4 §4.2.1); `asx-rs` checks it chains to the
    /// trust anchor and asserts `cRLSign`.
    pub crl_signer_pem_file: Option<PathBuf>,

    /// `[[as4.partner]]` — one entry per counterparty MP-ID.
    #[serde(default)]
    pub partner: Vec<As4PartnerConfig>,

    /// DEV/TEST ONLY. Start without AS4 key material and without an EDIFACT
    /// outbox webhook, so outbound EDIFACT is logged rather than sent.
    #[serde(default)]
    pub allow_no_signing: bool,

    /// Removed: refused at startup with the per-partner replacement named.
    #[serde(default)]
    pub lenient_receipts: Option<serde::de::IgnoredAny>,
}

/// `[[as4.partner]]` — one AS4 counterparty.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct As4PartnerConfig {
    /// The partner's MP-ID.
    pub mp_id: String,
    /// NAD DE3055 agency of the MP-ID; derived from its prefix when omitted.
    pub agency: Option<String>,
    /// The partner's AS4 inbox, `https://` only. Omit for a partner mako only
    /// receives from.
    pub endpoint: Option<String>,
    /// The partner's encryption certificate — required with `endpoint`.
    pub encryption_cert_pem_file: Option<PathBuf>,
    /// Signing certificates pinned for the partner (several for a rollover).
    /// Without any, a message from it is verified against the SM-PKI chain and
    /// the MP-ID in the certificate's OU.
    #[serde(default)]
    pub signing_cert_pem_files: Vec<PathBuf>,
    /// Interoperability quirks: `"receipt-without-non-repudiation"` accepts the
    /// partner's *signed* receipt without NonRepudiationInformation. An
    /// unsigned or mis-signed receipt is refused whatever is set here.
    #[serde(default)]
    pub quirks: Vec<String>,
}

/// The refusal of the removed `as4.lenient_receipts` key.
pub const LENIENT_RECEIPTS_REMOVED: &str = "as4.lenient_receipts (MAKOD_AS4_LENIENT_RECEIPTS) is \
     removed: every synchronous eb:Receipt is verified. For a partner whose signed receipt \
     carries no NonRepudiationInformation, declare it per partner: [[as4.partner]] quirks = \
     [\"receipt-without-non-repudiation\"]. An unsigned or mis-signed receipt is never accepted.";

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

/// `[marktd]` — master-data service: consent and Konfigurationsprodukt gates,
/// and the source the MaLo-Ident cache follows through the fan-out.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarktdConfig {
    /// Base URL of the `marktd` instance, e.g. `"http://marktd:8180"`.
    pub url: Option<String>,
    /// API key for `marktd`.
    pub api_key: Option<String>,
    /// Path to a file holding the `marktd` API key. Preferred over `api_key`.
    pub api_key_file: Option<PathBuf>,
    /// makod's own fan-out endpoint as `marktd` reaches it, e.g.
    /// `"http://makod:8080/webhooks/marktd"`. Required with `url`: the MaLo
    /// cache follows `marktd` through this subscription.
    pub webhook_url: Option<String>,
    /// Standard Webhooks secret `marktd` signs its deliveries to makod with.
    pub webhook_secret: Option<String>,
    /// Path to a file holding `webhook_secret`. Preferred over the inline form.
    pub webhook_secret_file: Option<PathBuf>,
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
/// An inline value carrying the `env:NAME` indirection is resolved through
/// [`mako_service::config::resolve_env`], the same convention every other
/// daemon gets from `mako_service::run`. makod assembles its own configuration
/// and so has to ask for it explicitly — and it is not cosmetic: an unresolved
/// `webhook_secret = "env:MAKOD_ERP_WEBHOOK_SECRET"` becomes the literal HMAC
/// key, and that literal is a constant published in this repository, so anyone
/// can forge a signed `de.mako.process.*` event. A referenced-but-unset
/// variable fails here, at startup, rather than on the first request.
///
/// The `*_file` companion is read verbatim: a secret file's contents are the
/// secret, and `env:` is a convention of the TOML, not of the file.
///
/// # Errors
///
/// Returns an error when both forms are present, when the file cannot be read,
/// or when an `env:` reference names a variable that is not set.
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
        (Some(v), None) => Ok(Some(
            mako_service::config::resolve_env(&v)
                .with_context(|| format!("config: resolving {field}"))?,
        )),
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

[deployment]
rollen = ["nb-strom", "lf-strom", "msb-strom"]

[[party]]
mp_id = "9900000000001"
roles = ["NB", "LF", "MSB"]

[http]
addr      = "0.0.0.0:8080"
auth_keys = ["erp-prod=token"]

[party.as4]
signing_cert_pem_file    = "/etc/makod/9900000000001/signing.cert.pem"
signing_key_pem_file     = "/etc/makod/9900000000001/signing.key.pem"
encryption_cert_pem_file = "/etc/makod/9900000000001/encryption.cert.pem"
encryption_key_pem_file  = "/etc/makod/9900000000001/encryption.key.pem"
tls_cert_pem_file        = "/etc/makod/9900000000001/tls.cert.pem"
tls_key_pem_file         = "/etc/makod/9900000000001/tls.key.pem"

[as4]
addr                  = "0.0.0.0:4080"
message_id_domain     = "as4.stadtwerke-musterstadt.de"
trust_anchor_pem_file = "/etc/makod/sm-pki-root.pem"
crl_signer_pem_file   = "/etc/makod/sm-pki-ccrl-s-root.pem"

[[as4.partner]]
mp_id                    = "9900000000002"
endpoint                 = "https://partner-a.example/as4/inbox"
encryption_cert_pem_file = "/etc/makod/partners/9900000000002.enc.pem"
signing_cert_pem_files   = ["/etc/makod/partners/9900000000002.sig.pem"]
quirks                   = ["receipt-without-non-repudiation"]

[policy.as4]
unbekannte_absender = "bdew_ou_zulassen"
"#;
        let cfg: ConfigFile = toml::from_str(src).expect("documented example parses");
        assert!(cfg.party.expect("party")[0].as4.is_some());
        assert_eq!(cfg.as4.expect("as4").partner.len(), 1);
        assert_eq!(
            cfg.policy.as4.unbekannte_absender.get(),
            UnbekannteAbsender::BdewOuZulassen
        );
        assert_eq!(cfg.deployment.expect("deployment").rollen.len(), 3);
        assert!(cfg.otel.expect("otel").endpoint.is_some());
    }

    /// Setting an inline secret and its file companion is a hard error rather
    /// than a silent precedence rule.
    #[test]
    fn inline_and_file_conflict_is_rejected() {
        let err = either_inline_or_file(
            "erp.webhook_secret",
            Some("inline".to_owned()),
            Some(&PathBuf::from("/dev/null")),
        )
        .expect_err("both forms must be rejected");
        assert!(err.to_string().contains("provide exactly one"), "{err}");
    }

    /// An `env:` indirection is resolved, not shipped as the value.
    ///
    /// The literal `env:NAME` is non-empty, so every emptiness check upstream
    /// passes it through. What makes it dangerous rather than merely broken is
    /// that the placeholder is a constant in this repository: shipped as an
    /// HMAC key it is a secret the whole world already holds.
    ///
    /// `PATH` stands in for the secret: the crate denies `unsafe_code` and
    /// `std::env::set_var` is unsafe, so the test reads a variable the process
    /// already has rather than weakening the lint for a fixture.
    #[test]
    fn an_env_indirection_is_resolved() {
        let expected = std::env::var("PATH").expect("PATH is set in a test process");
        let got = either_inline_or_file("erp.webhook_secret", Some("env:PATH".to_owned()), None)
            .expect("a set variable resolves");
        assert_eq!(got.as_deref(), Some(expected.as_str()));
        assert_ne!(
            got.as_deref(),
            Some("env:PATH"),
            "the literal must not ship"
        );
    }

    /// A referenced-but-unset variable fails at startup and names itself.
    #[test]
    fn an_unset_env_indirection_is_refused() {
        let err = either_inline_or_file(
            "erp.webhook_secret",
            Some("env:MAKOD_TEST_ENV_DEFINITELY_UNSET".to_owned()),
            None,
        )
        .expect_err("an unset variable must not become the literal");
        let msg = format!("{err:#}");
        assert!(msg.contains("MAKOD_TEST_ENV_DEFINITELY_UNSET"), "{msg}");
        assert!(msg.contains("erp.webhook_secret"), "{msg}");
    }

    /// A plain literal is untouched, so a config may mix both forms.
    #[test]
    fn a_literal_value_passes_through() {
        let got = either_inline_or_file("marktd.api_key", Some("plain".to_owned()), None)
            .expect("a literal resolves to itself");
        assert_eq!(got.as_deref(), Some("plain"));
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

    /// The removed `as4.lenient_receipts` key still parses, so startup can
    /// refuse it naming the per-partner setting instead of a generic
    /// unknown-field error.
    #[test]
    fn the_removed_lenient_receipts_key_is_recognised() {
        let cfg: ConfigFile = toml::from_str("[as4]\nlenient_receipts = true\n").expect("parses");
        assert!(cfg.as4.expect("as4").lenient_receipts.is_some());
        assert!(LENIENT_RECEIPTS_REMOVED.contains("receipt-without-non-repudiation"));
    }
}

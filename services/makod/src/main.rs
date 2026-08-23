//! `makod` — the Mako process engine daemon.
//!
//! Assembles the domain modules (GPKE, WiM, GeLi Gas, WiM Gas, MaBiS, GaBi Gas,
//! Redispatch 2.0) into one [`EngineContext`], opens the configured transports,
//! and runs until a shutdown signal arrives.
//!
//! Which modules are compiled in is a build-time choice — see the `role-*`
//! features in `Cargo.toml`. A build that selects no role fails at startup
//! rather than shipping a daemon whose router is empty.
//!
//! ## Where the reference lives
//!
//! Flags, environment variables and the `makod.toml` schema are **not**
//! restated here; a copy of `--help` in a doc comment drifts from the `Cli`
//! struct fifty lines below it. See:
//!
//! - `makod --help` — the authoritative flag list
//! - [`core::config`] — the `makod.toml` schema, field by field
//! - `services/makod/README.md` — port layout, module/PID table, quick start
//! - the operator guide at <https://hupe1980.github.io/mako/docs/services/makod/>
//!
//! ## Boot order
//!
//! The sequence in [`async_main`] is deliberate, and `--check` exits partway
//! through it:
//!
//! ```text
//! 1. identity      [[party]] entries → MpIdRegistry → roles, tenant key
//! 2. store         object store opens; exclusive data-dir lock
//! 3. engine        domain modules → EngineContext → PidRouter
//! 4. validation    profiles, adapter coverage, dispatch completeness, preflight
//!    ── `--check` exits here: everything above only reads ──
//! 5. reconcile     rebuild missing ProcessRegistry routing entries (writes)
//! 6. transports    HTTP :8080, AS4 :4080, API-Webdienste :8090
//! 7. workers       outbox, ERP webhook, deadlines, projections, inbox purge
//! ```
//!
//! ## Shutdown
//!
//! On `SIGTERM`/`SIGINT` the shared [`CancellationToken`] is cancelled, every
//! listener and worker is *joined*, the dead-letter buffer is flushed, and only
//! then is the store closed. The join is the load-bearing step: closing the
//! store under a running outbox worker can leave a message delivered but
//! unacknowledged, which the next start delivers again. An incomplete drain
//! exits non-zero.

// The daemon holds regulated data and market credentials; the one `unsafe`
// block it needs (clearing an env var before the runtime starts) opts out
// explicitly and documents why it is sound.
#![deny(unsafe_code)]

mod api;
mod core;
mod orchestrator;
mod startup;
mod transport;

// Flat-path aliases so every historical `crate::<module>` path keeps resolving
// inside the binary crate (which compiles the module tree independently of the
// lib target). Mirrors the re-export block in lib.rs.
use crate::api::{edifact_api, malo_admin_api, migration_api, partner_api};
use crate::core::{
    cedar_authz, config, erp_adapter, health, malo_cache, party_registry, preflight, worker_health,
};
use crate::orchestrator::{
    adapters, commands_api, deadline_dispatch, edifact_renderer, ingest_dispatcher, netzzugang,
    projection_worker,
};
use crate::transport::{
    api_bridge, as4_sender, contrl_ack, malo_ident_sender, redispatch_xml_ingest,
    verzeichnisdienst_worker,
};

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use clap::{Parser, ValueEnum};
use edi_energy::Platform;
use mako_engine::{
    marktrolle::{DeploymentRoles, Marktrolle},
    store_slatedb::SlateDbStore,
};
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;
use tracing::info;

// ── CLI ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Parser)]
#[command(
    name    = "makod",
    version,
    about   = "Mako process engine daemon for German energy market communication (MaKo/BDEW)",
    long_about = None,
)]
struct Cli {
    /// Path to a TOML configuration file.
    ///
    /// Settings loaded from the file are applied after CLI and environment
    /// variable resolution: CLI flags and env vars always take precedence.
    /// Every flag below has a config-file equivalent; secrets additionally have
    /// a `*_file` form that keeps them out of `ps` output and the environment.
    ///
    /// Can also be set via the `MAKOD_CONFIG` environment variable.
    #[arg(short = 'c', long, value_name = "FILE", env = "MAKOD_CONFIG")]
    config: Option<std::path::PathBuf>,

    /// Minimum log level to emit.
    ///
    /// Can also be set via the `MAKOD_LOG_LEVEL` environment variable.
    #[arg(short = 'l', long, value_enum, default_value_t = LogLevel::Info, env = "MAKOD_LOG_LEVEL")]
    log_level: LogLevel,

    /// Log output format.
    ///
    /// Can also be set via the `MAKOD_LOG_FORMAT` environment variable.
    #[arg(short = 'f', long, value_enum, default_value_t = LogFormat::Pretty, env = "MAKOD_LOG_FORMAT")]
    log_format: LogFormat,

    /// OTLP collector endpoint for OpenTelemetry span export.
    ///
    /// Setting it enables OTLP export and switches the subscriber to structured
    /// JSON (`--log-format` no longer applies). Inbound `traceparent` headers
    /// are propagated across the outbox boundary into outbound deliveries.
    ///
    /// The standard `OTEL_EXPORTER_OTLP_ENDPOINT` variable takes precedence, so
    /// a deployment can keep telemetry entirely in the orchestrator environment.
    #[arg(long, value_name = "URL", env = "MAKOD_OTEL_ENDPOINT")]
    otel_endpoint: Option<String>,

    /// `service.name` resource attribute for exported spans. Default: `makod`.
    ///
    /// The standard `OTEL_SERVICE_NAME` variable takes precedence.
    #[arg(long, value_name = "NAME", env = "MAKOD_OTEL_SERVICE_NAME")]
    otel_service_name: Option<String>,

    /// Path to the persistent event-store directory (local filesystem).
    ///
    /// When omitted, an in-memory (volatile) store is used.
    ///
    /// **WARNING: volatile mode is for development and testing only.**
    /// All events, outbox messages, snapshots, and deadlines are stored
    /// entirely in RAM and are permanently lost on process exit, crash,
    /// or restart. This means:
    ///
    /// - Outbound APERAK and CONTRL messages enqueued in the outbox are lost.
    /// - In-flight MaKo processes cannot be resumed after a restart.
    /// - Regulatory audit requirements (§ 147 AO / GoBD, BDEW AHB) cannot be met.
    ///
    /// Set `--data-dir` (or `MAKOD_DATA_DIR`) to a persistent path, or use
    /// `--object-store=s3` / `--object-store=gcs` / `--object-store=azure`
    /// for cloud-backed storage in production deployments.
    ///
    /// Ignored when `--object-store` is not `local`.
    ///
    /// Can also be set via the `MAKOD_DATA_DIR` environment variable.
    #[arg(long, value_name = "DIR", env = "MAKOD_DATA_DIR")]
    data_dir: Option<std::path::PathBuf>,

    /// Explicitly permit volatile (in-memory) mode.
    ///
    /// By default, makod refuses to start without `--data-dir` or a
    /// cloud-backed object store.  Set this flag to acknowledge that all
    /// event-store data will be lost on exit and that volatile mode is
    /// intentional (e.g. in integration tests, local smoke tests, or CI).
    ///
    /// **Do not set this in production.**  Volatile mode cannot meet the
    /// regulatory durability requirements of § 147 AO / GoBD and BDEW AHB.
    ///
    /// Can also be set via `MAKOD_ALLOW_VOLATILE=1`.
    #[arg(long, env = "MAKOD_ALLOW_VOLATILE", default_value_t = false)]
    allow_volatile: bool,

    /// Object store backend type.
    ///
    /// - `local`: local filesystem (requires `--data-dir`; in-memory when omitted)
    /// - `s3`: AWS S3 or compatible (requires `--s3-bucket`; reads standard AWS env vars)
    /// - `gcs`: Google Cloud Storage (requires `--gcs-bucket`; reads GCP credential env vars)
    /// - `azure`: Azure Blob Storage (requires `--azure-container` and `--azure-account`)
    ///
    /// Can also be set via the `MAKOD_OBJECT_STORE` environment variable.
    #[arg(long, value_enum, default_value_t = ObjectStoreBackend::Local, env = "MAKOD_OBJECT_STORE")]
    object_store: ObjectStoreBackend,

    /// S3 bucket name (required when `--object-store=s3`).
    ///
    /// Can also be set via the `MAKOD_S3_BUCKET` environment variable.
    #[arg(long, value_name = "BUCKET", env = "MAKOD_S3_BUCKET")]
    s3_bucket: Option<String>,

    /// S3 endpoint URL for MinIO or S3-compatible object stores.
    ///
    /// When omitted, the default AWS regional endpoint is used.
    /// Can also be set via the `MAKOD_S3_ENDPOINT` environment variable.
    #[arg(long, value_name = "URL", env = "MAKOD_S3_ENDPOINT")]
    s3_endpoint: Option<String>,

    /// Key prefix within the S3 bucket where SlateDB stores its files.
    ///
    /// Defaults to `"makod"`. Useful when multiple makod instances share
    /// a bucket and need isolated key spaces.
    /// Can also be set via the `MAKOD_S3_PREFIX` environment variable.
    #[arg(
        long,
        value_name = "PREFIX",
        default_value = "makod",
        env = "MAKOD_S3_PREFIX"
    )]
    s3_prefix: String,

    /// Google Cloud Storage bucket name (required when `--object-store=gcs`).
    ///
    /// Can also be set via the `MAKOD_GCS_BUCKET` environment variable.
    #[arg(long, value_name = "BUCKET", env = "MAKOD_GCS_BUCKET")]
    gcs_bucket: Option<String>,

    /// Key prefix within the GCS bucket where SlateDB stores its files.
    ///
    /// Defaults to `"makod"`. Useful when multiple makod instances share
    /// a bucket and need isolated key spaces.
    /// Can also be set via the `MAKOD_GCS_PREFIX` environment variable.
    #[arg(
        long,
        value_name = "PREFIX",
        default_value = "makod",
        env = "MAKOD_GCS_PREFIX"
    )]
    gcs_prefix: String,

    /// Azure Blob Storage container name (required when `--object-store=azure`).
    ///
    /// Can also be set via the `MAKOD_AZURE_CONTAINER` environment variable.
    #[arg(long, value_name = "NAME", env = "MAKOD_AZURE_CONTAINER")]
    azure_container: Option<String>,

    /// Azure Storage account name (required when `--object-store=azure`).
    ///
    /// Can also be set via the `MAKOD_AZURE_ACCOUNT` environment variable.
    #[arg(long, value_name = "ACCOUNT", env = "MAKOD_AZURE_ACCOUNT")]
    azure_account: Option<String>,

    /// Key prefix within the Azure Blob container where SlateDB stores its files.
    ///
    /// Defaults to `"makod"`. Useful when multiple makod instances share
    /// a container and need isolated key spaces.
    /// Can also be set via the `MAKOD_AZURE_PREFIX` environment variable.
    #[arg(
        long,
        value_name = "PREFIX",
        default_value = "makod",
        env = "MAKOD_AZURE_PREFIX"
    )]
    azure_prefix: String,

    /// TCP address on which the HTTP REST API listens.
    ///
    /// When set, makod exposes a `POST /edifact` endpoint as an alternative
    /// ingest path to AS4. Disabled when omitted.
    ///
    /// Examples: `127.0.0.1:8080`, `0.0.0.0:8080`
    ///
    /// Can also be set via the `MAKOD_HTTP_ADDR` environment variable.
    #[arg(long, value_name = "ADDR", env = "MAKOD_HTTP_ADDR")]
    http_addr: Option<std::net::SocketAddr>,

    /// Named API keys for Cedar authorization.
    ///
    /// Format: `NAME=TOKEN` (repeatable, or comma-separated via the environment
    /// variable).  Each key maps a bearer token to a named Cedar principal
    /// (`MaKo::Principal::"<NAME>"`).  The name appears in all audit logs.
    ///
    /// Example:
    /// ```text
    /// --auth-key erp-sap=<token1> --auth-key ci-pipeline=<token2>
    /// ```
    ///
    /// Can also be set via `MAKOD_AUTH_KEYS` (comma-separated `NAME=TOKEN` pairs).
    #[arg(
        long = "auth-key",
        value_name = "NAME=TOKEN",
        env = "MAKOD_AUTH_KEYS",
        value_delimiter = ',',
        hide_env_values = true
    )]
    auth_keys: Vec<String>,

    /// Directory containing additional Cedar policy files (`.cedar`).
    ///
    /// All `*.cedar` files in this directory are concatenated and loaded at
    /// startup to supplement or restrict the default policy.  Operators use
    /// this to implement fine-grained ABAC rules per principal, tenant,
    /// Marktrolle, or PID without recompiling the binary.
    ///
    /// See `src/cedar/default.cedar` for policy examples.
    ///
    /// Can also be set via `MAKOD_CEDAR_POLICY_DIR`.
    #[arg(long, value_name = "DIR", env = "MAKOD_CEDAR_POLICY_DIR")]
    cedar_policy_dir: Option<std::path::PathBuf>,

    /// Drop the built-in permit-all baseline and grant access only from
    /// `--cedar-policy-dir`.
    ///
    /// `src/cedar/default.cedar` permits every authenticated principal to
    /// perform every action. A Cedar request is allowed when *any* `permit`
    /// matches and no `forbid` does, so operator-supplied `permit` statements
    /// cannot narrow that baseline — without this flag a least-privilege policy
    /// set has no effect. Required to run `conservative.cedar` as intended, and
    /// to enforce §9 EnWG role separation in a combined-role (VIU) deployment.
    ///
    /// Refuses to start unless `--cedar-policy-dir` supplies the grants.
    ///
    /// Can also be set via `MAKOD_CEDAR_NO_DEFAULT_POLICY`.
    #[arg(long, env = "MAKOD_CEDAR_NO_DEFAULT_POLICY")]
    cedar_no_default_policy: bool,

    /// OIDC issuer URL for JWT bearer token validation.
    ///
    /// When set, `makod` fetches `<ISSUER>/.well-known/openid-configuration`
    /// at startup to locate the JWKS endpoint, downloads the public keys, and
    /// validates incoming JWT bearer tokens locally (no per-request network
    /// round-trip).  The JWT `sub` claim becomes the Cedar principal name.
    ///
    /// Supported identity providers: Azure AD/Entra ID, Keycloak, Okta,
    /// Google Workspace, AWS Cognito, Kubernetes workload identity, and any
    /// standards-compliant OIDC provider.
    ///
    /// Only asymmetric algorithms (RS256/384/512, ES256/384, PS256/384/512)
    /// are accepted.  HMAC tokens are rejected unconditionally.
    ///
    /// Requires `--oidc-audience`.  API-key auth (`--auth-key`) and OIDC
    /// coexist — either or both can be configured simultaneously.
    ///
    /// Can also be set via `MAKOD_OIDC_ISSUER`.
    #[arg(long, value_name = "URL", env = "MAKOD_OIDC_ISSUER")]
    oidc_issuer: Option<String>,

    /// Expected JWT `aud` claim (audience).
    ///
    /// Must match the audience configured in the identity provider for this
    /// `makod` instance.  Tokens with a different audience are rejected.
    ///
    /// Example: `api://makod` (Azure) or `https://makod.example.com` (custom).
    ///
    /// Required when `--oidc-issuer` is set.
    ///
    /// Can also be set via `MAKOD_OIDC_AUDIENCE`.
    #[arg(long, value_name = "AUD", env = "MAKOD_OIDC_AUDIENCE")]
    oidc_audience: Option<String>,

    /// JWKS background refresh interval in seconds.
    ///
    /// A Tokio task refreshes the cached JWKS on this cadence so that key
    /// rotations at the identity provider are picked up without restarting
    /// the daemon.  Default: 300 seconds (5 minutes).
    ///
    /// Can also be set via `MAKOD_OIDC_JWKS_REFRESH_SECS`.
    #[arg(
        long,
        value_name = "SECS",
        default_value_t = 300,
        env = "MAKOD_OIDC_JWKS_REFRESH_SECS"
    )]
    oidc_jwks_refresh_secs: u64,

    /// Maximum request body size for `POST /edifact`, in bytes.
    ///
    /// Defaults to 10 MiB. Increase for large MSCONS interchanges;
    /// decrease to limit memory usage under load.
    ///
    /// Can also be set via the `MAKOD_HTTP_MAX_BODY_BYTES` environment variable.
    #[arg(
        long,
        value_name = "BYTES",
        default_value_t = 10_485_760,
        env = "MAKOD_HTTP_MAX_BODY_BYTES"
    )]
    http_max_body_bytes: usize,

    /// Number of events between automatic workflow snapshots.
    ///
    /// After every N events on a stream a snapshot is written so future command
    /// dispatches replay at most N tail events rather than the full stream.
    /// Lower values write more frequently (lower replay latency, higher write
    /// amplification). Higher values write less often (higher latency on cold
    /// starts, lower I/O overhead for write-heavy workflows).
    ///
    /// Defaults to 100. Use 1 to always snapshot; use 0 to disable snapshots
    /// entirely (not recommended in production).
    ///
    /// Can also be set via the `MAKOD_SNAPSHOT_INTERVAL` environment variable.
    #[arg(
        long,
        value_name = "N",
        default_value_t = 100,
        env = "MAKOD_SNAPSHOT_INTERVAL"
    )]
    snapshot_interval: u64,

    /// How often (in seconds) the projection checkpoint worker persists its
    /// cursor to SlateDB.
    ///
    /// A shorter interval reduces replay time after an unclean restart at the
    /// cost of more I/O. Set to 0 to disable the projection checkpoint worker.
    ///
    /// Defaults to 60 seconds.
    ///
    /// Can also be set via the `MAKOD_PROJECTION_CHECKPOINT_INTERVAL` environment variable.
    #[arg(
        long,
        value_name = "SECS",
        default_value_t = 60,
        env = "MAKOD_PROJECTION_CHECKPOINT_INTERVAL"
    )]
    projection_checkpoint_interval: u64,

    /// TCP address on which the API-Webdienste Strom server listens.
    ///
    /// When set, makod exposes the BDEW API-Webdienste Strom endpoints
    /// (Control Measures v1, MaLo Identification v1). Disabled when omitted.
    ///
    /// Examples: `127.0.0.1:8090`, `0.0.0.0:8090`
    ///
    /// Can also be set via the `MAKOD_API_WEBDIENSTE_ADDR` environment variable.
    #[arg(long, value_name = "ADDR", env = "MAKOD_API_WEBDIENSTE_ADDR")]
    api_webdienste_addr: Option<std::net::SocketAddr>,

    /// Acknowledge that an external distributed lock (e.g. S3 conditional-put
    /// or DynamoDB conditional write) protects against concurrent multi-instance
    /// inbox duplication.
    ///
    /// By default makod emits `tracing::error!` at startup when the inbox store
    /// is wired, because running multiple instances without a distributed lock
    /// will silently deduplicate AS4 messages across instances. Pass this flag
    /// only when your infrastructure provides that guarantee.
    ///
    /// Can also be set via the `MAKOD_ALLOW_MULTI_INSTANCE` environment variable.
    #[arg(long, env = "MAKOD_ALLOW_MULTI_INSTANCE", default_value_t = false)]
    allow_multi_instance: bool,

    /// TCP address on which the AS4 inbound transport listens.
    ///
    /// When set, makod exposes `POST /as4/inbox` accepting BDEW EDIFACT
    /// UserMessages delivered via AS4/ebMS3. This is the mandatory production
    /// transport since 2024-04-01 (electricity) / 2025-04-01 (gas).
    ///
    /// Requires `--as4-signing-key-pem` and `--as4-signing-cert-pem`.
    ///
    /// Examples: `0.0.0.0:4080`, `127.0.0.1:4080`
    ///
    /// Can also be set via the `MAKOD_AS4_ADDR` environment variable.
    #[arg(long, value_name = "ADDR", env = "MAKOD_AS4_ADDR")]
    as4_addr: Option<std::net::SocketAddr>,

    /// PEM-encoded EC (BrainpoolP256r1) private key used to sign outbound AS4
    /// SOAP messages (WS-Security XML-DSig) and synchronous receipts.
    ///
    /// BDEW AS4-Profil v1.2 §2.2.6.2.2 mandates ECDSA over BrainpoolP256r1;
    /// this is the signing half of the keypair, distinct from
    /// `--as4-decryption-key-pem`. Required when `--as4-addr` is set.
    ///
    /// Prefer `as4.signing_key_pem_file` in `makod.toml`: a key passed as a
    /// flag is visible in `ps` output, and one passed by environment variable
    /// is visible to anything that can read the process environment.
    ///
    /// Can also be set via the `MAKOD_AS4_SIGNING_KEY_PEM` environment variable.
    #[arg(
        long,
        value_name = "PEM",
        env = "MAKOD_AS4_SIGNING_KEY_PEM",
        hide_env_values = true,
        value_parser = |s: &str| Ok::<SecretString, std::convert::Infallible>(SecretString::new(s.into())),
    )]
    as4_signing_key_pem: Option<SecretString>,

    /// PEM-encoded X.509 certificate corresponding to `--as4-signing-key-pem`.
    ///
    /// Embedded in the WS-Security `<wsse:BinarySecurityToken>` so receiving
    /// MSHs can verify the signature without out-of-band key exchange.
    /// Required when `--as4-addr` is set.
    ///
    /// Can also be set via the `MAKOD_AS4_SIGNING_CERT_PEM` environment variable.
    #[arg(long, value_name = "PEM", env = "MAKOD_AS4_SIGNING_CERT_PEM")]
    as4_signing_cert_pem: Option<String>,

    /// PEM-encoded X.509 CA certificate used as the AS4 WS-Security trust anchor.
    ///
    /// **Required for production.** Set this to the BDEW/BNetzA PKI Certificate
    /// Authority certificate so that inbound AS4 messages from counterparties
    /// (whose certs are issued by the BDEW CA) pass signature verification.
    ///
    /// When omitted, the operator's own signing certificate is used as the
    /// trust anchor — this means ALL inbound messages from real BDEW participants
    /// will be rejected, and a startup `error!` log is emitted.
    ///
    /// Download the BDEW PKI CA certificate from the BDEW Marktpartner portal.
    ///
    /// Can also be set via the `MAKOD_AS4_TRUST_ANCHOR_PEM` environment variable.
    #[arg(long, value_name = "PEM", env = "MAKOD_AS4_TRUST_ANCHOR_PEM")]
    as4_trust_anchor_pem: Option<String>,

    /// BDEW party ID (13-digit GLN) of this operator's AS4 Message Service Handler.
    ///
    /// Used as the `<eb:PartyId>` in generated AS4 signal messages (receipts,
    /// errors). Defaults to the primary `[[party]]` MP-ID when omitted.
    ///
    /// Can also be set via the `MAKOD_AS4_PARTY_ID` environment variable.
    #[arg(long, value_name = "GLN", env = "MAKOD_AS4_PARTY_ID")]
    as4_party_id: Option<String>,

    /// PEM-encoded ECDSA private key for AS4 inbound **decryption** (own encryption identity).
    ///
    /// This is the operator's own EC (BrainpoolP256r1) private key corresponding to
    /// the encryption certificate published to BDEW trading partners. Trading partners
    /// use the public key from this certificate to encrypt outbound AS4 messages.
    /// Provide this key to decrypt inbound AS4 messages.
    ///
    /// Separate from `--as4-signing-key-pem`: BDEW requires distinct keypairs for
    /// signing (ECDSA) and encryption (ECDH-ES), both using BrainpoolP256r1.
    ///
    /// Can also be set via the `MAKOD_AS4_DECRYPTION_KEY_PEM` environment variable.
    #[arg(
        long,
        value_name = "PEM",
        env = "MAKOD_AS4_DECRYPTION_KEY_PEM",
        hide_env_values = true,
        value_parser = |s: &str| Ok::<SecretString, std::convert::Infallible>(SecretString::new(s.into())),
    )]
    as4_decryption_key_pem: Option<SecretString>,

    /// Register a trading-partner encryption certificate for outbound AS4 encryption.
    ///
    /// Format: `<GLN>=<PEM>` where PEM is the partner's X.509 encryption certificate
    /// (not their signing certificate — BDEW uses separate keypairs for each).
    ///
    /// Repeat the flag to register multiple partners. Required for every partner
    /// when `security.encrypt = true` (which is the BDEW-compliant default).
    ///
    /// BDEW AS4-Profil v1.2 §2.2.6.2.2: the recipient's encryption certificate
    /// (BrainpoolP256r1) is used for ECDH-ES key agreement.
    ///
    /// Can also be set via the `MAKOD_AS4_PARTNER_CERT` environment variable
    /// (comma-separated for multiple entries).
    #[arg(
        long,
        value_name = "GLN=PEM",
        env = "MAKOD_AS4_PARTNER_CERT",
        value_delimiter = ','
    )]
    as4_partner_cert: Vec<String>,

    /// DEV/TEST ONLY: allow AS4 operation without encryption material.
    ///
    /// BDEW AS4-Profil v1.2 §2.2.6.2.2 requires every production AS4 message
    /// to be encrypted. Without this flag, `makod` refuses to start when AS4
    /// is active but the inbound decryption key (`--as4-decryption-key-pem`)
    /// is missing, or when a registered AS4 partner has no encryption
    /// certificate (`--as4-partner-cert`).
    ///
    /// Setting this flag downgrades both refusals to warnings. Never set it
    /// against the regulated market — messages would flow unencrypted.
    #[arg(long, env = "MAKOD_ALLOW_UNENCRYPTED_AS4")]
    allow_unencrypted_as4: bool,

    /// INTEROP DEBUGGING ONLY: treat a missing or mismatched synchronous
    /// `eb:Receipt` as a warning instead of a delivery failure.
    ///
    /// The BDEW MaKo AS4 MEP requires the receiver to return a synchronous
    /// `eb:Receipt` on the same HTTP connection. By default `makod` only
    /// acknowledges an outbox entry after that receipt is verified to
    /// reference the sent message id — an unverified delivery is retried and
    /// eventually dead-lettered. This flag downgrades the check to a warning
    /// for sessions against non-conformant counterparties.
    #[arg(long, env = "MAKOD_AS4_LENIENT_RECEIPTS")]
    as4_lenient_receipts: bool,

    /// Disable authentication on the `:8090` API-Webdienste port.
    ///
    /// By default every :8090 route requires a bearer/OIDC token and the
    /// Cedar `UseWebdienste` action. Set this only when a fronting proxy
    /// terminates mTLS with the BDEW PKI CA and enforces access itself.
    #[arg(long, env = "MAKOD_WEBDIENSTE_ALLOW_UNAUTHENTICATED")]
    webdienste_allow_unauthenticated: bool,

    /// Register a trading-partner AS4 endpoint for outbound EDIFACT delivery.
    ///
    /// Format: `<GLN>=<HTTPS-URL>` (e.g.
    /// `9900000000001=https://partner.example/as4/inbox`).
    ///
    /// Repeat the flag to register multiple partners.  Messages destined for
    /// an unregistered GLN are rescheduled with exponential backoff until a
    /// matching entry is added and the process is restarted.
    ///
    /// Required to deliver APERAK, CONTRL, and other EDIFACT messages via AS4.
    /// Has no effect if `--as4-signing-key-pem` and `--as4-signing-cert-pem`
    /// are not provided.
    ///
    /// Can also be set via the `MAKOD_AS4_PARTNER` environment variable
    /// (comma-separated pairs for multiple entries).
    #[arg(
        long,
        value_name = "GLN=URL",
        env = "MAKOD_AS4_PARTNER",
        value_delimiter = ','
    )]
    as4_partner: Vec<String>,

    /// Register a trading-partner callback URL for the MaLo Identification API.
    ///
    /// Format: `<GLN>=<HTTPS-URL>` (e.g.
    /// `9900000000001=https://lf.example/api-webdienste`).
    ///
    /// The URL is the base URL of the LF's API-Webdienste Strom server.
    /// `makod` appends `/maloId/dataForMarketLocationPositive/v1` (or
    /// `/maloId/dataForMarketLocationNegative/v1`) automatically.
    ///
    /// For dynamic URL discovery, configure `--verzeichnisdienst-url` instead.
    /// Static entries in this flag always take priority over Verzeichnisdienst
    /// lookups.
    ///
    /// Repeat the flag to register multiple LF partners.
    ///
    /// Can also be set via the `MAKOD_MALOID_PARTNER` environment variable
    /// (comma-separated pairs for multiple entries).
    #[arg(
        long,
        value_name = "GLN=URL",
        env = "MAKOD_MALOID_PARTNER",
        value_delimiter = ','
    )]
    maloid_partner: Vec<String>,

    /// Base URL of the BDEW Verzeichnisdienst for dynamic API-Webdienste
    /// endpoint discovery.
    ///
    /// When set, `makod` queries the Verzeichnisdienst for each LF's
    /// `maloIdV1` endpoint URL at delivery time and caches the result in the
    /// partner store.  A background task refreshes all cached entries every
    /// 5 minutes to catch partner URL changes.
    ///
    /// Example: `https://verzeichnisdienst.energy-solution.de`
    ///
    /// When omitted, only static `--maloid-partner` entries are used.
    ///
    /// Can also be set via the `MAKOD_VERZEICHNISDIENST_URL` environment variable.
    #[arg(long, value_name = "URL", env = "MAKOD_VERZEICHNISDIENST_URL")]
    verzeichnisdienst_url: Option<String>,

    /// ERP webhook URL for outbound BO4E event delivery.
    ///
    /// When set, `makod` starts an `OutboxErpWorker` that POSTs every ERP-
    /// relevant outbox entry (BO4E payload) to this URL as an `ErpEvent`
    /// JSON object.  The ERP endpoint must accept `POST` with
    /// `Content-Type: application/json` and return HTTP 2xx on success.
    ///
    /// When omitted, ERP outbound notifications are suppressed (only logged
    /// via `LogErpAdapter`).
    ///
    /// Can also be set via the `MAKOD_ERP_WEBHOOK_URL` environment variable.
    #[arg(long, value_name = "URL", env = "MAKOD_ERP_WEBHOOK_URL")]
    erp_webhook_url: Option<String>,

    /// Webhook URL for outbound EDIFACT delivery without AS4.
    ///
    /// When set and `--as4-signing-key-pem` is not configured, every outbound
    /// EDIFACT message (UTILMD 55003, APERAK, CONTRL, …) is POSTed to this
    /// URL as a CloudEvents 1.0 JSON object
    /// (`type = "de.mako.edifact.outbound"`) instead of being queued for AS4
    /// delivery.  Rendered EDIFACT wire bytes are included in `data.edifact`.
    ///
    /// Intended for development, testing, and direct ERP-to-ERP integrations
    /// that prefer HTTP over the BDEW AS4 transport profile.
    ///
    /// Can also be set via the `MAKOD_EDIFACT_OUTBOX_WEBHOOK_URL` environment
    /// variable.
    #[arg(long, value_name = "URL", env = "MAKOD_EDIFACT_OUTBOX_WEBHOOK_URL")]
    edifact_outbox_webhook_url: Option<String>,

    /// §20b EnWG Netzzugangsplattform endpoint URL (optional).
    ///
    /// No platform interface exists yet (no BNetzA Festlegung under §20b
    /// Abs. 3); when unset, §20b requests are delivered to the ERP webhook as
    /// `de.mako.netzzugang.uebermittlungsbedarf` CloudEvents so the operator
    /// can submit them via the Netzbetreiber's Webportal.
    #[arg(long, value_name = "URL", env = "MAKOD_NETZZUGANG_ENDPOINT_URL")]
    netzzugang_endpoint_url: Option<String>,

    /// Allow startup without AS4 signing credentials and without an EDIFACT
    /// outbox webhook.  By default (when this flag is absent) makod refuses
    /// to start if neither `--as4-signing-key-pem` nor
    /// `--edifact-outbox-webhook-url` is set, because outbound EDIFACT
    /// delivery would silently fail for all messages.
    ///
    /// Set this flag only in integration-test or CI environments where
    /// outbound delivery is intentionally disabled.
    ///
    /// Can also be set via the `MAKOD_ALLOW_NO_AS4_SIGNING` environment
    /// variable.
    #[arg(long, env = "MAKOD_ALLOW_NO_AS4_SIGNING", default_value_t = false)]
    allow_no_as4_signing: bool,

    /// Validate configuration, adapter coverage, and profile availability, then exit.
    ///
    /// In check mode makod:
    ///   1. Opens (or creates) the store as normal.
    ///   2. Runs ProcessRegistry reconciliation.
    ///   3. Builds the EngineContext (validates all profile covers and PIDs).
    ///   4. Runs the adapter coverage validation loop.
    ///   5. Exits 0 on success, non-zero on failure.
    ///
    /// No background workers (outbox, deadline scheduler, ingest transport)
    /// are started. The data-dir exclusive lock is still acquired to verify
    /// no other instance is running against the same directory. Safe to call
    /// from CI pipelines and Kubernetes init containers.
    ///
    /// Can also be set via the `MAKOD_CHECK` environment variable.
    #[arg(long, env = "MAKOD_CHECK", default_value_t = false)]
    check: bool,

    /// Marktrollen this makod instance is licensed for (comma-separated).
    ///
    /// When set, only commands whose required Marktrolle appears in this list
    /// are accepted via `POST /api/v1/commands`.  Commands for unconfigured
    /// roles are rejected with `422 role_not_configured`.
    ///
    /// Examples: `LF` (electricity supplier), `LF,LFG` (dual-fuel supplier),
    /// `NB,MSB` (integrated Stadtwerke acting as DSO and meter operator).
    ///
    /// When omitted, role configuration checking is skipped (permissive mode —
    /// all registry-permitted roles are accepted).  Set this in production to
    /// catch misconfigured ERP connectors at the API boundary.
    ///
    /// Can also be set via the `MAKOD_MARKTROLLEN` environment variable
    /// (comma-separated, e.g. `MAKOD_MARKTROLLEN=LF,LFG`).
    #[arg(
        long,
        value_name = "ROLES",
        env = "MAKOD_MARKTROLLEN",
        value_delimiter = ','
    )]
    marktrollen: Vec<String>,

    /// BDEW Marktrollen active for PID routing (comma-separated).
    ///
    /// Controls which inbound EDIFACT PID → workflow routes are registered at
    /// startup.  Shared PIDs (e.g. ORDRSP 19001/19002) are registered to
    /// different workflows depending on role:
    ///
    /// - `NB`  → ORDRSP 19001/19002 routes to `gpke-konfiguration`
    ///   (GPKE Konfiguration: NB receives ORDRSP from MSB in response to ORDERS 17134/17135)
    /// - `NMSB` → ORDRSP 19001/19002 routes to `wim-geraeteubernahme`
    ///   (WiM Geräteübernahme: nMSB receives ORDRSP from NB in response to ORDERS 17001/17009)
    ///
    /// **If both roles are listed and they share a PID, `build` will panic** —
    /// run separate makod instances with disjoint role sets instead.
    ///
    /// Available roles: `NB`, `LF`, `MSB`, `NMSB`, `AMSB`, `BKV`, `UENB`, `BIKO`,
    /// `ESA`
    ///
    /// When omitted, all PIDs are registered unconditionally (backward-compatible
    /// default, equivalent to `--deployment-roles NB,LF,MSB,BKV,UENB,BIKO`).
    ///
    /// Can also be set via the `MAKOD_DEPLOYMENT_ROLES` environment variable
    /// (comma-separated, e.g. `MAKOD_DEPLOYMENT_ROLES=NB,MSB`).
    #[arg(
        long,
        value_name = "ROLES",
        env = "MAKOD_DEPLOYMENT_ROLES",
        value_delimiter = ','
    )]
    deployment_roles: Vec<String>,

    /// Cluster-internal marktd base URL, e.g. `http://marktd:8180`.
    ///
    /// When set, inbound ESA messages (REQOTE 35003 Werteanfrage, ORDERS 17007
    /// Bestellung) are gated against the marktd consent registry: a revoked
    /// consent or an unestablished framework agreement is answered with an
    /// Ablehnung instead of being processed. Without it the gate is disabled.
    #[arg(long, value_name = "URL", env = "MAKOD_MARKTD_URL")]
    marktd_url: Option<String>,

    /// Bearer token for machine-to-machine calls to `--marktd-url`.
    #[arg(long, value_name = "TOKEN", env = "MAKOD_MARKTD_API_KEY")]
    marktd_api_key: Option<String>,

    /// Shared secret for Standard Webhooks signing on ERP webhook POSTs.
    ///
    /// When set alongside `--erp-webhook-url`, every webhook POST includes an
    /// `webhook-signature` header so the ERP can verify authenticity and refuse a replay.
    /// The ERP endpoint must verify the signature before processing.
    ///
    /// When omitted, the webhook is sent without a signature header.
    ///
    /// Can also be set via the `MAKOD_ERP_WEBHOOK_SECRET` environment variable.
    #[arg(
        long,
        value_name = "SECRET",
        env = "MAKOD_ERP_WEBHOOK_SECRET",
        hide_env_values = true,
        value_parser = |s: &str| Ok::<SecretString, std::convert::Infallible>(SecretString::new(s.into())),
    )]
    erp_webhook_secret: Option<SecretString>,

    /// Number of Tokio worker threads.
    ///
    /// Defaults to the number of logical CPUs on the host. Set to `1` for
    /// deterministic single-threaded operation or reduce to leave headroom for
    /// other processes on the same core. Setting this higher than the CPU count
    /// is generally harmful.
    ///
    /// Can also be set via the `MAKOD_WORKER_THREADS` environment variable.
    #[arg(long, value_name = "N", env = "MAKOD_WORKER_THREADS")]
    worker_threads: Option<usize>,

    /// Maximum time in seconds to wait for the store to flush and close cleanly
    /// after a shutdown signal is received.
    ///
    /// When the timeout expires before the store finishes closing, an error is
    /// logged and the process exits immediately. Increase this value when using
    /// object-store backends (S3, GCS, Azure) that may need extra time to flush
    /// write-ahead buffers to remote storage.
    ///
    /// Defaults to 30 seconds.
    ///
    /// Can also be set via the `MAKOD_SHUTDOWN_TIMEOUT_SECS` environment variable.
    #[arg(
        long,
        value_name = "SECS",
        default_value_t = 30,
        env = "MAKOD_SHUTDOWN_TIMEOUT_SECS"
    )]
    shutdown_timeout_secs: u64,

    /// Maximum number of events per stream before `append` is rejected.
    ///
    /// Applies a per-stream circuit-breaker that prevents runaway retry loops
    /// or malicious AS4 senders from causing unbounded event stream growth
    /// (which would increase replay latency proportionally). When a stream
    /// reaches this limit, `EngineError::StreamQuotaExceeded` is returned and
    /// the message is dead-lettered.
    ///
    /// A typical GPKE Lieferbeginn process has at most ~15 events; a MABIS
    /// billing stream ~50. The default 10 000 provides 650× safety headroom.
    ///
    /// Set to 0 to disable the limit (not recommended for production).
    ///
    /// Can also be set via the `MAKOD_MAX_STREAM_EVENTS` environment variable.
    #[arg(
        long,
        value_name = "N",
        default_value_t = 10_000,
        env = "MAKOD_MAX_STREAM_EVENTS"
    )]
    max_stream_events: u64,

    /// How often (in seconds) the deadline scheduler polls for due deadlines.
    ///
    /// The deadline scheduler fires compensation commands (e.g. GPKE 24h
    /// APERAK timeout, MABIS 1-Werktag Prüfmitteilung deadline) at this
    /// interval. For Redispatch workflows with 5-minute regulatory windows,
    /// reduce this to 30 seconds or less.
    ///
    /// Defaults to 30 seconds. Minimum 1 second.
    ///
    /// Can also be set via the `MAKOD_DEADLINE_POLL_INTERVAL_SECS` environment variable.
    #[arg(
        long,
        value_name = "SECS",
        default_value_t = 30,
        env = "MAKOD_DEADLINE_POLL_INTERVAL_SECS"
    )]
    deadline_poll_interval_secs: u64,

    /// `[[party]]` entries loaded from `makod.toml`.
    ///
    /// Not settable via CLI flags.  Populated by `apply_config_file` when the
    /// TOML config file contains `[[party]]` entries.  When non-empty, takes
    /// precedence over the primary `[[party]]` MP-ID for GLN routing.
    #[arg(skip)]
    parties: Vec<config::PartyConfig>,

    /// Subcommand to run instead of the daemon.
    #[command(subcommand)]
    command: Option<CliCommand>,
}

/// Top-level subcommands for `makod`.
///
/// When no subcommand is given, `makod` starts the daemon normally.
#[derive(Debug, Clone, clap::Subcommand)]
enum CliCommand {
    /// Run all pending state migrations and exit.
    ///
    /// Connects to the configured event store, executes the same migration
    /// pipeline as `POST /admin/migrations`, prints a JSON report to stdout,
    /// and exits with status 0 on success or 1 on any migration failure.
    ///
    /// Use this as a Kubernetes `initContainer` or Compose `depends_on` step
    /// to ensure schema migrations are applied before the daemon starts.
    ///
    /// Example:
    /// ```text
    /// makod --config makod.toml migrate
    /// ```
    Migrate,
}

#[derive(Debug, Clone, Copy, ValueEnum, Default)]
enum ObjectStoreBackend {
    /// Local filesystem (use `--data-dir`) or volatile in-memory when omitted.
    #[default]
    Local,
    /// AWS S3 or compatible (requires `--s3-bucket`; reads standard AWS env vars).
    ///
    /// Credential env vars: `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
    /// `AWS_REGION`. For MinIO or S3-compatible endpoints, set `--s3-endpoint`.
    S3,
    /// Google Cloud Storage (requires `--gcs-bucket`).
    ///
    /// Credential env vars: `GOOGLE_SERVICE_ACCOUNT_KEY` (JSON key contents),
    /// `GOOGLE_SERVICE_ACCOUNT` (service account email), or
    /// `GOOGLE_APPLICATION_CREDENTIALS` (path to key file).
    Gcs,
    /// Azure Blob Storage (requires `--azure-container` and `--azure-account`).
    ///
    /// Credential env vars: `AZURE_STORAGE_ACCOUNT_KEY`, `AZURE_CLIENT_ID` +
    /// `AZURE_TENANT_ID` + `AZURE_CLIENT_SECRET` (service principal), or
    /// `AZURE_STORAGE_SAS_TOKEN`.
    Azure,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn as_filter(self) -> tracing::Level {
        match self {
            LogLevel::Trace => tracing::Level::TRACE,
            LogLevel::Debug => tracing::Level::DEBUG,
            LogLevel::Info => tracing::Level::INFO,
            LogLevel::Warn => tracing::Level::WARN,
            LogLevel::Error => tracing::Level::ERROR,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LogFormat {
    /// Human-readable multi-line format (default for development).
    Pretty,
    /// Single-line compact format.
    Compact,
    /// Structured JSON (for log aggregators like Loki / OpenSearch).
    Json,
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() -> anyhow::Result<()> {
    use clap::{CommandFactory, FromArgMatches};

    // If MAKOD_DATA_DIR is set to an empty string (e.g. Docker `-e MAKOD_DATA_DIR=`
    // to clear the image's baked-in default), remove it from the environment before
    // clap parses arguments.  Clap treats an env var that is present-but-empty as
    // "the flag was invoked with no value", which fails for required-value args.
    //
    // Safety: main() runs single-threaded before any call to thread::spawn or
    // tokio::runtime::Builder::build(), so no other thread can race on the
    // environment here.
    if matches!(std::env::var("MAKOD_DATA_DIR").as_deref(), Ok("")) {
        // SAFETY: single-threaded at this point; no concurrent env access.
        #[allow(unsafe_code)]
        // SAFETY: main() is single-threaded before any tokio::spawn or thread::spawn
        // call, so no other thread races on the environment.
        unsafe {
            std::env::remove_var("MAKOD_DATA_DIR");
        }
    }

    // Use the low-level ArgMatches API so we can detect which fields still
    // hold their built-in default values and fill those from the config file
    // without overwriting explicit CLI / env-var settings.
    let matches = Cli::command().get_matches();
    let mut cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());

    // Load and apply the TOML config file (if --config / MAKOD_CONFIG is set).
    // Must happen before init_tracing so the file can control log level/format.
    if let Some(ref path) = cli.config.clone() {
        let cfg = config::load(path)?;
        apply_config_file(cfg, &matches, &mut cli)?;
    }

    // Build the Tokio runtime explicitly so the thread count is controllable
    // via `--worker-threads` / `MAKOD_WORKER_THREADS`. Defaulting to
    // `available_parallelism` matches what `#[tokio::main]` does internally.
    let worker_threads = match cli.worker_threads {
        Some(n) => n,
        None => std::thread::available_parallelism()
            .map(std::num::NonZero::get)
            .unwrap_or(1),
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .enable_all()
        .build()
        .expect("failed to build Tokio runtime");
    rt.block_on(async_main(cli))
}

async fn async_main(cli: Cli) -> anyhow::Result<()> {
    // Hold the guard for the process lifetime — dropping it flushes OTel spans.
    let _otel_guard = init_tracing(&cli);

    use party_registry::MpIdRegistry;

    // ── GLN registry ─────────────────────────────────────────────────────────
    //
    // `[[party]]` entries are the single source of truth for all GLN identity.
    // There is no `--tenant-id` fallback — a config file with at least one
    // `[[party]]` entry is required.
    anyhow::ensure!(
        !cli.parties.is_empty(),
        "makod requires at least one [[party]] entry in makod.toml.\n\
         Create a config file (--config / MAKOD_CONFIG) with:\n\
         \n\
         [[party]]\n\
         mp_id   = \"<13-digit-GLN>\"\n\
         roles = [\"NB\", \"LF\", \"MSB\"]  # adjust to operator's Marktrollen\n\
         \n\
         See site/content/docs/services/makod.md for the full configuration reference."
    );

    let mp_id_registry: Arc<MpIdRegistry> =
        Arc::new(MpIdRegistry::from_config(&cli.parties).context("invalid [[party]] config")?);

    // makod is single-tenant: the primary Marktpartner-ID *is* the tenant, and
    // this UUID scopes every event stream, outbox entry and cache key. Derived
    // once here rather than at each of the dozen sites that used to re-derive it.
    let tenant_id = mako_engine::ids::TenantId::from_party_id(mp_id_registry.primary_mp_id());

    info!(
        primary_mp_id  = %mp_id_registry.primary_mp_id(),
        primary_agency = %mp_id_registry.primary_agency(),
        own_mp_ids     = ?mp_id_registry.own_mp_ids().collect::<Vec<_>>(),
        party_count  = cli.parties.len(),
        "GLN registry built from [[party]] entries",
    );

    // ── Auto-derive engine roles from [[party]] ───────────────────────────────
    //
    // When --deployment-roles / MAKOD_DEPLOYMENT_ROLES is not set explicitly,
    // derive from the union of all [[party]] roles.  This eliminates the need
    // to configure the same role set in two places.
    let effective_deployment_roles = if cli.deployment_roles.is_empty() {
        let derived = mp_id_registry.deployment_role_strings();
        if !derived.is_empty() {
            info!(
                roles = ?derived,
                "deployment roles auto-derived from [[party]] entries \
                 (set --deployment-roles explicitly to override)",
            );
        }
        parse_deployment_roles(&derived)
    } else {
        parse_deployment_roles(&cli.deployment_roles)
    };

    // ── Auto-derive marktrollen from [[party]] ────────────────────────────────
    //
    // When --marktrollen / MAKOD_MARKTROLLEN is not set, derive from [[party]].
    let effective_marktrollen: Vec<String> = if cli.marktrollen.is_empty() {
        mp_id_registry
            .all_roles()
            .iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        cli.marktrollen.iter().map(|s| s.to_uppercase()).collect()
    };

    let store = open_store(&cli).await?;
    // Apply per-stream event quota — a circuit-breaker that prevents
    // runaway streams from causing unbounded replay latency.  Disabled when
    // --max-stream-events=0.
    let store = if cli.max_stream_events > 0 {
        store.with_max_stream_events(cli.max_stream_events)
    } else {
        store
    };

    // Refuse a second writer on the same SlateDB path before anything else
    // touches it. Held for the process lifetime.
    let _data_dir_lock = startup::lock_data_dir(cli.data_dir.as_deref())?;

    // `makod migrate` runs pending FV migrations and exits, so an operator can
    // use it as a Kubernetes initContainer or a Compose `depends_on` step
    // without starting the HTTP/AS4 servers.
    if matches!(cli.command, Some(CliCommand::Migrate)) {
        return run_migrations(&store).await;
    }

    // ── Dead-letter sink + worker ────────────────────────────────────
    //
    // The sink enqueues rejected messages into a bounded mpsc channel; the
    // worker drains the channel to SlateDB in the background.
    //
    // Three clones are made:
    //  • `dl_sink_shutdown` — signals graceful shutdown from the teardown path
    //  • `dl_sink_ingest`   — shared between the REST and AS4 EdifactApiState
    //                         instances so ingest-path rejections also land in
    //                         the durable dead-letter queue (§ 147 AO / GoBD)
    //  • the original `dl_sink` — consumed by EngineBuilder below
    let (dl_sink, dl_worker) = store.as_dead_letter_sink();
    let dl_sink_shutdown = dl_sink.clone();
    let dl_sink_ingest = dl_sink.clone();
    let dl_sink_workers = dl_sink.clone();
    let dl_worker_handle = tokio::spawn(dl_worker.run());

    let ctx = startup::build_engine(&store, dl_sink, effective_deployment_roles);

    let inbox_store = store.as_inbox_store();
    // Clone for the daily purge worker below; the original may be moved into
    // the AS4 ingest handler when --as4-addr is set.
    let inbox_store_for_purge = store.as_inbox_store();

    // ── MaLo cache ────────────────────────────────────────────────────────────
    //
    // Shared read-side snapshot of the operator's MaLo master data.
    // Populated via `PUT /admin/malo/{malo_id}` or the ERP command source.
    let malo_cache = std::sync::Arc::new(malo_cache::SlateDbMaloCache::new(store.clone()));

    let modules = ctx.registered_modules();
    let pid_count = ctx.pid_router().len();
    info!(
        modules = ?modules,
        pid_count,
        "Mako engine started",
    );
    info!(
        "AS4 inbox deduplication store wired (SlateDbInboxStore); \
         SSI transactions provide linearisable dedup within this process"
    );
    if !cli.allow_multi_instance {
        tracing::warn!(
            "SlateDbInboxStore uses SSI (Serializable Snapshot Isolation) within a \
             single SlateDB Db handle. This is safe and linearisable within one makod \
             instance. Multi-instance scale-out (horizontal scaling) WILL cause duplicate \
             AS4 message processing because two independent Db handles on the same storage \
             path do not share SSI isolation boundaries. \
             Do NOT run multiple makod instances against the same --data-dir without an \
             external distributed lock (e.g. object-storage conditional-put or DynamoDB \
             conditional writes). Use --allow-multi-instance to suppress this warning when \
             a distributed lock is in place."
        );
    } else {
        tracing::info!(
            "Multi-instance mode acknowledged via --allow-multi-instance. \
             Ensure an external distributed lock protects inbox deduplication."
        );
    }

    // ── Startup: validate MessageAdapter coverage ─────────────────
    //
    // Each domain workflow must have a registered adapter for every known
    // BDEW format version. A missing adapter means cross-FV inbound messages
    // would be silently dead-lettered. Panics on missing coverage.
    startup::validate_adapter_coverage();

    // Verify every PidRouter-registered workflow has a dispatch arm.
    // Panics if a domain crate registers a new PID without a matching arm in
    // EdifactIngestDispatcher — prevents silent dead-lettering at runtime.
    startup::validate_dispatch_completeness(ctx.pid_router());

    // Hard-fail if any Noop store is active — a misconfigured deployment
    // (e.g. missing [outbox] section in makod.toml) must never silently
    // start with a Noop backend, whether in check mode or full daemon mode.
    ctx.assert_production_stores();

    // ── Configuration preflight ────────────────────────────────────────────
    //
    // Everything judgeable from the configuration alone — AS4 key material and
    // partner registry, Cedar policies, credentials for every authenticated
    // port, callback URLs, the ingest and egress transport rules. It runs above
    // the `--check` exit so the two cannot diverge: check mode validates exactly
    // what the daemon then boots with, and the boot consumes this same result.
    let extra_policies = read_cedar_policy_dir(&cli.cedar_policy_dir)
        .context("loading Cedar policy files from --cedar-policy-dir")?;
    let preflight_input = preflight::PreflightInput {
        primary_mp_id: mp_id_registry.primary_mp_id(),
        as4_inbound_enabled: cli.as4_addr.is_some(),
        http_enabled: cli.http_addr.is_some(),
        webdienste_enabled: cli.api_webdienste_addr.is_some(),
        webdienste_allow_unauthenticated: cli.webdienste_allow_unauthenticated,
        as4_partner: &cli.as4_partner,
        as4_partner_cert: &cli.as4_partner_cert,
        as4_signing_key_pem: cli.as4_signing_key_pem.as_ref(),
        as4_signing_cert_pem: cli.as4_signing_cert_pem.as_deref(),
        as4_trust_anchor_pem: cli.as4_trust_anchor_pem.as_deref(),
        as4_decryption_key_pem: cli.as4_decryption_key_pem.as_ref(),
        as4_party_id: cli.as4_party_id.as_deref(),
        allow_unencrypted_as4: cli.allow_unencrypted_as4,
        allow_no_as4_signing: cli.allow_no_as4_signing,
        edifact_outbox_webhook_url: cli.edifact_outbox_webhook_url.as_deref(),
        erp_webhook_url: cli.erp_webhook_url.as_deref(),
        netzzugang_endpoint_url: cli.netzzugang_endpoint_url.as_deref(),
        maloid_partner: &cli.maloid_partner,
        verzeichnisdienst_url: cli.verzeichnisdienst_url.as_deref(),
        marktd_url: cli.marktd_url.as_deref(),
        marktd_api_key: cli.marktd_api_key.as_deref(),
        auth_keys: &cli.auth_keys,
        cedar_policies: extra_policies,
        cedar_no_default_policy: cli.cedar_no_default_policy,
        oidc_issuer: cli.oidc_issuer.as_deref(),
        oidc_audience: cli.oidc_audience.as_deref(),
    };
    let mut checked = preflight::preflight(&preflight_input)?;
    preflight::warn_on_degraded_config(&preflight_input, cli.data_dir.is_some());

    // ── --check mode early exit ────────────────────────────────────────
    //
    // All configuration-derived checks have now run: profile validator, adapter
    // coverage, dispatch completeness, store connectivity, data-dir lock, and
    // the preflight above. In check mode we exit here — no workers, no
    // transports, no listeners, and nothing written: everything above this line
    // reads, so a pipeline can point `--check` at a live data directory.
    if cli.check {
        info!(
            as4_partners = checked.as4_profile.registry().len(),
            maloid_partners = checked.maloid_partners.len(),
            "check mode: all startup validations passed"
        );
        return Ok(());
    }

    // ── ProcessRegistry startup reconciliation ─────────────────────────
    //
    // On restart after a crash or after an operator accidentally deleted a
    // registry entry, inbound APERAKs can no longer be routed to their target
    // process.  Reconciliation scans all `process/` streams, loads the first
    // event from each, and re-registers any entries missing from the registry.
    //
    // Below the `--check` exit deliberately: this is the one startup step that
    // *writes*. `--check` is run by deployment pipelines against live data
    // directories and must be able to answer "would this config start?" without
    // changing the store it was pointed at.
    //
    // A one-time operation — it does NOT block the server from accepting
    // requests.
    match reconcile_process_registry(&store).await {
        // Logged unconditionally, including the zero case. This is the boot's
        // only write step, and "it ran and found nothing to do" is a different
        // statement from "it did not run" — the second is what `--check`
        // promises, and only an unconditional line can distinguish them in a
        // log or a test.
        Ok(0) => info!(count = 0, "ProcessRegistry reconciliation complete"),
        Ok(n) => tracing::warn!(
            count = n,
            "ProcessRegistry reconciliation complete: reconstructed missing routing entries",
        ),
        Err(e) => tracing::error!(
            error = %e,
            "ProcessRegistry reconciliation failed (non-fatal — engine will start anyway)",
        ),
    }

    // ── Graceful-shutdown token ────────────────────────────────────────────────
    //
    // All long-running background tasks and HTTP servers are wired to this
    // token.  When the OS delivers SIGTERM / Ctrl-C, we cancel the token and
    // every listener drains its in-flight requests before the store is closed.
    let shutdown_token = CancellationToken::new();

    // Listener tasks are joined on shutdown alongside the background workers:
    // `with_graceful_shutdown` drains in-flight requests, but nothing waits for
    // that drain unless the handle is kept, and an in-flight command handler
    // writes to the same event store the teardown is about to close.
    let mut server_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    // ── Optional: HTTP REST API server ────────────────────────────────────────
    //
    // Enabled by --http-addr / MAKOD_HTTP_ADDR. Provides POST /edifact as a
    // direct EDIFACT ingest alternative to AS4 transport.
    //
    // Construct the Platform once and share the Arc across HTTP and AS4 servers
    // to avoid registering all ~40 generated profile modules twice.
    let platform = Arc::new(Platform::with_all_profiles());

    // ── marktd client ─────────────────────────────────────────────────────────
    //
    // Optional: gates inbound ESA messages against the consent registry and
    // backs the M1 Konfigurationsprodukt guard. Shared by the ingest dispatcher
    // and the commands API.
    let marktd_client = cli.marktd_url.as_ref().map(|url| {
        Arc::new(mako_markt::marktd_client::MarktdClient::new(
            url.clone(),
            SecretString::from(cli.marktd_api_key.clone().unwrap_or_default()),
            mako_service::http::default_client(),
        ))
    });

    // ── Phase 2 ingest dispatcher ─────────────────────────────────────────────
    //
    // Shared across HTTP REST and AS4 ingest — translates parsed EDIFACT
    // messages to typed domain commands and executes them on workflow processes.
    // Also used by the AS4 loopback path for combined-role deployments.
    let ingest_dispatcher = Arc::new(
        ingest_dispatcher::EdifactIngestDispatcher::new(
            Arc::new(store.clone()),
            store.as_snapshot_store(),
            cli.snapshot_interval,
            mako_engine::ids::TenantId::from_party_id(mp_id_registry.primary_mp_id()),
        )
        .with_marktd_client(marktd_client.clone())
        .with_mp_id_registry(Arc::clone(&mp_id_registry)),
    );

    // ── Shared health state ───────────────────────────────────────────────────
    //
    // GET /health is mounted on every exposed port so that container
    // orchestrators (Kubernetes, ECS, Docker Swarm) have a consistent liveness
    // + readiness probe target.  The handler pings the SlateDB store; 503 means
    // the store is closed or unreachable.
    let health_state = health::HealthState::new(store.clone());

    // Build the shared reqwest client for outbound HTTP (OIDC JWKS fetch,
    // MaLo-ID callbacks, AS4 delivery worker).
    // A 30-second timeout prevents slow-loris hangs on JWKS or callback endpoints.
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| anyhow::anyhow!("HTTP client build: {e}"))?;

    // ── Build Cedar authorizer (shared by :8080 REST, /mcp, and :8090) ───────
    //
    // The keys, the policy text and the baseline choice were validated by the
    // preflight above; the only thing added here is the OIDC verifier, which
    // needs the network and therefore cannot run in `--check`.
    let oidc = if let Some(issuer) = cli.oidc_issuer.clone() {
        let audience = cli
            .oidc_audience
            .clone()
            .expect("preflight rejects an issuer without an audience");
        let verifier = mako_service::oidc::OidcVerifier::new(issuer, audience, &http_client)
            .await
            .context("OIDC verifier initialisation failed")?;
        let _jwks_refresh = verifier.spawn_refresh_task(
            http_client.clone(),
            cli.oidc_jwks_refresh_secs,
            shutdown_token.clone(),
        );
        Some(verifier)
    } else {
        None
    };
    let cedar = Arc::new(
        cedar_authz::CedarAuthorizer::new(
            std::mem::take(&mut checked.auth_keys),
            checked.cedar_policies.clone(),
            oidc,
            // makod is single-tenant: the primary MP-ID *is* the tenant
            // (see the API states below, which key on the same value).
            Some(mp_id_registry.primary_mp_id().to_owned()),
            checked.cedar_default_policy,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?,
    );

    let server_deps = startup::servers::ServerDeps {
        store: store.clone(),
        pid_router: ctx.pid_router().clone(),
        mp_id_registry: Arc::clone(&mp_id_registry),
        tenant_id,
        cedar: Arc::clone(&cedar),
        platform: Arc::clone(&platform),
        health_state: health_state.clone(),
        shutdown_token: shutdown_token.clone(),
        ingest_dispatcher: Arc::clone(&ingest_dispatcher),
        dead_letter_sink: dl_sink_ingest,
    };

    // ── Optional: HTTP REST API server (--http-addr) ──────────────────────────
    if let Some(addr) = cli.http_addr {
        server_handles.push(
            startup::servers::serve_http(
                &server_deps,
                startup::servers::HttpServerConfig {
                    addr,
                    max_body_bytes: cli.http_max_body_bytes,
                    snapshot_interval: cli.snapshot_interval,
                    marktrollen: effective_marktrollen.clone(),
                    malo_cache: Arc::clone(&malo_cache),
                    marktd_client: marktd_client.clone(),
                    as4_partner: &cli.as4_partner,
                    volatile_mode: cli.data_dir.is_none() && cli.allow_volatile,
                },
            )
            .await?,
        );
    }

    // ── Optional: AS4 inbound transport (--as4-addr) ──────────────────────────
    //
    // The mandatory production transport for BDEW EDIFACT since 2024-04-01
    // (electricity) / 2025-04-01 (gas). The signing material, decryption key and
    // trust anchor were all proven usable by the preflight; the `expect`s below
    // restate that invariant rather than re-checking it.
    if let Some(addr) = cli.as4_addr {
        server_handles.push(
            startup::servers::serve_as4(
                &server_deps,
                startup::servers::As4ServerConfig {
                    addr,
                    party_id: checked.as4_party_id.clone(),
                    signing_key_pem: cli
                        .as4_signing_key_pem
                        .clone()
                        .expect("preflight requires signing key material when --as4-addr is set"),
                    signing_cert_pem: cli
                        .as4_signing_cert_pem
                        .clone()
                        .expect("preflight requires a signing certificate when --as4-addr is set"),
                    trust_anchor_pem: cli.as4_trust_anchor_pem.clone(),
                    decryption_key_pem: cli.as4_decryption_key_pem.clone(),
                    inbox_store,
                    dedup_is_durable: cli.data_dir.is_some(),
                },
            )
            .await?,
        );
    } else {
        // AS4 not configured, so nothing consumes the inbox store. The operator
        // warning for this case comes from `preflight::warn_on_degraded_config`,
        // which runs in `--check` mode as well.
        drop(inbox_store);
    }

    // ── Optional: API-Webdienste Strom (--api-webdienste-addr) ────────────────
    //
    // BDEW API-Webdienste Strom: Control Measures v1 and MaLo Identification v1.
    if let Some(addr) = cli.api_webdienste_addr {
        server_handles.push(
            startup::servers::serve_webdienste(
                &server_deps,
                startup::servers::WebdiensteServerConfig {
                    addr,
                    max_body_bytes: cli.http_max_body_bytes,
                    allow_unauthenticated: cli.webdienste_allow_unauthenticated,
                },
            )
            .await?,
        );
    }

    // ── Background workers ────────────────────────────────────────────────────
    //
    // Outbox delivery, ERP webhook, deadline scheduler, projection checkpoint,
    // inbox purge — all spawned as Tokio tasks that exit on shutdown_token.
    // See `startup::spawn_workers` and `startup::WorkersConfig` for details.
    let mut workers = startup::spawn_workers(startup::WorkersConfig {
        ctx,
        store: store.clone(),
        inbox_store_for_purge,
        platform: Arc::clone(&platform),
        ingest_dispatcher: Arc::clone(&ingest_dispatcher),
        http_client,
        malo_cache: Arc::clone(&malo_cache),
        shutdown_token: shutdown_token.clone(),
        mp_id_registry: Arc::clone(&mp_id_registry),
        checked,
        as4_signing_key_pem: cli.as4_signing_key_pem.clone(),
        as4_signing_cert_pem: cli.as4_signing_cert_pem.clone(),
        as4_trust_anchor_pem: cli.as4_trust_anchor_pem.clone(),
        as4_lenient_receipts: cli.as4_lenient_receipts,
        dead_letter_sink: dl_sink_workers,
        erp_webhook_url: cli.erp_webhook_url.clone(),
        erp_webhook_secret: cli.erp_webhook_secret.clone(),
        edifact_outbox_webhook_url: cli.edifact_outbox_webhook_url.clone(),
        netzzugang_endpoint_url: cli.netzzugang_endpoint_url.clone(),
        marktd_client: marktd_client.clone(),
        snapshot_interval: cli.snapshot_interval,
        deadline_poll_interval_secs: cli.deadline_poll_interval_secs,
        projection_checkpoint_interval: cli.projection_checkpoint_interval,
        health_state: health_state.clone(),
    })
    .await?;

    for h in server_handles {
        workers.push(h);
    }

    // ── Graceful shutdown ──────────────────────────────────────────────
    //
    // The order matters, and every step must *complete* before the next: the
    // event store is closed at the end, and anything still writing to it when
    // that happens can leave a half-applied outbox acknowledge — the
    // counterparty holds the message, the outbox still shows it pending, and
    // the next start delivers it a second time.
    //
    //   1. Cancel the token. Listeners stop accepting and drain in-flight
    //      requests; every background worker returns at its next message or
    //      tick boundary.
    //   2. Join the workers. This is the step that makes closing the store
    //      safe; without it the cancel is only a request.
    //   3. Drain the dead-letter buffer, which has no other durable home.
    //   4. Close the store.
    //
    // A step that times out does not abort the shutdown — the remaining steps
    // still run, and the process reports the unclean exit at the end.
    wait_for_shutdown().await;
    info!(
        timeout_secs = cli.shutdown_timeout_secs,
        "Mako engine shutting down — cancelling listeners and workers",
    );
    shutdown_token.cancel();

    // The whole drain shares one budget. A worker that hangs must not be able
    // to spend the store-close allowance as well.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(cli.shutdown_timeout_secs);
    let remaining = || deadline.saturating_duration_since(tokio::time::Instant::now());

    let workers_stopped = workers.join_all(remaining()).await;

    // Close the DL channel first so `reject()` becomes a no-op and the worker
    // can drain its buffer without new entries racing in.
    dl_sink_shutdown.signal_shutdown();
    let dl_drained = match tokio::time::timeout(remaining().min(DL_DRAIN_TIMEOUT), dl_worker_handle)
        .await
    {
        Ok(Ok(n)) => {
            info!(entries = n, "dead-letter worker drained and exited");
            true
        }
        Ok(Err(e)) => {
            tracing::error!(error = %e, "dead-letter worker panicked");
            false
        }
        Err(_) => {
            tracing::error!("dead-letter worker drain timed out; buffered rejections were lost");
            false
        }
    };

    // The store close always gets a floor, even if the workers ate the budget:
    // abandoning an unflushed SlateDB WAL is worse than overrunning the grace
    // period by a few seconds, and the container's own SIGKILL bounds it.
    let store_closed = match tokio::time::timeout(
        remaining().max(STORE_CLOSE_MIN_TIMEOUT),
        store.close_owned(),
    )
    .await
    {
        Ok(Ok(())) => true,
        Ok(Err(e)) => {
            tracing::error!(error = %e, "store close failed");
            false
        }
        Err(_elapsed) => {
            tracing::error!("store close timed out; data may not be fully flushed");
            false
        }
    };

    if workers_stopped && dl_drained && store_closed {
        info!("shutdown complete");
        Ok(())
    } else {
        // A non-zero exit is the only signal an orchestrator or an init system
        // gets that the drain was incomplete. Reporting success here would make
        // a lost write indistinguishable from a clean stop.
        anyhow::bail!(
            "unclean shutdown: workers_stopped={workers_stopped}, \
             dead_letters_drained={dl_drained}, store_closed={store_closed}"
        )
    }
}

/// Time allowed for the dead-letter buffer to reach the store on shutdown.
///
/// The buffer is small and in-memory; this is a cap on a flush, not a drain
/// budget, and it is bounded separately so a slow worker join cannot starve it.
const DL_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Floor for the SlateDB close, applied even when the rest of the drain has
/// already spent `--shutdown-timeout-secs`.
const STORE_CLOSE_MIN_TIMEOUT: Duration = Duration::from_secs(10);

/// Load and concatenate every `*.cedar` file in `dir`.
///
/// Files are sorted by name so loading order is deterministic.
/// Returns `None` when the directory is `None` or contains no `.cedar` files.
fn read_cedar_policy_dir(dir: &Option<std::path::PathBuf>) -> anyhow::Result<Option<String>> {
    let Some(dir) = dir else {
        return Ok(None);
    };
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading --cedar-policy-dir {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "cedar"))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    if entries.is_empty() {
        return Ok(None);
    }
    let mut buf = String::new();
    for entry in entries {
        let path = entry.path();
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("reading Cedar policy file {}", path.display()))?;
        buf.push('\n');
        buf.push_str(&content);
    }
    Ok(Some(buf))
}

/// Parse the `--deployment-roles` CLI argument into a [`DeploymentRoles`] value.
///
/// Accepts uppercase BDEW role codes: `NB`, `LF`, `MSB`, `NMSB`, `AMSB`, `BKV`, `UENB`, `BIKO`.
/// An empty list means no explicit roles were configured → returns [`DeploymentRoles::all()`]
/// (backward-compatible default: all PIDs registered unconditionally).
///
/// Unknown role strings are logged as warnings and ignored.
fn parse_deployment_roles(roles: &[String]) -> DeploymentRoles {
    if roles.is_empty() {
        return DeploymentRoles::all();
    }
    let parsed: Vec<Marktrolle> = roles
        .iter()
        .filter_map(|s| match s.to_uppercase().as_str() {
            "NB" => Some(Marktrolle::Nb),
            "LF" => Some(Marktrolle::Lf),
            "MSB" => Some(Marktrolle::Msb),
            "NMSB" => Some(Marktrolle::Nmsb),
            "AMSB" => Some(Marktrolle::Amsb),
            "BKV" => Some(Marktrolle::Bkv),
            // Strom ÜNB and Gas FNB (Fernleitungsnetzbetreiber) both map to
            // the Uenb engine role — both are transmission system operators.
            "UENB" | "ÜNB" | "UNB" | "FNB" => Some(Marktrolle::Uenb),
            "BIKO" => Some(Marktrolle::Biko),
            // Energieserviceanbieter — Strom only, consent-derived (§49 Abs. 2
            // Nr. 9 MsbG). A deployment that *is* an ESA; an MSB serving one
            // registers the inbound side under MSB.
            "ESA" => Some(Marktrolle::Esa),
            // Gas roles that have no distinct engine deployment role — their
            // PIDs are registered unconditionally by the Gas domain modules.
            // ANB/VNB are Strom NB sub-types, normalised by deployment_role_strings.
            "GNB" | "ANB" | "VNB" => Some(Marktrolle::Nb),
            "LFG" => Some(Marktrolle::Lf),
            "GMSB" => Some(Marktrolle::Msb),
            "MGV" => {
                // No engine deployment role for MGV; GaBi Gas registers its
                // PIDs unconditionally. Safe to ignore here.
                None
            }
            other => {
                tracing::warn!(
                    role = other,
                    "Unknown Marktrolle in --deployment-roles; valid values: \
                     NB, LF, MSB, NMSB, AMSB, BKV, UENB/FNB, BIKO, ESA, \
                     GNB, ANB, VNB, LFG, GMSB, MGV"
                );
                None
            }
        })
        .collect();
    DeploymentRoles::from_roles(parsed)
}

/// Merge `makod.toml` into the parsed CLI struct.
///
/// The file is the lowest-precedence source: a value is taken from it only when
/// the corresponding flag still holds its built-in default (`is_default`) or is
/// still unset. Every `Cli` field is reachable from here — the
/// `cli_fields_are_reachable_from_toml` test fails the build when a new flag is
/// added without a config-file path to it.
fn apply_config_file(
    cfg: config::ConfigFile,
    matches: &clap::ArgMatches,
    cli: &mut Cli,
) -> anyhow::Result<()> {
    use clap::{ValueEnum, parser::ValueSource};
    use config::{either_inline_or_file, read_keyed_files, read_pairs_file};

    // True iff the named arg got its value purely from the built-in default
    // (i.e. the user did not pass it on the CLI or via an env var).
    let is_default = |id: &str| matches.value_source(id) == Some(ValueSource::DefaultValue);

    // ── Logging ───────────────────────────────────────────────────────────────
    if let Some(logging) = cfg.logging {
        if is_default("log_level")
            && let Some(s) = logging.level
        {
            cli.log_level = LogLevel::from_str(&s, true)
                .map_err(|e| anyhow::anyhow!("config: logging.level: {e}"))?;
        }
        if is_default("log_format")
            && let Some(s) = logging.format
        {
            cli.log_format = LogFormat::from_str(&s, true)
                .map_err(|e| anyhow::anyhow!("config: logging.format: {e}"))?;
        }
    }

    // ── OpenTelemetry ─────────────────────────────────────────────────────────
    if let Some(otel) = cfg.otel {
        if cli.otel_endpoint.is_none() {
            cli.otel_endpoint = otel.endpoint;
        }
        if cli.otel_service_name.is_none() {
            cli.otel_service_name = otel.service_name;
        }
    }

    // ── Storage ───────────────────────────────────────────────────────────────
    if let Some(storage) = cfg.storage {
        if is_default("object_store")
            && let Some(s) = storage.backend
        {
            cli.object_store = ObjectStoreBackend::from_str(&s, true)
                .map_err(|e| anyhow::anyhow!("config: storage.backend: {e}"))?;
        }
        if cli.data_dir.is_none() {
            cli.data_dir = storage.data_dir;
        }
        if storage.allow_volatile {
            cli.allow_volatile = true;
        }
        if storage.allow_multi_instance {
            cli.allow_multi_instance = true;
        }
        if is_default("max_stream_events")
            && let Some(n) = storage.max_stream_events
        {
            cli.max_stream_events = n;
        }
        if let Some(s3) = storage.s3 {
            if cli.s3_bucket.is_none() {
                cli.s3_bucket = s3.bucket;
            }
            if cli.s3_endpoint.is_none() {
                cli.s3_endpoint = s3.endpoint;
            }
            if is_default("s3_prefix")
                && let Some(p) = s3.prefix
            {
                cli.s3_prefix = p;
            }
        }
        if let Some(gcs) = storage.gcs {
            if cli.gcs_bucket.is_none() {
                cli.gcs_bucket = gcs.bucket;
            }
            if is_default("gcs_prefix")
                && let Some(p) = gcs.prefix
            {
                cli.gcs_prefix = p;
            }
        }
        if let Some(azure) = storage.azure {
            if cli.azure_container.is_none() {
                cli.azure_container = azure.container;
            }
            if cli.azure_account.is_none() {
                cli.azure_account = azure.account;
            }
            if is_default("azure_prefix")
                && let Some(p) = azure.prefix
            {
                cli.azure_prefix = p;
            }
        }
    }

    // ── HTTP API ──────────────────────────────────────────────────────────────
    if let Some(http) = cfg.http {
        if cli.http_addr.is_none() {
            cli.http_addr = http.addr;
        }
        if is_default("http_max_body_bytes")
            && let Some(n) = http.max_body_bytes
        {
            cli.http_max_body_bytes = n;
        }
        if cli.auth_keys.is_empty() {
            let mut keys = http.auth_keys.unwrap_or_default();
            if let Some(ref path) = http.auth_keys_file {
                keys.extend(read_pairs_file("http.auth_keys_file", path)?);
            }
            cli.auth_keys = keys;
        }
    }

    // ── Authorization ─────────────────────────────────────────────────────────
    if let Some(authz) = cfg.authz {
        if cli.cedar_policy_dir.is_none() {
            cli.cedar_policy_dir = authz.cedar_policy_dir;
        }
        if authz.no_default_policy {
            cli.cedar_no_default_policy = true;
        }
    }

    // ── OIDC ──────────────────────────────────────────────────────────────────
    if let Some(oidc) = cfg.oidc {
        if cli.oidc_issuer.is_none() {
            cli.oidc_issuer = oidc.issuer;
        }
        if cli.oidc_audience.is_none() {
            cli.oidc_audience = oidc.audience;
        }
        if is_default("oidc_jwks_refresh_secs")
            && let Some(secs) = oidc.jwks_refresh_secs
        {
            cli.oidc_jwks_refresh_secs = secs;
        }
    }

    // ── API-Webdienste ────────────────────────────────────────────────────────
    if let Some(wd) = cfg.webdienste {
        if cli.api_webdienste_addr.is_none() {
            cli.api_webdienste_addr = wd.addr;
        }
        if wd.allow_unauthenticated {
            cli.webdienste_allow_unauthenticated = true;
        }
    }

    // ── Engine ────────────────────────────────────────────────────────────────
    if let Some(engine) = cfg.engine {
        if is_default("shutdown_timeout_secs")
            && let Some(secs) = engine.shutdown_timeout_secs
        {
            cli.shutdown_timeout_secs = secs;
        }
        if is_default("snapshot_interval")
            && let Some(n) = engine.snapshot_interval
        {
            cli.snapshot_interval = n;
        }
        if is_default("projection_checkpoint_interval")
            && let Some(n) = engine.projection_checkpoint_interval
        {
            cli.projection_checkpoint_interval = n;
        }
        if is_default("deadline_poll_interval_secs")
            && let Some(n) = engine.deadline_poll_interval_secs
        {
            cli.deadline_poll_interval_secs = n;
        }
        if cli.worker_threads.is_none() {
            cli.worker_threads = engine.worker_threads;
        }
        if cli.marktrollen.is_empty()
            && let Some(roles) = engine.marktrollen
        {
            cli.marktrollen = roles;
        }
        if cli.deployment_roles.is_empty()
            && let Some(roles) = engine.deployment_roles
        {
            cli.deployment_roles = roles;
        }
    }

    // ── AS4 ───────────────────────────────────────────────────────────────────
    if let Some(as4) = cfg.as4 {
        if cli.as4_addr.is_none() {
            cli.as4_addr = as4.addr;
        }
        if cli.as4_party_id.is_none() {
            cli.as4_party_id = as4.party_id;
        }
        if cli.as4_signing_key_pem.is_none()
            && let Some(pem) = either_inline_or_file(
                "as4.signing_key_pem",
                as4.signing_key_pem,
                as4.signing_key_pem_file.as_ref(),
            )?
        {
            cli.as4_signing_key_pem = Some(SecretString::new(pem.into()));
        }
        if cli.as4_signing_cert_pem.is_none() {
            cli.as4_signing_cert_pem = either_inline_or_file(
                "as4.signing_cert_pem",
                as4.signing_cert_pem,
                as4.signing_cert_pem_file.as_ref(),
            )?;
        }
        if cli.as4_decryption_key_pem.is_none()
            && let Some(pem) = either_inline_or_file(
                "as4.decryption_key_pem",
                as4.decryption_key_pem,
                as4.decryption_key_pem_file.as_ref(),
            )?
        {
            cli.as4_decryption_key_pem = Some(SecretString::new(pem.into()));
        }
        if cli.as4_trust_anchor_pem.is_none() {
            cli.as4_trust_anchor_pem = either_inline_or_file(
                "as4.trust_anchor_pem",
                as4.trust_anchor_pem,
                as4.trust_anchor_pem_file.as_ref(),
            )?;
        }
        // CLI partners take full precedence; config partners are used only
        // when the CLI list is empty (no --as4-partner flags were passed).
        if cli.as4_partner.is_empty()
            && let Some(partners) = as4.partners
        {
            cli.as4_partner = partners;
        }
        if cli.as4_partner_cert.is_empty() {
            let mut certs = as4.partner_certs.unwrap_or_default();
            if let Some(ref files) = as4.partner_cert_files {
                certs.extend(read_keyed_files("as4.partner_cert_files", files)?);
            }
            cli.as4_partner_cert = certs;
        }
        if as4.allow_unencrypted {
            cli.allow_unencrypted_as4 = true;
        }
        if as4.allow_no_signing {
            cli.allow_no_as4_signing = true;
        }
        if as4.lenient_receipts {
            cli.as4_lenient_receipts = true;
        }
    }

    // ── ERP ───────────────────────────────────────────────────────────────────
    if let Some(erp) = cfg.erp {
        if cli.erp_webhook_url.is_none() {
            cli.erp_webhook_url = erp.webhook_url;
        }
        if cli.erp_webhook_secret.is_none()
            && let Some(secret) = either_inline_or_file(
                "erp.webhook_secret",
                erp.webhook_secret,
                erp.webhook_secret_file.as_ref(),
            )?
        {
            cli.erp_webhook_secret = Some(SecretString::new(secret.trim().to_owned().into()));
        }
        if cli.edifact_outbox_webhook_url.is_none() {
            cli.edifact_outbox_webhook_url = erp.edifact_outbox_webhook_url;
        }
        if cli.netzzugang_endpoint_url.is_none() {
            cli.netzzugang_endpoint_url = erp.netzzugang_endpoint_url;
        }
    }

    // ── marktd ────────────────────────────────────────────────────────────────
    if let Some(marktd) = cfg.marktd {
        if cli.marktd_url.is_none() {
            cli.marktd_url = marktd.url;
        }
        if cli.marktd_api_key.is_none() {
            cli.marktd_api_key = either_inline_or_file(
                "marktd.api_key",
                marktd.api_key,
                marktd.api_key_file.as_ref(),
            )?
            .map(|s| s.trim().to_owned());
        }
    }

    // ── MaLo-ID / Verzeichnisdienst ───────────────────────────────────────────
    if let Some(maloid) = cfg.maloid {
        if cli.maloid_partner.is_empty()
            && let Some(partners) = maloid.partners
        {
            cli.maloid_partner = partners;
        }
        if cli.verzeichnisdienst_url.is_none() {
            cli.verzeichnisdienst_url = maloid.verzeichnisdienst_url;
        }
    }

    // ── [[party]] — multi-MP-ID identity table ───────────────────────────────
    //
    // Stored separately (not merged into a CLI string field) because the
    // array-of-tables structure has no CLI equivalent.
    if let Some(parties) = cfg.party
        && !parties.is_empty()
    {
        cli.parties = parties;
    }

    Ok(())
}

// ── `makod migrate` ───────────────────────────────────────────────────────────

/// Run every pending format-version migration and return.
///
/// Prints a one-line JSON summary on stdout for CI log capture. `workflows`
/// names what was actually covered — a count alone reads as complete whatever
/// the migration happens to include.
///
/// # Errors
///
/// Returns an error when any individual migration reported one, so the exit
/// code gates the deployment step that invoked it.
async fn run_migrations(store: &SlateDbStore) -> anyhow::Result<()> {
    // Migrate FV2025-10-01 → FV2026-10-01 (the only active transition).
    // When more transitions exist, iterate over them here in order.
    match migration_api::dispatch_migrations("FV2025-10-01", "FV2026-10-01", store).await {
        Some((report, workflows)) if report.errors.is_empty() => {
            // JSON summary on stdout for CI log capture. `workflows` names
            // what was actually covered — a count alone reads as complete
            // whatever the migration happens to include.
            println!(
                "{{\"migrated\":{},\"skipped\":{},\"workflows\":{},\"errors\":[]}}",
                report.migrated,
                report.skipped,
                serde_json::to_string(&workflows).unwrap_or_else(|_| "[]".to_owned()),
            );
            tracing::info!(
                migrated = report.migrated,
                skipped = report.skipped,
                workflows = workflows.len(),
                "makod migrate: all migrations completed successfully",
            );
            Ok(())
        }
        Some((report, _workflows)) => {
            for err in &report.errors {
                tracing::error!(error = %err, "migration error");
            }
            anyhow::bail!(
                "makod migrate: {} migration error(s) — see log for details",
                report.errors.len()
            );
        }
        None => {
            tracing::info!(
                "makod migrate: no applicable migration found for this transition; nothing to do"
            );
            Ok(())
        }
    }
}

// ── ProcessRegistry startup reconciliation ────────────────────────────

/// Scan all `process/` event streams and re-register any entries missing from
/// the [`SlateDbProcessRegistry`].
///
/// Returns the number of entries reconstructed.  On restart after a crash or
/// after an operator deleted a registry entry, inbound APERAKs would fail to
/// route until this reconciliation restores the lost mapping.
///
/// The function is intentionally best-effort: a failure to reconstruct a
/// single entry is logged as a warning and skipped rather than aborting
/// startup.  A single corrupt or empty stream is therefore not fatal.
async fn reconcile_process_registry(store: &SlateDbStore) -> anyhow::Result<usize> {
    use mako_engine::{
        event_store::EventStore as _,
        ids::{ProcessId, ProcessIdentity, TenantId},
        registry::{ProcessRegistry as _, RegistryKey},
    };

    let registry = store.as_process_registry();
    let streams = store
        .list_streams(Some("process/"))
        .await
        .context("reconcile_process_registry: list_streams")?;

    let mut reconstructed = 0usize;

    for stream_id in &streams {
        // Parse tenant_id and process_id from the stream ID
        // format: process/{tenant_uuid}/{process_uuid}
        let raw = stream_id.as_str();
        let mut parts = raw.splitn(3, '/');
        let (prefix, tenant_str, process_str) = match (parts.next(), parts.next(), parts.next()) {
            (Some(p), Some(t), Some(pr)) => (p, t, pr),
            _ => {
                tracing::warn!(stream_id = %stream_id, "unexpected stream ID format — skipping");
                continue;
            }
        };
        if prefix != "process" {
            continue;
        }
        let tenant_id = match tenant_str.parse::<uuid::Uuid>() {
            Ok(u) => TenantId::from_uuid(u),
            Err(e) => {
                tracing::warn!(stream_id = %stream_id, error = %e, "bad tenant UUID — skipping");
                continue;
            }
        };
        let process_id = match process_str.parse::<uuid::Uuid>() {
            Ok(u) => ProcessId::from_uuid(u),
            Err(e) => {
                tracing::warn!(stream_id = %stream_id, error = %e, "bad process UUID — skipping");
                continue;
            }
        };

        let key = RegistryKey::from_process(process_id);
        match registry.lookup(tenant_id, &key).await {
            Ok(Some(_)) => {
                // Entry present — nothing to do.
            }
            Ok(None) => {
                // Missing entry — load the first event to get workflow_id, then reconstruct.
                let events = match store.load_from(stream_id, 0).await {
                    Ok(evs) => evs,
                    Err(e) => {
                        tracing::warn!(
                            stream_id = %stream_id,
                            error = %e,
                            "failed to load events for reconciliation — skipping",
                        );
                        continue;
                    }
                };
                let Some(first) = events.into_iter().next() else {
                    tracing::warn!(stream_id = %stream_id, "empty stream — skipping");
                    continue;
                };
                let identity = ProcessIdentity::new(
                    first.process_id,
                    first.tenant_id,
                    first.workflow_id.clone(),
                );
                match registry.register(first.tenant_id, &key, identity).await {
                    Ok(()) => {
                        tracing::info!(
                            stream_id = %stream_id,
                            process_id = %process_id,
                            workflow_id = %first.workflow_id,
                            "reconciled: reconstructed missing registry entry",
                        );
                        reconstructed += 1;
                    }
                    Err(e) => {
                        tracing::warn!(
                            stream_id = %stream_id,
                            error = %e,
                            "failed to reconstruct registry entry — skipping",
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    stream_id = %stream_id,
                    error = %e,
                    "registry lookup failed during reconciliation — skipping",
                );
            }
        }
    }

    Ok(reconstructed)
}

// ── Store initialisation ──────────────────────────────────────────────────────

/// Open the event store based on the CLI / environment configuration.
async fn open_store(cli: &Cli) -> anyhow::Result<SlateDbStore> {
    match cli.object_store {
        ObjectStoreBackend::Local => match &cli.data_dir {
            Some(dir) => {
                info!(path = %dir.display(), "opening persistent local-filesystem SlateDB store");
                Ok(SlateDbStore::open_local(dir).await?)
            }
            None => {
                if !cli.allow_volatile {
                    anyhow::bail!(
                        "volatile mode is disabled by default.\n\n\
                         Provide --data-dir <DIR> (or MAKOD_DATA_DIR) for a persistent store,\n\
                         or set --allow-volatile (MAKOD_ALLOW_VOLATILE=1) to acknowledge that\n\
                         all event-store data will be lost on exit.\n\n\
                         Volatile mode cannot meet the regulatory durability requirements of\n\
                         § 147 AO / GoBD and BDEW AHB. Never use it in production."
                    );
                }
                tracing::warn!(
                    "VOLATILE MODE: no --data-dir provided; using volatile in-memory SlateDB store \u{2014} all data will be lost on restart"
                );
                Ok(SlateDbStore::open_in_memory().await?)
            }
        },
        ObjectStoreBackend::S3 => {
            let bucket = cli.s3_bucket.as_deref().ok_or_else(|| {
                anyhow::anyhow!(
                    "--s3-bucket (or MAKOD_S3_BUCKET) is required when --object-store=s3"
                )
            })?;
            let prefix = cli.s3_prefix.as_str();
            info!(bucket, prefix, "opening S3-backed SlateDB store");

            let mut builder =
                object_store::aws::AmazonS3Builder::from_env().with_bucket_name(bucket);

            if let Some(endpoint) = &cli.s3_endpoint {
                // MinIO or other S3-compatible endpoint. Allow plain HTTP for
                // local development; production endpoints should use HTTPS.
                let allow_http = endpoint.starts_with("http://");
                builder = builder.with_endpoint(endpoint).with_allow_http(allow_http);
                if allow_http {
                    tracing::warn!(
                        endpoint,
                        "S3 endpoint uses plain HTTP — event data is transmitted \
                         unencrypted. This violates § 147 AO / GoBD audit-trail \
                         confidentiality requirements. Use HTTPS in production."
                    );
                } else {
                    info!(endpoint, "using custom S3-compatible endpoint (HTTPS)");
                }
            }

            let store = std::sync::Arc::new(builder.build()?);
            Ok(SlateDbStore::open(prefix, store).await?)
        }
        ObjectStoreBackend::Gcs => {
            let bucket = cli.gcs_bucket.as_deref().ok_or_else(|| {
                anyhow::anyhow!(
                    "--gcs-bucket (or MAKOD_GCS_BUCKET) is required when --object-store=gcs"
                )
            })?;
            let prefix = cli.gcs_prefix.as_str();
            info!(bucket, prefix, "opening GCS-backed SlateDB store");

            let store = std::sync::Arc::new(
                object_store::gcp::GoogleCloudStorageBuilder::from_env()
                    .with_bucket_name(bucket)
                    .build()?,
            );
            Ok(SlateDbStore::open(prefix, store).await?)
        }
        ObjectStoreBackend::Azure => {
            let container = cli.azure_container.as_deref().ok_or_else(|| {
                anyhow::anyhow!(
                    "--azure-container (or MAKOD_AZURE_CONTAINER) is required when --object-store=azure"
                )
            })?;
            let account = cli.azure_account.as_deref().ok_or_else(|| {
                anyhow::anyhow!(
                    "--azure-account (or MAKOD_AZURE_ACCOUNT) is required when --object-store=azure"
                )
            })?;
            let prefix = cli.azure_prefix.as_str();
            info!(
                container,
                account, prefix, "opening Azure Blob-backed SlateDB store"
            );

            let store = std::sync::Arc::new(
                object_store::azure::MicrosoftAzureBuilder::from_env()
                    .with_account(account)
                    .with_container_name(container)
                    .build()?,
            );
            Ok(SlateDbStore::open(prefix, store).await?)
        }
    }
}

// ── Tracing setup ─────────────────────────────────────────────────────────────

/// Initialise tracing; returns a guard that flushes OTel spans on drop.
///
/// With an OTLP endpoint configured — `OTEL_EXPORTER_OTLP_ENDPOINT`, or
/// `[otel] endpoint` / `--otel-endpoint` — spans (including the AS4 ingest and
/// outbox delivery spans) export via OTLP with W3C propagation, joining the
/// header-level `traceparent` chain the outbox already persists. The subscriber
/// is then the structured JSON layer. Without an endpoint, the local fmt
/// subscriber keeps the pretty/compact/json behaviour selected by
/// `--log-format`.
///
/// The environment variables win over the config file so a deployment can move
/// telemetry into the orchestrator without editing `makod.toml`.
fn init_tracing(cli: &Cli) -> Option<mako_service::telemetry::OtelGuard> {
    use tracing_subscriber::{EnvFilter, fmt};

    if std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok() {
        return Some(mako_service::telemetry::init_tracing_from_env("makod"));
    }
    if let Some(endpoint) = cli.otel_endpoint.clone() {
        let otel = mako_service::telemetry::OtelConfig {
            endpoint,
            service_name: cli
                .otel_service_name
                .clone()
                .unwrap_or_else(|| "makod".to_owned()),
        };
        return Some(mako_service::telemetry::init_tracing(
            "makod",
            cli.log_level.as_filter().as_str(),
            Some(&otel),
        ));
    }

    let filter = EnvFilter::builder()
        .with_default_directive(cli.log_level.as_filter().into())
        .from_env_lossy();

    match cli.log_format {
        LogFormat::Pretty => {
            fmt().with_env_filter(filter).pretty().init();
        }
        LogFormat::Compact => {
            fmt().with_env_filter(filter).compact().init();
        }
        LogFormat::Json => {
            fmt().with_env_filter(filter).json().init();
        }
    }
    None
}

// ── Graceful shutdown ─────────────────────────────────────────────────────────

/// Await an OS shutdown signal (SIGTERM on Unix, Ctrl-C everywhere).
/// Returns after the first signal is received.
async fn wait_for_shutdown() {
    use tokio::signal;

    #[cfg(unix)]
    {
        use signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).expect("failed to register SIGTERM");
        tokio::select! {
            _ = signal::ctrl_c() => {},
            _ = sigterm.recv()   => {},
        }
    }

    #[cfg(not(unix))]
    {
        let _ = signal::ctrl_c().await;
    }
}

// ── Configuration surface guard ───────────────────────────────────────────────

#[cfg(test)]
mod config_surface_tests {
    /// Every `makod.toml` field must actually be read.
    ///
    /// The companion test below guards the other direction — that no flag is
    /// missing a file form. This one guards the failure that is quieter and
    /// therefore worse: a field declared in the schema, documented in the
    /// operator guide, accepted by `deny_unknown_fields` at parse time, and then
    /// read by nobody. The operator sets it, sees no error, and gets the default
    /// behaviour. For a field like `as4.trust_anchor_pem_file` that silence is
    /// the difference between verifying counterparty signatures and not.
    #[test]
    fn toml_fields_are_all_consumed() {
        const CONFIG_SOURCE: &str = include_str!("core/config.rs");
        const MAIN_SOURCE: &str = include_str!("main.rs");

        // `[[party]]` is applied wholesale (`cli.parties = parties`) rather than
        // field by field, so its members never appear by name.
        const APPLIED_WHOLESALE: &[&str] = &["mp_id", "roles", "primary", "agency"];

        let merge_body = MAIN_SOURCE
            .split_once("fn apply_config_file(")
            .expect("apply_config_file is defined in main.rs")
            .1
            .split_once("\n}\n")
            .expect("apply_config_file is closed")
            .0;

        // Section-struct fields are declared as `    pub <name>: <type>,`.
        let fields: Vec<&str> = CONFIG_SOURCE
            .lines()
            .filter_map(|line| {
                let rest = line.strip_prefix("    pub ")?;
                let (name, tail) = rest.split_once(": ")?;
                (!tail.is_empty()
                    && !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
                .then_some(name)
            })
            .collect();
        assert!(
            fields.len() > 50,
            "the field scan found only {} fields — the declaration style in \
             core/config.rs changed and this guard is no longer looking at anything",
            fields.len(),
        );

        let ignored: Vec<&str> = fields
            .iter()
            .filter(|f| !APPLIED_WHOLESALE.contains(f))
            .filter(|f| !merge_body.contains(**f))
            .copied()
            .collect();
        assert!(
            ignored.is_empty(),
            "these makod.toml fields are declared but never read, so setting them \
             silently does nothing: {ignored:?}\n\
             Read each one in apply_config_file, or delete it from core/config.rs."
        );
    }

    /// Every CLI flag must be reachable from `makod.toml`.
    ///
    /// `config.rs` promises operators that anything settable by flag is also
    /// settable by file. That promise decayed silently: the AS4 inbound
    /// decryption key and the per-partner encryption certificates — both
    /// mandatory in production — were flag-and-environment only, so the one
    /// deployment shape that keeps key material out of `ps` output and out of
    /// the container environment could not express them at all.
    ///
    /// The check is a source scan rather than a runtime assertion because the
    /// mapping lives in `apply_config_file`'s body: a field the function never
    /// mentions has no path from the file, whatever the schema declares.
    #[test]
    fn cli_fields_are_reachable_from_toml() {
        const SOURCE: &str = include_str!("main.rs");

        // Flags that describe *how to run this invocation* rather than what the
        // daemon is, and therefore have no file equivalent by design.
        const INVOCATION_ONLY: &[&str] = &[
            // Names the file itself — it cannot live inside it.
            "config",
            // One-shot validation mode; a config that turned itself into a
            // check-only run would never start.
            "check", // Subcommand selector (`makod migrate`).
            "command",
            // Populated from the file's `[[party]]` array, not from a flag.
            "parties",
        ];

        let struct_body = SOURCE
            .split_once("struct Cli {")
            .expect("Cli struct is defined in main.rs")
            .1
            .split_once("\n}\n")
            .expect("Cli struct is closed")
            .0;
        // Field lines are exactly `    <name>: <type>,` — four spaces of
        // indentation, a snake_case name, then the type. Doc comments and
        // `#[arg(...)]` attributes fail the name-character test.
        let fields: Vec<&str> = struct_body
            .lines()
            .filter_map(|line| {
                let rest = line.strip_prefix("    ")?;
                let (name, tail) = rest.split_once(": ")?;
                (!name.is_empty()
                    && !tail.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
                .then_some(name)
            })
            .collect();
        assert!(
            fields.len() > 40,
            "the Cli field scan found only {} fields — the parser drifted from \
             the struct's formatting",
            fields.len()
        );

        let merge_body = SOURCE
            .split_once("fn apply_config_file(")
            .expect("apply_config_file is defined in main.rs")
            .1;
        let unreachable: Vec<&str> = fields
            .iter()
            .filter(|f| !INVOCATION_ONLY.contains(f))
            .filter(|f| !merge_body.contains(&format!("cli.{f}")))
            .copied()
            .collect();
        assert!(
            unreachable.is_empty(),
            "these CLI flags cannot be set from makod.toml: {unreachable:?}\n\
             Add a field to the matching section in core/config.rs and read it \
             in apply_config_file, or list the flag in INVOCATION_ONLY with a \
             reason if it genuinely has no file form."
        );
    }
}

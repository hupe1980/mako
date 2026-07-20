//! `makod` — Mako process engine daemon.
//!
//! Assembles all domain modules (GPKE, WiM, GeLi Gas, MABIS) into a single
//! [`EngineContext`] and runs until a graceful shutdown signal is received.
#![deny(unsafe_code)]
//!
//! ## Usage
//!
//! ```text
//! makod [OPTIONS]
//!
//! Options:
//!   -l, --log-level <LEVEL>                Log level [env: MAKOD_LOG_LEVEL=]  [default: info]
//!   -f, --log-format <FORMAT>              Log format [env: MAKOD_LOG_FORMAT=] [default: pretty]
//!       --data-dir <DIR>                   Persistent store path [env: MAKOD_DATA_DIR=]
//!                                          WARNING: omitting this flag enables volatile (in-memory)
//!                                          mode — all data is lost on exit. NOT for production use.
//!       --object-store <BACKEND>           Object store backend [env: MAKOD_OBJECT_STORE=] [default: local]
//!       --s3-bucket <BUCKET>               S3 bucket [env: MAKOD_S3_BUCKET=]
//!       --s3-endpoint <URL>                S3 endpoint URL (for MinIO/compat) [env: MAKOD_S3_ENDPOINT=]
//!       --gcs-bucket <BUCKET>              GCS bucket name [env: MAKOD_GCS_BUCKET=]
//!       --azure-container <NAME>           Azure Blob container [env: MAKOD_AZURE_CONTAINER=]
//!       --azure-account <ACCOUNT>          Azure Storage account [env: MAKOD_AZURE_ACCOUNT=]
//!       --http-addr <ADDR>                 HTTP REST API listen address [env: MAKOD_HTTP_ADDR=]
//!       --api-webdienste-addr <ADDR>       API-Webdienste Strom listen address [env: MAKOD_API_WEBDIENSTE_ADDR=]
//!       --tenant-id <ID>                   Operator tenant identifier (BDEW code / GLN / EIC) [env: MAKOD_TENANT_ID=] [default: default]
//!       --as4-addr <ADDR>                  AS4 inbound transport address [env: MAKOD_AS4_ADDR=]
//!       --as4-signing-key-pem <PEM>        PEM private key for AS4 signing (ECDSA BrainpoolP256r1) [env: MAKOD_AS4_SIGNING_KEY_PEM=]
//!       --as4-signing-cert-pem <PEM>       PEM X.509 certificate for AS4 signing [env: MAKOD_AS4_SIGNING_CERT_PEM=]
//!       --as4-decryption-key-pem <PEM>     PEM private key for AS4 inbound decryption (ECDH-ES, BrainpoolP256r1) [env: MAKOD_AS4_DECRYPTION_KEY_PEM=]
//!       --as4-partner-cert <GLN=PEM>       Per-partner encryption certificate for outbound AS4 (repeatable) [env: MAKOD_AS4_PARTNER_CERT=]
//!       --as4-party-id <GLN>               AS4 party ID (operator GLN) [env: MAKOD_AS4_PARTY_ID=]
//!       --as4-partner <GLN=URL>            Trading partner AS4 endpoint (repeatable) [env: MAKOD_AS4_PARTNER=]
//!       --check                            Validate config/profiles/adapters and exit (no workers started) [env: MAKOD_CHECK=]
//!   -h, --help                             Print help
//!   -V, --version                          Print version
//! ```
//!
//! ## REST API
//!
//! When `--http-addr` is set (e.g. `127.0.0.1:8080`), makod exposes:
//!
//! - `POST /api/v1/commands` — ERP submits a BO4E object (JSON) to initiate a MaKo process
//! - `POST /edifact` — submit a raw EDIFACT interchange as an alternative to AS4
//! - `GET  /admin/partners` — list all trading-partner records for this tenant
//! - `GET  /admin/partners/{mp_id}` — retrieve a single partner record
//! - `PUT  /admin/partners/{mp_id}` — create or update a partner record (JSON body)
//! - `DELETE /admin/partners/{mp_id}` — remove a partner record
//! - `POST /admin/partners/import` — import partners from a raw PARTIN EDIFACT interchange
//!
//! See [`edifact_api`] and [`partner_api`] for full request/response documentation.
//!
//! ## API-Webdienste Strom
//!
//! When `--api-webdienste-addr` is set (e.g. `127.0.0.1:8090`), makod exposes
//! the BDEW API-Webdienste Strom endpoints (Control Measures v1, MaLo
//! Identification v1) on a separate port.
//!
//! - **MaLo Identification** — active. `POST /maloId/request/v1` performs inbox
//!   idempotency dedup and enqueues a `MaloIdentCallback` outbox message for
//!   async cache lookup. Cache is populated via `PUT /admin/malo/{malo_id}`.
//! - **Control Measures** — endpoints return `405 Method Not Allowed` until
//!   Redispatch 2.0 is fully specified.
//!
//! ## Environment variables
//!
//! Every CLI flag has a corresponding `MAKOD_` environment variable. The CLI
//! flag takes precedence over the environment variable when both are set.
//!
//! For AWS credentials, the standard `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
//! and `AWS_REGION` environment variables are used by the `object_store` crate
//! automatically — no `MAKOD_`-prefixed credential variables are needed.
//!
//! For GCS, set `GOOGLE_SERVICE_ACCOUNT_KEY` (JSON key content) or
//! `GOOGLE_SERVICE_ACCOUNT` (path to a key file). For Azure, set
//! `AZURE_STORAGE_ACCOUNT_KEY` alongside `--azure-account`.
//!
//! ## Architecture
//!
//! ```text
//! makod
//!   └── EngineContext (SlateDbStore — in-memory by default, local FS via --data-dir)
//!         ├── GpkeModule    — UTILMD PIDs 55001–55002, 55016 (`gpke-supplier-change`)
//!         │                   + INVOIC PIDs 31001–31002, 31005–31006 (`gpke-abrechnung`)
//!         │                   [role-lf-strom OR role-nb-strom OR no role flags]
//!         ├── WimModule     — PIDs 55039, 55042, 55051, 55168 (WiM Strom, BK6-24-174)
//!         │                   [role-msb-strom OR role-nb-strom OR no role flags]
//!         ├── GeliGasModule — PIDs 44001–44021 (GeLi Gas; 44022–44024 registered by WimGasModule) + PID 31011 (AWH Rechnung)
//!         │                   [role-lf-gas OR role-nb-gas OR no role flags]
//!         ├── WimGasModule      — PIDs 44022–44024, 44039–44053, 44168–44170, 31003, 31004 (WiM Gas MSB-Wechsel + INVOIC billing)
//!         │                       [role-msb-gas OR role-nb-gas OR no role flags]
//!         ├── GaBiGasModule     — PIDs 31010/31007/31008 (INVOIC billing, BK7-24-01-008)
//!         │                   + PID 33001 (REMADV Zahlungsavis) + PID 29001 (COMDIS Ablehnung)
//!         │                   + PID 13013 (MSCONS Gas Allokationsliste, `gabi-gas-mmma`)
//!         │                   [role-nb-gas OR no role flags]
//!         ├── MabisModule       — PID 13003 only (MABIS Bilanzkreisabrechnung Strom, MSCONS Summenzeitreihe)
//!         │                       [role-nb-strom OR no role flags]
//!         └── RedispatchModule  — Redispatch 2.0 (§§ 13/13a/14 EnWG); XML routing + IFTSTA PIDs 21037/21038
//!                                 [always registered]
//!
//! Background tasks:
//!   ├── OutboxWorker      — drains pending outbox messages via MaloIdentSender
//!   ├── OutboxErpWorker   — POSTs BO4E outbox entries to ERP webhook (optional; --erp-webhook-url)
//!   ├── DeadlineScheduler — fires overdue process deadlines every 30 s
//!   │                       (dispatches TimeoutExpired to each workflow family)
//!   ├── HTTP server       — REST API (optional; enabled via --http-addr)
//!   └── API-Webdienste    — BDEW Webdienste Strom (optional; --api-webdienste-addr)
//! ```

mod adapters;
mod api_bridge;
mod as4_ingest;
mod as4_sender;
mod cedar_authz;
mod commands_api;
mod config;
mod contrl_ack;
mod deadline_dispatch;
mod edifact_api;
mod edifact_renderer;
mod erp_adapter;
mod health;
mod ingest_dispatcher;
mod invoic_api;
mod malo_admin_api;
mod malo_cache;
mod malo_ident_sender;
mod mcp_server;
mod metrics_api;
mod migration_api;
mod oidc_verifier;
mod openapi;
mod partner_api;
mod party_registry;
mod projection_worker;
mod startup;
mod verzeichnisdienst_worker;
mod webdienste;
mod worker_health;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use asx_rs::core::SessionContextBuilder;
use asx_rs::observability::EventBus;
use clap::{Parser, ValueEnum};
use edi_energy::Platform;
use mako_engine::{
    builder::EngineBuilder,
    marktrolle::{DeploymentRoles, Marktrolle},
    store_slatedb::SlateDbStore,
};
use mako_gabi_gas::GaBiGasModule;
use mako_geli_gas::GeliGasModule;
use mako_gpke::GpkeModule;
use mako_mabis::MabisModule;
#[cfg(any(
    not(any(
        feature = "role-lf-strom",
        feature = "role-lf-gas",
        feature = "role-nb-strom",
        feature = "role-nb-gas",
        feature = "role-msb-strom",
        feature = "role-msb-gas",
    )),
    feature = "role-nb-strom",
))]
use mako_redispatch::RedispatchModule;
use mako_wim::WimModule;
use mako_wim_gas::WimGasModule;
use secrecy::{ExposeSecret as _, SecretString};
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
    /// Minimum log level to emit.
    ///
    /// Path to a TOML configuration file.
    ///
    /// Settings loaded from the file are applied after CLI and environment
    /// variable resolution: CLI flags and env vars always take precedence.
    /// See `config.rs` for the full schema and an annotated example.
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
    /// - Regulatory audit requirements (§22 MessZV, BDEW AHB) cannot be met.
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
    /// regulatory durability requirements of §22 MessZV and BDEW AHB.
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

    /// PEM-encoded RSA private key used to sign outbound AS4 SOAP messages
    /// (WS-Security XML-DSig) and synchronous receipts.
    ///
    /// Must be a PKCS#8 or traditional RSA private key in PEM format.
    /// Required when `--as4-addr` is set.
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
    /// errors). Defaults to `--tenant-id` when omitted.
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

    /// GLNs of counterparties that act as an Energieserviceanbieter (ESA).
    ///
    /// REQOTE 35002 is shared: an ESA Werteanfrage (WiM Teil 2 Kap. 4 UC 4.1
    /// Nr. 1) and a Preisanfrage arrive under the same Prüfidentifikator,
    /// because no ESA-specific REQOTE PID exists. The message carries only the
    /// sender's GLN, not its role, so listing the known ESA partners here turns
    /// on the decisive discriminator. Without it the classifier falls back to
    /// the `PIA` Messprodukt marker alone.
    ///
    /// Comma-separated, e.g. `MAKOD_ESA_PARTNER_GLNS=9900555000005`.
    #[arg(
        long,
        value_name = "GLNS",
        env = "MAKOD_ESA_PARTNER_GLNS",
        value_delimiter = ','
    )]
    esa_partner_mp_ids: Vec<String>,

    /// Shared secret for `X-Mako-Signature` HMAC-SHA256 on ERP webhook POSTs.
    ///
    /// When set alongside `--erp-webhook-url`, every webhook POST includes an
    /// `X-Mako-Signature: <hex>` header so the ERP can verify authenticity.
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
    /// precedence over `--tenant-id` / `[engine] tenant_id` for GLN routing.
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
         See docs/makod.md for the full configuration reference."
    );

    let mp_id_registry: Arc<MpIdRegistry> =
        Arc::new(MpIdRegistry::from_config(&cli.parties).context("invalid [[party]] config")?);

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

    // ── exclusive file lock on data directory ──────────────────────────
    //
    // Prevent two makod instances from opening the same SlateDB data directory.
    // Two concurrent writers against the same SlateDB path would corrupt the
    // write-ahead log and produce split-brain event sequences.
    //
    // `Box::leak` intentionally leaks the `RwLock<File>` allocation so the
    // write guard can hold a `'static` reference.  The file descriptor is
    // reclaimed by the OS when the process exits.  No `unsafe` is required
    // because `Box::leak` is a safe standard-library function.
    let _data_dir_lock: Option<fd_lock::RwLockWriteGuard<'static, std::fs::File>> =
        if let Some(ref data_dir) = cli.data_dir {
            std::fs::create_dir_all(data_dir)?;
            let lock_path = data_dir.join(".makod.lock");
            let lock_file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(false)
                .open(&lock_path)?;
            let lock_ref: &'static mut fd_lock::RwLock<std::fs::File> =
                Box::leak(Box::new(fd_lock::RwLock::new(lock_file)));
            match lock_ref.try_write() {
                Ok(guard) => {
                    info!(path = %lock_path.display(), "acquired exclusive data-dir lock");
                    Some(guard)
                }
                Err(e) => {
                    tracing::error!(
                        path = %lock_path.display(),
                        error = %e,
                        "Another makod instance is already using this data directory. \
                         Refusing to start to prevent write-ahead log corruption.",
                    );
                    std::process::exit(1);
                }
            }
        } else {
            None
        };

    // ── `makod migrate` subcommand ─────────────────────────────────────────
    //
    // Run all pending FV migrations and exit.  This allows operators to use
    // `makod migrate` as a Kubernetes initContainer or Compose `depends_on`
    // step without starting the full HTTP/AS4 server.
    if matches!(cli.command, Some(CliCommand::Migrate)) {
        // Migrate FV2025-10-01 → FV2026-10-01 (the only active transition).
        // When more transitions exist, iterate over them here in order.
        match migration_api::dispatch_migrations("FV2025-10-01", "FV2026-10-01", &store).await {
            Some((report, _count)) if report.errors.is_empty() => {
                // Print a JSON summary to stdout for CI log capture.
                println!(
                    "{{\"migrated\":{},\"skipped\":{},\"errors\":[]}}",
                    report.migrated, report.skipped
                );
                tracing::info!(
                    migrated = report.migrated,
                    skipped = report.skipped,
                    "makod migrate: all migrations completed successfully",
                );
                return Ok(());
            }
            Some((report, _count)) => {
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
                return Ok(());
            }
        }
    }

    // ── ProcessRegistry startup reconciliation ─────────────────────────
    //
    // On restart after a crash or after an operator accidentally deleted a
    // registry entry, inbound APERAKs can no longer be routed to their target
    // process.  Reconciliation scans all `process/` streams, loads the first
    // event from each, and re-registers any entries missing from the registry.
    //
    // This is a one-time startup operation — it does NOT block the server from
    // accepting requests and runs before the engine context is built.
    match reconcile_process_registry(&store).await {
        Ok(0) => {}
        Ok(n) => tracing::warn!(
            count = n,
            "ProcessRegistry reconciled: reconstructed missing routing entries on startup",
        ),
        Err(e) => tracing::error!(
            error = %e,
            "ProcessRegistry reconciliation failed (non-fatal — engine will start anyway)",
        ),
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
    //                         the durable dead-letter queue (§22 MessZV)
    //  • the original `dl_sink` — consumed by EngineBuilder below
    let (dl_sink, dl_worker) = store.as_dead_letter_sink();
    let dl_sink_shutdown = dl_sink.clone();
    let dl_sink_ingest = dl_sink.clone();
    let dl_sink_workers = dl_sink.clone();
    let dl_worker_handle = tokio::spawn(dl_worker.run());

    // ── Domain module selection ────────────────────────────────────────────────
    //
    // When role feature flags are compiled in, only the modules relevant to the
    // declared roles are registered.  This reduces binary size and eliminates
    // unwanted PID registrations for role-scoped deployments (LF-only, NB-only,
    // MSB-only, etc.).
    //
    // Default (no feature flags active): all modules are registered — fully
    // backward-compatible with existing deployments.
    //
    // Role → module mapping:
    //   role-nb-strom / role-nb  → GpkeModule (NB-side GPKE + Sperrung)
    //   role-lf-strom / role-lf  → GpkeModule (LF-side GPKE; GpkeModule handles
    //                              both sides via PidRouter role dispatch)
    //   role-msb-strom / role-msb → WimModule (MSB Strom)
    //   role-nb-strom / role-nb  → WimModule (NB-side WiM coordination)
    //   role-nb-gas / role-nb    → GeliGasModule + GaBiGasModule
    //   role-lf-gas  / role-lf   → GeliGasModule (LF-side GeLi Gas)
    //   role-nb-gas / role-msb-gas → WimGasModule
    //   role-nb-strom / role-nb  → MabisModule (MABIS PID 13003)
    //
    // Clippy's vec_init_then_push fires here because the pushes are
    // #[cfg]-gated and cannot be merged into a vec![] literal.
    #[expect(clippy::vec_init_then_push)]
    let modules: Vec<Box<dyn mako_engine::builder::EngineModule>> = {
        let mut m: Vec<Box<dyn mako_engine::builder::EngineModule>> = Vec::new();
        // GpkeModule: GPKE PIDs 55001-55018, 55022-55024, 55555, 55607-55609 +
        //   INVOIC 31001/31002/31005/31006 + ORDERS Sperrung 17115-17117 +
        //   ORDERS/ORDRSP Konfiguration 17134/17135/19001/19002 + PARTIN 37000-37006.
        //   Required for both LF (Strom) and NB (Strom) roles.
        #[cfg(any(
            not(any(
                feature = "role-lf-strom",
                feature = "role-lf-gas",
                feature = "role-nb-strom",
                feature = "role-nb-gas",
                feature = "role-msb-strom",
                feature = "role-msb-gas",
            )),
            feature = "role-lf-strom",
            feature = "role-nb-strom",
        ))]
        m.push(Box::new(GpkeModule));

        // WimModule: Messstellenbetrieb Strom (55039, 55042, 55051, 55168) +
        //   ORDERS Geräteübernahme 17001-17011 + INSRPT 23001-23012.
        //   Required for MSB (Strom) and NB (Strom, WiM coordination) roles.
        #[cfg(any(
            not(any(
                feature = "role-lf-strom",
                feature = "role-lf-gas",
                feature = "role-nb-strom",
                feature = "role-nb-gas",
                feature = "role-msb-strom",
                feature = "role-msb-gas",
            )),
            feature = "role-msb-strom",
            feature = "role-nb-strom",
        ))]
        m.push(Box::new(WimModule));

        // GeliGasModule: GeLi Gas 3.0 (44001-44024) + ORDERS Sperrung Gas
        //   17115-17117 + PARTIN Gas 37008-37014 + INVOIC 31011 (AWH Rechnung).
        //   Required for both LF (Gas) and NB (Gas) roles.
        #[cfg(any(
            not(any(
                feature = "role-lf-strom",
                feature = "role-lf-gas",
                feature = "role-nb-strom",
                feature = "role-nb-gas",
                feature = "role-msb-strom",
                feature = "role-msb-gas",
            )),
            feature = "role-lf-gas",
            feature = "role-nb-gas",
        ))]
        m.push(Box::new(GeliGasModule));

        // WimGasModule: WiM Gas (44022-44024, 44039-44053, 44168-44170) +
        //   INVOIC 31003/31004 + INSRPT Gas 23005/23009.
        //   Required for gMSB (Gas) and GNB (Gas) roles.
        #[cfg(any(
            not(any(
                feature = "role-lf-strom",
                feature = "role-lf-gas",
                feature = "role-nb-strom",
                feature = "role-nb-gas",
                feature = "role-msb-strom",
                feature = "role-msb-gas",
            )),
            feature = "role-msb-gas",
            feature = "role-nb-gas",
        ))]
        m.push(Box::new(WimGasModule));

        // GaBiGasModule: GaBi Gas (31010/31007/31008 INVOIC + 13013 MSCONS +
        //   33001 REMADV + 29001 COMDIS).
        //   Required for NB (Gas) role; BKV/MGV interactions are NB-side.
        #[cfg(any(
            not(any(
                feature = "role-lf-strom",
                feature = "role-lf-gas",
                feature = "role-nb-strom",
                feature = "role-nb-gas",
                feature = "role-msb-strom",
                feature = "role-msb-gas",
            )),
            feature = "role-nb-gas",
        ))]
        m.push(Box::new(GaBiGasModule));

        // MabisModule: MABIS Bilanzkreisabrechnung Strom, PID 13003 only (BKV↔ÜNB).
        //   Required for NB (Strom) role.
        #[cfg(any(
            not(any(
                feature = "role-lf-strom",
                feature = "role-lf-gas",
                feature = "role-nb-strom",
                feature = "role-nb-gas",
                feature = "role-msb-strom",
                feature = "role-msb-gas",
            )),
            feature = "role-nb-strom",
        ))]
        m.push(Box::new(MabisModule));

        // RedispatchModule: Redispatch 2.0 (§§ 13/13a/14 EnWG); IFTSTA 21037/21038.
        // Applicable to NB (VNB/ANB) and ÜNB roles only — Lieferant (LF) and MSB
        // deployments are out of scope for Redispatch 2.0 per BK6-20-059/060/061.
        //
        // Gate: registered unconditionally when no role feature flags are active
        // (backward-compatible default), and when role-nb-strom is active (covers
        // VNB, ANB, and ÜNB roles that share the NB Strom deployment role).
        // Excluded for LF-only, MSB-only, and gas-only deployments — none of those
        // roles have Redispatch 2.0 obligations.
        #[cfg(any(
            not(any(
                feature = "role-lf-strom",
                feature = "role-lf-gas",
                feature = "role-nb-strom",
                feature = "role-nb-gas",
                feature = "role-msb-strom",
                feature = "role-msb-gas",
            )),
            feature = "role-nb-strom",
        ))]
        m.push(Box::new(RedispatchModule));
        m
    };

    let ctx = EngineBuilder::with_stores(
        store.clone(),
        store.as_deadline_store(),
        store.as_process_registry(),
    )
    .with_event_store(store.clone())
    // Wire the durable snapshot store so replay cost is bounded to at most
    // 100 tail events per command dispatch instead of O(n) full replay.
    .with_snapshot_store(store.as_snapshot_store())
    // Wire the buffered dead-letter sink so every rejected EDIFACT message is
    // persisted to SlateDB for regulatory audit.
    .with_dead_letter_sink(dl_sink)
    // Validate at startup that each domain module has an active edi-energy
    // profile for its declared message types.  The validator runs inside
    // EngineBuilder::build so a missing profile panics with an actionable message
    // rather than silently dead-lettering at first dispatch.
    .with_profile_validator({
        let today = time::OffsetDateTime::now_utc().date();
        move |msg_type| {
            // If the type code is unrecognised, treat as missing (fail-safe).
            let Some(mt) = edi_energy::MessageType::from_unh_code(msg_type) else {
                return false;
            };
            edi_energy::registry::ReleaseRegistry::global()
                .profiles_for(mt)
                .any(|p| match (p.valid_from(), p.valid_until()) {
                    (Some(from), Some(until)) => from <= today && today <= until,
                    (Some(from), None) => from <= today,
                    (None, _) => true, // legacy profile — always active
                })
        }
    })
    .register_many(modules)
    .with_deployment_roles(effective_deployment_roles)
    .build();

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

    // ── --check mode early exit ────────────────────────────────────────
    //
    // All critical startup checks (profile validator, adapter coverage, data-dir
    // lock acquisition, ProcessRegistry reconciliation) have now completed.
    // In check mode we exit here — no workers, no transports, no listeners.
    if cli.check {
        info!(
            "check mode: all startup validations passed \
             (profiles, adapter coverage, store connectivity, ProcessRegistry reconciliation)"
        );
        return Ok(());
    }

    // ── Graceful-shutdown token ────────────────────────────────────────────────
    //
    // All long-running background tasks and HTTP servers are wired to this
    // token.  When the OS delivers SIGTERM / Ctrl-C, we cancel the token and
    // every listener drains its in-flight requests before the store is closed.
    let shutdown_token = CancellationToken::new();

    // ── Optional: HTTP REST API server ────────────────────────────────────────
    //
    // Enabled by --http-addr / MAKOD_HTTP_ADDR. Provides POST /edifact as a
    // direct EDIFACT ingest alternative to AS4 transport.
    //
    // Construct the Platform once and share the Arc across HTTP and AS4 servers
    // to avoid registering all ~40 generated profile modules twice.
    let platform = Arc::new(Platform::with_all_profiles());

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
        .with_esa_partners(cli.esa_partner_mp_ids.clone()),
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
    // Required whenever any authenticated port is enabled.
    let needs_auth = cli.http_addr.is_some()
        || (cli.api_webdienste_addr.is_some() && !cli.webdienste_allow_unauthenticated);
    if needs_auth && cli.auth_keys.is_empty() && cli.oidc_issuer.is_none() {
        anyhow::bail!(
            "--auth-key / MAKOD_AUTH_KEYS or --oidc-issuer / MAKOD_OIDC_ISSUER is \
             required when --http-addr or --api-webdienste-addr is set.\n\
             These ports perform privileged operations (submitting commands, \
             triggering migrations, API-Webdienste requests) and must not be \
             exposed unauthenticated.\n\
             Provide at least one named API key with --auth-key NAME=TOKEN \
             (e.g. --auth-key erp-prod=$(openssl rand -hex 32)), or configure \
             an OIDC issuer with --oidc-issuer <URL> --oidc-audience <AUD>."
        );
    }
    let oidc = if let Some(issuer) = cli.oidc_issuer.clone() {
        let audience = cli.oidc_audience.clone().context(
            "--oidc-audience / MAKOD_OIDC_AUDIENCE is required when \
             --oidc-issuer / MAKOD_OIDC_ISSUER is set",
        )?;
        let verifier = oidc_verifier::OidcVerifier::new(issuer, audience, &http_client)
            .await
            .context("OIDC verifier initialisation failed")?;
        verifier.spawn_refresh_task(
            http_client.clone(),
            cli.oidc_jwks_refresh_secs,
            shutdown_token.clone(),
        );
        Some(verifier)
    } else {
        None
    };
    let extra_policies = read_cedar_policy_dir(&cli.cedar_policy_dir)
        .context("loading Cedar policy files from --cedar-policy-dir")?;
    let auth_keys = cli
        .auth_keys
        .iter()
        .map(|s| cedar_authz::NamedKey::from_arg(s))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let cedar = Arc::new(
        cedar_authz::CedarAuthorizer::new(auth_keys, extra_policies, oidc)
            .map_err(|e| anyhow::anyhow!("{e}"))?,
    );

    if let Some(addr) = cli.http_addr {
        let api_state = Arc::new(edifact_api::EdifactApiState {
            platform: Arc::clone(&platform),
            pid_router: ctx.pid_router().clone(),
            cedar: Arc::clone(&cedar),
            max_body_bytes: cli.http_max_body_bytes,
            partner_store: Some(Arc::new(store.as_partner_store())),
            tenant_id: mako_engine::ids::TenantId::from_party_id(mp_id_registry.primary_mp_id()),
            dl_sink: Arc::new(dl_sink_ingest.clone()),
            dispatcher: Some(Arc::clone(&ingest_dispatcher)),
            contrl_ack: Some(Arc::new(contrl_ack::ContrlAckService::new(
                Arc::new(store.clone()),
                mako_engine::ids::TenantId::from_party_id(mp_id_registry.primary_mp_id()),
                mp_id_registry.primary_mp_id().to_owned(),
            ))),
        });
        let admin_state = Arc::new(malo_admin_api::MaloAdminState {
            cache: malo_cache::SlateDbMaloCache::new(store.clone()),
            cedar: Arc::clone(&cedar),
            tenant_id: mp_id_registry.primary_mp_id().to_owned(),
        });
        let partner_store = store.as_partner_store();
        let partner_tenant_id =
            mako_engine::ids::TenantId::from_party_id(mp_id_registry.primary_mp_id());
        partner_api::seed_from_config(&partner_store, partner_tenant_id, &cli.as4_partner)
            .await
            .context("seeding partner store from config")?;
        let partner_admin_state = Arc::new(partner_api::PartnerAdminState {
            store: partner_store,
            tenant_id: partner_tenant_id,
            cedar: Arc::clone(&cedar),
            platform: Arc::clone(&platform),
        });
        let commands_state = Arc::new(commands_api::CommandsApiState {
            tenant_id: mako_engine::ids::TenantId::from_party_id(mp_id_registry.primary_mp_id()),
            sender_party_id: mp_id_registry.primary_mp_id().to_owned(),
            configured_marktrollen: effective_marktrollen.to_vec(),
            max_body_bytes: cli.http_max_body_bytes,
            snapshot_interval: cli.snapshot_interval,
            cedar: Arc::clone(&cedar),
            store: Arc::new(store.clone()),
            snapshot_store: store.as_snapshot_store(),
            malo_cache: malo_cache.clone(),
            maloid_result_cache: malo_cache::MaloIdentResultCache::new(store.clone()),
            // M1: Konfigurationsprodukt guard — wired up from config when
            // [m1] marktd_url and marktd_api_key are provided.
            // Falls back to `None` (guard disabled) when not configured.
            marktd_client: None,
        });
        let metrics_state = Arc::new(
            metrics_api::MetricsState::new(
                store.clone(),
                Arc::clone(&cedar),
                mp_id_registry.primary_mp_id().to_owned(),
            )
            .with_volatile_mode(cli.data_dir.is_none() && cli.allow_volatile),
        );
        let migration_state = Arc::new(migration_api::MigrationApiState {
            store: Arc::new(store.clone()),
            cedar: Arc::clone(&cedar),
            tenant: mp_id_registry.primary_mp_id().to_owned(),
        });
        let mcp_state = Arc::new(mcp_server::MakodMcpState {
            tenant: mp_id_registry.primary_mp_id().to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            cedar: Arc::clone(&cedar),
            commands: Arc::clone(&commands_state),
            malo_cache: malo_cache.clone(),
            partner_store: Arc::new(store.as_partner_store()),
            process_store: Arc::new(store.clone()),
            deadline_store: store.as_deadline_store(),
        });
        let invoic_api_state = Arc::new(invoic_api::InvoicApiState {
            store: Arc::new(store.clone()),
            tenant_id: mako_engine::ids::TenantId::from_party_id(mp_id_registry.primary_mp_id()),
            cedar: Arc::clone(&cedar),
            tenant: mp_id_registry.primary_mp_id().to_owned(),
        });
        let app = edifact_api::router(api_state)
            .merge(malo_admin_api::router(admin_state))
            .merge(partner_api::router(partner_admin_state))
            .merge(commands_api::router(commands_state))
            .merge(invoic_api::router(invoic_api_state))
            .merge(metrics_api::router(metrics_state))
            .merge(migration_api::router(migration_state))
            .merge(mcp_server::router(mcp_state, shutdown_token.clone()))
            .merge(health::router(health_state.clone()))
            // W3C trace-context capture for end-to-end tracing.
            .layer(axum::middleware::from_fn(trace_ctx_middleware))
            .merge(openapi::router());
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| anyhow::anyhow!("HTTP server bind {addr}: {e}"))?;
        info!(
            addr         = %addr,
            max_body_mib = cli.http_max_body_bytes / (1024 * 1024),
            "HTTP REST API listening",
        );
        let http_token = shutdown_token.clone();
        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app)
                .with_graceful_shutdown(http_token.cancelled_owned())
                .await
            {
                tracing::error!(error = %e, "HTTP server error");
            }
        });
    }

    // ── Optional: AS4 inbound transport ─────────────────────────────────────
    //
    // Enabled by --as4-addr / MAKOD_AS4_ADDR.  Provides POST /as4/inbox for
    // BDEW EDIFACT UserMessages delivered over AS4/ebMS3 — the mandatory
    // production transport since 2024-04-01 (electricity) / 2025-04-01 (gas).
    //
    // SessionContext requires a signing key + certificate PEM.  For development
    // without real cert material, omit --as4-addr and use the REST API instead.
    if let Some(addr) = cli.as4_addr {
        let party_id = cli
            .as4_party_id
            .clone()
            .unwrap_or_else(|| mp_id_registry.primary_mp_id().to_owned());

        let key_pem = cli.as4_signing_key_pem.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "--as4-signing-key-pem / MAKOD_AS4_SIGNING_KEY_PEM is required when --as4-addr is set"
            )
        })?;
        let cert_pem = cli.as4_signing_cert_pem.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "--as4-signing-cert-pem / MAKOD_AS4_SIGNING_CERT_PEM is required when --as4-addr is set"
            )
        })?;

        //  warn when the trust anchor is the same as the signing cert.
        // The correct production trust anchor is the BDEW/BNetzA PKI CA certificate.
        // Using the self-signed operator cert accepts ONLY the operator's own cert
        // as a peer — all counterparty certificates (signed by the BDEW CA) are
        // rejected.
        if cli
            .as4_trust_anchor_pem
            .as_deref()
            .is_none_or(|ta| ta == cert_pem.as_str())
        {
            tracing::error!(
                "AS4 trust anchor is set to the operator's own signing certificate. \
                 Inbound AS4 messages from all counterparties will be REJECTED because \
                 their certificates are signed by the BDEW PKI CA, not by this operator. \
                 Set --as4-trust-anchor-pem / MAKOD_AS4_TRUST_ANCHOR_PEM to the \
                 BDEW/BNetzA PKI CA certificate to fix this."
            );
        }

        // BDEW AS4-Profil v1.2 §2.2.6.2.2 requires every inbound message to be
        // encrypted.  Without an own decryption private key, `bdew_push_policy`
        // cannot enable `require_encrypted_inbound` and would silently accept
        // unencrypted inbound messages — so this is fail-closed: the daemon
        // refuses to start unless the operator explicitly opts out for dev/test.
        if cli.as4_decryption_key_pem.is_none() {
            if cli.allow_unencrypted_as4 {
                tracing::warn!(
                    "--allow-unencrypted-as4: AS4 inbound decryption key not configured. \
                     Inbound AS4 messages will be accepted WITHOUT verifying that they \
                     are encrypted, violating BDEW AS4-Profil v1.2 §2.2.6.2.2. \
                     This mode is for dev/test only."
                );
            } else {
                anyhow::bail!(
                    "AS4 inbound decryption key not configured \
                     (--as4-decryption-key-pem / MAKOD_AS4_DECRYPTION_KEY_PEM not set). \
                     BDEW AS4-Profil v1.2 §2.2.6.2.2 requires every inbound AS4 message \
                     to be encrypted; without your own EC (BrainpoolP256r1) private key, \
                     unencrypted inbound cannot be rejected. Provide the key, or pass \
                     --allow-unencrypted-as4 for dev/test."
                );
            }
        }
        let session = {
            let session_id = format!("makod-{}", uuid::Uuid::new_v4());
            let trust_anchor = cli
                .as4_trust_anchor_pem
                .clone()
                .unwrap_or_else(|| cert_pem.clone());
            SessionContextBuilder::new(&session_id, &party_id)
                .with_signing_material(cert_pem.clone(), key_pem.expose_secret())
                .with_trust_anchor_pem(trust_anchor)
                .build()
                .map_err(|e| anyhow::anyhow!("AS4 SessionContext build failed: {e}"))?
        };

        let event_bus = Arc::new(
            EventBus::new(256).map_err(|e| anyhow::anyhow!("AS4 EventBus init failed: {e}"))?,
        );

        let dedup: Arc<dyn asx_rs::storage::DedupStorage> =
            Arc::new(as4_ingest::SlateDbDedupBridge::new(
                Arc::new(inbox_store),
                // Durable = true only when backed by a persistent store.
                // In volatile (in-memory) mode, `is_durable` signals to the
                // asx-rs pipeline that dedup state is not preserved across restarts.
                cli.data_dir.is_some(),
            ));

        // ── Startup warning: non-durable dedup + AS4 enabled ─────────────────
        //
        // When --as4-addr is set but no --data-dir is configured, the inbox
        // dedup store is purely in-memory (volatile). A crash or restart will
        // lose all dedup state, allowing replayed AS4 UserMessages to be
        // ingested again as duplicates. This violates BDEW AS4 conformance
        // (the BDEW AS4 profile requires durable duplicate detection per
        // ebMS3 §6.6.1). Set --data-dir to a persistent path in production.
        if cli.data_dir.is_none() {
            tracing::warn!(
                "AS4 inbox dedup storage is volatile (in-memory): duplicate detection \
                 is lost on restart. Set --data-dir / MAKOD_DATA_DIR to a persistent \
                 path to enable durable dedup (required for BDEW AS4 conformance)."
            );
        }

        let ingest_state = Arc::new(edifact_api::EdifactApiState {
            platform: Arc::clone(&platform),
            pid_router: ctx.pid_router().clone(),
            cedar: Arc::new(
                cedar_authz::CedarAuthorizer::unauthenticated()
                    .expect("CedarAuthorizer::unauthenticated is infallible"),
            ),
            max_body_bytes: mako_as4::bdew_router_config().max_body_bytes,
            partner_store: Some(Arc::new(store.as_partner_store())),
            tenant_id: mako_engine::ids::TenantId::from_party_id(mp_id_registry.primary_mp_id()),
            dl_sink: Arc::new(dl_sink_ingest),
            dispatcher: Some(Arc::clone(&ingest_dispatcher)),
            contrl_ack: Some(Arc::new(contrl_ack::ContrlAckService::new(
                Arc::new(store.clone()),
                mako_engine::ids::TenantId::from_party_id(mp_id_registry.primary_mp_id()),
                mp_id_registry.primary_mp_id().to_owned(),
            ))),
        });

        let contrl_svc = Arc::new(contrl_ack::ContrlAckService::new(
            Arc::new(store.clone()),
            mako_engine::ids::TenantId::from_party_id(mp_id_registry.primary_mp_id()),
            mp_id_registry.primary_mp_id().to_owned(),
        ));
        let handler = Arc::new(
            as4_ingest::BdewAs4IngestHandler::new(
                ingest_state,
                Arc::new(session),
                event_bus,
                dedup,
            )
            .with_decryption_key_pem(
                cli.as4_decryption_key_pem
                    .as_ref()
                    .map(|s| s.expose_secret().as_bytes().to_vec()),
            )
            // BDEW AS4-Profil §2.2.4: sign synchronous receipts (NRR) with the
            // operator's signing key pair — the same material used for the
            // AS4 session signing context above.
            .with_receipt_credentials(
                key_pem.expose_secret().as_bytes().to_vec(),
                cert_pem.clone().into_bytes(),
            )
            .with_contrl_ack(Arc::clone(&contrl_svc)),
        );

        let app = as4_ingest::router(handler, mako_as4::bdew_router_config())
            .merge(health::router(health_state.clone()))
            // OWASP A05 — rate limit the AS4 inbound endpoint to prevent
            // capacity exhaustion by a misconfigured or malicious counterparty.
            // Per-peer GCRA token bucket: 100 req/s sustained, burst of 50,
            // keyed by client IP. Returns HTTP 429 when a peer's bucket is
            // exhausted.
            .layer(axum::middleware::from_fn(
                as4_ingest::as4_rate_limit_middleware,
            ))
            // W3C trace-context capture for end-to-end tracing.
            .layer(axum::middleware::from_fn(trace_ctx_middleware));
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| anyhow::anyhow!("AS4 server bind {addr}: {e}"))?;
        info!(
            addr     = %addr,
            party_id = %party_id,
            "AS4 inbound transport listening (BDEW MaKo mandatory since 2024-04-01)",
        );
        let as4_token = shutdown_token.clone();
        tokio::spawn(async move {
            // `into_make_service_with_connect_info` provides the peer socket
            // address the per-IP rate limiter keys on.
            if let Err(e) = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(as4_token.cancelled_owned())
            .await
            {
                tracing::error!(error = %e, "AS4 server error");
            }
        });
    } else {
        // AS4 not configured: retain inbox_store so its startup lock-map is
        // initialised and available for future use.
        let _inbox_store = inbox_store;
        tracing::warn!(
            "AS4 inbound transport is NOT configured \
             (--as4-addr / MAKOD_AS4_ADDR unset). \
             BDEW EDIFACT messages cannot be received via the mandatory AS4 \
             transport. Set --as4-addr and provide signing key/cert PEM for production."
        );
    }

    // ── Optional: API-Webdienste Strom server ────────────────────────────
    //
    // Enabled by --api-webdienste-addr / MAKOD_API_WEBDIENSTE_ADDR.
    // Provides BDEW API-Webdienste Strom endpoints (Control Measures v1,
    // MaLo Identification v1) on a separate port.
    //
    // MaLo Identification is active. Control Measures are wired to
    // WimSteuerungsauftragWorkflow.
    if let Some(addr) = cli.api_webdienste_addr {
        let handler = Arc::new(webdienste::MakodApiHandler {
            store: store.clone(),
            tenant_id: mako_engine::ids::TenantId::from_party_id(mp_id_registry.primary_mp_id()),
            sender_party_id: mp_id_registry.primary_mp_id().to_owned(),
        });
        // ── API-Webdienste authentication ─────────────────────────────────
        //
        // The BDEW API-Webdienste specification requires authenticated
        // access. Every route on :8090 sits behind bearer/OIDC
        // authentication plus the Cedar `UseWebdienste` action, and a
        // body-size limit. `--webdienste-allow-unauthenticated` disables the
        // auth layer for deployments that terminate mTLS (BDEW PKI CA) at a
        // fronting proxy and enforce access there.
        let wd_routes = webdienste::router(handler).layer(axum::extract::DefaultBodyLimit::max(
            cli.http_max_body_bytes,
        ));
        let wd_routes = if cli.webdienste_allow_unauthenticated {
            tracing::warn!(
                addr = %addr,
                "--webdienste-allow-unauthenticated: API-Webdienste Strom port \
                 has NO authentication. Only acceptable behind a proxy that \
                 terminates mTLS with the BDEW PKI CA.",
            );
            wd_routes
        } else {
            wd_routes.layer(axum::middleware::from_fn_with_state(
                webdienste::WebdiensteAuthState {
                    cedar: Arc::clone(&cedar),
                    tenant: Arc::from(mp_id_registry.primary_mp_id()),
                },
                webdienste::webdienste_auth_middleware,
            ))
        };
        // Per-peer rate limit, same GCRA policy as the AS4 port.
        let app = wd_routes
            .layer(axum::middleware::from_fn(as4_ingest::rate_limit_middleware))
            // W3C trace-context capture for end-to-end tracing.
            .layer(axum::middleware::from_fn(trace_ctx_middleware))
            .merge(health::router(health_state.clone()));
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| anyhow::anyhow!("API-Webdienste server bind {addr}: {e}"))?;
        info!(
            addr = %addr,
            primary_mp_id = mp_id_registry.primary_mp_id(),
            "API-Webdienste Strom server listening (MaLo Identification active)",
        );
        let wd_token = shutdown_token.clone();
        tokio::spawn(async move {
            if let Err(e) = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(wd_token.cancelled_owned())
            .await
            {
                tracing::error!(error = %e, "API-Webdienste server error");
            }
        });
    }

    // ── Background workers ────────────────────────────────────────────────────
    //
    // Outbox delivery, ERP webhook, deadline scheduler, projection checkpoint,
    // inbox purge — all spawned as Tokio tasks that exit on shutdown_token.
    // See `startup::spawn_workers` and `startup::WorkersConfig` for details.
    startup::spawn_workers(startup::WorkersConfig {
        ctx,
        store: store.clone(),
        inbox_store_for_purge,
        platform: Arc::clone(&platform),
        ingest_dispatcher: Arc::clone(&ingest_dispatcher),
        http_client,
        malo_cache: Arc::clone(&malo_cache),
        shutdown_token: shutdown_token.clone(),
        mp_id_registry: Arc::clone(&mp_id_registry),
        as4_partner: cli.as4_partner.clone(),
        as4_signing_key_pem: cli.as4_signing_key_pem.clone(),
        as4_signing_cert_pem: cli.as4_signing_cert_pem.clone(),
        as4_trust_anchor_pem: cli.as4_trust_anchor_pem.clone(),
        as4_partner_certs: cli.as4_partner_cert.clone(),
        allow_unencrypted_as4: cli.allow_unencrypted_as4,
        as4_lenient_receipts: cli.as4_lenient_receipts,
        dead_letter_sink: dl_sink_workers,
        as4_party_id: cli.as4_party_id.clone(),
        maloid_partner: cli.maloid_partner.clone(),
        verzeichnisdienst_url: cli.verzeichnisdienst_url.clone(),
        erp_webhook_url: cli.erp_webhook_url.clone(),
        erp_webhook_secret: cli.erp_webhook_secret.clone(),
        edifact_outbox_webhook_url: cli.edifact_outbox_webhook_url.clone(),
        allow_no_as4_signing: cli.allow_no_as4_signing,
        snapshot_interval: cli.snapshot_interval,
        deadline_poll_interval_secs: cli.deadline_poll_interval_secs,
        projection_checkpoint_interval: cli.projection_checkpoint_interval,
        no_transport_configured: cli.as4_addr.is_none() && cli.http_addr.is_none(),
        health_state: health_state.clone(),
    })
    .await?;

    wait_for_shutdown().await;
    info!("Mako engine shutting down — cancelling listeners");
    // Signal all HTTP/AS4/API-Webdienste servers to stop accepting new connections
    // and drain in-flight requests before we close the event store.
    shutdown_token.cancel();

    // ── Graceful dead-letter drain ────────────────────────────────────
    //
    // 1. Close the DL channel so `reject()` becomes a no-op and the worker
    //    can drain its buffer without new entries racing in.
    // 2. Give the worker up to 5 s to persist any buffered entries.
    // 3. Then close the store — safe because the worker is done.
    dl_sink_shutdown.signal_shutdown();
    match tokio::time::timeout(Duration::from_secs(5), dl_worker_handle).await {
        Ok(Ok(n)) => info!(entries = n, "dead-letter worker drained and exited"),
        Ok(Err(e)) => tracing::error!(error = %e, "dead-letter worker panicked"),
        Err(_) => tracing::warn!("dead-letter worker drain timed out after 5 s"),
    }

    let shutdown_timeout = Duration::from_secs(cli.shutdown_timeout_secs);
    match tokio::time::timeout(shutdown_timeout, store.close_owned()).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::error!(error = %e, "store close failed"),
        Err(_elapsed) => tracing::error!(
            timeout_secs = cli.shutdown_timeout_secs,
            "store close timed out; data may not be fully flushed"
        ),
    }
    Ok(())
}

// ── Config file merging ──────────────────────────────────────────────────────

// ── Cedar helpers ─────────────────────────────────────────────────────────────

/// Load and concatenate all `*.cedar` files from `dir`.
///
/// Scope the request's W3C `traceparent` header into the engine task-local.
///
/// Every `OutboxMessage` created while handling the request captures it into
/// its persisted `trace_context`, and the delivery workers re-inject it into
/// outbound HTTP — end-to-end tracing across the asynchronous outbox
/// boundary.
async fn trace_ctx_middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let tp = req
        .headers()
        .get("traceparent")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    mako_engine::trace_ctx::TRACEPARENT
        .scope(tp, next.run(req))
        .await
}

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

fn apply_config_file(
    cfg: config::ConfigFile,
    matches: &clap::ArgMatches,
    cli: &mut Cli,
) -> anyhow::Result<()> {
    use clap::{ValueEnum, parser::ValueSource};

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
    if let Some(wd) = cfg.webdienste
        && cli.api_webdienste_addr.is_none()
    {
        cli.api_webdienste_addr = wd.addr;
    }

    // ── Engine ────────────────────────────────────────────────────────────────
    if let Some(engine) = cfg.engine
        && is_default("shutdown_timeout_secs")
        && let Some(secs) = engine.shutdown_timeout_secs
    {
        cli.shutdown_timeout_secs = secs;
    }

    // ── AS4 ───────────────────────────────────────────────────────────────────
    if let Some(as4) = cfg.as4 {
        if cli.as4_addr.is_none() {
            cli.as4_addr = as4.addr;
        }
        if cli.as4_party_id.is_none() {
            cli.as4_party_id = as4.party_id;
        }
        // Inline PEM takes precedence over a file reference.
        if cli.as4_signing_key_pem.is_none() {
            if let Some(pem) = as4.signing_key_pem {
                cli.as4_signing_key_pem = Some(SecretString::new(pem.into()));
            } else if let Some(ref path) = as4.signing_key_pem_file {
                cli.as4_signing_key_pem = Some(SecretString::new(
                    std::fs::read_to_string(path)
                        .with_context(|| format!("reading AS4 signing key: {}", path.display()))?
                        .into(),
                ));
            }
        }
        if cli.as4_signing_cert_pem.is_none() {
            if let Some(pem) = as4.signing_cert_pem {
                cli.as4_signing_cert_pem = Some(pem);
            } else if let Some(ref path) = as4.signing_cert_pem_file {
                cli.as4_signing_cert_pem =
                    Some(std::fs::read_to_string(path).with_context(|| {
                        format!("reading AS4 signing cert: {}", path.display())
                    })?);
            }
        }
        // CLI partners take full precedence; config partners are used only
        // when the CLI list is empty (no --as4-partner flags were passed).
        if cli.as4_partner.is_empty()
            && let Some(partners) = as4.partners
        {
            cli.as4_partner = partners;
        }
    }

    // ── ERP ───────────────────────────────────────────────────────────────────
    if let Some(erp) = cfg.erp {
        if cli.erp_webhook_url.is_none() {
            cli.erp_webhook_url = erp.webhook_url;
        }
        if cli.erp_webhook_secret.is_none() {
            cli.erp_webhook_secret = erp.webhook_secret.map(|s| SecretString::new(s.into()));
        }
    }

    // ── [[party]] — multi-GLN identity table ─────────────────────────────────
    //
    // Takes precedence over `[engine] tenant_id` when present.  Stored
    // separately (not merged into the CLI struct's string fields) because the
    // array-of-tables structure has no CLI equivalent.
    if let Some(parties) = cfg.party
        && !parties.is_empty()
    {
        cli.parties = parties;
    }

    Ok(())
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
                         §22 MessZV and BDEW AHB. Never use it in production."
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
                         unencrypted. This violates §22 MessZV audit-trail \
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
/// When `OTEL_EXPORTER_OTLP_ENDPOINT` is set, delegates to
/// `mako_service::telemetry` — spans (including the AS4 ingest and outbox
/// delivery spans) export via OTLP/gRPC with W3C propagation, joining the
/// header-level `traceparent` chain the outbox already persists. Without the
/// endpoint, the local fmt subscriber keeps the existing pretty/compact/json
/// behaviour.
fn init_tracing(cli: &Cli) -> Option<mako_service::telemetry::OtelGuard> {
    use tracing_subscriber::{EnvFilter, fmt};

    if std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok() {
        return Some(mako_service::telemetry::init_tracing_from_env("makod"));
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

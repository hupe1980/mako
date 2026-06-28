//! `makod` — Mako process engine daemon.
//!
//! Assembles all domain modules (GPKE, WiM, GeLi Gas, MABIS) into a single
//! [`EngineContext`] and runs until a graceful shutdown signal is received.
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
//!       --as4-signing-key-pem <PEM>        PEM private key for AS4 signing [env: MAKOD_AS4_SIGNING_KEY_PEM=]
//!       --as4-signing-cert-pem <PEM>       PEM X.509 certificate for AS4 signing [env: MAKOD_AS4_SIGNING_CERT_PEM=]
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
//! - `GET  /admin/partners/{gln}` — retrieve a single partner record
//! - `PUT  /admin/partners/{gln}` — create or update a partner record (JSON body)
//! - `DELETE /admin/partners/{gln}` — remove a partner record
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
//!         ├── GpkeModule    — UTILMD PIDs 55001–55002, 55017, 56001–56004 (`gpke-supplier-change`)
//!         │                   + INVOIC PIDs 31001–31002, 31004–31008 (`gpke-abrechnung`)
//!         ├── WimModule     — PIDs 11001–11099 (WiM Gerätewechsel/-betrieb)
//!         ├── GeliGasModule — PIDs 44001–44006, 44017–44018, 44555 (GeLi Gas Lieferbeginn/-ende)
//!         ├── WimGasModule  — PIDs 44022–44024, 44039–44053, 44168–44170 (WiM Gas MSB-Wechsel)
//!         └── MabisModule   — PID 13003 only (MABIS Bilanzkreisabrechnung Strom, MSCONS Summenzeitreihe)
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
mod commands_api;
mod config;
mod deadline_dispatch;
mod edifact_api;
mod edifact_renderer;
mod erp_adapter;
mod health;
mod malo_admin_api;
mod malo_cache;
mod malo_ident_sender;
mod metrics_api;
mod partner_api;
mod projection_worker;
mod verzeichnisdienst_worker;
mod webdienste;

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
use mako_geli_gas::GeliGasModule;
use mako_gpke::GpkeModule;
use mako_mabis::MabisModule;
use mako_wim::WimModule;
use mako_wim_gas::WimGasModule;
use secrecy::{ExposeSecret as _, SecretString};
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

    /// Bearer token for the HTTP REST API.
    ///
    /// When set, every request to `POST /edifact` must include
    /// `Authorization: Bearer <TOKEN>`. `GET /health` is always public.
    /// When omitted, the API runs unauthenticated — a warning is logged.
    ///
    /// Can also be set via the `MAKOD_HTTP_API_TOKEN` environment variable.
    #[arg(
        long,
        value_name = "TOKEN",
        env = "MAKOD_HTTP_API_TOKEN",
        hide_env_values = true,
        value_parser = |s: &str| Ok::<SecretString, std::convert::Infallible>(SecretString::new(s.to_owned())),
    )]
    http_api_token: Option<SecretString>,

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

    /// Operator market-participant identifier (BDEW code 13-digit, GLN 13-digit,
    /// EIC 16-char, or any opaque string used as the tenant scope).
    ///
    /// In EDIFACT NAD segments this value is emitted with agency code `"293"`
    /// (BDEW) by default. Use `--party-agency` to override for EIC / GS1 parties.
    ///
    /// Used to scope MaLo cache entries and inbox idempotency keys to this
    /// operator instance. Defaults to `"default"` when omitted.
    ///
    /// Can also be set via the `MAKOD_TENANT_ID` environment variable.
    #[arg(
        long,
        value_name = "ID",
        default_value = "default",
        env = "MAKOD_TENANT_ID"
    )]
    tenant_id: String,

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
        value_parser = |s: &str| Ok::<SecretString, std::convert::Infallible>(SecretString::new(s.to_owned())),
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
    /// Available roles: `NB`, `LF`, `MSB`, `NMSB`, `AMSB`, `BKV`, `UENB`, `BIKO`
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
        value_parser = |s: &str| Ok::<SecretString, std::convert::Infallible>(SecretString::new(s.to_owned())),
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
    init_tracing(&cli);

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
    // worker drains the channel to SlateDB in the background.  We keep a
    // clone of the sink for shutdown signalling (see below).
    let (dl_sink, dl_worker) = store.as_dead_letter_sink();
    let dl_sink_shutdown = dl_sink.clone();
    let dl_worker_handle = tokio::spawn(dl_worker.run());

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
    .register(Box::new(GpkeModule))
    .register(Box::new(WimModule))
    .register(Box::new(GeliGasModule))
    .register(Box::new(WimGasModule))
    .register(Box::new(MabisModule))
    .with_deployment_roles(parse_deployment_roles(&cli.deployment_roles))
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
    // would be silently dead-lettered. Fail fast at startup.
    {
        let known = adapters::known_fvs();
        let fv_names: Vec<&str> = known.iter().map(|fv| fv.as_str()).collect();

        let gpke_check = adapters::gpke_registry().validate_policy(
            &mako_engine::version::WorkflowVersionPolicy::ForwardCompatible,
            &known,
        );
        let gpke_lf_anmeldung_check = adapters::gpke_lf_anmeldung_registry().validate_policy(
            &mako_engine::version::WorkflowVersionPolicy::ForwardCompatible,
            &known,
        );
        let gpke_neuanlage_check = adapters::gpke_neuanlage_registry().validate_policy(
            &mako_engine::version::WorkflowVersionPolicy::ForwardCompatible,
            &known,
        );
        let gpke_lf_abmeldung_check = adapters::gpke_lf_abmeldung_registry().validate_policy(
            &mako_engine::version::WorkflowVersionPolicy::ForwardCompatible,
            &known,
        );
        let gpke_sperrung_check = adapters::gpke_sperrung_registry().validate_policy(
            &mako_engine::version::WorkflowVersionPolicy::ForwardCompatible,
            &known,
        );
        let gpke_abrechnung_check = adapters::gpke_abrechnung_registry().validate_policy(
            &mako_engine::version::WorkflowVersionPolicy::ForwardCompatible,
            &known,
        );
        let gpke_konfiguration_check = adapters::gpke_konfiguration_registry().validate_policy(
            &mako_engine::version::WorkflowVersionPolicy::ForwardCompatible,
            &known,
        );
        let wim_check = adapters::wim_registry().validate_policy(
            &mako_engine::version::WorkflowVersionPolicy::ForwardCompatible,
            &known,
        );
        let wim_geraeteubernahme_check = adapters::wim_geraeteubernahme_registry().validate_policy(
            &mako_engine::version::WorkflowVersionPolicy::ForwardCompatible,
            &known,
        );
        let wim_stammdaten_check = adapters::wim_stammdaten_registry().validate_policy(
            &mako_engine::version::WorkflowVersionPolicy::ForwardCompatible,
            &known,
        );
        let wim_stornierung_check = adapters::wim_stornierung_registry().validate_policy(
            &mako_engine::version::WorkflowVersionPolicy::ForwardCompatible,
            &known,
        );
        let wim_rechnung_check = adapters::wim_rechnung_registry().validate_policy(
            &mako_engine::version::WorkflowVersionPolicy::ForwardCompatible,
            &known,
        );
        let geli_check = adapters::geli_gas_registry().validate_policy(
            &mako_engine::version::WorkflowVersionPolicy::ForwardCompatible,
            &known,
        );
        let geli_sperrung_check = adapters::geli_gas_sperrung_registry().validate_policy(
            &mako_engine::version::WorkflowVersionPolicy::ForwardCompatible,
            &known,
        );
        let wim_gas_anmeldung_check = adapters::wim_gas_anmeldung_registry().validate_policy(
            &mako_engine::version::WorkflowVersionPolicy::ForwardCompatible,
            &known,
        );
        let wim_gas_kuendigung_check = adapters::wim_gas_kuendigung_registry().validate_policy(
            &mako_engine::version::WorkflowVersionPolicy::ForwardCompatible,
            &known,
        );
        let wim_gas_verpflichtungsanfrage_check =
            adapters::wim_gas_verpflichtungsanfrage_registry().validate_policy(
                &mako_engine::version::WorkflowVersionPolicy::ForwardCompatible,
                &known,
            );
        let mabis_check = adapters::mabis_registry().validate_policy(
            &mako_engine::version::WorkflowVersionPolicy::ForwardCompatible,
            &known,
        );

        for (name, result) in [
            ("gpke-supplier-change", gpke_check),
            ("gpke-lf-anmeldung", gpke_lf_anmeldung_check),
            ("gpke-neuanlage", gpke_neuanlage_check),
            ("gpke-lf-abmeldung", gpke_lf_abmeldung_check),
            ("gpke-sperrung", gpke_sperrung_check),
            ("gpke-abrechnung", gpke_abrechnung_check),
            ("gpke-konfiguration", gpke_konfiguration_check),
            ("wim-device-change", wim_check),
            ("wim-geraeteubernahme", wim_geraeteubernahme_check),
            ("wim-stammdaten", wim_stammdaten_check),
            ("wim-stornierung", wim_stornierung_check),
            ("wim-rechnung", wim_rechnung_check),
            ("geli-gas-supplier-change", geli_check),
            ("geli-gas-sperrung", geli_sperrung_check),
            ("wim-gas-anmeldung", wim_gas_anmeldung_check),
            ("wim-gas-kuendigung", wim_gas_kuendigung_check),
            (
                "wim-gas-verpflichtungsanfrage",
                wim_gas_verpflichtungsanfrage_check,
            ),
            // mabis-billing: IFTSTA adapter is registered (PIDs 21000–21007).
            // MSCONS PID 13003 billing commands are constructed by the
            // aggregation layer; this check validates IFTSTA coverage only.
            ("mabis-billing (IFTSTA)", mabis_check),
        ] {
            match result {
                Ok(()) => {
                    info!(workflow = name, format_versions = ?fv_names, "adapter coverage validated")
                }
                Err(uncovered) => {
                    let missing: Vec<&str> = uncovered.iter().map(|fv| fv.as_str()).collect();
                    // Panic at startup rather than silently dead-lettering
                    // messages in production.
                    panic!(
                        "startup failure: workflow {name:?} has no registered MessageAdapter \
                         for format versions {missing:?}. Register adapters in adapters.rs."
                    );
                }
            }
        }

        // mabis-billing IFTSTA adapter is now in the validation loop above.
        // MSCONS PID 13003 billing commands continue to be constructed by the
        // aggregation layer; the adapter registered for mabis-billing handles
        // only inbound IFTSTA messages (PIDs 21000–21007).
    }

    // ── --check mode early exit ────────────────────────────────────────
    //
    // All critical startup checks (profile validator, adapter coverage, data-dir
    // lock acquisition, ProcessRegistry reconciliation) have now completed.
    // In check mode we exit here — no workers, no transports, no listeners.
    if cli.check {
        // Hard-fail if any Noop store is active — a misconfigured deployment
        // (e.g. missing [outbox] section in makod.toml) must never silently
        // start with a Noop backend.
        ctx.assert_production_stores();
        info!(
            "check mode: all startup validations passed \
             (profiles, adapter coverage, store connectivity, ProcessRegistry reconciliation)"
        );
        return Ok(());
    }

    // ── Optional: HTTP REST API server ────────────────────────────────────────
    //
    // Enabled by --http-addr / MAKOD_HTTP_ADDR. Provides POST /edifact as a
    // direct EDIFACT ingest alternative to AS4 transport.
    //
    // Construct the Platform once and share the Arc across HTTP and AS4 servers
    // to avoid registering all ~40 generated profile modules twice.
    let platform = Arc::new(Platform::with_all_profiles());

    // ── Shared health state ───────────────────────────────────────────────────
    //
    // GET /health is mounted on every exposed port so that container
    // orchestrators (Kubernetes, ECS, Docker Swarm) have a consistent liveness
    // + readiness probe target.  The handler pings the SlateDB store; 503 means
    // the store is closed or unreachable.
    let health_state = health::HealthState::new(store.clone());

    if let Some(addr) = cli.http_addr {
        if cli.http_api_token.is_none() {
            tracing::warn!(
                "HTTP REST API is running WITHOUT authentication — \
                 set --http-api-token / MAKOD_HTTP_API_TOKEN for production"
            );
        }
        let api_state = Arc::new(edifact_api::EdifactApiState {
            platform: Arc::clone(&platform),
            pid_router: ctx.pid_router().clone(),
            optional_token: cli.http_api_token.clone(),
            max_body_bytes: cli.http_max_body_bytes,
        });
        let admin_state = Arc::new(malo_admin_api::MaloAdminState {
            cache: malo_cache::SlateDbMaloCache::new(store.clone()),
            optional_token: cli.http_api_token.clone(),
        });
        let partner_store = store.as_partner_store();
        let partner_tenant_id = mako_engine::ids::TenantId::from_party_id(&cli.tenant_id);
        partner_api::seed_from_config(&partner_store, partner_tenant_id, &cli.as4_partner)
            .await
            .context("seeding partner store from config")?;
        let partner_admin_state = Arc::new(partner_api::PartnerAdminState {
            store: partner_store,
            tenant_id: partner_tenant_id,
            optional_token: cli.http_api_token.clone(),
        });
        let commands_state = Arc::new(commands_api::CommandsApiState {
            tenant_id: mako_engine::ids::TenantId::from_party_id(&cli.tenant_id),
            sender_party_id: cli.tenant_id.clone(),
            configured_marktrollen: cli.marktrollen.iter().map(|s| s.to_uppercase()).collect(),
            max_body_bytes: cli.http_max_body_bytes,
            snapshot_interval: cli.snapshot_interval,
            optional_token: cli.http_api_token.clone(),
            store: Arc::new(store.clone()),
            snapshot_store: store.as_snapshot_store(),
            malo_cache: malo_cache.clone(),
            maloid_result_cache: malo_cache::MaloIdentResultCache::new(store.clone()),
        });
        let metrics_state = Arc::new(metrics_api::MetricsState::new(
            store.clone(),
            cli.http_api_token.clone(),
        ));
        let app = edifact_api::router(api_state)
            .merge(malo_admin_api::router(admin_state))
            .merge(partner_api::router(partner_admin_state))
            .merge(commands_api::router(commands_state))
            .merge(metrics_api::router(metrics_state))
            .merge(health::router(health_state.clone()));
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| anyhow::anyhow!("HTTP server bind {addr}: {e}"))?;
        info!(
            addr          = %addr,
            authenticated = cli.http_api_token.is_some(),
            max_body_mib  = cli.http_max_body_bytes / (1024 * 1024),
            "HTTP REST API listening",
        );
        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
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
            .unwrap_or_else(|| cli.tenant_id.clone());

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
        let session = {
            let session_id = format!("makod-{}", uuid::Uuid::new_v4());
            let trust_anchor = cli
                .as4_trust_anchor_pem
                .clone()
                .unwrap_or_else(|| cert_pem.clone());
            SessionContextBuilder::new(&session_id, &party_id)
                .with_signing_cert_pem(cert_pem.clone())
                .with_signing_key_pem(key_pem.expose_secret().clone())
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

        let ingest_state = Arc::new(edifact_api::EdifactApiState {
            platform: Arc::clone(&platform),
            pid_router: ctx.pid_router().clone(),
            optional_token: None,
            max_body_bytes: mako_as4::bdew_router_config().max_body_bytes,
        });

        let handler = Arc::new(as4_ingest::BdewAs4IngestHandler::new(
            ingest_state,
            Arc::new(session),
            event_bus,
            dedup,
        ));

        let app = as4_ingest::router(handler, mako_as4::bdew_router_config())
            .merge(health::router(health_state.clone()));
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| anyhow::anyhow!("AS4 server bind {addr}: {e}"))?;
        info!(
            addr     = %addr,
            party_id = %party_id,
            "AS4 inbound transport listening (BDEW MaKo mandatory since 2024-04-01)",
        );
        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
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
            snapshot_store: store.as_snapshot_store(),
            tenant_id: mako_engine::ids::TenantId::from_party_id(&cli.tenant_id),
            sender_party_id: cli.tenant_id.clone(),
            snapshot_interval: cli.snapshot_interval,
        });
        let app = webdienste::router(handler).merge(health::router(health_state.clone()));
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| anyhow::anyhow!("API-Webdienste server bind {addr}: {e}"))?;
        info!(
            addr = %addr,
            tenant_id = cli.tenant_id,
            "API-Webdienste Strom server listening (MaLo Identification active)",
        );
        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!(error = %e, "API-Webdienste server error");
            }
        });
    }

    // ── Background: outbox delivery worker ───────────────────────────────────
    //
    // Drains pending outbox messages and delivers them via AS4.
    //
    // If AS4 signing credentials are configured (`--as4-signing-key-pem` +
    // `--as4-signing-cert-pem`), we wire `BdewAs4Sender` — a real AS4 client
    // that POSTs signed SOAP envelopes to each recipient's registered endpoint.
    //
    // Otherwise we fall back to `MaloIdentSender` which handles only
    // `MaloIdentCallback` messages (MaLo identification callbacks) and logs all
    // other outbox messages at WARN level without transmitting them.

    // Build the shared reqwest client for outbound HTTP (MaLo-ID callbacks).
    let http_client = reqwest::Client::builder()
        .build()
        .map_err(|e| anyhow::anyhow!("HTTP client build: {e}"))?;

    // Parse --maloid-partner GLN=URL pairs.
    let maloid_partners = {
        let mut map = std::collections::HashMap::new();
        for pair in &cli.maloid_partner {
            let (gln, url_str) = pair.split_once('=').ok_or_else(|| {
                anyhow::anyhow!("--maloid-partner: expected GLN=URL, got {pair:?}")
            })?;
            let url = reqwest::Url::parse(url_str)
                .map_err(|e| anyhow::anyhow!("--maloid-partner: invalid URL {url_str:?}: {e}"))?;
            map.insert(gln.to_owned(), url);
        }
        if !map.is_empty() {
            let glns: Vec<&str> = map.keys().map(String::as_str).collect();
            info!(partners = ?glns, "MaLo-ID partner directory loaded");
        }
        map
    };

    // Build Verzeichnisdienst lookup helper when --verzeichnisdienst-url is set.
    let verzeichnisdienst_lookup: Option<verzeichnisdienst_worker::VerzeichnisdienstLookup> =
        if let Some(ref vz_url_str) = cli.verzeichnisdienst_url {
            let base_url = reqwest::Url::parse(vz_url_str).map_err(|e| {
                anyhow::anyhow!("--verzeichnisdienst-url: invalid URL {vz_url_str:?}: {e}")
            })?;
            let vz_client = energy_api::directory::DirectoryServiceClient::new(
                base_url.clone(),
                http_client.clone(),
            );
            let vz_partner_store = store.as_partner_store();
            let vz_tenant_id = mako_engine::ids::TenantId::from_party_id(&cli.tenant_id);
            info!(url = %base_url, "Verzeichnisdienst integration enabled");
            let lookup = verzeichnisdienst_worker::VerzeichnisdienstLookup::new(
                vz_client,
                vz_partner_store,
                vz_tenant_id,
            );
            // Spawn the periodic refresh task (every 5 minutes).
            let refresh_lookup = lookup.clone();
            tokio::spawn(verzeichnisdienst_worker::verzeichnisdienst_refresh_task(
                refresh_lookup,
                std::time::Duration::from_secs(300),
            ));
            Some(lookup)
        } else {
            None
        };

    let malo_sender = malo_ident_sender::MaloIdentSender::new(
        (*malo_cache).clone(),
        http_client,
        maloid_partners,
        verzeichnisdienst_lookup,
        store.clone(),
    );

    // Parse --as4-partner GLN=URL pairs into the partner directory.
    let partners = mako_as4::PartnerDirectory::from_cli_pairs(&cli.as4_partner)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if !partners.is_empty() {
        let glns: Vec<&str> = partners.iter().map(|(g, _)| g).collect();
        info!(partners = ?glns, "AS4 partner directory loaded");
    }

    // Spawn the outbox worker: BdewAs4Sender when signing credentials are
    // available, MaloIdentSender otherwise.  The two branches produce
    // different concrete types, so the worker is spawned inside each branch
    // rather than assigned to a shared variable.
    if let (Some(key_pem), Some(cert_pem)) = (
        cli.as4_signing_key_pem
            .as_ref()
            .map(|s| s.expose_secret().as_str()),
        cli.as4_signing_cert_pem.as_deref(),
    ) {
        let party_id = cli
            .as4_party_id
            .clone()
            .unwrap_or_else(|| cli.tenant_id.clone());

        // Build a dedicated outbound session (may be separate from the AS4
        // inbound server session if --as4-addr is also configured).
        let outbound_session = {
            let session_id = format!("makod-outbound-{}", uuid::Uuid::new_v4());
            let trust_anchor = cli
                .as4_trust_anchor_pem
                .clone()
                .unwrap_or_else(|| cert_pem.to_owned());
            asx_rs::core::SessionContextBuilder::new(&session_id, &party_id)
                .with_signing_cert_pem(cert_pem.to_owned())
                .with_signing_key_pem(key_pem.to_owned())
                .with_trust_anchor_pem(trust_anchor)
                .build()
                .map_err(|e| anyhow::anyhow!("AS4 outbound session build failed: {e}"))?
        };
        let outbound_bus = Arc::new(
            asx_rs::observability::EventBus::new(256)
                .map_err(|e| anyhow::anyhow!("AS4 EventBus (outbound) init failed: {e}"))?,
        );

        let sender = as4_sender::BdewAs4Sender::new(
            Arc::new(outbound_session),
            outbound_bus,
            Arc::new(partners),
            malo_sender,
            cli.tenant_id.as_str(),
        )?;

        info!(
            party_id        = %party_id,
            tenant_party_id = %cli.tenant_id,
            "AS4 outbound sender active (BdewAs4Sender); \
             UTILMD/APERAK/CONTRL/ORDERS/ORDRSP/INVOIC/REMADV rendered to conformant \
             EDIFACT wire bytes. MSCONS requires external meter readings and is \
             dead-lettered (RendererNotImplemented) until metering data is available.",
        );

        let worker = ctx.run_outbox_worker(sender, 50, Duration::from_secs(5), 48);
        tokio::spawn(async move { worker.run().await });
    } else {
        tracing::warn!(
            "AS4 signing credentials not configured \
             (--as4-signing-key-pem / --as4-signing-cert-pem not set). \
             Outbox delivery is running in MaloIdentCallback-only mode — \
             all EDIFACT messages (APERAK, CONTRL, billing) will be logged \
             and rescheduled without transmission. \
             Provide signing credentials to enable full AS4 outbound delivery."
        );
        let worker = ctx.run_outbox_worker(malo_sender, 50, Duration::from_secs(5), 48);
        tokio::spawn(async move { worker.run().await });
    }

    info!("outbox delivery worker started");

    // ── Optional: ERP webhook outbound worker ─────────────────────────────────
    //
    // Enabled by --erp-webhook-url / MAKOD_ERP_WEBHOOK_URL.
    //
    // When set, starts an `OutboxErpWorker` that drains outbox entries that
    // carry a BO4E payload (i.e. `payload_schema` is set) and POSTs them to
    // the configured ERP endpoint as `ErpEvent` JSON objects.
    //
    // When omitted, ERP events are only logged via `LogErpAdapter` — no HTTP
    // delivery occurs.  This is the safe default for environments where no ERP
    // integration is configured yet.
    if let Some(erp_url) = cli.erp_webhook_url.clone() {
        let adapter =
            erp_adapter::WebhookErpAdapter::new(erp_url.clone(), cli.erp_webhook_secret.clone());
        let worker = erp_adapter::OutboxErpWorker::new(
            store.clone(),
            adapter,
            50,
            std::time::Duration::from_secs(5),
        );
        info!(
            erp_webhook_url = %erp_url,
            "ERP webhook outbound worker started (OutboxErpWorker)",
        );
        tokio::spawn(async move { worker.run().await });
    } else {
        // No ERP URL configured — wire LogErpAdapter for structured log output.
        let adapter = mako_engine::erp::LogErpAdapter;
        let worker = erp_adapter::OutboxErpWorker::new(
            store.clone(),
            adapter,
            50,
            std::time::Duration::from_secs(30),
        );
        tracing::debug!(
            "ERP outbound notifications are logged only \
             (--erp-webhook-url not set; set to enable HTTP delivery)",
        );
        tokio::spawn(async move { worker.run().await });
    }

    //
    // A deployment with no --as4-addr and no --http-addr cannot receive any
    // inbound messages.  This is almost always a misconfiguration.  Fail fast
    // so the operator notices immediately rather than silently discarding all
    // traffic.  An explicit --http-addr 0.0.0.0:8080 is sufficient to
    // acknowledge that no-transport mode is intentional (e.g. local dev).
    if cli.as4_addr.is_none() && cli.http_addr.is_none() {
        tracing::error!(
            "No ingest transport configured: neither --as4-addr nor --http-addr \
             is set.  The engine cannot receive any inbound messages.  \
             Set at least one of these options and restart."
        );
        std::process::exit(1);
    }

    // ── Background: deadline scheduler ───────────────────────────────────────
    //
    // Polls DeadlineStore::due_now every 30 s. When a deadline fires, the
    // scheduler looks up the workflow name from the Deadline, reconstructs a
    // ProcessIdentity, and dispatches a TimeoutExpired command to the correct
    // workflow. This is the regulatory enforcement mechanism: processes that
    // miss the APERAK window or settlement deadline are transitioned to
    // Rejected/Disputed state and the event is recorded in the audit log.
    //
    // The dispatch table lives in `deadline_dispatch::build_scheduler`. That
    // function also cross-checks registered workflow names against the table
    // at startup and panics if any module's workflow is uncovered.
    let event_store_for_scheduler = Arc::clone(ctx.event_store());
    let scheduler =
        deadline_dispatch::build_scheduler(&ctx, event_store_for_scheduler, cli.snapshot_interval);
    tokio::spawn(async move { scheduler.run().await });
    info!(
        "deadline scheduler started (poll_interval=30s, dispatches TimeoutExpired to all workflow families)"
    );

    // ── Background: projection checkpoint workers ────────────────────────────
    //
    // Each domain projection is fed new events from the event store on each
    // tick and persists its cursor (GlobalProjectionCheckpoint) to SlateDB.
    // On restart the worker resumes from the last persisted cursor, bounding
    // replay to O(events since last checkpoint) instead of O(all events).
    //
    // The checkpoint interval is tunable via --projection-checkpoint-interval.
    // Set to 0 to disable (not recommended in production).
    if cli.projection_checkpoint_interval > 0 {
        let interval = Duration::from_secs(cli.projection_checkpoint_interval);

        // KonfigurationProjection: tracks GPKE market-partner configuration
        // state (MSB mandates, MaLo registration).
        let worker = projection_worker::ProjectionWorker::new(
            store.clone(),
            mako_gpke::KonfigurationProjection::default(),
            Some("gpke/"),
            interval,
        );
        tokio::spawn(async move { worker.run().await });

        // SupplierChangeProjection: aggregates GPKE Lieferantenwechsel process
        // outcomes (accepted/rejected/pending) per MaLo stream.
        let worker = projection_worker::ProjectionWorker::new(
            store.clone(),
            mako_gpke::SupplierChangeProjection::default(),
            Some("gpke/"),
            interval,
        );
        tokio::spawn(async move { worker.run().await });

        info!(
            interval_secs = cli.projection_checkpoint_interval,
            "projection checkpoint workers started (KonfigurationProjection, SupplierChangeProjection)",
        );
    } else {
        tracing::warn!(
            "--projection-checkpoint-interval=0: projection checkpoints disabled; \
             every restart will trigger a full event-store replay",
        );
    }

    // ── Background: inbox purge worker ────────────────────────────────────────
    //
    // Deletes `ib/` (seen-set) and `it/` (time-index) keys older than 72 hours
    // once per day. The 72-hour window covers the maximum AS4 retry period;
    // after that, duplicate EB headers will never arrive legitimately.
    //
    // Without purging, the inbox deduplication store grows unboundedly at
    // ~1 KB per distinct MessageId.
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(24 * 3600));
        loop {
            interval.tick().await;
            let cutoff = time::OffsetDateTime::now_utc() - time::Duration::hours(72);
            match inbox_store_for_purge.purge_expired(cutoff).await {
                Ok(n) => tracing::info!(removed = n, "inbox purge complete"),
                Err(e) => tracing::error!(error = %e, "inbox purge failed"),
            }
        }
    });
    info!("inbox purge worker started (daily, 72h TTL)");

    wait_for_shutdown().await;

    info!("Mako engine shutting down");

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

/// Merge `cfg` into `cli`, skipping any field that was already set via a CLI
/// flag or environment variable.
///
/// The `matches` reference is used to distinguish "user-provided" values from
/// "still at its built-in default" values via [`clap::parser::ValueSource`].
/// Only fields whose source is `DefaultValue` are eligible for override.
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
            "UENB" | "ÜNB" => Some(Marktrolle::Uenb),
            "BIKO" => Some(Marktrolle::Biko),
            other => {
                tracing::warn!(
                    role = other,
                    "Unknown Marktrolle in --deployment-roles; valid values: \
                     NB, LF, MSB, NMSB, AMSB, BKV, UENB, BIKO"
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
        if is_default("log_level") {
            if let Some(s) = logging.level {
                cli.log_level = LogLevel::from_str(&s, true)
                    .map_err(|e| anyhow::anyhow!("config: logging.level: {e}"))?;
            }
        }
        if is_default("log_format") {
            if let Some(s) = logging.format {
                cli.log_format = LogFormat::from_str(&s, true)
                    .map_err(|e| anyhow::anyhow!("config: logging.format: {e}"))?;
            }
        }
    }

    // ── Storage ───────────────────────────────────────────────────────────────
    if let Some(storage) = cfg.storage {
        if is_default("object_store") {
            if let Some(s) = storage.backend {
                cli.object_store = ObjectStoreBackend::from_str(&s, true)
                    .map_err(|e| anyhow::anyhow!("config: storage.backend: {e}"))?;
            }
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
            if is_default("s3_prefix") {
                if let Some(p) = s3.prefix {
                    cli.s3_prefix = p;
                }
            }
        }
        if let Some(gcs) = storage.gcs {
            if cli.gcs_bucket.is_none() {
                cli.gcs_bucket = gcs.bucket;
            }
            if is_default("gcs_prefix") {
                if let Some(p) = gcs.prefix {
                    cli.gcs_prefix = p;
                }
            }
        }
        if let Some(azure) = storage.azure {
            if cli.azure_container.is_none() {
                cli.azure_container = azure.container;
            }
            if cli.azure_account.is_none() {
                cli.azure_account = azure.account;
            }
            if is_default("azure_prefix") {
                if let Some(p) = azure.prefix {
                    cli.azure_prefix = p;
                }
            }
        }
    }

    // ── HTTP API ──────────────────────────────────────────────────────────────
    if let Some(http) = cfg.http {
        if cli.http_addr.is_none() {
            cli.http_addr = http.addr;
        }
        if cli.http_api_token.is_none() {
            cli.http_api_token = http.api_token.map(SecretString::new);
        }
        if is_default("http_max_body_bytes") {
            if let Some(n) = http.max_body_bytes {
                cli.http_max_body_bytes = n;
            }
        }
    }

    // ── API-Webdienste ────────────────────────────────────────────────────────
    if let Some(wd) = cfg.webdienste {
        if cli.api_webdienste_addr.is_none() {
            cli.api_webdienste_addr = wd.addr;
        }
    }

    // ── Engine ────────────────────────────────────────────────────────────────
    if let Some(engine) = cfg.engine {
        if is_default("tenant_id") {
            if let Some(id) = engine.tenant_id {
                cli.tenant_id = id;
            }
        }
        if is_default("shutdown_timeout_secs") {
            if let Some(secs) = engine.shutdown_timeout_secs {
                cli.shutdown_timeout_secs = secs;
            }
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
        // Inline PEM takes precedence over a file reference.
        if cli.as4_signing_key_pem.is_none() {
            if let Some(pem) = as4.signing_key_pem {
                cli.as4_signing_key_pem = Some(SecretString::new(pem));
            } else if let Some(ref path) = as4.signing_key_pem_file {
                cli.as4_signing_key_pem = Some(SecretString::new(
                    std::fs::read_to_string(path)
                        .with_context(|| format!("reading AS4 signing key: {}", path.display()))?,
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
        if cli.as4_partner.is_empty() {
            if let Some(partners) = as4.partners {
                cli.as4_partner = partners;
            }
        }
    }

    // ── ERP ───────────────────────────────────────────────────────────────────
    if let Some(erp) = cfg.erp {
        if cli.erp_webhook_url.is_none() {
            cli.erp_webhook_url = erp.webhook_url;
        }
        if cli.erp_webhook_secret.is_none() {
            cli.erp_webhook_secret = erp.webhook_secret.map(SecretString::new);
        }
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
                info!(endpoint, allow_http, "using custom S3-compatible endpoint");
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

fn init_tracing(cli: &Cli) {
    use tracing_subscriber::{EnvFilter, fmt};

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
}

// ── Graceful shutdown ─────────────────────────────────────────────────────────

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

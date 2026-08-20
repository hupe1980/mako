//! `edmd` configuration — loaded from `edmd.toml` + `env:` substitution.
//!
//! # Minimal `edmd.toml`
//!
//! ```toml
//! [http]
//! addr = "0.0.0.0:8380"
//!
//! [database]
//! url = "env:DATABASE_URL"
//!
//! [identity]
//! tenant = "9900357000004"
//!
//! [marktd]
//! url     = "http://marktd:8180"
//! api_key = "env:EDMD_MARKTD_API_KEY"
//!
//! [webhook]
//! inbound_secret     = "env:EDMD_INBOUND_SECRET"
//! erp_webhook_url    = "http://erp:9000/hooks/edmd"
//! erp_webhook_secret = "env:EDMD_ERP_WEBHOOK_SECRET"  # signs outbound events
//!
//! [subscription]
//! webhook_url   = "http://edmd:8380/webhook"
//! subscriber_id = "edmd"
//!
//! # Cold tier. The `storage_uri` scheme picks the warehouse backend (file /
//! # memory / s3 / gs / abfss); meterstore owns tiering through its watermark.
//! # [archive]
//! # enabled     = true
//! # storage_uri = "s3://my-bucket/edmd/warehouse"
//! # region      = "eu-central-1"
//! # # access_key_id / secret_access_key optional — omit to use an instance role.
//!
//! # [oidc]
//! # issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
//! # audience = "api://mako-edmd"
//! # [otel]
//! # endpoint = "http://otel-collector:4317"
//! ```

use serde::Deserialize;

// ── meterstore / cold-tier config ───────────────────────────────────────────

/// In-process meterstore configuration.
///
/// meterstore runs **in-process** inside edmd and reads no config of its own —
/// every knob it needs is supplied here, through the `[archive]` section of
/// `edmd.toml`. meterstore owns tiering and retention through its watermark (there
/// is no edmd archival worker); this section says *where* the cold tier lives and
/// *when/how* intervals settle into it.
///
/// ```toml
/// [archive]
/// enabled             = true
/// storage_uri         = "s3://my-bucket/edmd/warehouse"
/// region              = "eu-central-1"
/// settlement_lag_days = 7
/// # access_key_id / secret_access_key optional — omit to use an instance role.
/// ```
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveConfig {
    /// Enable the cold tier. When `false`, meterstore runs hot-only against an
    /// in-memory warehouse. Default: `false`.
    #[serde(default)]
    pub enabled: bool,
    /// Warehouse root URI meterstore writes Iceberg data files under. The scheme
    /// selects the object-store backend:
    ///
    /// - `file://` — local filesystem
    /// - `memory://` — in-process (dev / hot-only)
    /// - `s3://` (and S3-compatible `minio://` / `r2://`) — credentials below or
    ///   the instance-role chain
    /// - `gs://` — Google Cloud Storage (ADC credentials)
    /// - `abfss://` — Azure Data Lake (managed identity)
    #[serde(default)]
    pub storage_uri: String,
    /// Days after an interval occurs before it is **settled** — eligible to move
    /// from the mutable hot PostgreSQL tier into the append-only cold Iceberg
    /// tier. Below this age a reading can still be corrected in place. Default: 7.
    #[serde(default = "archive_default_settlement_lag_days")]
    pub settlement_lag_days: u32,
    /// Cold-tier partition granularity, in days. Default: 1.
    #[serde(default = "archive_default_step_days")]
    pub partition_step_days: u32,
    /// How far each archival sweep advances the tiering watermark, in days.
    /// Default: 1.
    #[serde(default = "archive_default_step_days")]
    pub archival_step_days: u32,
    /// Target Parquet file size in the cold tier, in MiB. Default: 512.
    #[serde(default = "archive_default_cold_file_mib")]
    pub cold_file_target_mib: u32,
    /// How often meterstore's in-process maintenance loop runs a tiering cycle
    /// (archives settled windows hot → cold, checks the tier invariant), in
    /// seconds. Without it settled intervals never leave the hot tier. Default:
    /// 3600 (hourly).
    #[serde(default = "archive_default_maintenance_interval_secs")]
    pub maintenance_interval_secs: u32,
    /// Object-store region for an `s3://` warehouse (or `AWS_REGION`).
    #[serde(default)]
    pub region: Option<String>,
    /// S3-compatible endpoint override (MinIO, Ceph, R2, LocalStack). Its presence
    /// switches on path-style addressing.
    #[serde(default)]
    pub endpoint_url: Option<String>,
    /// S3 access key ID. Prefer `"env:AWS_ACCESS_KEY_ID"`; **omit entirely** to use
    /// the instance-role / IRSA credential chain — the recommended production path.
    #[serde(default)]
    pub access_key_id: Option<String>,
    /// S3 secret access key. Prefer `"env:AWS_SECRET_ACCESS_KEY"`; omit to use the
    /// credential chain. GCS (`gs://`) and Azure (`abfss://`) use their platform
    /// chains (ADC / managed identity) and need no keys here.
    #[serde(default)]
    pub secret_access_key: Option<String>,
}

fn archive_default_settlement_lag_days() -> u32 {
    7
}
fn archive_default_step_days() -> u32 {
    1
}
fn archive_default_cold_file_mib() -> u32 {
    512
}
fn archive_default_maintenance_interval_secs() -> u32 {
    3_600
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            storage_uri: String::new(),
            settlement_lag_days: archive_default_settlement_lag_days(),
            partition_step_days: archive_default_step_days(),
            archival_step_days: archive_default_step_days(),
            cold_file_target_mib: archive_default_cold_file_mib(),
            maintenance_interval_secs: archive_default_maintenance_interval_secs(),
            region: None,
            endpoint_url: None,
            access_key_id: None,
            secret_access_key: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub http: HttpConfig,
    pub database: DatabaseConfig,
    pub identity: IdentityConfig,
    pub marktd: MarktdConfig,
    #[serde(default)]
    pub webhook: WebhookConfig,
    #[serde(default)]
    pub subscription: SubscriptionConfig,
    #[serde(default)]
    pub oidc: Option<OidcConfig>,
    #[serde(default)]
    pub otel: OtelConfig,
    /// MCP server authentication. Supports OIDC + API-key fallback, or dev mode.
    /// See `[mcp]` in TOML — e.g. `api_key = "env:EDMD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
    /// Cold-tier warehouse for meterstore's Iceberg history. Disabled by default.
    #[serde(default)]
    pub archive: ArchiveConfig,
    /// Request rate limits, global and per tenant. See `[rate_limit]` in TOML.
    #[serde(default)]
    pub rate_limit: mako_service::RateLimitConfig,
    /// Kafka ingest consumer. Disabled unless the section is present with
    /// `enabled = true`. See [`KafkaIngestConfig`].
    #[serde(default)]
    pub kafka_ingest: Option<KafkaIngestConfig>,
    /// § 60 Abs. 2 MsbG confirmation loop for estimated/substituted readings.
    #[serde(default)]
    pub confirmation: ConfirmationConfig,
    /// §14a SMGW/CLS compliance sweep thresholds. See [`SmgwConfig`].
    #[serde(default)]
    pub smgw: SmgwConfig,
    /// Delivery surveillance — which measuring points have gone quiet. See
    /// [`SurveillanceConfig`].
    #[serde(default)]
    pub surveillance: SurveillanceConfig,
    /// Start without token verification.
    ///
    /// With `[oidc]` absent the verifier admits every request as `dev-admin`
    /// holding every market role, which satisfies every Cedar policy — including
    /// GDPR erasure and the SQL query endpoint. That posture must be asked for
    /// by name rather than reached by leaving a section out.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,
}

/// `[kafka_ingest]` — high-throughput meter-reading intake from a Kafka topic.
///
/// ```toml
/// [kafka_ingest]
/// enabled           = true
/// bootstrap_servers = "kafka-1:9092,kafka-2:9092"
/// topic             = "edmd.meter-reads"
/// group_id          = "edmd-ingest"
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KafkaIngestConfig {
    /// Enable the consumer. Default: `false`.
    #[serde(default)]
    pub enabled: bool,
    /// Comma-separated bootstrap servers.
    pub bootstrap_servers: String,
    /// Topic carrying the JSON batch documents.
    #[serde(default = "kafka_default_topic")]
    pub topic: String,
    /// Consumer group id.
    #[serde(default = "kafka_default_group")]
    pub group_id: String,
    /// Poll timeout in milliseconds.
    #[serde(default = "kafka_default_poll_ms")]
    pub poll_ms: u64,
    /// Optional per-message HMAC-SHA256 authentication.
    ///
    /// When set (supports `"env:VAR"`), every record must carry an
    /// Standard Webhooks record headers (`v1,<base64>` over `{id}.{ts}.{value}`,
    /// same scheme as the platform's webhook signing); records with a
    /// missing or wrong signature are skipped and counted. When unset, the
    /// topic itself is the trust boundary — restrict topic ACLs to the
    /// head-end system.
    #[serde(default)]
    pub message_hmac_secret: Option<String>,
}

fn kafka_default_topic() -> String {
    "edmd.meter-reads".to_owned()
}
fn kafka_default_group() -> String {
    "edmd-ingest".to_owned()
}
fn kafka_default_poll_ms() -> u64 {
    500
}

/// `edmd` is a [`mako_service::Daemon`], so the runner owns the lifecycle:
/// tracing, the tuned pool with `application_name = edmd`, migrations, a real
/// DB-ping readiness probe, the `/metrics` and `/health/*` routes, and — the
/// one edmd previously lacked — graceful shutdown on **`SIGTERM`** as well as
/// `SIGINT`. A hand-rolled `main` that only watched Ctrl-C never shut down
/// cleanly under Kubernetes or systemd, which send `SIGTERM`: in-flight ingest
/// requests were cut off at the end of the termination grace period.
impl mako_service::ServiceConfig for Config {
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        Some(&self.database)
    }
    fn bind_addr(&self) -> String {
        self.http.addr.clone()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    #[serde(default = "default_http_addr")]
    pub addr: String,
}

fn default_http_addr() -> String {
    "0.0.0.0:8380".to_owned()
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            addr: default_http_addr(),
        }
    }
}

/// PostgreSQL config — shared struct from `mako-service`.
pub use mako_service::config::DatabaseConfig;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityConfig {
    /// Tenant identifier written to every DB row and used in Cedar resource checks.
    pub tenant: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarktdConfig {
    /// `marktd` base URL.  Example: `http://marktd:8180`
    pub url: String,
    /// Bearer token / API key.  Use `"env:EDMD_MARKTD_API_KEY"`.
    pub api_key: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    /// HMAC-SHA256 secret for verifying inbound webhooks from `marktd`.
    /// Use `"env:EDMD_INBOUND_SECRET"`.
    pub inbound_secret: Option<String>,
    /// ERP webhook URL for outbound CloudEvents (`de.messwert.reading.direct.stored`,
    /// `de.messwert.reading.quality.warning`). Omit to disable outbound notifications.
    pub erp_webhook_url: Option<String>,
    /// Shared secret for signing **outbound** CloudEvents with an
    /// Standard Webhooks headers over the body, so the ERP receiver
    /// can authenticate every edmd-originated event (the counterpart to
    /// `inbound_secret`). Omit to send unsigned, trusting the transport.
    pub erp_webhook_secret: Option<String>,
}

/// `[confirmation]` — § 60 Abs. 2 MsbG estimated-reading confirmation loop.
///
/// Every stored ESTIMATED/SUBSTITUTED interval opens an obligation to
/// replace it with a plausibilised real value. The daily worker marks
/// obligations older than `deadline_weeks` as UEBERFAELLIG and emits
/// `de.messwert.reading.confirmation.overdue`. No statute fixes the deadline —
/// the 8-week default aligns with the MaBiS Bilanzkreisabrechnung
/// correction window.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmationConfig {
    /// Whether the overdue-escalation worker runs. Default: true — the
    /// tracking table is always populated; only the escalation is optional.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Weeks until an unreplaced estimate counts as overdue. Default: 8.
    #[serde(default = "default_confirmation_deadline_weeks")]
    pub deadline_weeks: i64,
}

fn default_true() -> bool {
    true
}
fn default_confirmation_deadline_weeks() -> i64 {
    8
}

impl Default for ConfirmationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            deadline_weeks: default_confirmation_deadline_weeks(),
        }
    }
}

/// `[smgw]` — §14a SMGW/CLS compliance sweep.
///
/// These were function parameters that every call site passed `30` and `2` to,
/// while the docs called them configurable. Now they are.
///
/// ```toml
/// [smgw]
/// cert_warning_days          = 30
/// comm_fault_threshold_hours = 2
/// sweep_interval_secs        = 86400
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmgwConfig {
    /// Whether the daily compliance and certificate-expiry sweeps run.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// How far ahead of `valid_to` a certificate is reported as expiring.
    ///
    /// Not a statutory number: BSI TR-03109-4 binds certificate runtimes and the
    /// Root-CP fixes the renewal lead time. 30 days is a common operational
    /// choice, not a deadline the TR states.
    #[serde(default = "smgw_default_cert_warning_days")]
    pub cert_warning_days: i32,
    /// Hours of silence after which a gateway counts as a communication fault,
    /// which is what leaves § 60 Abs. 2 MsbG Ersatzwerte owing.
    #[serde(default = "smgw_default_comm_fault_hours")]
    pub comm_fault_threshold_hours: i64,
    /// Seconds between sweeps. Default: daily.
    #[serde(default = "default_daily_secs")]
    pub sweep_interval_secs: u64,
}

fn smgw_default_cert_warning_days() -> i32 {
    30
}
fn smgw_default_comm_fault_hours() -> i64 {
    2
}
fn default_daily_secs() -> u64 {
    86_400
}

impl Default for SmgwConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cert_warning_days: smgw_default_cert_warning_days(),
            comm_fault_threshold_hours: smgw_default_comm_fault_hours(),
            sweep_interval_secs: default_daily_secs(),
        }
    }
}

/// `[surveillance]` — delivery surveillance for measuring points that go quiet.
///
/// The V-rules can only judge data that arrived. A measuring point that stops
/// delivering produces no ingest, so no validation runs and no quality warning
/// fires: the failure is invisible until a settlement run comes up short. This
/// worker is the other half — it looks for the **absence** of data.
///
/// ```toml
/// [surveillance]
/// enabled            = true
/// silent_after_hours = 36     # RLM/iMSys: a day plus a retry window
/// min_coverage_pct   = 95.0
/// coverage_window_days = 7
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurveillanceConfig {
    /// Whether the sweep runs. Default: true — an MSB that does not notice a
    /// silent meter cannot meet § 60 Abs. 2 MsbG in time.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Hours since a measuring point's newest interval **ended** after which it
    /// counts as overdue.
    ///
    /// Default 36: a daily delivery cadence plus a retry window, so an ordinary
    /// late batch does not raise an alarm but a missed day does.
    #[serde(default = "surveillance_default_silent_hours")]
    pub silent_after_hours: i64,
    /// Coverage below which a point is reported even though it is still
    /// delivering — a meter sending one interval an hour instead of four is not
    /// silent, but it is not billable either.
    #[serde(default = "surveillance_default_min_coverage")]
    pub min_coverage_pct: f64,
    /// The window coverage is measured over. Default: 7 days.
    #[serde(default = "surveillance_default_coverage_days")]
    pub coverage_window_days: i64,
    /// Seconds between sweeps. Default: hourly — a settlement deadline is
    /// measured in working days, so an hour of latency costs nothing and a
    /// daily sweep can miss most of one.
    #[serde(default = "surveillance_default_interval_secs")]
    pub sweep_interval_secs: u64,
    /// Maximum measuring points reported per sweep, so one broken head-end
    /// cannot emit a hundred thousand CloudEvents in a burst. The count of
    /// suppressed points is logged and carried on the summary.
    #[serde(default = "surveillance_default_max_events")]
    pub max_events_per_sweep: usize,
}

fn surveillance_default_silent_hours() -> i64 {
    36
}
fn surveillance_default_min_coverage() -> f64 {
    95.0
}
fn surveillance_default_coverage_days() -> i64 {
    7
}
fn surveillance_default_interval_secs() -> u64 {
    3_600
}
fn surveillance_default_max_events() -> usize {
    500
}

impl Default for SurveillanceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            silent_after_hours: surveillance_default_silent_hours(),
            min_coverage_pct: surveillance_default_min_coverage(),
            coverage_window_days: surveillance_default_coverage_days(),
            sweep_interval_secs: surveillance_default_interval_secs(),
            max_events_per_sweep: surveillance_default_max_events(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionConfig {
    /// URL that `marktd` will POST events to.
    pub webhook_url: String,
    #[serde(default = "default_subscriber_id")]
    pub subscriber_id: String,
    /// Comma-separated CloudEvent types.
    #[serde(default = "default_event_types")]
    pub event_types: Vec<String>,
}

fn default_subscriber_id() -> String {
    "edmd".to_owned()
}
fn default_event_types() -> Vec<String> {
    // Exactly the two types `handler.rs` branches on. Subscribing to more
    // would register a marktd fan-out edge whose deliveries edmd silently
    // discards.
    vec![
        mako_events::mako::PROCESS_INITIATED.to_owned(),
        mako_events::mako::PROCESS_COMPLETED.to_owned(),
    ]
}

impl Default for SubscriptionConfig {
    fn default() -> Self {
        Self {
            webhook_url: "http://edmd:8380/webhook".to_owned(),
            subscriber_id: default_subscriber_id(),
            event_types: default_event_types(),
        }
    }
}

/// OIDC configuration — re-exported from `mako-service` (shared across all daemons).
pub use mako_service::oidc::OidcConfig;

/// OpenTelemetry config — shared struct from `mako-service`.
pub use mako_service::telemetry::OtelConfig;

pub fn resolve_env(value: &str) -> anyhow::Result<String> {
    if let Some(var) = value.strip_prefix("env:") {
        std::env::var(var).map_err(|_| {
            anyhow::anyhow!("environment variable {var:?} is not set (referenced in edmd.toml)")
        })
    } else {
        Ok(value.to_owned())
    }
}

pub fn resolve_env_secret(value: &str) -> anyhow::Result<secrecy::SecretString> {
    resolve_env(value).map(secrecy::SecretString::from)
}

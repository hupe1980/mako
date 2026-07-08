//! `processd` configuration — CLI + environment.
//!
//! All secrets can be provided via `env:VAR_NAME` substitution or directly
//! on the command line.

use clap::Parser;
use secrecy::SecretString;

/// Process decision engine for German energy market automation.
///
/// Consumes `de.mako.process.initiated` CloudEvents from `marktd` and applies
/// role-specific policy to make automated decisions within regulatory deadlines.
#[derive(Debug, Parser)]
#[command(
    name = "processd",
    about = "Process decision engine for German energy market"
)]
pub struct Config {
    // ── HTTP ───────────────────────────────────────────────────────────────
    /// Listen address for the HTTP API.
    #[arg(long, default_value = "0.0.0.0:8580", env = "PROCESSD_LISTEN")]
    pub listen: String,

    // ── Database ───────────────────────────────────────────────────────────
    /// PostgreSQL connection string.
    #[arg(long, env = "DATABASE_URL")]
    pub database_url: String,

    /// PostgreSQL connection pool size.
    #[arg(long, default_value = "10", env = "DB_POOL_SIZE")]
    pub db_pool_size: u32,

    // ── Inbound webhook ────────────────────────────────────────────────────
    /// HMAC-SHA256 secret used to verify inbound webhooks from `marktd`.
    /// Must match `marktd`'s `webhook_secret` for the `processd` subscription.
    ///
    /// Leave unset to disable signature verification (dev only).
    #[arg(long, env = "INBOUND_WEBHOOK_SECRET")]
    pub inbound_secret: Option<SecretString>,

    // ── makod connection ───────────────────────────────────────────────────
    /// `makod` base URL (cluster-internal, e.g. `http://makod:8080`).
    #[arg(long, env = "MAKOD_URL")]
    pub makod_url: String,

    /// `makod` API key (Bearer token).
    #[arg(long, env = "MAKOD_API_KEY")]
    pub makod_api_key: SecretString,

    // ── marktd connection ──────────────────────────────────────────────────
    /// `marktd` base URL (cluster-internal, e.g. `http://marktd:8180`).
    #[arg(long, env = "MARKTD_URL")]
    pub marktd_url: String,

    /// `marktd` API key (Bearer token for machine-to-machine auth).
    #[arg(long, env = "MARKTD_API_KEY")]
    pub marktd_api_key: SecretString,

    // ── Identity ───────────────────────────────────────────────────────────
    /// Operator primary GLN (must match `makod.toml` `[[party]] primary = true`).
    ///
    /// Used to derive `initiator_is_affiliate` for §20 EnWG parity reporting:
    /// when the initiating LF GLN equals `own_mp_id`, the decision is flagged as
    /// affiliate-initiated.
    #[arg(long, env = "OWN_MP_ID")]
    pub own_mp_id: String,

    /// Operator tenant identifier (written to every DB row).
    #[arg(long, env = "TENANT", default_value = "")]
    pub tenant: String,

    // ── OIDC ───────────────────────────────────────────────────────────────
    /// OIDC issuer URL (e.g. `https://login.microsoftonline.com/{tenant}/v2.0`).
    /// Omit to disable authentication (dev mode only).
    #[arg(long, env = "OIDC_ISSUER")]
    pub oidc_issuer: Option<String>,

    /// OIDC audience.
    #[arg(long, env = "OIDC_AUDIENCE")]
    pub oidc_audience: Option<String>,

    /// JWKS background refresh interval in seconds.
    #[arg(long, default_value = "3600", env = "OIDC_JWKS_REFRESH_SECS")]
    pub oidc_jwks_refresh_secs: u64,

    // ── NB module ─────────────────────────────────────────────────────────
    /// `NB` — Automatically accept validated Anmeldungen (dispatches `bestaetigen`).
    ///
    /// When `false` (default), `processd` runs all 6 netz-checker checks and
    /// writes `anmeldung_decisions` but does NOT dispatch `bestaetigen`.
    /// Activate only after verifying grid record and partner coverage.
    #[arg(long, env = "NB_AUTO_ACCEPT", default_value = "false")]
    pub nb_auto_accept: bool,

    // ── LF module ─────────────────────────────────────────────────────────
    /// `LF` — Automatically respond to E_0624 queries (dispatches `einwilligung`/`ablehnen`).
    ///
    /// When `false` (default), all E_0624 events are routed to `approval_queue`.
    #[arg(long, env = "LF_AUTO_RESPOND", default_value = "true")]
    pub lf_auto_respond: bool,

    /// `LF` — Approval queue entry TTL in seconds (default: 2700 = 45 min).
    ///
    /// Entries older than this are auto-expired (status = `Expired`).
    #[arg(long, default_value = "2700", env = "LF_QUEUE_TTL_SECS")]
    pub lf_queue_ttl_secs: u64,

    // ── Self-registration with marktd ──────────────────────────────────────
    /// Webhook URL that `marktd` should POST `de.mako.process.initiated` events to.
    ///
    /// When set, `processd` calls `PUT {marktd_url}/api/v1/subscriptions/{subscriber_id}`
    /// on startup and self-registers as a subscriber. This makes the subscription
    /// topology part of the deployment config (env var / Helm values.yaml) rather
    /// than an imperative bootstrap script.  Retries for up to 30 s to tolerate
    /// marktd startup ordering.
    ///
    /// Typical value: `http://<processd-service-dns>:8580/webhook`
    /// Helm values.yaml: `processd.selfRegister.webhookUrl`
    #[arg(long, env = "PROCESSD_SELF_REGISTER_WEBHOOK_URL")]
    pub self_register_webhook_url: Option<String>,

    /// Subscriber ID used when self-registering with `marktd`.
    ///
    /// Must be unique per deployment (e.g. `processd-prod`, `processd-nb-only`).
    /// Used as the `PUT /api/v1/subscriptions/:id` path segment — idempotent upsert.
    #[arg(long, env = "PROCESSD_SUBSCRIBER_ID", default_value = "processd")]
    pub subscriber_id: String,

    /// Comma-separated CloudEvent types to subscribe to.
    ///
    /// Default covers all process lifecycle events that processd needs.
    #[arg(
        long,
        env = "PROCESSD_SUBSCRIBER_EVENT_TYPES",
        default_value = "de.mako.process.initiated"
    )]
    pub subscriber_event_types: String,

    // ── Observability ─────────────────────────────────────────────────────
    /// Log level (RUST_LOG syntax, e.g. `info`, `debug`, `processd=trace`).
    #[arg(long, default_value = "info", env = "RUST_LOG")]
    pub log_level: String,

    /// OpenTelemetry OTLP endpoint (optional).
    #[arg(long, env = "OTEL_EXPORTER_OTLP_ENDPOINT")]
    pub otel_endpoint: Option<String>,
}

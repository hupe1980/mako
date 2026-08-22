//! `processd` configuration.
//!
//! Loaded by [`mako_service::load_config`]: `processd.toml` first (path from
//! `PROCESSD_CONFIG`, default `./processd.toml`), then `PROCESSD_*` environment
//! variables with `__` as the section separator, then any `*_FILE` variable read
//! from a file. The file is optional — a container can be configured entirely
//! from the environment:
//!
//! ```text
//! PROCESSD_DATABASE__URL=postgres://processd:secret@postgres/processd
//! PROCESSD_IDENTITY__OWN_MP_ID=9900000000002
//! PROCESSD_MAKOD__API_KEY_FILE=/run/secrets/makod-api-key
//! ```
//!
//! Individual values may additionally use the `"env:VAR_NAME"` indirection;
//! [`resolve_env`] / [`resolve_env_secret`] resolve those, and every secret
//! field is resolved in `main` before use.
//!
//! # Minimal `processd.toml`
//!
//! ```toml
//! [http]
//! addr = "0.0.0.0:8580"
//!
//! [database]
//! url = "env:DATABASE_URL"
//!
//! [identity]
//! own_mp_id = "9900357000004"
//!
//! [makod]
//! url     = "http://makod:8080"
//! api_key = "env:MAKOD_API_KEY"
//!
//! [marktd]
//! url     = "http://marktd:8180"
//! api_key = "env:MARKTD_API_KEY"
//!
//! [webhook]
//! inbound_secret = "env:INBOUND_WEBHOOK_SECRET"
//!
//! [subscription]
//! webhook_url   = "http://processd:8580/webhook"
//! subscriber_id = "processd"
//!
//! [nb]
//! auto_accept = false   # true: dispatch bestaetigen automatically on Accept
//!
//! [lf]
//! auto_respond = true
//!
//! [msb]
//! auto_accept       = false   # true: dispatch the MSB-Wechsel Bestätigung
//! auto_preisanfrage = true    # false: the REQOTE goes to the approval queue
//!
//! # [oidc]                # omit to disable auth (dev mode only)
//! # issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
//! # audience = "api://mako-processd"
//! #
//! # [otel]                # omit to disable tracing
//! # endpoint = "http://otel-collector:4317"
//! ```

use serde::Deserialize;

// ── Top-level ─────────────────────────────────────────────────────────────────

/// Full `processd.toml` configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub http: HttpConfig,
    pub database: DatabaseConfig,
    pub identity: IdentityConfig,
    pub makod: MakodConfig,
    pub marktd: MarktdConfig,
    /// `vertragd`, when this deployment runs the contract layer.
    ///
    /// Optional because a pure NB deployment has no contracts of its own. Both
    /// the LF and the MSB module read it — the split is by *kind of fact*, not
    /// by role: supply and market state come from `marktd`, contract state from
    /// `vertragd`.
    #[serde(default)]
    pub vertragd: Option<VertragdConfig>,
    #[serde(default)]
    pub webhook: WebhookConfig,
    #[serde(default)]
    pub subscription: SubscriptionConfig,
    #[serde(default)]
    pub nb: NbConfig,
    #[serde(default)]
    pub lf: LfConfig,
    #[serde(default)]
    pub msb: MsbConfig,
    #[serde(default)]
    pub eog: EogConfig,
    /// OIDC configuration.  When omitted, authentication is **disabled** and
    /// all API requests are accepted with synthetic dev-admin claims.
    /// **Never omit this in production.**
    #[serde(default)]
    pub oidc: Option<OidcConfig>,
    #[serde(default)]
    pub otel: OtelConfig,
    /// MCP server authentication. Supports OIDC + API-key fallback, or dev mode.
    /// See `[mcp]` in TOML — e.g. `api_key = "env:PROCESSD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
}

impl mako_service::ServiceConfig for Config {
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        Some(&self.database)
    }
    fn bind_addr(&self) -> String {
        self.http.addr.clone()
    }
}

// ── HTTP ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    #[serde(default = "default_http_addr")]
    pub addr: String,
}

fn default_http_addr() -> String {
    "0.0.0.0:8580".to_owned()
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            addr: default_http_addr(),
        }
    }
}

// ── Database ──────────────────────────────────────────────────────────────────

/// PostgreSQL config — shared struct from `mako-service`.
pub use mako_service::config::DatabaseConfig;

// ── Identity ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityConfig {
    /// Operator primary MP-ID (BDEW-Codenummer starting with `99`, or DVGW `98`).
    ///
    /// Must match `makod.toml` `[[party]] primary = true`.
    /// Used for `initiator_is_affiliate` §20 EnWG parity reporting.
    pub own_mp_id: String,
    /// Tenant identifier written to every DB row.  Defaults to `own_mp_id`.
    #[serde(default)]
    pub tenant: String,
}

// ── makod connection ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakodConfig {
    /// `makod` base URL.  Example: `http://makod:8080`
    pub url: String,
    /// Bearer token / API key.  Use `"env:MAKOD_API_KEY"`.
    pub api_key: String,
}

// ── marktd connection ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarktdConfig {
    /// `marktd` base URL.  Example: `http://marktd:8180`
    pub url: String,
    /// Bearer token / API key.  Use `"env:MARKTD_API_KEY"`.
    pub api_key: String,
}

/// `vertragd` connection details.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VertragdConfig {
    /// `vertragd` base URL. Example: `http://vertragd:9780`
    pub url: String,
    /// Bearer token / API key. Use `"env:VERTRAGD_API_KEY"`.
    pub api_key: String,
}

// ── Inbound webhook ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    /// HMAC-SHA256 secret for verifying inbound webhooks from `marktd`.
    /// Must match `marktd`'s subscription `webhook_secret`.
    /// Leave unset to disable signature verification (dev only).
    /// Use `"env:INBOUND_WEBHOOK_SECRET"`.
    pub inbound_secret: Option<String>,
}

// ── Self-registration with marktd ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionConfig {
    /// URL that `marktd` will POST `de.mako.process.initiated` events to.
    ///
    /// When set, `processd` calls `PUT {marktd.url}/api/v1/subscriptions/{subscriber_id}`
    /// on startup and self-registers as a subscriber.  Retries for up to 30 s to
    /// tolerate `marktd` startup ordering.
    ///
    /// Typical value: `http://<processd-service-dns>:8580/webhook`
    pub webhook_url: Option<String>,
    /// Unique subscription ID for this deployment.  Used as the path segment in
    /// `PUT /api/v1/subscriptions/:id` — idempotent upsert.
    #[serde(default = "default_subscriber_id")]
    pub subscriber_id: String,
    /// Comma-separated CloudEvent types to subscribe to.
    #[serde(default = "default_event_types")]
    pub event_types: String,
}

fn default_subscriber_id() -> String {
    "processd".to_owned()
}
fn default_event_types() -> String {
    // PROCESS_INITIATED drives the NB/LF/MSB STP modules; the versorgung
    // events drive the EoG gap-closure automation (§38 EnWG) — CHANGED closes
    // a case once regular supply resumes.
    format!(
        "{},{},{},{}",
        mako_events::mako::PROCESS_INITIATED,
        mako_events::markt::VERSORGUNG_GAP_DETECTED,
        mako_events::markt::VERSORGUNG_EOG_BEGONNEN,
        mako_events::markt::VERSORGUNG_CHANGED,
    )
}

impl Default for SubscriptionConfig {
    fn default() -> Self {
        Self {
            webhook_url: None,
            subscriber_id: default_subscriber_id(),
            event_types: default_event_types(),
        }
    }
}

// ── NB module ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NbConfig {
    /// When `true`, `processd` dispatches `bestaetigen` automatically on `Accept`.
    ///
    /// When `false` (default), decisions are written to `anmeldung_decisions` but
    /// `bestaetigen` is NOT dispatched — operator must approve via
    /// `PUT /api/v1/queue/{id}/approve`.  Activate only after verifying grid
    /// record and partner coverage (STP target ≥ 95 %).
    #[serde(default)]
    pub auto_accept: bool,

    /// Gas Bearbeitungsfrist (Werktage) added to the 6-week retroactive
    /// Anmeldung window (AWH GeLi Gas 2.0 Kap. 2.2). The AWH quantifies it only
    /// for the Ersatz-/Grundversorgung (3 WT); the same value is the default
    /// here. Override when the operator's AWH reading differs. Defaults to `3`.
    #[serde(default = "default_gas_bearbeitungsfrist_wt")]
    pub gas_bearbeitungsfrist_wt: u32,

    /// Base URL of the `einsd` EEG-/KWKG-Register, for the one fact an
    /// Anmeldung erzeugender Marktlokation needs and the UTILMD cannot carry:
    /// the **bestehende** Veräußerungsform (`E_0622` Prüfschritt 400 / 600).
    ///
    /// Without it every 55077 escalates — the § 20 EnWG-safe outcome, since
    /// `E_0622` chooses between six published Vorlauffristen and none of them
    /// is a defensible default.
    #[serde(default)]
    pub einsd_url: Option<String>,

    /// Bearer token for [`Self::einsd_url`].
    #[serde(default)]
    pub einsd_api_key: Option<String>,
}

fn default_gas_bearbeitungsfrist_wt() -> u32 {
    mako_pruefung::nb::anmeldung::GAS_BEARBEITUNGSFRIST_WT_DEFAULT
}

impl Default for NbConfig {
    fn default() -> Self {
        Self {
            auto_accept: false,
            gas_bearbeitungsfrist_wt: default_gas_bearbeitungsfrist_wt(),
            einsd_url: None,
            einsd_api_key: None,
        }
    }
}

// ── EoG module (§36/§38 EnWG gap closure) ─────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EogConfig {
    /// When `true`, a detected supply gap dispatches `gpke.eog.anmelden`
    /// (UTILMD 55013) automatically. Requires a Grundversorger Feststellung
    /// in marktd (`PUT /api/v1/grundversorger/{nb_mp_id}`).
    #[serde(default)]
    pub auto_activate: bool,
    /// SG4 STS Transaktionsgrund for automatic Anmeldungen (default `ZT6`).
    #[serde(default = "default_eog_transaktionsgrund")]
    pub default_transaktionsgrund: String,
    /// Days before the §38 Abs. 4 3-month maximum at which the warning fires.
    #[serde(default = "default_eog_warn_days")]
    pub warn_days_before_expiry: u32,
    /// Webhook for `de.markt.versorgung.ersatz-auslaufend` CloudEvents.
    #[serde(default)]
    pub notify_webhook_url: Option<String>,
    /// HMAC-SHA256 secret for signing the outbound `notify_webhook_url`
    /// CloudEvents (Standard Webhooks). Supports `"env:VAR_NAME"`,
    /// which `main` resolves before use — an unresolved reference would sign
    /// with the ASCII bytes of the literal `env:VAR_NAME` and every receiver
    /// verifying the signature would reject the event.
    ///
    /// Leave unset only in dev.
    #[serde(default)]
    pub notify_webhook_secret: Option<String>,
}

fn default_eog_transaktionsgrund() -> String {
    "ZT6".to_owned()
}
fn default_eog_warn_days() -> u32 {
    14
}

impl Default for EogConfig {
    fn default() -> Self {
        Self {
            auto_activate: false,
            default_transaktionsgrund: default_eog_transaktionsgrund(),
            warn_days_before_expiry: default_eog_warn_days(),
            notify_webhook_url: None,
            notify_webhook_secret: None,
        }
    }
}

// ── LF module ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LfConfig {
    /// When `true` (default), `processd` dispatches the resolved answer itself.
    ///
    /// `false` does **not** mean "nobody answers": the decision still runs and
    /// its outcome is queued for an operator with the Antwortfrist attached.
    #[serde(default = "default_lf_auto_respond")]
    pub auto_respond: bool,
}

fn default_lf_auto_respond() -> bool {
    true
}

impl Default for LfConfig {
    fn default() -> Self {
        Self {
            auto_respond: default_lf_auto_respond(),
        }
    }
}

// ── MSB module ─────────────────────────────────────────────────────────────────

/// MSB process automation configuration.
///
/// When `auto_preisanfrage = true` (default), `processd` automatically dispatches
/// a QUOTES response when a REQOTE Preisanfrage (PIDs 35001/35002/35004/35005)
/// arrives,
/// sourcing prices from the current `PreisblattMessung` in `marktd`.
///
/// If no active `PreisblattMessung` exists for the aMSB MP-ID, the auto-response
/// is skipped and the REQOTE is escalated to the operator.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MsbConfig {
    /// When `true`, an MSB-Wechsel `Accept` verdict dispatches the Bestätigung
    /// automatically. When `false` (default), it goes to the approval queue
    /// with its WiM Antwortfrist attached and an operator dispatches it.
    #[serde(default)]
    pub auto_accept: bool,

    /// When `true` (default), dispatch QUOTES automatically from
    /// `PreisblattMessung`. Set `false` to require operator approval — the
    /// REQOTE still lands in the approval queue with its Frist either way.
    #[serde(default = "default_msb_auto_preisanfrage")]
    pub auto_preisanfrage: bool,
}

fn default_msb_auto_preisanfrage() -> bool {
    true
}

impl Default for MsbConfig {
    fn default() -> Self {
        Self {
            auto_accept: false,
            auto_preisanfrage: default_msb_auto_preisanfrage(),
        }
    }
}

// ── OIDC ──────────────────────────────────────────────────────────────────────

/// OIDC configuration — re-exported from `mako-service` (shared across all daemons).
pub use mako_service::oidc::OidcConfig;

// ── OpenTelemetry ─────────────────────────────────────────────────────────────

/// OpenTelemetry config — shared struct from `mako-service`.
pub use mako_service::telemetry::OtelConfig;

// ── Loader + env resolution ───────────────────────────────────────────────────

/// Resolve an `"env:VAR_NAME"` reference or return the value as-is.
///
/// # Errors
///
/// Returns an error if the `env:` variable is not set.
pub fn resolve_env(value: &str) -> anyhow::Result<String> {
    if let Some(var) = value.strip_prefix("env:") {
        std::env::var(var).map_err(|_| {
            anyhow::anyhow!("environment variable {var:?} is not set (referenced in processd.toml)")
        })
    } else {
        Ok(value.to_owned())
    }
}

/// Like [`resolve_env`] but wraps the result in `secrecy::SecretString`.
pub fn resolve_env_secret(value: &str) -> anyhow::Result<secrecy::SecretString> {
    resolve_env(value).map(secrecy::SecretString::from)
}

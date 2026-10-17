//! Configuration for `netzbilanzd`.

use serde::Deserialize;

/// Top-level configuration loaded from `netzbilanzd.toml` / env vars.
///
/// Unknown keys are refused: a misspelt key is otherwise a setting that does
/// nothing, and for `[mabis]` that is a Summenzeitreihe filed under a default
/// nobody chose.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetzbilanzConfig {
    /// PostgreSQL connection + pool tuning (`[database]` block).
    pub database: mako_service::config::DatabaseConfig,
    /// `[http]` — the listen address. Defaults to every interface on `8680`.
    #[serde(default = "default_http")]
    pub http: mako_service::config::HttpConfig,
    /// Tenant identifier for multi-tenant deployments. Defaults to `"default"`.
    #[serde(default = "default_tenant")]
    pub tenant: String,
    /// `marktd` base URL for tariff lookups.
    pub marktd_url: String,
    /// `marktd` API key.
    pub marktd_api_key: String,
    /// `makod` base URL for command dispatch.
    pub makod_url: String,
    /// `makod` API key.
    pub makod_api_key: String,
    /// `edmd` base URL — auto-fetches imbalance data for MMM auto-run, and the
    /// Lastgänge a MaBiS Summenzeitreihe aggregates. Required when `[mabis]`
    /// is present.
    pub edmd_url: Option<String>,
    /// `edmd` bearer token.
    pub edmd_api_key: Option<String>,

    /// MCP server authentication. Supports API-key, OIDC, or dev mode.
    /// See `[mcp]` section in TOML — e.g. `api_key = "env:NETZBILANZD_MCP_API_KEY"`.
    ///
    /// With neither a key here nor `[oidc]`, `/mcp` runs in dev mode and serves
    /// the whole invoice register to anyone who can reach the port.
    /// [`Self::check_auth_posture`] refuses that.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,

    /// OIDC/JWT verification for every REST route.
    ///
    /// Absent, every request is admitted with synthetic dev claims — invoice
    /// dispatch, Storno, mark-paid, the § 147 AO export and the Redispatch
    /// Kostenblatt submission included. [`Self::check_auth_posture`] refuses
    /// that unless [`Self::allow_insecure_no_auth`] asks for it by name.
    #[serde(default)]
    pub oidc: Option<mako_service::oidc::OidcConfig>,

    /// Start without authentication.
    ///
    /// A posture that has to be asked for by name rather than reached by
    /// leaving a section out of the TOML.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,
    /// Optional ERP webhook URL — receives CloudEvents
    /// `de.netzbilanz.invoic.drafted` and `de.netzbilanz.invoic.dispatched`.
    pub erp_webhook_url: Option<String>,
    /// HMAC-SHA256 secret for signing the outbound ERP webhook CloudEvents
    /// (Standard Webhooks). Use `env:VAR_NAME`. Leave unset only
    /// in dev — a receiver verifying the signature rejects unsigned events.
    pub erp_webhook_secret: Option<String>,
    /// HMAC-SHA256 secret for verifying INBOUND REMADV CloudEvents on
    /// `POST /webhooks/remadv`. When unset the endpoint accepts unsigned bodies
    /// (dev mode); set it in production so a forged REMADV cannot mark a
    /// Bilanzkreis INVOIC paid/disputed.
    #[serde(default)]
    pub inbound_secret: Option<String>,
    /// How often (seconds) to look for drafts stuck undispatched.
    /// Default: 3600 (1 hour). Set to 0 to disable the worker.
    pub dispatch_alert_interval_secs: Option<u64>,
    /// How old (hours) an undispatched draft must be before it is reported.
    /// Default: 48. Set it to your Zahlungsziel minus the AS4 transit time.
    pub dispatch_stale_hours: Option<i64>,
    /// How often (seconds) to check for pending Kostenblatt near the 15th-of-month deadline.
    /// Default: 86400 (1 day). Set to 0 to disable.
    pub kostenblatt_alert_interval_secs: Option<u64>,
    /// MaBiS Summenzeitreihen filed with the BIKO. Absent, the MaBiS routes
    /// are not mounted and no submission is scheduled.
    #[serde(default)]
    pub mabis: Option<crate::mabis::config::MabisConfig>,
}

fn default_tenant() -> String {
    "default".to_owned()
}

impl NetzbilanzConfig {
    /// Refuse to start in a posture that leaves money-moving routes open.
    ///
    /// The four mechanisms are checked together because each guards a
    /// different door into the same daemon, and any one of them left open is
    /// enough: OIDC guards the operator API, the `[mcp]` key or OIDC guards the
    /// MCP surface, the inbound HMAC guards the one route no bearer token ever
    /// reaches, and the outbound HMAC is what lets the ERP tell an event this
    /// service emitted from one anybody posted.
    ///
    /// # Errors
    ///
    /// When any of them is unconfigured and `allow_insecure_no_auth` is not set.
    pub fn check_auth_posture(&self) -> anyhow::Result<()> {
        if self.allow_insecure_no_auth {
            tracing::warn!(
                "netzbilanzd: allow_insecure_no_auth is set — invoice dispatch, Storno, \
                 mark-paid, the § 147 AO audit export and the Kostenblatt submission are \
                 served to any caller, and the REMADV webhook accepts unsigned bodies"
            );
            return Ok(());
        }
        let mut fehlt = Vec::new();
        if self.oidc.is_none() {
            fehlt.push(
                "[oidc] — without it every REST route is admitted with dev claims: \
                 PUT /api/v1/billing/drafts/{id}/dispatch sends an INVOIC to a market \
                 partner over AS4, POST …/storno reverses one, PUT …/mark-paid falsifies \
                 a receivable, GET /api/v1/billing/audit exports the § 147 AO / § 14b UStG \
                 record, POST /api/v1/redispatch/kostenblatt/submit/{year}/{month} \
                 files the month's Redispatch costs, and POST /api/v1/mabis/sync files a \
                 binding Summenzeitreihe with the BIKO"
                    .to_owned(),
            );
        }
        if self.oidc.is_none() && !self.has_mcp_key() {
            fehlt.push(
                "[mcp] api_key (or [oidc]) — without either, /mcp runs in dev mode and \
                 serves the tenant's whole invoice register and its Redispatch cost \
                 sheets to any caller"
                    .to_owned(),
            );
        }
        if self.inbound_secret.is_none() {
            fehlt.push(
                "inbound_secret — without it POST /api/v1/webhooks/remadv accepts any \
                 unsigned body, and a forged REMADV marks an invoice paid or disputes one \
                 that was not"
                    .to_owned(),
            );
        }
        if self.erp_webhook_url.is_some() && self.erp_webhook_secret.is_none() {
            fehlt.push(
                "erp_webhook_secret — erp_webhook_url is set, and `OutboxWorker` signs \
                 only when it has a secret: de.netzbilanz.invoic.drafted/.dispatched and \
                 the stale-draft and Kostenblatt alerts all reach the ERP unsigned, and \
                 the receiver cannot tell one from a forgery"
                    .to_owned(),
            );
        }
        anyhow::ensure!(
            fehlt.is_empty(),
            "netzbilanzd refuses to start: {}. Configure them, or set \
             allow_insecure_no_auth = true to accept an unauthenticated deployment.",
            fehlt.join("; ")
        );
        Ok(())
    }

    /// Refuse a `[mabis]` section that cannot produce a valid submission.
    ///
    /// Run at startup, so a misconfigured filing fails here and not at 05:00 on
    /// the Erstaufschlag-Werktag, after a month of metering data has been
    /// aggregated and the run's version number consumed.
    ///
    /// # Errors
    ///
    /// When `[mabis]` is present and its submission target is unimplemented,
    /// its Bilanzierungsgebiet is not a Y-type EIC, or `edmd_url` — where every
    /// Lastgang the aggregate sums comes from — is unset.
    pub fn check_mabis(&self) -> anyhow::Result<()> {
        let Some(mabis) = &self.mabis else {
            return Ok(());
        };
        mabis.validate()?;
        anyhow::ensure!(
            self.edmd_url.is_some(),
            "[mabis] is configured but edmd_url is not: every Lastgang a Summenzeitreihe \
             aggregates is read from edmd, so the filing cannot be built without it"
        );
        Ok(())
    }

    /// Whether `[mcp]` carries a usable API key — primary or named.
    ///
    /// An empty string is not a key: `McpAuth::from_auth_config_oidc` skips it,
    /// so treating it as configured would report a door as locked that is open.
    fn has_mcp_key(&self) -> bool {
        self.mcp.api_key.as_ref().is_some_and(|k| !k.is_empty())
            || self.mcp.named_keys.iter().any(|k| !k.api_key.is_empty())
    }

    /// `edmd`, addressed and credentialed, or `None` when it is not configured.
    ///
    /// The single place `edmd_url` and `edmd_api_key` are read: every caller
    /// takes the result rather than the two fields, so where the credential
    /// goes is decided once.
    #[must_use]
    pub fn edmd(&self, client: reqwest::Client) -> Option<mako_service::http::Upstream> {
        self.edmd_url.as_deref().map(|url| {
            mako_service::http::Upstream::new(
                "edmd",
                url,
                self.edmd_api_key.clone().map(secrecy::SecretString::from),
                client,
            )
        })
    }
}

impl mako_service::ServiceConfig for NetzbilanzConfig {
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        Some(&self.database)
    }
    fn bind_addr(&self) -> String {
        self.http.addr.clone()
    }
}

fn default_http() -> mako_service::config::HttpConfig {
    mako_service::config::HttpConfig::on_port(8680)
}

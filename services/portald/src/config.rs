//! Configuration for `portald`.
//!
//! # Authentication model
//!
//! `portald` does **not** verify customer JWTs itself. Every portal request
//! carries the customer's `Authorization: Bearer` header, which is forwarded
//! verbatim to `vertragd GET /api/v1/kunden/authenticate?malo_id=…`. `vertragd`
//! owns the OIDC verifier, the customer record and the customer↔MaLo mapping,
//! so it is the only place that can answer "may this identity read this
//! delivery point". A second verifier here could only ever disagree with it,
//! and a portal that disagrees about ownership discloses one customer's
//! consumption, invoices and balance to another.
//!
//! There is deliberately no `oidc_issuer` / `oidc_audience` here: `portald`
//! verifies no tokens, and a key suggesting otherwise misstates where the trust
//! boundary is.
//!
//! # Minimal `portald.toml`
//!
//! ```toml
//! port   = 9480
//! tenant = "9900357000004"
//!
//! # Required: the authorization authority. Without it portald cannot decide
//! # who owns a MaLo, and refuses to start (see `allow_insecure_no_auth`).
//! vertragd_url     = "http://vertragd:9780"
//! vertragd_api_key = "env:PORTALD_VERTRAGD_SERVICE_KEY"
//!
//! edmd_url        = "http://edmd:8380"
//! billingd_url    = "http://billingd:9280"
//! accountingd_url = "http://accountingd:9380"
//! einsd_url       = "http://einsd:9180"
//! marktd_url      = "http://marktd:8180"
//! outputd_url     = "http://outputd:9880"
//! ```

use serde::Deserialize;

/// Runtime configuration loaded from `portald.toml` or `PORTALD_*` env vars.
// No `deny_unknown_fields`: `mako_service::load_config` surfaces the
// `PORTALD_CONFIG` path variable as a stray `config` key, which would otherwise
// make every deployment that points at its config via that variable fail.
#[derive(Debug, Clone, Deserialize)]
pub struct PortaldConfig {
    /// HTTP listen port (default: 9480).
    #[serde(default = "default_port")]
    pub port: u16,

    /// Operator tenant identifier (BDEW-Codenummer).
    pub tenant: String,

    /// `vertragd` base URL — **the authorization authority**.
    ///
    /// Every portal route resolves `(customer token, malo_id) → kunden_id`
    /// through `GET /api/v1/kunden/authenticate` here, and the self-service
    /// write routes proxy to it. Absent, the service refuses to start unless
    /// [`Self::allow_insecure_no_auth`] is set.
    pub vertragd_url: Option<String>,
    /// Service credential for `vertragd`, sent as `X-Api-Key`.
    ///
    /// Never as `Authorization` — that header carries the **customer's** token
    /// and is what `vertragd` decides ownership from. Overwriting it with a
    /// service credential authorises every MaLo for every caller.
    pub vertragd_api_key: Option<String>,

    /// `edmd` base URL — `GET /api/v1/lastgang/{malo_id}`, `/billing-period/{malo_id}`
    pub edmd_url: Option<String>,
    /// Bearer token for `edmd` API.
    pub edmd_api_key: Option<String>,

    /// `billingd` base URL — `GET /api/v1/billing?malo_id=…`
    pub billingd_url: Option<String>,
    /// Bearer token for `billingd` API.
    pub billingd_api_key: Option<String>,

    /// `accountingd` base URL — `GET /api/v1/accounts/{malo_id}/…`
    pub accountingd_url: Option<String>,
    /// Bearer token for `accountingd` API.
    pub accountingd_api_key: Option<String>,

    /// `einsd` base URL — `GET /api/v1/anlagen?malo_id=…`
    pub einsd_url: Option<String>,
    /// Bearer token for `einsd` API.
    pub einsd_api_key: Option<String>,

    /// `marktd` base URL — `GET /api/v1/versorgung/{malo_id}`
    pub marktd_url: Option<String>,
    /// Bearer token for `marktd` API.
    pub marktd_api_key: Option<String>,

    /// `outputd` base URL — the customer's **document inbox**:
    /// `GET /api/v1/documents?malo_id=…` and the stored bytes of each one.
    ///
    /// Distinct from `billingd_url`, which lists billing *records* — what was
    /// calculated, drafts included. This lists what the customer was actually
    /// sent, which is what an inbox shows and what a § 41f EnWG dispute asks
    /// about.
    pub outputd_url: Option<String>,
    /// Bearer token for `outputd`.
    pub outputd_api_key: Option<String>,

    /// LF MP-ID (BDEW-Codenummer) used when registering a SEPA mandate.
    /// Must match the `lf_mp_id` configured in `accountingd`. Defaults to
    /// [`Self::tenant`].
    pub lf_mp_id: Option<String>,

    /// Serve portal routes without resolving customer ownership.
    ///
    /// Intended for local development against stub upstreams. It must be named
    /// in the config rather than reached by omitting `vertragd_url`, so that
    /// serving one customer's ledger to another is always a decision someone
    /// wrote down. Every request is logged at `warn` in this mode.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,

    /// MCP server authentication.
    ///
    /// The MCP surface is **operator-facing**: its tools take a `malo_id`
    /// parameter and carry no customer token, so whoever can reach `/mcp` can
    /// read every customer in the tenant. Configure an API key (or OIDC) and
    /// keep it off the public ingress; `McpAuth::dev()` is development only.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
}

fn default_port() -> u16 {
    9480
}

impl PortaldConfig {
    /// Resolve every `env:VARNAME` indirection in the loaded config.
    ///
    /// Without this the placeholder is used verbatim: a documented
    /// `edmd_api_key = "env:PORTALD_EDMD_SERVICE_KEY"` is sent as the literal
    /// bearer token `env:PORTALD_EDMD_SERVICE_KEY`, so every upstream call 401s
    /// and the portal shows a customer an empty dashboard instead of a
    /// configuration failure.
    ///
    /// # Errors
    ///
    /// Fails when a referenced environment variable is unset.
    pub fn resolve_env_refs(&mut self) -> anyhow::Result<()> {
        use mako_service::config::resolve_env;
        for slot in [
            &mut self.vertragd_url,
            &mut self.vertragd_api_key,
            &mut self.edmd_url,
            &mut self.edmd_api_key,
            &mut self.billingd_url,
            &mut self.billingd_api_key,
            &mut self.accountingd_url,
            &mut self.accountingd_api_key,
            &mut self.einsd_url,
            &mut self.einsd_api_key,
            &mut self.marktd_url,
            &mut self.marktd_api_key,
            &mut self.outputd_url,
            &mut self.outputd_api_key,
            &mut self.lf_mp_id,
        ] {
            if let Some(v) = slot.as_deref() {
                *slot = Some(resolve_env(v)?);
            }
        }
        self.tenant = resolve_env(&self.tenant)?;
        Ok(())
    }

    /// The MP-ID a self-service SEPA mandate is registered under.
    #[must_use]
    pub fn lf_mp_id(&self) -> &str {
        self.lf_mp_id.as_deref().unwrap_or(&self.tenant)
    }
}

impl mako_service::ServiceConfig for PortaldConfig {
    /// `portald` is stateless — no pool, no migrations.
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        None
    }
    fn bind_addr(&self) -> String {
        format!("0.0.0.0:{}", self.port)
    }
}

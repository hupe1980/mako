//! Configuration for `productd`.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ProductdConfig {
    /// PostgreSQL connection + pool tuning (`[database]` block).
    pub database: mako_service::config::DatabaseConfig,

    /// HTTP listen port.  Defaults to `9080`.
    pub port: Option<u16>,

    /// Tenant identifier — the data-isolation key written to every row and
    /// enforced on every token.
    pub tenant: String,

    /// The Lieferant market-partner ID this catalogue's products are sold
    /// under, and the default for requests that do not name one.
    ///
    /// Distinct from `tenant`: the isolation key and the market identity are
    /// the same string in a single-mandant install and different in a shared
    /// one. Using `tenant` for both files every MaLo→product assignment under a
    /// market party that does not trade, where `billingd`'s lookup by the real
    /// MP-ID finds nothing. Defaults to `tenant` so a single-mandant deployment
    /// need not repeat itself.
    #[serde(default, rename = "lf_mp_id")]
    lf_mp_id_override: Option<String>,

    /// OIDC/JWT authentication configuration.
    ///
    /// When absent, auth is **disabled** (dev mode only).  Must be set in
    /// production.  All REST management endpoints require a valid Bearer
    /// token; only the public comparison feed is unauthenticated.
    pub oidc: Option<mako_service::oidc::OidcConfig>,

    /// ERP webhook URL for `de.tarif.product.updated` notifications.
    pub erp_webhook_url: Option<String>,

    /// HMAC-SHA256 secret for outbound webhook signing.
    /// When absent, the signature headers are omitted from outbound
    /// CloudEvent webhooks.  Required in production.
    pub erp_hmac_secret: Option<String>,

    /// MCP server authentication. Supports API-key, OIDC, or dev mode.
    /// See `[mcp]` section in TOML — e.g. `api_key = "env:PRODUCTD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,

    /// Start without `[oidc]`, admitting every request with dev claims.
    ///
    /// Deliberately explicit: those synthetic claims carry `LF`/`MSB`/`ESA`/
    /// `ADMIN`, so Cedar permits **everything** — including writing any
    /// supplier's tariff catalogue and its prices. An operator running this
    /// posture should have chosen it.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,
}

impl ProductdConfig {
    /// The market identity products are sold under.
    #[must_use]
    pub fn lf_mp_id(&self) -> &str {
        self.lf_mp_id_override.as_deref().unwrap_or(&self.tenant)
    }

    /// Refuse to start in a posture that authenticates nothing.
    ///
    /// productd sets prices. Without `[oidc]` every request is admitted with
    /// synthetic dev claims that carry `LF`/`MSB`/`ESA`/`ADMIN`, so the Cedar
    /// policy permits every action — a `PUT /api/v1/products/{lf}/{code}` from
    /// anyone who can reach the port rewrites a supplier's catalogue, prices
    /// included. The § 41c comparison feed then publishes those prices.
    ///
    /// The same shape `netzbilanzd`, `outputd`, `sperrd` and `vertragd` refuse:
    /// an unauthenticated deployment is a choice, not a default.
    ///
    /// # Errors
    ///
    /// When `[oidc]` is absent and `allow_insecure_no_auth` is not set.
    pub fn check_auth_posture(&self) -> anyhow::Result<()> {
        if self.allow_insecure_no_auth {
            tracing::warn!(
                "productd: allow_insecure_no_auth is set — every request is admitted with \
                 dev claims carrying LF/MSB/ESA/ADMIN, so any caller may write any \
                 supplier's products and prices"
            );
            return Ok(());
        }
        anyhow::ensure!(
            self.oidc.is_some(),
            "productd refuses to start: [oidc] is not configured — without it every \
             request is admitted with dev claims that Cedar grants every action, \
             including writing any supplier's tariff catalogue and the prices the \
             § 41c EnWG comparison feed publishes. Configure it, or set \
             allow_insecure_no_auth = true to accept an unauthenticated deployment."
        );
        Ok(())
    }
}

impl mako_service::ServiceConfig for ProductdConfig {
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        Some(&self.database)
    }
    fn bind_addr(&self) -> String {
        format!("0.0.0.0:{}", self.port.unwrap_or(9080))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A default config carries no `[oidc]`, and productd sets prices.
    ///
    /// Without this guard the service started happily and admitted every
    /// request with synthetic dev claims carrying `LF`/`MSB`/`ESA`/`ADMIN` —
    /// which the Cedar policy grants every action, so the authorization layer
    /// was present and bypassed at the same time.
    #[test]
    fn an_unauthenticated_posture_is_refused_unless_chosen() {
        // Parsed from TOML rather than built by hand: that is how a deployment
        // reaches this state, and it exercises the serde defaults that decide it.
        let minimal = r#"
            tenant = "9900000000001"
            [database]
            url = "env:DATABASE_URL"
        "#;

        let cfg: ProductdConfig = toml::from_str(minimal).expect("minimal config parses");
        assert!(cfg.oidc.is_none());
        assert!(
            !cfg.allow_insecure_no_auth,
            "the insecure posture must never be the default"
        );
        assert!(
            cfg.check_auth_posture().is_err(),
            "productd must refuse to start with no [oidc] and no explicit opt-in"
        );

        // The key goes *before* `[database]`: a TOML table captures every
        // key after it, so appending would have set it on the database section
        // and silently left the top-level flag false.
        let chosen: ProductdConfig = toml::from_str(
            r#"
            tenant = "9900000000001"
            allow_insecure_no_auth = true
            [database]
            url = "env:DATABASE_URL"
        "#,
        )
        .expect("opt-in config parses");
        assert!(chosen.allow_insecure_no_auth, "the opt-in actually parsed");
        assert!(
            chosen.check_auth_posture().is_ok(),
            "an operator that chose the insecure posture may run it"
        );
    }
}

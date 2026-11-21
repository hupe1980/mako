//! Configuration for `productd`.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductdConfig {
    /// PostgreSQL connection + pool tuning (`[database]` block).
    pub database: mako_service::config::DatabaseConfig,

    /// `[http]` — the listen address. Defaults to every interface on `9080`.
    #[serde(default = "default_http")]
    pub http: mako_service::config::HttpConfig,

    /// Tenant identifier — the data-isolation key written to every row and
    /// enforced on every token.
    pub tenant: String,

    /// The Lieferant MP-ID this catalogue's products are sold under — the only
    /// catalogue a write may change, and the default for a request that names
    /// none. Required and distinct from `tenant`, the isolation key: „Für jede
    /// Sparte (Gas bzw. Strom) und jede Rolle hat ein Marktteilnehmer genau
    /// eine MP-ID" (Identifikatoren AWH V1.2 Kap. 2.1), so the Lieferant's code
    /// is never derived from the tenant.
    lf_mp_id: String,

    /// OIDC/JWT authentication configuration.
    ///
    /// When absent, auth is **disabled** (dev mode only).  Must be set in
    /// production.  All REST management endpoints require a valid Bearer
    /// token; only the public comparison feed is unauthenticated.
    pub oidc: Option<mako_service::oidc::OidcConfig>,

    /// The ERP webhook: the subscription endpoint `default`, which receives
    /// every CloudEvent productd emits, and the receiver of the contract
    /// request an accepted Angebot owes (`vertragd`'s `/api/v1/webhooks/angebot`).
    pub erp_webhook_url: Option<String>,

    /// The `default` endpoint's signing secret (Standard Webhooks). When
    /// absent, deliveries to it are unsigned. Required in production.
    pub erp_hmac_secret: Option<String>,

    /// `[subscriptions]` — webhook endpoints registered through the API:
    /// retention of the event feed, and the loopback admission of `just dev`.
    #[serde(default)]
    pub subscriptions: mako_service::subscriptions::SubscriptionsConfig,

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

    /// `billingd` base URL. A Gas quotation for a year § 10 BEHG fixes no
    /// price for is priced under billingd's `[policy] behg_regime`, read from
    /// its `GET /api/v1/policy`; without one such a position is refused
    /// `BEHG_PREIS_FEHLT`.
    pub billingd_url: Option<String>,

    /// `billingd` bearer token — register it there as an
    /// `[[oidc.service_keys]]` entry of this tenant, which Cedar's
    /// `read-policy` admits.
    pub billingd_api_key: Option<String>,

    /// `marktd` base URL — the Netzbetreiber's Preisblätter (Netznutzung,
    /// Messung, Konzessionsabgabe) and a MaLo's Netzbetreiber, read for an
    /// Endkunden-Angebot. Without it that route answers `503
    /// marktd_nicht_konfiguriert`; no offer is priced without the NB's sheets.
    pub marktd_url: Option<String>,

    /// `marktd` bearer token — a service credential of this tenant there,
    /// which its Cedar `read-preisblatt` and `read-malo-grid` admit.
    pub marktd_api_key: Option<String>,

    /// Removed: the regime is billingd's `[policy] behg_regime`. Named so a
    /// config still carrying it is refused at startup instead of ignored.
    #[serde(default)]
    pub behg_regime: Option<String>,
}

impl ProductdConfig {
    /// The market identity products are sold under.
    #[must_use]
    pub fn lf_mp_id(&self) -> &str {
        &self.lf_mp_id
    }

    /// Every finding that keeps productd from running as documented: a
    /// removed key, and the authentication posture ([`Self::check_auth_posture`]).
    pub fn validate(&self) -> Vec<mako_service::Befund> {
        use mako_service::befund::{Befund, Bereich};
        let mut out = Vec::new();
        if let Some(old) = &self.behg_regime {
            out.push(Befund::blockiert(
                "behg_regime",
                format!(
                    "productd: `behg_regime = {old:?}` is not read; the regime is billingd's \
                     `[policy] behg_regime`, which productd reads from billingd. Set it there \
                     and configure `billingd_url` (and `billingd_api_key`) here."
                ),
            ));
        }
        if let Err(e) = self.check_auth_posture() {
            out.push(Befund::aus_fehler("oidc", e).bereich(Bereich::Zugriff));
        }
        if let Err(e) = rubo4e::identifiers::MarktpartnerId::new(self.lf_mp_id.trim()) {
            out.push(Befund::blockiert(
                "lf_mp_id",
                format!(
                    "lf_mp_id {:?} is not an MP-ID ({e}) — set the Lieferant's 13-digit \
                     BDEW- or DVGW-Codenummer",
                    self.lf_mp_id
                ),
            ));
        }
        out.extend(self.subscriptions.befunde());
        out
    }

    /// The posture productd runs under — logged, exported, recorded and served
    /// at `GET /api/v1/policy` and by the MCP tool `get_policy`.
    #[must_use]
    pub fn policy_report(&self) -> mako_service::policy::PolicyReport {
        mako_service::policy::PolicyReport::new()
            .posture_flag("allow_insecure_no_auth", self.allow_insecure_no_auth)
            .posture_presence("oidc", self.oidc.is_some(), true)
            .posture_presence("billingd_url", self.billingd_url.is_some(), false)
            .posture_presence("marktd_url", self.marktd_url.is_some(), false)
    }

    /// Refuse to start in a posture that authenticates nothing.
    ///
    /// productd sets prices. Without `[oidc]` every request is admitted with
    /// synthetic dev claims that carry `LF`/`MSB`/`ESA`/`ADMIN`, so the Cedar
    /// policy permits every action — a `PUT /api/v1/products/{lf}/{code}` from
    /// anyone who can reach the port rewrites a supplier's catalogue, prices
    /// included. The § 41c comparison feed then publishes those prices.
    ///
    /// The same shape `netzbilanzd`, `outputd`, `processd` and `vertragd` refuse:
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
        self.http.addr.clone()
    }
    fn oidc(&self) -> Option<&mako_service::oidc::OidcConfig> {
        self.oidc.as_ref()
    }
}

fn default_http() -> mako_service::config::HttpConfig {
    mako_service::config::HttpConfig::on_port(9080)
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
    /// The Lieferant's MP-ID is a setting of its own: absent, the config does
    /// not parse; set to something that is no MP-ID (the tenant key), the
    /// start is refused at `lf_mp_id`.
    #[test]
    fn the_lf_mp_id_is_required_and_never_the_tenant() {
        let ohne = r#"
            tenant = "stadtwerke-musterstadt"
            [database]
            url = "env:DATABASE_URL"
        "#;
        assert!(toml::from_str::<ProductdConfig>(ohne).is_err());

        let tenant_als_lf = r#"
            tenant = "stadtwerke-musterstadt"
            lf_mp_id = "stadtwerke-musterstadt"
            allow_insecure_no_auth = true
            [database]
            url = "env:DATABASE_URL"
        "#;
        let cfg: ProductdConfig = toml::from_str(tenant_als_lf).expect("parses");
        let pfade: Vec<String> = cfg.validate().into_iter().map(|b| b.pfad).collect();
        assert_eq!(pfade, ["lf_mp_id"]);
    }

    #[test]
    fn an_unauthenticated_posture_is_refused_unless_chosen() {
        // Parsed from TOML rather than built by hand: that is how a deployment
        // reaches this state, and it exercises the serde defaults that decide it.
        let minimal = r#"
            tenant = "stadtwerke-musterstadt"
            lf_mp_id = "9900000000001"
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
            tenant = "stadtwerke-musterstadt"
            lf_mp_id = "9900000000001"
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

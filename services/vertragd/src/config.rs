//! Deployment configuration for `vertragd`.

use serde::Deserialize;

/// `vertragd.toml`.
#[derive(Debug, Deserialize)]
pub struct VertragdConfig {
    /// PostgreSQL connection + pool tuning (`[database]` block).
    pub database: mako_service::config::DatabaseConfig,
    pub port: Option<u16>,
    /// Tenant identifier — the data-isolation key written to every row and
    /// enforced on every token. Typically the operator's BDEW- or
    /// DVGW-Codenummer, but any stable unique string is valid.
    pub tenant: String,
    /// The Lieferant market-partner ID this deployment registers supply under.
    /// Distinct from `tenant`: the isolation key and the market identity are
    /// the same string in a single-mandant install and different in a shared
    /// one, and sending the wrong one produces UTILMDs from a party the NB does
    /// not know.
    pub lf_mp_id: String,

    /// `processd` — Lieferbeginn/Lieferende per Vertragskomponente.
    pub processd_url: String,
    pub processd_api_key: Option<String>,
    /// `accountingd` — billing account once a contract is in supply.
    pub accountingd_url: String,
    pub accountingd_api_key: Option<String>,
    /// `edmd` — Beginn-/Schlussablesung reading orders.
    pub edmd_url: String,
    pub edmd_api_key: Option<String>,

    /// ERP webhook receiving the `de.vertrag.*` CloudEvents.
    ///
    /// Without it the statutory notices are still persisted in `event_outbox`,
    /// but nothing delivers them — a deployment that owes customers a § 41
    /// Abs. 5 EnWG notice needs this set.
    pub erp_webhook_url: Option<String>,
    /// Standard Webhooks secret signing the outbound events.
    pub erp_hmac_secret: Option<String>,

    /// HMAC-SHA256 secret verifying INBOUND CloudEvents from `makod`,
    /// `processd` and `productd` on `POST /api/v1/events` and
    /// `POST /api/v1/webhooks/angebot`.
    ///
    /// Those two routes carry no operator token, so this is their only
    /// authentication: a forged event moves supply and creates contracts.
    /// `check_auth_posture` refuses to start without it.
    #[serde(default)]
    pub inbound_secret: Option<String>,

    /// `outputd` — renders and delivers the § 41 Abs. 5 EnWG
    /// Preisänderungsanzeige.
    ///
    /// Absent → the CloudEvent is the notice: it carries the Umfang and the
    /// Sonderkündigungsrecht, and an ERP composes the letter. A deployment with
    /// neither this nor an ERP webhook schedules price changes and tells
    /// nobody.
    pub outputd_url: Option<String>,
    pub outputd_api_key: Option<String>,

    /// `[absender]` — the operator as § 126b BGB's declarant, printed on every
    /// customer notice this service issues.
    ///
    /// Configured rather than derived: `vertragd` holds customers, not the
    /// operator's own letterhead. Required wherever `outputd_url` is set: a
    /// Textform declaration that does not name its declarant is not Textform,
    /// so the notice fails and is retried rather than going out unsigned.
    #[serde(default)]
    pub absender: Option<AbsenderConfig>,

    /// MCP server authentication (API key, OIDC, or dev mode).
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,

    /// OIDC/JWT authentication for every REST endpoint.
    pub oidc: Option<mako_service::oidc::OidcConfig>,

    /// Maximum active portal identities per Kunde — bounds the damage a
    /// compromised B2B admin account can do by creating logins.
    #[serde(default = "VertragdConfig::default_max_identitaeten")]
    pub max_identitaeten_per_kunde: u32,

    /// Start without authentication.
    ///
    /// With `[oidc]` absent every request is admitted with synthetic dev
    /// claims, and without `inbound_secret` any unsigned body can move supply.
    /// That posture has to be asked for by name rather than reached by leaving
    /// a section out.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,
}

impl VertragdConfig {
    const fn default_max_identitaeten() -> u32 {
        50
    }

    /// Refuse to start in a posture that exposes customer data or lets an
    /// unauthenticated caller move supply.
    ///
    /// The two authentication mechanisms are checked together because they
    /// protect different halves of the same surface: OIDC guards the operator
    /// API (GDPR export, IBAN writes, contract mutation), the inbound HMAC
    /// guards the two webhook routes (`de.mako.process.*` outcomes and CPQ
    /// Angebote) that no token ever reaches.
    ///
    /// # Errors
    ///
    /// When either is unconfigured and `allow_insecure_no_auth` is not set.
    pub fn check_auth_posture(&self) -> anyhow::Result<()> {
        if self.allow_insecure_no_auth {
            tracing::warn!(
                "vertragd: allow_insecure_no_auth is set — requests are admitted with dev \
                 claims and inbound webhooks are accepted unsigned"
            );
            return Ok(());
        }
        let mut fehlt = Vec::new();
        if self.oidc.is_none() {
            fehlt.push(
                "[oidc] — without it every request is admitted with dev claims, GDPR \
                 customer data, IBANs and contract mutation included",
            );
        }
        if self.inbound_secret.is_none() {
            fehlt.push(
                "inbound_secret — without it POST /api/v1/events and \
                 POST /api/v1/webhooks/angebot accept any unsigned body, and a forged one \
                 confirms supply or creates a contract",
            );
        }
        anyhow::ensure!(
            fehlt.is_empty(),
            "vertragd refuses to start: {}. Configure them, or set \
             allow_insecure_no_auth = true to accept an unauthenticated deployment.",
            fehlt.join("; ")
        );
        Ok(())
    }
}

impl mako_service::ServiceConfig for VertragdConfig {
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        Some(&self.database)
    }
    fn bind_addr(&self) -> String {
        format!("0.0.0.0:{}", self.port.unwrap_or(9780))
    }
}

/// The operator's own identity on a customer notice — § 126b BGB's declarant.
#[derive(Debug, Deserialize, Clone)]
pub struct AbsenderConfig {
    /// The legal name, as it must appear on the page.
    pub name: Option<String>,
    /// Street and house number.
    pub line1: Option<String>,
    pub post_code: Option<String>,
    pub city: Option<String>,
    /// ISO 3166-1 alpha-2. Omitted on a domestic letter.
    pub country: Option<String>,
    pub vat_id: Option<String>,
    /// The department a customer replies to — printed beside the phone number,
    /// and named in the § 41 Abs. 5 notice as where to exercise the
    /// Sonderkündigungsrecht.
    pub contact_name: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
}

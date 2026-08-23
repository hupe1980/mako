//! Configuration for `outputd`: TOML (`outputd.toml`) + `OUTPUTD_` env
//! overrides, loaded by `mako_service`.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct OutputdConfig {
    pub database: mako_service::config::DatabaseConfig,
    pub port: Option<u16>,
    /// The operator's MP-ID — the tenant every template row is scoped to.
    pub tenant: String,
    /// OIDC verification for the HTTP API. Fail closed: without it, anyone can
    /// publish the layout every customer document renders with, and render
    /// arbitrary documents under the operator's Briefkopf.
    #[serde(default)]
    pub oidc: Option<mako_service::oidc::OidcConfig>,
    /// Dev-only escape hatch, named loudly on startup.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,
    /// `[delivery]` — how issued documents reach the customer.
    #[serde(default)]
    pub delivery: DeliveryConfig,
}

/// How documents leave this daemon.
///
/// outputd embeds no SMTP client and no print driver. Each outbound channel is
/// an HTTP relay an operator points at whatever they already run — the same
/// contract `accountingd` uses for its bank adapter. A deployment that
/// configures none still gets the **portal** channel, which is the one an
/// energy supplier actually owes: § 41 Abs. 5 EnWG and § 126b BGB ask for
/// Textform on a durable medium, not for registered post.
#[derive(Debug, Deserialize)]
pub struct DeliveryConfig {
    /// Run the delivery worker. Default **true**.
    ///
    /// Off means documents are still stored and served — a portal customer can
    /// still fetch an invoice — but nothing is pushed anywhere and queued
    /// deliveries stay `PENDING`, visibly, rather than silently succeeding.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Mail relay: `POST {url}` with the JSON body described in
    /// [`crate::delivery::worker`], answering 2xx on acceptance.
    pub email_relay_url: Option<String>,
    /// Bearer credential for the mail relay. `"env:VAR"` for injection.
    pub email_relay_api_key: Option<secrecy::SecretString>,

    /// Print-service push endpoint. Optional: without it, `POST` deliveries
    /// wait in `GET /api/v1/spool` for a print service to **pull**, which is
    /// how most Druckdienstleister integrate.
    pub postal_relay_url: Option<String>,
    /// Bearer credential for the print service.
    pub postal_relay_api_key: Option<secrecy::SecretString>,

    /// The operator's own system, for deployments that take delivery
    /// themselves.
    pub erp_webhook_url: Option<String>,
    /// Bearer credential for the ERP endpoint.
    pub erp_api_key: Option<secrecy::SecretString>,

    /// The `From:` a mail relay should use. Passed through, not validated —
    /// the relay owns its own envelope rules.
    pub from_address: Option<String>,

    /// Subject line per document kind, e.g.
    /// `MAHNUNG = "Zahlungserinnerung"`. A kind with no entry falls back to
    /// [`DeliveryConfig::default_subject`].
    #[serde(default)]
    pub subjects: std::collections::HashMap<String, String>,

    /// How many attempts before a delivery is `FAILED`. Default **8**, which
    /// with the doubling backoff spans about half a day — long enough that a
    /// relay outage does not permanently fail a Mahnung, short enough that a
    /// misconfigured URL surfaces the same day.
    #[serde(default = "default_max_attempts")]
    pub max_attempts: i32,
}

const fn default_true() -> bool {
    true
}
const fn default_max_attempts() -> i32 {
    8
}

impl Default for DeliveryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            email_relay_url: None,
            email_relay_api_key: None,
            postal_relay_url: None,
            postal_relay_api_key: None,
            erp_webhook_url: None,
            erp_api_key: None,
            from_address: None,
            subjects: std::collections::HashMap::new(),
            max_attempts: default_max_attempts(),
        }
    }
}

impl DeliveryConfig {
    /// The subject line for a document kind.
    #[must_use]
    pub fn subject_for(&self, kind: &str) -> String {
        self.subjects
            .get(kind)
            .cloned()
            .unwrap_or_else(|| Self::default_subject(kind).to_owned())
    }

    /// The built-in subject for a kind — plain German, no marketing, because
    /// these are statutory notices and one of them threatens a disconnection.
    #[must_use]
    pub const fn default_subject(kind: &str) -> &'static str {
        match kind.as_bytes() {
            b"INVOICE" => "Ihre Rechnung",
            b"MAHNUNG" => "Zahlungserinnerung",
            b"PREISANPASSUNG" => "Änderung Ihrer Preise",
            _ => "Ihr Dokument",
        }
    }
}

impl mako_service::ServiceConfig for OutputdConfig {
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        Some(&self.database)
    }
    fn bind_addr(&self) -> String {
        format!("0.0.0.0:{}", self.port.unwrap_or(9880))
    }
}

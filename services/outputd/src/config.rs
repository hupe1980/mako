//! Configuration for `outputd`: TOML (`outputd.toml`) + `OUTPUTD_` env
//! overrides, loaded by `mako_service`.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputdConfig {
    pub database: mako_service::config::DatabaseConfig,
    /// `[http]` — the listen address. Defaults to every interface on `9880`.
    #[serde(default = "default_http")]
    pub http: mako_service::config::HttpConfig,
    /// Tenant identifier — data-isolation key written to every template row.
    /// Typically the operator's BDEW- or DVGW-Codenummer, but any stable
    /// unique string is valid.
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
    /// `[policy]` — the operator readings this deployment runs under.
    #[serde(default)]
    pub policy: OutputdPolicy,
}

/// `[policy]` — outputd's operator settings.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputdPolicy {
    /// How a customer letter addresses its reader.
    #[serde(default)]
    pub anrede: mako_service::policy::Policy<Anrede>,
}

mako_service::policy_choice! {
    /// How a customer letter addresses its reader — „Sie“ or „du“.
    ///
    /// No statute fixes the form of address of a Textform letter, so it is the
    /// operator's. Two documents do not follow it: the Widerrufsbelehrung inside
    /// the Vertragsbestätigung is the statutory Muster, and the Gebührenbescheid
    /// is a Verwaltungsakt — both keep „Sie“ ([`Anrede::fuer`]).
    pub enum Anrede {
        key: "anrede",
        doc: "@/docs/services/outputd.md#policy-anrede",
        default: tightening(Sie, "the formal address the Muster-Widerrufsbelehrung and a Gebührenbescheid keep regardless: an unset deployment never addresses a customer more familiarly than the operator chose, and its letters never mix two forms"),
        options: {
            /// „Sie“, „Ihr“ — capitalised, the formal address.
            Sie = "sie" => "§ 126b BGB (Textform) fixes no form of address; the formal „Sie“ is the address the Muster-Widerrufsbelehrung under Art. 246a § 1 Abs. 2 EGBGB uses",
            /// „du“, „dein“ — lower-case in a letter, as the current spelling
            /// rules write it.
            Du = "du" => "§ 126b BGB (Textform) fixes no form of address; the informal „du“ is the operator's choice of voice",
        }
    }
}

impl Anrede {
    /// The address a document of `kind` is written in under this setting.
    ///
    /// A `GEBUEHRENBESCHEID` is always „Sie“: it is a Verwaltungsakt of a
    /// municipal operator, not a supplier's letter. The Widerrufsbelehrung is
    /// exempt inside the Vertragsbestätigung's template, not here, because the
    /// rest of that letter follows the setting.
    #[must_use]
    pub fn fuer(self, kind: &str) -> Self {
        if kind == "GEBUEHRENBESCHEID" {
            Self::Sie
        } else {
            self
        }
    }

    /// The option by its config name, `sie` or `du`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        use mako_service::policy::PolicyChoice as _;
        Self::options().iter().copied().find(|a| a.name() == name)
    }

    /// `sie` or `du` — the value a template reads as `anrede` and the
    /// document store records.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        use mako_service::policy::PolicyChoice as _;
        self.name()
    }
}

impl OutputdConfig {
    /// The policy and posture outputd runs under — logged, exported, recorded
    /// and served by the runner and `GET /api/v1/policy`.
    #[must_use]
    pub fn policy_report(&self) -> mako_service::policy::PolicyReport {
        mako_service::policy::PolicyReport::new()
            .with(&self.policy.anrede)
            .posture_flag("allow_insecure_no_auth", self.allow_insecure_no_auth)
            .posture_presence("oidc", self.oidc.is_some(), true)
    }
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
#[serde(deny_unknown_fields)]
pub struct DeliveryConfig {
    /// Set deliveries going and run the relay workers. Default **true**.
    ///
    /// Off means documents are still stored and served — a portal customer can
    /// still fetch an invoice — but nothing is pushed anywhere and queued
    /// deliveries stay `PENDING`, visibly, rather than silently succeeding.
    /// A pushed delivery retries with backoff and is `FAILED` once the shared
    /// outbox policy gives up on it.
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
}

impl DeliveryConfig {
    /// Relay URLs configured without a credential, by config key.
    ///
    /// A relay body carries a customer's document — the invoice or Mahnung
    /// itself, base64-encoded, with their name, e-mail address, MaLo and
    /// Kundennummer beside it. Without a credential the push is neither
    /// authenticated nor signed, so the receiver cannot tell it from anything
    /// else that reached the URL and anybody who can is served the document.
    #[must_use]
    pub fn unsigned_relays(&self) -> Vec<&'static str> {
        let configured = |url: &Option<String>| url.as_deref().is_some_and(|u| !u.is_empty());
        let mut open = Vec::new();
        for (url, key, name) in [
            (
                &self.email_relay_url,
                &self.email_relay_api_key,
                "email_relay_api_key",
            ),
            (
                &self.postal_relay_url,
                &self.postal_relay_api_key,
                "postal_relay_api_key",
            ),
            (&self.erp_webhook_url, &self.erp_api_key, "erp_api_key"),
        ] {
            if configured(url) && key.is_none() {
                open.push(name);
            }
        }
        open
    }
}

const fn default_true() -> bool {
    true
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
        }
    }
}

impl DeliveryConfig {
    /// Whether `POST` deliveries are pushed to a print service rather than
    /// left in the spool for one to pull.
    #[must_use]
    pub fn has_postal_relay(&self) -> bool {
        self.postal_relay_url
            .as_deref()
            .is_some_and(|u| !u.is_empty())
    }

    /// The subject line for a document kind, in the document's `anrede`.
    ///
    /// An operator's `[delivery.subjects]` entry is used verbatim: it is their
    /// own wording, in whichever form they wrote it.
    #[must_use]
    pub fn subject_for(&self, kind: &str, anrede: Anrede) -> String {
        self.subjects
            .get(kind)
            .cloned()
            .unwrap_or_else(|| Self::default_subject(kind, anrede).to_owned())
    }

    /// The built-in subject for a kind — plain German, no marketing, because
    /// these are statutory notices and one of them threatens a disconnection.
    #[must_use]
    pub const fn default_subject(kind: &str, anrede: Anrede) -> &'static str {
        let du = matches!(anrede, Anrede::Du);
        match kind.as_bytes() {
            b"INVOICE" if du => "Deine Rechnung",
            b"INVOICE" => "Ihre Rechnung",
            b"MAHNUNG" => "Zahlungserinnerung",
            b"PREISANPASSUNG" if du => "Änderung deiner Preise",
            b"PREISANPASSUNG" => "Änderung Ihrer Preise",
            b"GEBUEHRENBESCHEID" => "Gebührenbescheid",
            b"VERTRAGSZUSAMMENFASSUNG" if du => "Zusammenfassung deines Vertrags",
            b"VERTRAGSZUSAMMENFASSUNG" => "Zusammenfassung Ihres Vertrags",
            b"VERTRAGSBESTAETIGUNG" if du => "Bestätigung deines Vertrags",
            b"VERTRAGSBESTAETIGUNG" => "Bestätigung Ihres Vertrags",
            b"WIDERRUFSBESTAETIGUNG" if du => "Eingangsbestätigung deines Widerrufs",
            b"WIDERRUFSBESTAETIGUNG" => "Eingangsbestätigung Ihres Widerrufs",
            _ if du => "Dein Dokument",
            _ => "Ihr Dokument",
        }
    }
}

impl mako_service::ServiceConfig for OutputdConfig {
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        Some(&self.database)
    }
    fn bind_addr(&self) -> String {
        self.http.addr.clone()
    }
}

fn default_http() -> mako_service::config::HttpConfig {
    mako_service::config::HttpConfig::on_port(9880)
}

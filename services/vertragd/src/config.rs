//! Deployment configuration for `vertragd`.

use mako_service::policy::{Policy, PolicyChoice};
use serde::Deserialize;

/// `vertragd.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VertragdConfig {
    /// PostgreSQL connection + pool tuning (`[database]` block).
    pub database: mako_service::config::DatabaseConfig,
    /// `[http]` — the listen address. Defaults to every interface on `9780`.
    #[serde(default = "default_http")]
    pub http: mako_service::config::HttpConfig,
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

    /// ERP webhook receiving the `de.mako.vertrag.*` CloudEvents.
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

    /// `productd` — the frozen Endkunden-Angebot an online order accepts
    /// (`GET /api/v1/endkunden-angebote/{id}`). Required once
    /// `[policy] vertragsschluss` is set: an order whose offer cannot be read
    /// cannot be checked against it.
    #[serde(default)]
    pub productd_url: Option<String>,
    #[serde(default)]
    pub productd_api_key: Option<String>,

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

    /// `[policy]` — the operator readings this deployment runs under.
    #[serde(default)]
    pub policy: VertragdPolicy,

    /// `[subscriptions]` — the webhook endpoints integrators register for the
    /// `de.mako.vertrag.*` CloudEvents; `erp_webhook_url` is the endpoint `default`.
    #[serde(default)]
    pub subscriptions: mako_service::subscriptions::SubscriptionsConfig,
}

/// `[policy]` — vertragd's operator settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VertragdPolicy {
    /// `[policy.dsr]` — the DPO's readings for a data-subject export.
    #[serde(default)]
    pub dsr: crate::dsr_policy::DsrPolicy,
    /// `vertragsschluss` — when an online order becomes a contract. No
    /// default: absent, `POST /api/v1/bestellungen` refuses with
    /// `policy_nicht_gesetzt` and every other route runs as before.
    #[serde(default)]
    pub vertragsschluss: Option<Policy<Vertragsschluss>>,
    /// `lieferbeginn_in_widerrufsfrist` — whether supply may start inside the
    /// Widerrufsfrist without the consumer's express request.
    #[serde(default)]
    pub lieferbeginn_in_widerrufsfrist: Policy<LieferbeginnInWiderrufsfrist>,
    /// `bestellbutton_beschriftungen` — the order-button labels admitted.
    /// § 312j Abs. 3 Satz 2 BGB: „mit nichts anderem als den Wörtern
    /// „zahlungspflichtig bestellen“ oder mit einer entsprechenden eindeutigen
    /// Formulierung" — the statutory wording alone by default; any other is the
    /// operator's reading, listed here.
    #[serde(default = "default_bestellbutton")]
    pub bestellbutton_beschriftungen: Vec<String>,
}

impl Default for VertragdPolicy {
    fn default() -> Self {
        Self {
            dsr: crate::dsr_policy::DsrPolicy::default(),
            vertragsschluss: None,
            lieferbeginn_in_widerrufsfrist: Policy::default(),
            bestellbutton_beschriftungen: default_bestellbutton(),
        }
    }
}

/// The statutory label of § 312j Abs. 3 Satz 2 BGB.
pub const BESTELLBUTTON_GESETZLICH: &str = "zahlungspflichtig bestellen";

fn default_bestellbutton() -> Vec<String> {
    vec![BESTELLBUTTON_GESETZLICH.to_owned()]
}

mako_service::policy_choice! {
    /// When an online order becomes a contract: at the order, or at the
    /// operator's acceptance. No default — the operator's AGB decide it.
    pub enum Vertragsschluss {
        key: "vertragsschluss",
        doc: "@/docs/services/vertragd.md#policy-vertragsschluss",
        default: required,
        options: {
            /// The frozen offer is binding; the consumer's click concludes the
            /// contract, and the Widerrufsfrist starts then.
            MitBestellung = "mit_bestellung" => "the operator's AGB make the offer binding, so the order concludes the contract and starts the Widerrufsfrist (§ 356 Abs. 2 Nr. 2 BGB)",
            /// The order is the consumer's offer; the contract comes about
            /// when the operator records its acceptance (after its own credit
            /// check).
            MitAnnahme = "mit_annahme" => "the operator's AGB make the order the consumer's offer, so the acceptance concludes the contract and starts the Widerrufsfrist (§ 356 Abs. 2 Nr. 2 BGB)",
        }
    }
}

mako_service::policy_choice! {
    /// Whether supply may start inside the Widerrufsfrist without the
    /// consumer's express request (§ 357a Abs. 2 Satz 1 Nr. 1 BGB).
    pub enum LieferbeginnInWiderrufsfrist {
        key: "lieferbeginn_in_widerrufsfrist",
        doc: "@/docs/services/vertragd.md#policy-vertragsschluss",
        default: tightening(NurAufVerlangen, "supplying inside the Widerrufsfrist only on the express request never delivers energy the consumer may withdraw from without owing Wertersatz"),
        options: {
            /// Without the express request, the earliest Lieferbeginn is the
            /// day after the Widerrufsfrist ends.
            NurAufVerlangen = "nur_auf_verlangen" => "Wertersatz is owed only for supply the consumer expressly asked to start inside the Widerrufsfrist (§ 357a Abs. 2 Satz 1 Nr. 1 BGB)",
            /// Supply may start inside the Widerrufsfrist in any case; energy
            /// delivered without the request is the operator's risk.
            Immer = "immer" => "an earlier start is not barred, but supply without the request owes no Wertersatz (§ 357a Abs. 2 Satz 1 Nr. 1 BGB)",
        }
    }
}

impl VertragdConfig {
    /// The policy and posture vertragd runs under — logged, exported, recorded
    /// and served by the runner and `GET /api/v1/policy`.
    #[must_use]
    pub fn policy_report(&self) -> mako_service::policy::PolicyReport {
        let mut r = mako_service::policy::PolicyReport::new();
        r.entries.extend(self.policy.dsr.entries());
        r.entries
            .push(self.policy.vertragsschluss.as_ref().map_or_else(
                || mako_service::policy::PolicyEntry {
                    key: <Vertragsschluss as PolicyChoice>::KEY.to_owned(),
                    value: "nicht_gesetzt".to_owned(),
                    origin: mako_service::policy::Origin::Default,
                    class: mako_service::policy::PolicyClass::Policy,
                    quelle: None,
                    doc: Some(<Vertragsschluss as PolicyChoice>::DOC.to_owned()),
                    insecure: false,
                    review_by: None,
                    review_grund: None,
                },
                Policy::entry,
            ));
        r.entries
            .push(self.policy.lieferbeginn_in_widerrufsfrist.entry());
        r.entries.push(mako_service::policy::PolicyEntry {
            key: "bestellbutton_beschriftungen".to_owned(),
            value: self.policy.bestellbutton_beschriftungen.join(" | "),
            origin: if self.policy.bestellbutton_beschriftungen == default_bestellbutton() {
                mako_service::policy::Origin::Default
            } else {
                mako_service::policy::Origin::Explicit
            },
            class: mako_service::policy::PolicyClass::Policy,
            quelle: Some(
                "§ 312j Abs. 3 Satz 2 BGB: „zahlungspflichtig bestellen“ oder eine entsprechende \
                 eindeutige Formulierung"
                    .to_owned(),
            ),
            doc: Some(<Vertragsschluss as PolicyChoice>::DOC.to_owned()),
            insecure: false,
            review_by: None,
            review_grund: None,
        });
        r.posture_flag("allow_insecure_no_auth", self.allow_insecure_no_auth)
            .posture_presence("oidc", self.oidc.is_some(), true)
            .posture_presence("inbound_secret", self.inbound_secret.is_some(), true)
            .posture_presence(
                "erp_hmac_secret",
                self.erp_hmac_secret.is_some(),
                self.erp_webhook_url.is_some(),
            )
    }

    const fn default_max_identitaeten() -> u32 {
        50
    }

    /// Refuse to start in a posture that exposes customer data or lets an
    /// unauthenticated caller move supply.
    ///
    /// The three mechanisms are checked together because each guards a
    /// different door: OIDC guards the operator API (GDPR export, IBAN writes,
    /// contract mutation), the inbound HMAC guards the two webhook routes
    /// (`de.mako.engine.process.*` outcomes and CPQ Angebote) that no token ever
    /// reaches, and the outbound HMAC is what lets the ERP tell a notice this
    /// service issued from one anybody posted.
    ///
    /// # Errors
    ///
    /// When either is unconfigured and `allow_insecure_no_auth` is not set.
    pub fn check_auth_posture(&self) -> anyhow::Result<()> {
        let fehlt: Vec<String> = self.auth_befunde().into_iter().map(|b| b.text).collect();
        if fehlt.is_empty() {
            return Ok(());
        }
        Err(anyhow::Error::msg(format!(
            "vertragd refuses to start: {}. Configure them, or set \
             allow_insecure_no_auth = true to accept an unauthenticated deployment.",
            fehlt.join("; ")
        )))
    }

    /// What `[policy] vertragsschluss` needs beside it: the order route reads
    /// the offer from `productd` and owes the Vertragszusammenfassung and the
    /// Vertragsbestätigung through `outputd`. An online contract concluded
    /// without either is one whose statutory documents are never issued.
    #[must_use]
    pub fn bestellung_befunde(&self) -> Vec<mako_service::Befund> {
        use mako_service::befund::Befund;
        if self.policy.vertragsschluss.is_none() {
            return Vec::new();
        }
        let mut out = Vec::new();
        if self.productd_url.is_none() {
            out.push(Befund::blockiert(
                "productd_url",
                "productd_url — [policy] vertragsschluss is set, and an order is checked \
                 against the frozen Endkunden-Angebot productd serves",
            ));
        }
        if self.outputd_url.is_none() {
            out.push(Befund::blockiert(
                "outputd_url",
                "outputd_url — [policy] vertragsschluss is set, and every online contract owes \
                 the § 41 Abs. 4 EnWG Vertragszusammenfassung and the § 312f Abs. 2 BGB \
                 Vertragsbestätigung, issued through outputd",
            ));
        }
        if self
            .absender
            .as_ref()
            .and_then(|a| a.name.as_ref())
            .is_none()
        {
            out.push(Befund::blockiert(
                "absender.name",
                "[absender] name — [policy] vertragsschluss is set, and the documents an online \
                 contract owes are Textform declarations that must name their declarant",
            ));
        }
        if self.policy.bestellbutton_beschriftungen.is_empty() {
            out.push(Befund::blockiert(
                "policy.bestellbutton_beschriftungen",
                "[policy] bestellbutton_beschriftungen is empty — every order would be refused",
            ));
        }
        out
    }

    /// The `[subscriptions]` findings: `allow_loopback`, the config endpoints.
    #[must_use]
    pub fn subscription_befunde(&self) -> Vec<mako_service::Befund> {
        self.subscriptions.befunde()
    }

    /// One finding per missing credential of [`Self::check_auth_posture`],
    /// each at its own key — none under `allow_insecure_no_auth`.
    pub fn auth_befunde(&self) -> Vec<mako_service::Befund> {
        use mako_service::befund::{Befund, Bereich};
        if self.allow_insecure_no_auth {
            tracing::warn!(
                "vertragd: allow_insecure_no_auth is set — requests are admitted with dev \
                 claims and inbound webhooks are accepted unsigned"
            );
            return Vec::new();
        }
        let mut out = Vec::new();
        if self.oidc.is_none() {
            out.push((
                "oidc",
                "[oidc] — without it every request is admitted with dev claims, GDPR \
                 customer data, IBANs and contract mutation included",
            ));
        }
        if self.inbound_secret.is_none() {
            out.push((
                "inbound_secret",
                "inbound_secret — without it POST /api/v1/events and \
                 POST /api/v1/webhooks/angebot accept any unsigned body, and a forged one \
                 confirms supply or creates a contract",
            ));
        }
        if self.erp_webhook_url.is_some() && self.erp_hmac_secret.is_none() {
            out.push((
                "erp_hmac_secret",
                "erp_hmac_secret — erp_webhook_url is set, and its webhook endpoint \
                 `default` signs only when it has a secret: every de.mako.vertrag.* CloudEvent, the § 41 Abs. 5 EnWG \
                 notices among them, goes to the ERP unsigned and the receiver cannot tell \
                 one from a forgery",
            ));
        }
        out.into_iter()
            .map(|(pfad, text)| Befund::blockiert(pfad, text).bereich(Bereich::Zugriff))
            .collect()
    }
}

impl mako_service::ServiceConfig for VertragdConfig {
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

/// The operator's own identity on a customer notice — § 126b BGB's declarant.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
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

fn default_http() -> mako_service::config::HttpConfig {
    mako_service::config::HttpConfig::on_port(9780)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal startable configuration, plus whatever `extra` adds.
    fn config(extra: &str) -> VertragdConfig {
        let base = r#"
            tenant       = "9900357000004"
            lf_mp_id     = "9900357000004"
            processd_url = "http://localhost:9080"
            accountingd_url = "http://localhost:9380"
            edmd_url     = "http://localhost:9280"
            inbound_secret = "s3cret"

            [database]
            url = "postgres://localhost/vertragd"

            [oidc]
            issuer   = "https://login.example.test/v2.0"
            audience = "api://mako-vertragd"
        "#;
        toml::from_str(&format!("{extra}\n{base}")).expect("the test configuration parses")
    }

    /// A delivery target without a signing secret is refused.
    ///
    /// The endpoint `default` signs only when it has one, so this posture ships every
    /// `de.mako.vertrag.*` CloudEvent — the § 41 Abs. 5 EnWG notices among them — to
    /// the ERP unsigned, and nothing at runtime says so.
    #[test]
    fn startup_refuses_a_webhook_url_without_its_signing_secret() {
        let err = config(r#"erp_webhook_url = "https://erp.example.test/events""#)
            .check_auth_posture()
            .expect_err("an unsigned outbound webhook must not start");
        assert!(
            err.to_string().contains("erp_hmac_secret"),
            "the refusal must name erp_hmac_secret: {err}"
        );
    }

    #[test]
    fn a_signed_webhook_and_no_webhook_at_all_both_start() {
        config(
            "erp_webhook_url = \"https://erp.example.test/events\"\n\
             erp_hmac_secret = \"signing-key\"",
        )
        .check_auth_posture()
        .expect("a URL with its secret is a startable posture");

        config("")
            .check_auth_posture()
            .expect("no erp_webhook_url means no unsigned delivery");
    }

    /// Every missing credential is its own finding, not the first.
    #[test]
    fn every_missing_credential_is_reported() {
        let mut cfg = config(r#"erp_webhook_url = "https://erp.example.test/events""#);
        cfg.oidc = None;
        let pfade: Vec<String> = cfg.auth_befunde().into_iter().map(|b| b.pfad).collect();
        assert_eq!(pfade, ["oidc", "erp_hmac_secret"]);
    }

    /// The escape hatch still opens every door, this one included.
    #[test]
    fn allow_insecure_no_auth_still_covers_it() {
        config(
            "allow_insecure_no_auth = true\n\
             erp_webhook_url = \"https://erp.example.test/events\"",
        )
        .check_auth_posture()
        .expect("allow_insecure_no_auth is the dev escape hatch");
    }
}

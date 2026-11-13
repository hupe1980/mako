//! `invoicd` configuration — loaded from `invoicd.toml` + `env:` substitution.
//!
//! # Minimal `invoicd.toml`
//!
//! ```toml
//! [http]
//! addr = "0.0.0.0:8280"
//!
//! [database]
//! url = "env:DATABASE_URL"
//!
//! [identity]
//! tenant     = "9900357000004"
//! marktrolle = "lieferant"   # lieferant (default) | netzbetreiber | esa
//!
//! [makod]
//! url     = "http://makod:8080"
//! api_key = "env:INVOICD_MAKOD_API_KEY"
//!
//! [marktd]
//! url     = "http://marktd:8180"
//! api_key = "env:INVOICD_MARKTD_API_KEY"
//!
//! [webhook]
//! inbound_secret = "env:INVOICD_INBOUND_SECRET"
//!
//! [subscription]
//! webhook_url   = "http://invoicd:8280/webhook"
//! subscriber_id = "invoicd"
//!
//! [check]
//! # All tolerances are relative (0.01 = 1 %)
//! arithmetic_tolerance = 0.01
//! total_tolerance      = 0.01
//! tariff_tolerance     = 0.03
//! require_tariff       = false
//! # EUR above which a Warn outcome triggers a Dispute (0.0 = never)
//! auto_dispute_threshold_eur = "0.00"
//!
//! # [oidc]
//! # issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
//! # audience = "api://mako-invoicd"
//! ```

use invoic_checker::CheckConfig;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub http: HttpConfig,
    /// Required for § 147 AO / § 14b UStG compliance — received INVOIC are
    /// Buchungsbelege, retained 8 years.
    pub database: DatabaseConfig,
    pub identity: IdentityConfig,
    pub makod: MakodConfig,
    pub marktd: MarktdConfig,
    #[serde(default)]
    pub webhook: WebhookConfig,
    #[serde(default)]
    pub subscription: SubscriptionConfig,
    #[serde(default)]
    pub check: CheckSectionConfig,
    /// Optional ERP webhook for outbound payment CloudEvents (A8).
    #[serde(default)]
    pub erp: ErpConfig,
    /// OIDC token verification for the HTTP API. Without it every request is
    /// admitted with synthetic dev-admin claims, which is a posture
    /// [`Self::check_auth_posture`] refuses to start in.
    #[serde(default)]
    pub oidc: Option<OidcConfig>,
    /// MCP server authentication. Supports OIDC + API-key fallback, or dev mode.
    /// See `[mcp]` in TOML — e.g. `api_key = "env:INVOICD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,

    /// Start without HTTP token verification and without inbound webhook
    /// signing (dev/test only).
    ///
    /// Both doors lead to the § 147 AO receipt store and to the answer this
    /// service sends a market partner, so the posture is asked for by name.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,
}

impl Config {
    /// Refuse to start in a posture that leaves the invoice register and the
    /// market answer open.
    ///
    /// The two mechanisms are checked together because each guards a different
    /// door into the same daemon: OIDC guards the REST API, and the inbound
    /// HMAC guards `POST /webhook`, the one route no bearer token ever reaches.
    ///
    /// # Errors
    ///
    /// When either is unconfigured and `allow_insecure_no_auth` is not set.
    pub fn check_auth_posture(&self) -> anyhow::Result<()> {
        if self.allow_insecure_no_auth {
            tracing::warn!(
                "invoicd: allow_insecure_no_auth is set — the § 147 AO receipt register, the \
                 Zahlungsstatus of every MaLo and the re-dispatch route are served to any \
                 caller, and POST /webhook accepts unsigned INVOIC events"
            );
            return Ok(());
        }
        let mut fehlt = Vec::new();
        if self.oidc.is_none() {
            fehlt.push(
                "[oidc] — without it every REST route is admitted with dev claims: the \
                 § 147 AO / § 14b UStG receipt register with its stored Rechnungen, the \
                 Zahlungsstatus of every MaLo, and the route that re-dispatches a \
                 REMADV/COMDIS answer to a market partner"
                    .to_owned(),
            );
        }
        if self.webhook.inbound_secret.is_none() {
            fehlt.push(
                "webhook.inbound_secret — without it POST /webhook accepts any unsigned body, \
                 and a forged INVOIC event writes a Buchungsbeleg and sends the market \
                 partner an answer in this operator's name"
                    .to_owned(),
            );
        }
        anyhow::ensure!(
            fehlt.is_empty(),
            "invoicd refuses to start: {}. Configure them, or set \
             allow_insecure_no_auth = true to accept an unauthenticated deployment.",
            fehlt.join("; ")
        );
        Ok(())
    }

    #[must_use]
    pub fn check_config(&self) -> CheckConfig {
        CheckConfig {
            arithmetic_tolerance_ppm: (self.check.arithmetic_tolerance * 1_000_000.0).round()
                as u32,
            total_tolerance_ppm: (self.check.total_tolerance * 1_000_000.0).round() as u32,
            tariff_tolerance_ppm: (self.check.tariff_tolerance * 1_000_000.0).round() as u32,
            require_tariff: self.check.require_tariff,
            max_zahlungsziel_days: self.check.max_zahlungsziel_days,
            // A fact of each invoice, set per check (`handler::check_config_for`).
            empfaenger_sparte: None,
        }
    }
}

impl mako_service::ServiceConfig for Config {
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        Some(&self.database)
    }
    fn bind_addr(&self) -> String {
        self.http.addr.clone()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    #[serde(default = "default_http_addr")]
    pub addr: String,
}

fn default_http_addr() -> String {
    "0.0.0.0:8280".to_owned()
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            addr: default_http_addr(),
        }
    }
}

/// PostgreSQL config — shared struct from `mako-service`.
pub use mako_service::config::DatabaseConfig;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityConfig {
    /// Tenant identifier — data-isolation key written to every receipt row.
    /// Typically the operator's BDEW- or DVGW-Codenummer, but any stable
    /// unique string is valid.
    pub tenant: String,
    /// The Marktrolle this deployment **receives** invoices in.
    ///
    /// PID 31009 carries five Use-Cases and the message body names none of
    /// them: BDEW Allgemeine Festlegungen Kap. 2.13 gives every Marktrolle its own
    /// MP-ID, so the recipient's role is half of the answer (`IMD+7081` is the
    /// other half). Without it a Preisblatt-B invoice from an MSB cannot be
    /// told from an NB's — `E_0270` and `E_0273` publish the same codes under
    /// different numbers, and an answer naming the wrong one is undefined at
    /// the counterparty.
    ///
    /// Defaults to [`EmpfaengerRolle::Lieferant`].
    #[serde(default)]
    pub marktrolle: EmpfaengerRolle,
}

/// The Marktrolle an `invoicd` deployment receives invoices in.
///
/// Deliberately narrower than `mako_fristen::vorlauf::RechnungEmpfaenger`,
/// which folds LF and MSB into one arm because they share an answer window.
/// They do **not** share a Preisblatt-B tree, so the split matters here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmpfaengerRolle {
    /// Lieferant — `E_0210` (Messstellenbetrieb) resp. `E_0270` (Preisblatt B).
    #[default]
    Lieferant,
    /// Netzbetreiber — `E_0566` resp. `E_0273`.
    Netzbetreiber,
    /// Energieserviceanbieter — `E_0264`, whichever the Gegenstand.
    Esa,
}

impl EmpfaengerRolle {
    /// The answer-window arm this role falls in.
    #[must_use]
    pub const fn rechnung_empfaenger(self) -> mako_fristen::vorlauf::RechnungEmpfaenger {
        use mako_fristen::vorlauf::RechnungEmpfaenger as R;
        match self {
            Self::Lieferant => R::LieferantOderMsb,
            Self::Netzbetreiber => R::Netzbetreiber,
            Self::Esa => R::Esa,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakodConfig {
    /// `makod` base URL.  Example: `http://makod:8080`
    pub url: String,
    /// API key for the `makod` command endpoint.  Use `"env:INVOICD_MAKOD_API_KEY"`.
    pub api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarktdConfig {
    /// `marktd` base URL.  Example: `http://marktd:8180`
    pub url: String,
    /// Bearer token.  Use `"env:INVOICD_MARKTD_API_KEY"`.
    pub api_key: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    /// HMAC-SHA256 secret for verifying inbound webhooks from `marktd`.
    /// Use `"env:INVOICD_INBOUND_SECRET"`.
    pub inbound_secret: Option<String>,
}

/// The `marktd` subscription registered at startup.
///
/// The event type and the PID filter are deliberately not configurable:
/// `invoicd` acts on `de.mako.process.initiated` as
/// [`crate::routing::disposition_for`] decides and nothing else, so a key offering to change
/// either would promise a behaviour the service does not have.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionConfig {
    /// URL that `marktd` will POST events to.
    pub webhook_url: String,
    #[serde(default = "default_subscriber_id")]
    pub subscriber_id: String,
}

fn default_subscriber_id() -> String {
    "invoicd".to_owned()
}

impl Default for SubscriptionConfig {
    fn default() -> Self {
        Self {
            webhook_url: "http://invoicd:8280/webhook".to_owned(),
            subscriber_id: default_subscriber_id(),
        }
    }
}

/// `invoic-checker` plausibility tolerances and dispute policy.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckSectionConfig {
    /// Per-line arithmetic tolerance as a fraction (0.01 = 1 %).
    /// Converted to ppm when building `CheckConfig`.
    #[serde(default = "default_arithmetic_tolerance")]
    pub arithmetic_tolerance: f64,
    /// Document-total tolerance as a fraction (0.01 = 1 %).
    #[serde(default = "default_total_tolerance")]
    pub total_tolerance: f64,
    /// Tariff unit-price tolerance as a fraction (0.03 = 3 %).
    #[serde(default = "default_tariff_tolerance")]
    pub tariff_tolerance: f64,
    /// When `true`, a missing PRICAT entry escalates from `Warn` to `Dispute`.
    #[serde(default)]
    pub require_tariff: bool,
    /// INVOIC net amount in EUR, a string (`"250.00"`), above which a `Warn`
    /// outcome becomes a `Dispute`. `"0"` (default) means `Warn` is always
    /// auto-approved.
    #[serde(default)]
    pub auto_dispute_threshold_eur: rust_decimal::Decimal,
    /// The longest Zahlungsziel (payment term) this operator accepts, in
    /// calendar days from `rechnungsdatum` to `faelligkeitsdatum`
    /// (`SG8 DTM+265`); a longer one is a `Warn`. Unset (the default) checks
    /// no ceiling: no Festlegung publishes one for an invoice the recipient
    /// pays. The published **floor** — 10 Werktage after receipt — is checked
    /// regardless.
    #[serde(default)]
    pub max_zahlungsziel_days: Option<u16>,
}

fn default_arithmetic_tolerance() -> f64 {
    0.01
}
fn default_total_tolerance() -> f64 {
    0.01
}
fn default_tariff_tolerance() -> f64 {
    0.03
}

impl CheckSectionConfig {
    /// Threshold in `Amount<5>` raw units (10⁻⁵ EUR) — the resolution of
    /// `CheckReport::total_net_invoic.to_raw()` — converted exactly from the
    /// configured EUR amount.
    ///
    /// # Errors
    ///
    /// A negative threshold, or one finer than 10⁻⁵ EUR, which the comparison
    /// could only round.
    pub fn auto_dispute_threshold_raw(&self) -> anyhow::Result<i64> {
        let eur = self.auto_dispute_threshold_eur;
        anyhow::ensure!(
            eur >= rust_decimal::Decimal::ZERO,
            "check.auto_dispute_threshold_eur {eur} is negative"
        );
        invoic_checker::EuroAmount::from_decimal(eur)
            .map(invoic_checker::EuroAmount::to_raw)
            .map_err(|_| {
                anyhow::anyhow!(
                    "check.auto_dispute_threshold_eur {eur} is not an EUR amount of at most \
                     five decimal places"
                )
            })
    }
}

impl Default for CheckSectionConfig {
    fn default() -> Self {
        Self {
            arithmetic_tolerance: default_arithmetic_tolerance(),
            total_tolerance: default_total_tolerance(),
            tariff_tolerance: default_tariff_tolerance(),
            require_tariff: false,
            auto_dispute_threshold_eur: rust_decimal::Decimal::ZERO,
            max_zahlungsziel_days: None,
        }
    }
}

/// OIDC configuration — re-exported from `mako-service` (shared across all daemons).
pub use mako_service::oidc::OidcConfig;

/// ERP integration config — outbound payment CloudEvents.
///
/// When `webhook_url` is set, `invoicd` POSTs a `de.invoic.receipt.settled`
/// or `de.invoic.receipt.disputed` CloudEvent after each validated INVOIC.
///
/// Example TOML:
/// ```toml
/// [erp]
/// webhook_url = "https://erp.example.com/webhooks/invoicd"
/// hmac_secret = "env:INVOICD_ERP_HMAC_SECRET"
/// ```
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ErpConfig {
    /// ERP webhook URL for `de.invoic.receipt.*` CloudEvents.
    pub webhook_url: Option<String>,
    /// Optional secret for Standard Webhooks signing on outbound requests.
    /// Use `"env:INVOICD_ERP_HMAC_SECRET"` to load from environment.
    pub hmac_secret: Option<String>,
}

/// `env:VARNAME` indirection — re-exported from `mako-service`, so every daemon
/// resolves a config reference the same way and reports a missing variable with
/// the same error.
pub use mako_service::config::{resolve_env, resolve_env_secret};

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_threshold(eur: rust_decimal::Decimal) -> CheckSectionConfig {
        CheckSectionConfig {
            auto_dispute_threshold_eur: eur,
            ..CheckSectionConfig::default()
        }
    }

    /// The threshold and `EuroAmount::to_raw()` share one unit (10⁻⁵ EUR) and
    /// compare exactly: an invoice total at the threshold does not escalate,
    /// one raw unit above does.
    #[test]
    fn threshold_matches_euro_amount_raw_units() {
        let threshold_raw = config_with_threshold(rust_decimal::dec!(0.30))
            .auto_dispute_threshold_raw()
            .expect("a representable threshold");
        assert_eq!(threshold_raw, 30_000);
        let raw = |d| {
            invoic_checker::EuroAmount::from_decimal_rounded(
                d,
                invoic_checker::amount::Rounding::HalfAwayFromZero,
            )
            .unwrap()
            .to_raw()
        };
        // 0,10 + 0,20 EUR is 0,30 EUR exactly, so it sits at the threshold.
        assert_eq!(
            raw(rust_decimal::dec!(0.10) + rust_decimal::dec!(0.20)),
            threshold_raw
        );
        assert!(
            raw(rust_decimal::dec!(0.30001)) > threshold_raw,
            "one raw unit above"
        );
    }

    /// The TOML value is read as a decimal string, never through a float.
    #[test]
    fn the_threshold_is_read_as_a_decimal_string() {
        let cfg: CheckSectionConfig =
            serde_json::from_str(r#"{"auto_dispute_threshold_eur": "250.00"}"#).unwrap();
        assert_eq!(cfg.auto_dispute_threshold_raw().unwrap(), 25_000_000);
        assert!(
            config_with_threshold(rust_decimal::dec!(-1))
                .auto_dispute_threshold_raw()
                .is_err()
        );
        assert!(
            config_with_threshold(rust_decimal::dec!(0.000001))
                .auto_dispute_threshold_raw()
                .is_err(),
            "finer than 10⁻⁵ EUR"
        );
    }
}

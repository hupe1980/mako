//! Configuration for `invoicd`.

use clap::Parser;
use invoic_checker::CheckConfig;
use secrecy::SecretString;

/// INVOIC plausibility-check daemon.
#[derive(Debug, Parser)]
#[command(name = "invoicd", about, version)]
pub struct Config {
    /// Bind address for the HTTP server.
    #[arg(
        long = "listen",
        env = "INVOICD_LISTEN",
        default_value = "0.0.0.0:8280"
    )]
    pub listen: String,

    /// Base URL of the `makod` command API.
    #[arg(
        long = "makod-url",
        env = "INVOICD_MAKOD_URL",
        default_value = "http://localhost:8180"
    )]
    pub makod_url: String,

    /// Base URL of the `mdmd` subscription API.
    #[arg(
        long = "mdmd-url",
        env = "INVOICD_MDMD_URL",
        default_value = "http://localhost:9180"
    )]
    pub mdmd_url: String,

    /// CloudEvents subscriber ID registered with `mdmd`.
    #[arg(
        long = "subscriber-id",
        env = "INVOICD_SUBSCRIBER_ID",
        default_value = "invoicd"
    )]
    pub subscriber_id: String,

    /// Public webhook URL that `mdmd` will POST events to.
    #[arg(long = "webhook-url", env = "INVOICD_WEBHOOK_URL")]
    pub webhook_url: String,

    /// HMAC-SHA256 secret that `mdmd` will use when signing outbound payloads.
    ///
    /// When set, `invoicd` registers this secret with `mdmd` and verifies
    /// the `X-Mdm-Signature` header on every inbound webhook request.
    #[arg(long = "webhook-secret", env = "INVOICD_WEBHOOK_SECRET")]
    pub webhook_secret: Option<SecretString>,

    /// HMAC secret for verifying inbound `X-Mdm-Signature` headers.
    ///
    /// Defaults to `--webhook-secret` when not set explicitly.
    #[arg(long = "inbound-secret", env = "INVOICD_INBOUND_SECRET")]
    pub inbound_secret: Option<SecretString>,

    /// Relative tolerance for per-line arithmetic checks (0.01 = 1 %).
    #[arg(
        long = "arithmetic-tolerance",
        env = "INVOICD_ARITHMETIC_TOLERANCE",
        default_value_t = 0.01
    )]
    pub arithmetic_tolerance: f64,

    /// Relative tolerance for total-amount checks (0.01 = 1 %).
    #[arg(
        long = "total-tolerance",
        env = "INVOICD_TOTAL_TOLERANCE",
        default_value_t = 0.01
    )]
    pub total_tolerance: f64,

    /// Relative tolerance for tariff unit-price checks (0.03 = 3 %).
    #[arg(
        long = "tariff-tolerance",
        env = "INVOICD_TARIFF_TOLERANCE",
        default_value_t = 0.03
    )]
    pub tariff_tolerance: f64,

    /// Require a tariff entry; a missing tariff escalates to `Dispute`.
    ///
    /// Default `false`: missing tariffs generate a `Warn` finding, not a dispute.
    #[arg(
        long = "require-tariff",
        env = "INVOICD_REQUIRE_TARIFF",
        default_value_t = false
    )]
    pub require_tariff: bool,

    /// INVOIC net-amount (EUR) above which a `Warn` outcome triggers a dispute
    /// instead of automatic approval.
    ///
    /// `0.0` (default) means `Warn` outcomes are always approved automatically.
    #[arg(
        long = "auto-dispute-threshold",
        env = "INVOICD_AUTO_DISPUTE_THRESHOLD",
        default_value_t = 0.0_f64
    )]
    pub auto_dispute_threshold_eur: f64,
}

impl Config {
    /// Build a [`CheckConfig`] from the relevant tolerance fields.
    #[must_use]
    pub fn check_config(&self) -> CheckConfig {
        CheckConfig {
            arithmetic_tolerance: self.arithmetic_tolerance,
            total_tolerance: self.total_tolerance,
            tariff_tolerance: self.tariff_tolerance,
            require_tariff: self.require_tariff,
        }
    }

    /// Return the effective inbound secret (falls back to `--webhook-secret`).
    #[must_use]
    pub fn effective_inbound_secret(&self) -> Option<&SecretString> {
        self.inbound_secret
            .as_ref()
            .or(self.webhook_secret.as_ref())
    }
}

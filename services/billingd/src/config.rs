//! Configuration for `billingd`.

use rust_decimal::Decimal;
use serde::Deserialize;

/// Platform-level statutory rate configuration.
/// Configure under `[rates]` in `billingd.toml`.
/// Update annually as BNetzA / BMWK publish new levies.
#[derive(Debug, Deserialize)]
pub struct RatesConfig {
    /// Stromsteuer §3 StromStG — ct/kWh (default 2.05, valid since 01.04.2003).
    pub stromsteuer_ct_per_kwh: Option<Decimal>,
    /// Energiesteuer Erdgas §2 Nr. 3 EnergieStG — ct/kWh_Hs (default 0.55).
    pub energiesteuer_gas_ct_per_kwh: Option<Decimal>,
    /// CO₂-Abgabe BEHG Erdgas — ct/kWh_Hs (default 1.3104 = 65 EUR/t × 0.20160 kg/kWh ÷ 10, 2026).
    /// From 2026 the nEHS price is set by auction inside the §10 BEHG corridor;
    /// configure the operator's actual procurement cost here.
    pub behg_gas_ct_per_kwh: Option<Decimal>,
    /// CO₂ conversion factor (kg CO₂/kWh_Hs) used when deriving the BEHG
    /// component from the nEHS market-price series (EUR/t → ct/kWh).
    ///
    /// Defaults to the H-Gas factor 0.20160 (DVGW G 685). An L-Gas deployment
    /// (primarily NW Germany) configures 0.20140 here
    /// (`energy_billing::BEHG_CO2_FACTOR_L_GAS`). Only relevant when no
    /// explicit `behg_gas_ct_per_kwh` override is set — an explicit ct/kWh
    /// override bypasses the market-price derivation entirely.
    pub behg_co2_factor_kg_per_kwh: Option<Decimal>,
    /// MwSt rate as decimal fraction (default 0.19).
    pub mwst_rate: Option<Decimal>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct BillingdConfig {
    /// `[database]` block — connection URL plus pool tuning. The daemon runner
    /// connects a tuned pool (with `application_name = "billingd"`) from this.
    pub database: mako_service::config::DatabaseConfig,

    /// HTTP listen port.  Defaults to `9280`.
    pub port: Option<u16>,

    /// Tenant identifier — data-isolation key written to every database row.
    /// Typically the operator's BDEW- or DVGW-Codenummer, but any stable unique string is valid.
    pub tenant: String,

    /// `tarifbd` base URL — product catalog and EPEX prices.
    pub tarifbd_url: String,

    /// `edmd` base URL — `MeterBillingPeriod` for consumption data.
    pub edmd_url: String,

    /// `edmd` bearer token.
    pub edmd_api_key: Option<String>,

    /// `marktd` base URL — `PreisblattNetznutzung` + `PreisblattKonzessionsabgabe`.
    pub marktd_url: String,

    /// `marktd` bearer token.
    pub marktd_api_key: Option<String>,

    /// `vertragd` base URL — Rahmenvertrag + MaLo enumeration for Sammelrechnung (L2).
    pub vertragd_url: Option<String>,

    /// ERP webhook URL — receives `de.billing.rechnung.erstellt` CloudEvents.
    pub erp_webhook_url: Option<String>,

    /// HMAC-SHA256 secret for signing outbound webhooks.
    pub erp_hmac_secret: Option<String>,

    /// Seller name for XRechnung generation (BG-4, BT-27). Defaults to tenant ID.
    pub seller_name: Option<String>,

    /// Seller VAT registration number (Umsatzsteuer-ID) for XRechnung output.
    pub seller_vat_id: Option<String>,

    /// §40 Abs. 2 Nr. 1 EnWG — supplier postal address as shown on invoices.
    pub seller_address: Option<String>,

    /// §40 Abs. 2 Nr. 8 EnWG — annual consumption of the comparable customer
    /// group in kWh/a (e.g. Stromspiegel reference value for the operator's
    /// dominant customer segment). Pro-rated to each billing period. When
    /// unset, the comparison-group line is omitted from invoices.
    pub vergleichsgruppe_kwh_pro_jahr: Option<Decimal>,

    /// Label for the comparable customer group, e.g. `"2-Personen-Haushalt"`.
    pub vergleichsgruppe_label: Option<String>,

    /// §40 Abs. 2 Nr. 1 EnWG — customer-service contact (hotline / e-mail)
    /// as shown on invoices.
    pub seller_contact: Option<String>,

    /// Seller payment account IBAN (BT-84) for the XRechnung BG-16 SEPA credit
    /// transfer. Required for a `BR-DE-1`-conformant B2G document.
    pub seller_iban: Option<String>,
    /// Seller bank BIC (BT-86), optional.
    pub seller_bic: Option<String>,

    /// Statutory rate defaults.  Override here instead of per-product.
    pub rates: Option<RatesConfig>,

    /// MCP server authentication. Supports API-key, OIDC, or dev mode.
    /// See `[mcp]` section in TOML — e.g. `api_key = "env:BILLINGD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,

    /// OIDC token verification for the HTTP API.  When omitted, every request
    /// is accepted with synthetic dev-admin claims — `main` refuses to start
    /// in that state unless [`Self::allow_insecure_no_auth`] is set.
    #[serde(default)]
    pub oidc: Option<mako_service::oidc::OidcConfig>,

    /// Start without HTTP token verification (dev/test only).
    ///
    /// Without `[oidc]` every billing endpoint — calculate, correction,
    /// VPP contract mutation — is open to anyone who can reach the port.
    /// That posture must be asked for by name.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,

    /// §40b EnWG scheduled billing runs. When omitted or `enabled = false`,
    /// billing stays on-demand via `POST …/calculate`.
    #[serde(default)]
    pub billing_runs: BillingRunsConfig,

    /// Deterministic invoice risk scoring and the HELD dispatch gate.
    /// See `[risk]` — `crate::risk::RiskConfig` for bands and thresholds.
    #[serde(default)]
    pub risk: crate::risk::RiskConfig,

    /// Shared secret for verifying inbound webhook HMAC-SHA256 signatures.
    ///
    /// When set, `POST /api/v1/webhooks/vpp-dispatch` (and future inbound webhook
    /// endpoints) validate the `X-Mako-Signature: sha256=<hex>` header.
    /// When absent, signature verification is disabled (dev mode).
    pub inbound_webhook_secret: Option<String>,

    /// Enable automatic VPP settlement billing triggered by
    /// `de.vpp.dispatch.confirmed` CloudEvents on `POST /api/v1/webhooks/vpp-dispatch`.
    ///
    /// When `false` (default), the webhook endpoint still accepts events but
    /// returns `202 Accepted` without triggering billing.  The `POST
    /// /api/v1/billing/vpp/{vpp_id}` endpoint remains available for manual
    /// settlement in all configurations.
    #[serde(default)]
    pub vpp_auto_billing: bool,
}

impl mako_service::ServiceConfig for BillingdConfig {
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        Some(&self.database)
    }
    fn bind_addr(&self) -> String {
        format!("0.0.0.0:{}", self.port.unwrap_or(9280))
    }
}

/// §40b EnWG billing-run worker configuration (`[billing_runs]`).
///
/// The worker sweeps once per day after `run_hour_utc`: it pulls the active
/// contracts from vertragd, computes each contract's most recently completed
/// billing period from its `abrechnungszyklus`, bills every period that has
/// no invoice yet, and accumulates the month's `billing_run_log` row. For
/// iMSys MaLos it additionally delivers the free monthly
/// Abrechnungsinformation (§40b Abs. 2 EnWG) as a CloudEvent.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillingRunsConfig {
    /// Whether the scheduled billing worker is active. Default: false.
    #[serde(default)]
    pub enabled: bool,
    /// UTC hour (0–23) after which the daily sweep runs. Default: 4.
    #[serde(default = "default_billing_run_hour")]
    pub run_hour_utc: u8,
    /// Emit the §40b Abs. 2 monthly Abrechnungsinformation for iMSys MaLos.
    /// Default: true (only effective while `enabled`).
    #[serde(default = "default_true")]
    pub abrechnungsinformation: bool,
}

fn default_billing_run_hour() -> u8 {
    4
}
fn default_true() -> bool {
    true
}

impl Default for BillingRunsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            run_hour_utc: default_billing_run_hour(),
            abrechnungsinformation: true,
        }
    }
}

impl BillingdConfig {
    /// Build `RegulatoryRates` from config, falling back to statutory defaults.
    pub fn regulatory_rates(&self) -> energy_billing::RegulatoryRates {
        use rust_decimal::dec;
        let r = self.rates.as_ref();
        energy_billing::RegulatoryRates {
            stromsteuer_ct_per_kwh: r
                .and_then(|r| r.stromsteuer_ct_per_kwh)
                .unwrap_or(dec!(2.05)),
            energiesteuer_gas_ct_per_kwh: r
                .and_then(|r| r.energiesteuer_gas_ct_per_kwh)
                .unwrap_or(dec!(0.55)),
            behg_gas_ct_per_kwh: r
                .and_then(|r| r.behg_gas_ct_per_kwh)
                .unwrap_or(dec!(1.3104)),
            mwst_rate: r.and_then(|r| r.mwst_rate).unwrap_or(dec!(0.19)),
        }
    }
}

impl BillingdConfig {
    /// Regulatory rates for a billing period and commodity, not for today.
    ///
    /// A correction re-opens an old period, and that period is billed under its
    /// own rates: 2021 BEHG was 25 EUR/t, the second half of 2020 had 16 % VAT,
    /// and gas/Fernwärme carried **7 % USt from 01.10.2022 to 31.03.2024**
    /// (§28 Abs. 5/6 UStG) — which is why the product `category` is part of the
    /// lookup: the VAT history of gas differs from electricity.
    ///
    /// An explicitly configured rate still wins — configuration is the operator
    /// saying "I know better" — but the *defaults* come from the year tables.
    ///
    /// A period straddling a statutory rate boundary has **no** correct single
    /// rate, so [`Self::regulatory_rates_for_period`] refuses it rather than
    /// choosing one. See [`Self::steuer_stichtage`] for the split dates.
    /// Explicitly configured BEHG override (ct/kWh), when the operator pinned
    /// one in `[rates]`. An explicit override always wins over the nEHS
    /// market-price series.
    pub fn behg_override(&self) -> Option<rust_decimal::Decimal> {
        self.rates.as_ref().and_then(|r| r.behg_gas_ct_per_kwh)
    }

    /// Configured CO₂ conversion factor (kg CO₂/kWh_Hs) for the EUR/t → ct/kWh
    /// derivation from the nEHS market-price series. `None` = H-Gas default
    /// (0.20160, DVGW G 685); an L-Gas deployment configures 0.20140.
    pub fn behg_co2_factor(&self) -> Option<rust_decimal::Decimal> {
        self.rates
            .as_ref()
            .and_then(|r| r.behg_co2_factor_kg_per_kwh)
    }

    /// The statutory rate boundaries inside a period, if any.
    ///
    /// Empty means one set of rates governs the whole period. Otherwise each
    /// date is the first day of a new regime, and the period must be split
    /// there and billed in parts.
    #[must_use]
    pub fn steuer_stichtage(
        &self,
        category: &str,
        period_from: time::Date,
        period_to: time::Date,
    ) -> Vec<time::Date> {
        // An operator who pinned an explicit VAT rate has taken the decision
        // themselves; splitting would contradict it.
        if self.rates.as_ref().and_then(|r| r.mwst_rate).is_some() {
            return Vec::new();
        }
        energy_billing::steuer_stichtage_im_zeitraum(category, period_from, period_to)
    }

    /// Resolve the statutory rates for a period.
    ///
    /// # Errors
    ///
    /// Returns the split dates when the period straddles a statutory boundary.
    /// Silently picking one rate would bill part of the period wrong — for a
    /// gas period crossing 31.03.2024 that is the whole period at 19 % where
    /// the earlier portion was legally 7 %, a customer overcharge that reads
    /// exactly like a correct invoice downstream.
    pub fn try_regulatory_rates_for_period(
        &self,
        category: &str,
        period_from: time::Date,
        period_to: time::Date,
    ) -> Result<energy_billing::RegulatoryRates, StraddlesRateBoundary> {
        let stichtage = self.steuer_stichtage(category, period_from, period_to);
        if !stichtage.is_empty() {
            return Err(StraddlesRateBoundary {
                category: category.to_owned(),
                period_from,
                period_to,
                stichtage,
            });
        }
        Ok(self.regulatory_rates_for_period(category, period_from, period_to))
    }

    /// Resolve the statutory rates for a period, assuming it is uniform.
    ///
    /// Prefer [`Self::try_regulatory_rates_for_period`] on any path that bills:
    /// this one falls back to the configured default where a period straddles a
    /// boundary, which is only safe for previews and estimates.
    pub fn regulatory_rates_for_period(
        &self,
        category: &str,
        period_from: time::Date,
        period_to: time::Date,
    ) -> energy_billing::RegulatoryRates {
        let year = period_from.year();
        let configured = self.rates.as_ref();
        let defaults = self.regulatory_rates();
        energy_billing::RegulatoryRates {
            stromsteuer_ct_per_kwh: configured
                .and_then(|r| r.stromsteuer_ct_per_kwh)
                .or_else(|| energy_billing::stromsteuer_for_year(year))
                .unwrap_or(defaults.stromsteuer_ct_per_kwh),
            energiesteuer_gas_ct_per_kwh: configured
                .and_then(|r| r.energiesteuer_gas_ct_per_kwh)
                .or_else(|| energy_billing::energiesteuer_gas_for_year(year))
                .unwrap_or(defaults.energiesteuer_gas_ct_per_kwh),
            behg_gas_ct_per_kwh: configured
                .and_then(|r| r.behg_gas_ct_per_kwh)
                .or_else(|| energy_billing::behg_ct_per_kwh_for_year(year))
                .unwrap_or(defaults.behg_gas_ct_per_kwh),
            mwst_rate: configured
                .and_then(|r| r.mwst_rate)
                .or_else(|| {
                    if matches!(category, "GAS" | "WAERME") {
                        energy_billing::mwst_rate_for_gas_waerme_period(period_from, period_to)
                    } else {
                        energy_billing::mwst_rate_for_period(period_from, period_to)
                    }
                })
                .unwrap_or(defaults.mwst_rate),
        }
    }
}

/// A billing period crosses a statutory rate boundary and cannot be billed whole.
///
/// Carries the split dates so the caller can act on it rather than being told
/// only that something is wrong.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "Abrechnungszeitraum {period_from}..{period_to} ({category}) überschreitet eine \
     gesetzliche Satzgrenze: kein einzelner Steuersatz ist für den gesamten Zeitraum \
     korrekt. Am Stichtag splitten und die Teilzeiträume jeweils mit ihrem eigenen \
     Satz abrechnen. Stichtage: {stichtage:?}"
)]
pub struct StraddlesRateBoundary {
    pub category: String,
    pub period_from: time::Date,
    pub period_to: time::Date,
    /// First day of each new regime inside the period.
    pub stichtage: Vec<time::Date>,
}

#[cfg(test)]
mod straddle_tests {
    use super::*;
    use time::macros::date;

    /// Build a config from JSON — the struct is `Deserialize`, and this avoids
    /// a TOML dev-dependency for what is a two-field fixture.
    fn cfg(pinned_mwst: Option<&str>) -> BillingdConfig {
        let mut v = serde_json::json!({
            "database": { "url": "postgres://localhost/x" },
            "tenant": "9900357000004",
            "tarifbd_url": "http://localhost:9080",
            "edmd_url": "http://localhost:8380",
            "marktd_url": "http://localhost:8080"
        });
        if let Some(rate) = pinned_mwst {
            v["rates"] = serde_json::json!({ "mwst_rate": rate });
        }
        serde_json::from_value(v).expect("config parses")
    }

    /// The defect this refusal exists to prevent.
    ///
    /// Gas carried 7 % USt until 31.03.2024 and 19 % after (§28 Abs. 5/6 UStG).
    /// A March–April period has no correct single rate, and the old resolver
    /// answered the `None` with the 19 % default — billing the March portion,
    /// legally 7 %, at 19 %. That reads exactly like a correct invoice
    /// downstream, so nothing else would have caught it.
    #[test]
    fn a_gas_period_crossing_the_vat_window_is_refused_with_its_stichtag() {
        let c = cfg(None);
        let err = c
            .try_regulatory_rates_for_period("GAS", date!(2024 - 03 - 01), date!(2024 - 04 - 30))
            .expect_err("a straddling gas period must be refused");
        assert_eq!(err.stichtage, vec![date!(2024 - 04 - 01)]);
        assert!(
            err.to_string().contains("splitten"),
            "the refusal must say what to do: {err}"
        );
    }

    /// Electricity never had the 7 % window, so the same period bills whole.
    #[test]
    fn the_same_period_is_fine_for_electricity() {
        let c = cfg(None);
        assert!(
            c.try_regulatory_rates_for_period(
                "STROM",
                date!(2024 - 03 - 01),
                date!(2024 - 04 - 30)
            )
            .is_ok()
        );
    }

    /// §10 BEHG steps at each calendar-year boundary, so a year-crossing gas
    /// period carries two levy rates and is refused too.
    #[test]
    fn a_year_crossing_gas_period_is_refused() {
        let c = cfg(None);
        let err = c
            .try_regulatory_rates_for_period("GAS", date!(2023 - 12 - 01), date!(2024 - 01 - 31))
            .expect_err("the BEHG year boundary must be refused");
        assert!(err.stichtage.contains(&date!(2024 - 01 - 01)), "{err:?}");
    }

    /// An operator who pinned an explicit rate has taken the decision; splitting
    /// would contradict it, so the refusal stands down.
    #[test]
    fn an_explicitly_configured_rate_wins_over_the_split() {
        let c = cfg(Some("0.19"));
        let rates = c
            .try_regulatory_rates_for_period("GAS", date!(2024 - 03 - 01), date!(2024 - 04 - 30))
            .expect("a pinned rate suppresses the refusal");
        assert_eq!(rates.mwst_rate, rust_decimal::dec!(0.19));
    }

    /// A uniform period resolves normally.
    #[test]
    fn a_uniform_gas_period_resolves() {
        let c = cfg(None);
        assert!(
            c.try_regulatory_rates_for_period("GAS", date!(2026 - 05 - 01), date!(2026 - 05 - 31))
                .is_ok()
        );
    }
}

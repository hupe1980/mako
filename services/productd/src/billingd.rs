//! The 2027 BEHG regime, read from `billingd`.
//!
//! Which law prices nEHS certificates in a year § 10 BEHG fixes no price for
//! is one operator reading, `billingd`'s `[policy] behg_regime`. A quotation
//! is an offer of the price the invoice will later bill, so productd holds no
//! copy of that reading: a second key could say `korridor` while billingd
//! bills under `rvo_bezugspreis`, and the customer would sign for a CO₂ price
//! the invoice refuses. productd asks billingd's `GET /api/v1/policy` (Cedar
//! `read-policy`) for each quotation whose Stichtag year needs the regime.
//!
//! A billingd that is not configured, not reachable or not reporting the
//! setting is no answer: the Gas position is refused `BEHG_PREIS_FEHLT`, and
//! no default regime stands in.

use energy_billing::BehgRegime;
use mako_service::http::Upstream;
use mako_service::policy::PolicyReport;

/// The policy key billingd reports its regime under.
const BEHG_REGIME_KEY: &str = "behg_regime";

/// Where the regime comes from: billingd, or nowhere.
#[derive(Debug, Clone)]
pub struct BehgRegimeQuelle {
    billingd: Option<Upstream>,
}

impl BehgRegimeQuelle {
    /// Ask the billingd at `base_url`, presenting `api_key` as its service
    /// credential (an `[[oidc.service_keys]]` entry there).
    #[must_use]
    pub fn billingd(base_url: &str, api_key: Option<String>, http: reqwest::Client) -> Self {
        Self {
            billingd: Some(Upstream::new(
                "billingd",
                base_url,
                api_key.map(secrecy::SecretString::from),
                http,
            )),
        }
    }

    /// No billingd is configured: every year that needs the regime is refused.
    #[must_use]
    pub const fn keine() -> Self {
        Self { billingd: None }
    }

    /// billingd's effective `[policy] behg_regime`.
    ///
    /// # Errors
    ///
    /// Why there is no regime — no `billingd_url`, billingd unreachable or
    /// refusing, or a report that carries no readable `behg_regime`.
    pub async fn regime(&self) -> Result<BehgRegime, String> {
        let Some(up) = &self.billingd else {
            return Err(
                "kein billingd konfiguriert (billingd_url) — das BEHG-Regime ist billingds \
                 [policy] behg_regime und wird nicht angenommen"
                    .to_owned(),
            );
        };
        let report = up
            .json::<PolicyReport>(up.get("/api/v1/policy"))
            .await
            .map_err(|e| format!("billingd [policy] behg_regime nicht lesbar: {e}"))?
            .ok_or_else(|| "billingd bedient GET /api/v1/policy nicht".to_owned())?;
        regime_aus_bericht(&report)
    }
}

/// The regime a billingd policy report states.
///
/// # Errors
///
/// When the report carries no `behg_regime` entry or one this build does not
/// know.
pub fn regime_aus_bericht(report: &PolicyReport) -> Result<BehgRegime, String> {
    let entry = report
        .entries
        .iter()
        .find(|e| e.key == BEHG_REGIME_KEY)
        .ok_or_else(|| "billingds Policy-Bericht nennt kein behg_regime".to_owned())?;
    serde_json::from_value(serde_json::Value::String(entry.value.clone())).map_err(|_| {
        format!(
            "billingds behg_regime {:?} ist kein bekanntes Regime",
            entry.value
        )
    })
}

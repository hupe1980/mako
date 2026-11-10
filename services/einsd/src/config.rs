//! Configuration for `einsd`.

use serde::Deserialize;

/// `einsd` runtime configuration — loaded via `mako_service::load_config`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EinsdConfig {
    /// PostgreSQL connection + pool tuning (`application_name` = `einsd`).
    pub database: mako_service::config::DatabaseConfig,

    /// `[http]` — the listen address. Defaults to every interface on `9180`.
    #[serde(default = "default_http")]
    pub http: mako_service::config::HttpConfig,

    /// Tenant identifier — data-isolation key written to every database row.
    /// Typically the operator’s BDEW- or DVGW-Codenummer, but any stable unique string is valid.
    pub tenant: String,

    /// ERP webhook URL.  When set, `de.eeg.*` CloudEvents are POSTed here.
    pub erp_webhook_url: Option<String>,

    /// HMAC-SHA256 secret for signing outbound CloudEvent POSTs.
    pub erp_hmac_secret: Option<String>,

    /// Optional `edmd` base URL — used to auto-fetch Einspeisemenge when
    /// `einspeisemenge_kwh` is not provided in a settlement request.
    ///
    /// When set, `POST /api/v1/anlagen/{tr_id}/settle/{year}/{month}` without
    /// `einspeisemenge_kwh` calls
    /// `GET {edmd_url}/api/v1/energy/{malo_id}?direction=EINSPEISUNG` over the
    /// billing month and sums the projected intervals.
    ///
    /// Deliberately **not** `/api/v1/billing-period/{malo_id}`: its
    /// `arbeitsmenge_kwh` is the Bezug, projected onto the *consumption*
    /// registers. An Erzeugungs-MaLo reports only `1-0:2.8.x`, so that
    /// projection is empty and the field reads `Some(0)` rather than absent —
    /// a settlement on it pays nothing while a dry-run still counts the plant
    /// as having data. Only `/energy` lets the direction be stated.
    /// See `handlers::fetch_einspeisemenge_from_edmd`.
    pub edmd_url: Option<String>,

    /// API key used for authenticated requests to `edmd`.
    pub edmd_api_key: Option<String>,

    /// `accountingd` base URL — the EEG/KWKG payments actually made, which the
    /// EnFG ledger is built from (`GET /api/v1/eeg/enfg-zahlungen`). Without it
    /// no ledger can be built, because a computed settlement is not a payment.
    #[serde(default)]
    pub accountingd_url: Option<String>,

    /// Bearer credential for `accountingd`.
    #[serde(default)]
    pub accountingd_api_key: Option<String>,

    /// `marktd` base URL — each plant's Messlokation (`GET /api/v1/melos/{id}`)
    /// for the Regelzone that names its ÜNB and the Netzebene reported as its
    /// Spannungsebene (§ 72 Abs. 2 Nr. 1 EEG 2023).
    #[serde(default)]
    pub marktd_url: Option<String>,

    /// Bearer credential for `marktd`.
    #[serde(default)]
    pub marktd_api_key: Option<String>,

    /// How often (in seconds) the background alert worker checks for plants
    /// whose `foerderendedatum` is within 180 days.  Defaults to 21600 (6 h).
    pub alert_interval_secs: Option<u64>,

    /// URL template for auto-importing Anlage 1 technology-specific Jahresmarktwert.
    ///
    /// When set, `einsd` auto-fetches technology-specific Marktwert values from the
    /// ÜNB publication (netztransparenz.de or a custom aggregator) on the 5th of each
    /// month. The URL must return a JSON array of `{ erzeugungsart, avg_ct_kwh }` objects
    /// for the given billing period.
    ///
    /// Example: `"https://api.netztransparenz.de/eeg/marktwert/{year}/{month}"`
    /// (The `{year}` and `{month}` placeholders are replaced with the billing period.)
    ///
    /// The feed is the **monthly** series (Anlage 1 Nr. 3); the Jahresmarktwert
    /// of Nr. 4 lands once a year and is imported by hand.
    ///
    /// When absent, operators import every value manually via
    /// `PUT /api/v1/marktwert/{year}/{art}/{erzeugungsart}`.
    pub jahresmarktwert_url: Option<String>,

    /// Interval in seconds between Jahresmarktwert auto-import runs (default: 86400, once/day).
    /// On startup, the worker runs once after a 60-second delay.
    pub jahresmarktwert_import_interval_secs: Option<u64>,

    /// Earliest day of the month on which the auto-settle worker settles the
    /// previous month. Defaults to 7.
    ///
    /// The ÜNB publishes the Marktwert around the 5th and edmd's month is not
    /// complete before then. Running earlier wrote `price_missing` / `no_data`
    /// receipts for plants that were merely early, and those had to be settled
    /// again afterwards.
    pub auto_settle_from_day: Option<u8>,

    /// How many months back the auto-settle worker sweeps on each run.
    /// Defaults to 3; clamped to 1–24.
    ///
    /// Settling only the previous month meant a period the service was down for
    /// — or whose ÜNB Marktwert arrived late — was never revisited: the window
    /// moved on and the plant simply went unpaid. §23 EEG 2023 puts the monthly
    /// payment on the Netzbetreiber, so the sweep re-checks a short tail.
    pub auto_settle_catchup_months: Option<u8>,
    /// MCP server authentication. Supports API-key, OIDC, or dev mode.
    /// See `[mcp]` section in TOML — e.g. `api_key = "env:EINSD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,

    /// OIDC token verification for the REST API.
    ///
    /// Required unless `allow_insecure_no_auth` is set: the settlement endpoints
    /// create a payment obligation to the Anlagenbetreiber, so serving them
    /// unauthenticated has to be a decision someone wrote down.
    #[serde(default)]
    pub oidc: Option<mako_service::oidc::OidcConfig>,

    /// The Beihilfevorbehalt approvals recorded so far — `[beihilfevorbehalt]`.
    #[serde(default)]
    pub beihilfevorbehalt: BeihilfevorbehaltConfig,

    /// `[policy]` — the operator readings this deployment settles under.
    #[serde(default)]
    pub policy: EinsdPolicy,

    /// Start without token verification.
    ///
    /// Intended for local development and the demos. Every REST route is then
    /// reachable by any caller that can open a socket.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,
}

/// `[beihilfevorbehalt]` — the state-aid approvals recorded so far.
///
/// The Commission's approval of the EEG 2023 „bis zum 31. Dezember 2026
/// befristet ist" (BT-Drs. 21/7867, Begründung zu § 104; C(2022) 9900 final,
/// SA.102084, recital (211)), so a payment for a supply period from
/// 01.01.2027 has none until one is recorded here. What einsd does with an
/// uncovered payment is `[policy] beihilfe_politik`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeihilfevorbehaltConfig {
    /// Each recorded approval: `regelung` (`eeg2023_verlaengerung` — a
    /// short-term extension of the EEG 2023 approval, under which the periods
    /// it covers settle on the EEG 2023 — or `eeg2027`), `eingang`, `gilt_ab`,
    /// `gilt_bis` (required for an extension), `bundesanzeiger`. A list in
    /// which an extension and an EEG 2027 approval cover the same day is
    /// refused at load ([`eeg_billing::genehmigungen_pruefen`]).
    #[serde(default, deserialize_with = "genehmigungen")]
    pub genehmigungen: Vec<eeg_billing::Beihilfegenehmigung>,
    /// Removed: the choice is `[policy] beihilfe_politik`. Named so a config
    /// still carrying it is refused at startup instead of running under the
    /// default while the operator believes their value is in force.
    #[serde(default)]
    pub politik: Option<String>,
}

/// Read `[beihilfevorbehalt] genehmigungen`, refusing a list the gate cannot
/// read together.
fn genehmigungen<'de, D>(d: D) -> Result<Vec<eeg_billing::Beihilfegenehmigung>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let liste = Vec::<eeg_billing::Beihilfegenehmigung>::deserialize(d)?;
    eeg_billing::genehmigungen_pruefen(&liste).map_err(serde::de::Error::custom)?;
    Ok(liste)
}

/// `[policy]` — einsd's operator settings.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EinsdPolicy {
    /// What happens to a Teil-3 payment no recorded approval covers.
    #[serde(default)]
    pub beihilfe_politik: mako_service::policy::Policy<BeihilfePolitik>,
    /// Whether a § 52 Pflichtzahlung is set off against the month's payout.
    #[serde(default)]
    pub sect52_aufrechnung: mako_service::policy::Policy<Sect52Aufrechnung>,
}

mako_service::policy_choice! {
    /// Whether the Netzbetreiber sets a § 52 EEG 2023 Pflichtzahlung off
    /// against the plant's § 19 Abs. 1 claim of the same month.
    ///
    /// § 52 Abs. 6 Satz 2 permits it („können … aufgerechnet werden"), § 27
    /// Abs. 1 Satz 1 otherwise admits an Aufrechnung only „soweit die Forderung
    /// unbestritten oder rechtskräftig festgestellt ist". The election is the
    /// Netzbetreiber's.
    pub enum Sect52Aufrechnung {
        key: "sect52_aufrechnung",
        doc: "@/docs/services/einsd.md#policy-sect52-aufrechnung",
        default: tightening(Gesondert, "§ 27 Abs. 1 EEG 2023 bars the Aufrechnung unless the Netzbetreiber elects the § 52 Abs. 6 Satz 2 set-off; the payout stays whole"),
        options: {
            /// Pay the Vergütung in full; the month's Pflichtzahlung is a
            /// separate claim, due on the 15th of the following month.
            Gesondert = "gesondert" => "§ 52 Abs. 6 Satz 1 EEG 2023: Fälligkeit zum 15. Kalendertag des Folgemonats",
            /// Set the month's Pflichtzahlung off against its § 19 Abs. 1
            /// claim; what the claim does not cover stays owed.
            Aufrechnen = "aufrechnen" => "§ 52 Abs. 6 Satz 2 EEG 2023: Aufrechnung mit Ansprüchen nach § 19 Abs. 1",
        }
    }
}

mako_service::policy_choice! {
    /// What einsd does with a Teil-3 payment for a supply period no recorded
    /// state-aid approval covers.
    ///
    /// The legal answer is the operator's to give; none of the three is a legal
    /// outcome mako asserts. The default refuses, because paying is the step
    /// that cannot be taken back.
    pub enum BeihilfePolitik {
        key: "beihilfe_politik",
        doc: "@/docs/services/einsd.md#policy-beihilfe-politik",
        default: tightening(Verweigern, "paying is the step that cannot be taken back; a refused period is retried by the sweep once an approval is recorded"),
        options: {
            /// Refuse the settlement, naming the reservation; no receipt is
            /// written, and the sweep retries the period once an approval is
            /// recorded.
            Verweigern = "verweigern" => "BT-Drs. 21/7867 S. 248, Zu § 104: Genehmigungsvorbehalt, die Beihilfegenehmigung für das EEG 2023 ist bis zum 31. Dezember 2026 befristet",
            /// Settle and pay, with the reservation stated as a zero-euro
            /// position on the receipt and the Gutschrift.
            UnterVorbehalt = "unter_vorbehalt" => "BT-Drs. 21/7867 S. 248, Zu § 104: Genehmigungsvorbehalt, die Beihilfegenehmigung für das EEG 2023 ist bis zum 31. Dezember 2026 befristet",
            /// Settle without paying: the receipt is stored as `gehalten` with
            /// no Gutschrift number and no payout event, and the sweep
            /// releases it once an approval covering the period is recorded.
            Halten = "halten" => "BT-Drs. 21/7867 S. 248, Zu § 104: Genehmigungsvorbehalt, die Beihilfegenehmigung für das EEG 2023 ist bis zum 31. Dezember 2026 befristet",
        }
    }
}

impl EinsdConfig {
    /// Every finding that keeps einsd from running as documented: a removed
    /// key, or a posture that serves payment obligations unauthenticated.
    pub fn validate(&self) -> Vec<mako_service::Befund> {
        use mako_service::befund::{Befund, Bereich};
        let mut out = Vec::new();
        if let Some(old) = &self.beihilfevorbehalt.politik {
            out.push(
                Befund::blockiert(
                    "beihilfevorbehalt.politik",
                    format!(
                        "einsd: `[beihilfevorbehalt] politik = {old:?}` is not read; the choice is \
                         `[policy] beihilfe_politik` (verweigern | unter_vorbehalt | halten). Move \
                         the value there."
                    ),
                )
                .referenz_auf::<BeihilfePolitik>(),
            );
        }
        // Settling a plant creates a payment obligation to the Anlagenbetreiber, so
        // the API is closed by default; running it open has to be stated explicitly.
        if self.oidc.is_none() && !self.allow_insecure_no_auth {
            out.push(
                Befund::blockiert(
                    "oidc",
                    "einsd: no [oidc] section and allow_insecure_no_auth is not set — \
                     refusing to serve the settlement API unauthenticated",
                )
                .bereich(Bereich::Zugriff),
            );
        }
        // `OutboxWorker` signs only when it has a secret. With a webhook URL and
        // none, every de.eeg.* CloudEvent — the settled amounts the ERP pays out
        // on among them — reaches the receiver unsigned and indistinguishable
        // from a body anybody posted.
        if self.erp_webhook_url.is_some()
            && self.erp_hmac_secret.is_none()
            && !self.allow_insecure_no_auth
        {
            out.push(
                Befund::blockiert(
                    "erp_hmac_secret",
                    "einsd: erp_webhook_url is set but erp_hmac_secret is not — the outbox \
                     worker signs only when it has a secret, so every de.eeg.* CloudEvent, \
                     settlement amounts included, would reach the ERP unsigned. Configure \
                     erp_hmac_secret, or set allow_insecure_no_auth = true",
                )
                .bereich(Bereich::Zugriff),
            );
        }
        out
    }

    /// The policy and posture einsd runs under — logged, exported, recorded
    /// and served by the runner and `GET /api/v1/policy`.
    #[must_use]
    pub fn policy_report(&self) -> mako_service::policy::PolicyReport {
        mako_service::policy::PolicyReport::new()
            .with(&self.policy.beihilfe_politik)
            .with(&self.policy.sect52_aufrechnung)
            .posture_flag("allow_insecure_no_auth", self.allow_insecure_no_auth)
            .posture_presence("oidc", self.oidc.is_some(), true)
            .posture_presence(
                "erp_hmac_secret",
                self.erp_hmac_secret.is_some(),
                self.erp_webhook_url.is_some(),
            )
    }

    /// `edmd`, addressed and credentialed, or `None` when it is not configured.
    ///
    /// The single place `edmd_url` and `edmd_api_key` are read, so where the
    /// credential goes is decided once.
    #[must_use]
    pub fn edmd(&self, client: reqwest::Client) -> Option<mako_service::http::Upstream> {
        self.edmd_url.as_deref().map(|url| {
            mako_service::http::Upstream::new(
                "edmd",
                url,
                self.edmd_api_key.clone().map(secrecy::SecretString::from),
                client,
            )
        })
    }
}

impl EinsdConfig {
    /// `accountingd`, addressed and credentialed, or `None` when not configured.
    #[must_use]
    pub fn accountingd(&self, client: reqwest::Client) -> Option<mako_service::http::Upstream> {
        self.accountingd_url.as_deref().map(|url| {
            mako_service::http::Upstream::new(
                "accountingd",
                url,
                self.accountingd_api_key
                    .clone()
                    .map(secrecy::SecretString::from),
                client,
            )
        })
    }

    /// `marktd`, addressed and credentialed, or `None` when not configured.
    #[must_use]
    pub fn marktd(&self, client: reqwest::Client) -> Option<mako_service::http::Upstream> {
        self.marktd_url.as_deref().map(|url| {
            mako_service::http::Upstream::new(
                "marktd",
                url,
                self.marktd_api_key.clone().map(secrecy::SecretString::from),
                client,
            )
        })
    }
}

impl mako_service::ServiceConfig for EinsdConfig {
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        Some(&self.database)
    }
    fn bind_addr(&self) -> String {
        self.http.addr.clone()
    }
}

fn default_http() -> mako_service::config::HttpConfig {
    mako_service::config::HttpConfig::on_port(9180)
}

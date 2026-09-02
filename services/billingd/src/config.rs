//! Configuration for `billingd`.

use rust_decimal::Decimal;
use serde::Deserialize;

/// Platform-level statutory rate configuration.
/// Configure under `[rates]` in `billingd.toml`.
/// Update annually as BNetzA / BMWK publish new levies.
#[derive(Debug, Deserialize)]
// A mistyped rate key is a wrong tax on every invoice, so unknown keys are
// refused at startup rather than ignored: `behg_gas_ct_per_kw` would otherwise
// leave the year-table default silently in place.
#[serde(deny_unknown_fields)]
pub struct RatesConfig {
    /// Stromsteuer §3 StromStG — ct/kWh (default 2.05, valid since 01.04.2003).
    pub stromsteuer_ct_per_kwh: Option<Decimal>,
    /// Energiesteuer Erdgas §2 Nr. 3 EnergieStG — ct/kWh_Hs (default 0.55).
    pub energiesteuer_gas_ct_per_kwh: Option<Decimal>,
    /// CO₂-Abgabe BEHG Erdgas — ct/kWh_Hs
    /// (default 1.17906516 = 65 EUR/t × 0.18139464 kg/kWh_Hs ÷ 10, 2026).
    /// From 2026 the nEHS price is set by auction inside the §10 BEHG corridor;
    /// configure the operator's actual procurement cost here.
    pub behg_gas_ct_per_kwh: Option<Decimal>,
    /// MwSt rate as decimal fraction (default 0.19).
    pub mwst_rate: Option<Decimal>,
    /// Reduced MwSt rate as decimal fraction (§ 12 Abs. 2 UStG, default 0.07).
    /// Applies to the Anlage-2 supplies this platform bills — Trinkwasser.
    pub mwst_rate_reduced: Option<Decimal>,
}

/// `[billing]` — how a document is shaped, where the law offers a choice.
#[derive(Debug, Deserialize, Default)]
pub struct BillingConfig {
    /// § 14 Abs. 5 Satz 2 UStG — the default settlement form for an invoice
    /// that deducts advances.
    ///
    /// `"ENDRECHNUNG"` (the default) states the whole supply and deducts the
    /// advances together with the tax contained in them. `"RESTRECHNUNG"`
    /// invoices only the remainder and lists no advances — what the BMF
    /// recommends for e-invoices (Schreiben v. 15.10.2024, Rn. 48), because
    /// EN 16931's core profiles have nowhere to carry per-advance tax.
    ///
    /// Both are lawful and the customer pays the same amount either way. A
    /// request may override it per invoice.
    pub settlement_form: Option<energy_billing::SettlementForm>,
}

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

    /// `productd` base URL — product catalog and EPEX prices.
    pub productd_url: String,

    /// `productd` bearer token — register it there as an `[[oidc.service_keys]]`
    /// entry. Without it every catalogue lookup is rejected and the invoice has
    /// no prices, no EPEX series and no nEHS certificate cost.
    pub productd_api_key: Option<String>,

    /// `edmd` base URL — `MeterBillingPeriod` for consumption data.
    pub edmd_url: String,

    /// `edmd` bearer token.
    pub edmd_api_key: Option<String>,

    /// `marktd` base URL — `PreisblattNetznutzung` + `PreisblattKonzessionsabgabe`.
    pub marktd_url: String,

    /// `marktd` bearer token.
    pub marktd_api_key: Option<String>,

    /// `vertragd` base URL — contract facts, BG-7 buyers, Rahmenvertrag MaLo
    /// enumeration, §41e Aggregatorverträge and the §40b billing candidates.
    pub vertragd_url: Option<String>,

    /// `vertragd` bearer token — register it there as an
    /// `[[oidc.service_keys]]` entry. Without it every contract lookup is
    /// rejected, and the invoice loses its § 40 Abs. 1 EnWG facts and its BG-7
    /// buyer.
    pub vertragd_api_key: Option<String>,

    /// `outputd` base URL — renders the ZUGFeRD PDF from the stored model.
    /// Defaults to `http://localhost:9880`. Without a reachable outputd the
    /// PDF endpoint returns 502; the XML endpoints need no renderer.
    pub outputd_url: Option<String>,

    /// `accountingd` base URL — the Massenkontokorrent that holds the
    /// advance-payment register (`GET /accounts/{malo}/abschlaege`).
    ///
    /// Deliberately **not** defaulted: the §40b sweep needs it to itemise and
    /// deduct the paid Abschläge a Jahresrechnung must show (§ 40 Abs. 1 EnWG,
    /// § 14 Abs. 5 Satz 2 UStG), and a localhost default would make a missing
    /// deployment look like a customer who paid nothing. Absent → the sweep
    /// refuses settling cadences and says why.
    pub accountingd_url: Option<String>,

    /// `accountingd` bearer token.
    pub accountingd_api_key: Option<String>,

    /// `outputd` bearer token.
    pub outputd_api_key: Option<String>,

    /// ERP webhook URL — receives `de.billing.rechnung.erstellt` CloudEvents.
    pub erp_webhook_url: Option<String>,

    /// HMAC-SHA256 secret for signing outbound webhooks.
    pub erp_hmac_secret: Option<String>,

    /// Seller name for XRechnung generation (BG-4, BT-27). Defaults to tenant ID.
    pub seller_name: Option<String>,

    /// BT-31 — seller VAT identifier (Umsatzsteuer-Identifikationsnummer).
    ///
    /// § 14 Abs. 4 Nr. 2 UStG is a **disjunction**: an invoice must carry either
    /// this or [`Self::seller_tax_number`]. See [`Self::seller_tax_identifier`].
    pub seller_vat_id: Option<String>,

    /// BT-32 — the seller's Steuernummer, as issued by their Finanzamt.
    ///
    /// The other half of the § 14 Abs. 4 Nr. 2 UStG disjunction. A § 19 UStG
    /// Kleinunternehmer generally holds no USt-IdNr. and states this instead.
    pub seller_tax_number: Option<String>,

    /// §40 Abs. 2 Nr. 1 EnWG / BG-5 — the supplier's postal address, stated
    /// field by field.
    ///
    /// Stated, never parsed out of a free-text line: an address that does not
    /// happen to read `"Street 1, 12345 City"` — no comma, a two-word city, a PO
    /// box — loses BT-52 and BT-53 to any split heuristic, which is a BR-DE-8/9
    /// failure at the B2G path and an unusable letterhead everywhere else.
    #[serde(default)]
    pub seller: Option<SellerConfig>,

    /// Removed. Replaced by the structured `[seller]` block.
    ///
    /// `BillingdConfig` does not `deny_unknown_fields` — a stale key would
    /// otherwise be ignored in silence, and the first anyone would know of it
    /// is invoices going out without the § 40 Abs. 2 Nr. 1 EnWG supplier
    /// address. Naming the field keeps it a startup error instead.
    #[serde(default)]
    pub seller_address: Option<String>,
    /// Removed. Replaced by `[seller] phone` / `[seller] email`.
    #[serde(default)]
    pub seller_contact: Option<String>,

    /// §40 Abs. 2 Nr. 8 EnWG — annual consumption of the comparable customer
    /// group in kWh/a (e.g. Stromspiegel reference value for the operator's
    /// dominant customer segment). Pro-rated to each billing period. When
    /// unset, the comparison-group line is omitted from invoices.
    pub vergleichsgruppe_kwh_pro_jahr: Option<Decimal>,

    /// Label for the comparable customer group, e.g. `"2-Personen-Haushalt"`.
    pub vergleichsgruppe_label: Option<String>,

    /// Seller payment account IBAN (BT-84) for the XRechnung BG-16 SEPA credit
    /// transfer. Required for a `BR-DE-1`-conformant B2G document.
    pub seller_iban: Option<String>,
    /// Seller bank BIC (BT-86), optional.
    pub seller_bic: Option<String>,

    /// Statutory rate defaults.  Override here instead of per-product.
    pub rates: Option<RatesConfig>,

    /// `[billing]` — document-shape defaults that are the operator's choice
    /// rather than a statutory rate.
    #[serde(default)]
    pub billing: Option<BillingConfig>,

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
    /// endpoints) validate the Standard Webhooks headers.
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

impl BillingdConfig {
    /// Refuse to start on a configuration that cannot behave as documented.
    ///
    /// Covers removed keys and settings whose values contradict each other.
    /// Both are silent failures otherwise: a stale key is ignored, and a
    /// mis-ordered risk band produces invoices routed to a queue nobody reads.
    ///
    /// # Errors
    ///
    /// Names every problem found, so one restart reports all of them.
    /// The § 14 Abs. 4 Nr. 2 UStG tax identifier this operator invoices under.
    ///
    /// The statute names two and requires one: the USt-IdNr. (BT-31) or the
    /// Steuernummer (BT-32). The USt-IdNr. wins where both exist — an EU
    /// counterparty can verify it (VIES) and XRechnung's BR-CO-09 constrains it.
    /// `None` cannot issue a lawful Rechnung; [`Self::validate`] refuses it.
    #[must_use]
    pub fn seller_tax_identifier(&self) -> Option<&str> {
        self.seller_vat_id
            .as_deref()
            .or(self.seller_tax_number.as_deref())
    }

    /// # Errors
    ///
    /// Refuses a configuration that cannot produce a lawful invoice.
    pub fn validate(&self) -> anyhow::Result<()> {
        self.reject_removed_keys()?;
        self.risk.validate()?;
        // Neither identifier means every document this service issues omits a
        // § 14 Abs. 4 Nr. 2 UStG Pflichtangabe. Fail at startup, like [oidc],
        // not per invoice at 02:00 on the first of the month.
        anyhow::ensure!(
            self.seller_tax_identifier().is_some(),
            "refusing to start without a seller tax identifier: § 14 Abs. 4 Nr. 2 UStG \
             requires every Rechnung to carry the seller's USt-IdNr. or Steuernummer. \
             Set `seller_vat_id` (BT-31) or `seller_tax_number` (BT-32)."
        );
        anyhow::ensure!(
            self.billing_runs.run_hour_utc < 24,
            "[billing_runs] run_hour_utc must be an hour of the day (0–23), got {}",
            self.billing_runs.run_hour_utc
        );
        anyhow::ensure!(
            self.billing_runs.catch_up_periods > 0,
            "[billing_runs] catch_up_periods must be at least 1, or the sweep bills nothing"
        );
        Ok(())
    }

    /// Refuse to start on configuration keys that have been replaced.
    ///
    /// # Errors
    ///
    /// Names each removed key and what replaced it.
    pub fn reject_removed_keys(&self) -> anyhow::Result<()> {
        let removed = [
            (
                self.seller_address.is_some(),
                "seller_address",
                "[seller] street / post_code / city",
            ),
            (
                self.seller_contact.is_some(),
                "seller_contact",
                "[seller] phone / email",
            ),
        ];
        let found: Vec<String> = removed
            .iter()
            .filter(|(present, _, _)| *present)
            .map(|(_, key, replacement)| format!("`{key}` → `{replacement}`"))
            .collect();
        anyhow::ensure!(
            found.is_empty(),
            "billingd.toml uses configuration keys that no longer exist: {}. \
             They were free-text lines that einvoice split on punctuation; the \
             replacement states each EN 16931 term (BT-35/37/38, BT-42/43) on its \
             own. Ignoring them would ship invoices without the § 40 Abs. 2 Nr. 1 \
             EnWG supplier address.",
            found.join(", ")
        );
        Ok(())
    }
}

impl mako_service::ServiceConfig for BillingdConfig {
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        Some(&self.database)
    }
    fn bind_addr(&self) -> String {
        format!("0.0.0.0:{}", self.port.unwrap_or(9280))
    }
}

/// `[seller]` — the supplier's own identity as it appears on every document.
///
/// BG-5 (postal address) and BG-6 (contact) of the EN 16931 seller party, and
/// the §40 Abs. 2 Nr. 1 EnWG supplier statement on the BO4E Rechnung. Stated,
/// not parsed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SellerConfig {
    /// BT-35 — street and house number.
    pub street: Option<String>,
    /// BT-36 — additional address line (c/o, building, PO box).
    #[serde(default)]
    pub address_line2: Option<String>,
    /// BT-38 — post code.
    pub post_code: Option<String>,
    /// BT-37 — city.
    pub city: Option<String>,
    /// BT-40 — ISO 3166-1 alpha-2 country code. Defaults to `DE`.
    #[serde(default)]
    pub country: Option<String>,
    /// BT-41 — contact point name, e.g. `"Kundenservice"`.
    #[serde(default)]
    pub contact_name: Option<String>,
    /// BT-42 — contact telephone.
    #[serde(default)]
    pub phone: Option<String>,
    /// BT-43 — contact e-mail.
    #[serde(default)]
    pub email: Option<String>,
}

impl SellerConfig {
    /// The §40 Abs. 2 Nr. 1 EnWG address as one line, for the BO4E Rechnung.
    ///
    /// The EN 16931 model takes the fields individually; the BO4E
    /// `Verbraucherinformationen` carries a single human-readable string, and
    /// this is where the two representations meet — composed from the parts,
    /// never parsed back out of the whole.
    #[must_use]
    pub fn anschrift(&self) -> Option<String> {
        let street = self.street.as_deref().unwrap_or_default();
        let plz = self.post_code.as_deref().unwrap_or_default();
        let city = self.city.as_deref().unwrap_or_default();
        let parts: Vec<&str> = [street, self.address_line2.as_deref().unwrap_or_default()]
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect();
        let head = parts.join(", ");
        let tail = format!("{plz} {city}").trim().to_owned();
        match (head.is_empty(), tail.is_empty()) {
            (true, true) => None,
            (false, true) => Some(head),
            (true, false) => Some(tail),
            (false, false) => Some(format!("{head}, {tail}")),
        }
    }

    /// The §40 Abs. 2 Nr. 1 EnWG customer-service contact as one line.
    #[must_use]
    pub fn kontakt(&self) -> Option<String> {
        let parts: Vec<String> = [
            self.phone.as_ref().map(|p| format!("Tel. {p}")),
            self.email.clone(),
        ]
        .into_iter()
        .flatten()
        .collect();
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

/// §40b EnWG billing-run worker configuration (`[billing_runs]`).
///
/// The worker sweeps once per day after `run_hour_utc`: it pulls the active
/// contracts from vertragd, computes each contract's most recently completed
/// billing period from its `abrechnungszyklus`, bills every period that has
/// no invoice yet, and accumulates the month's `billing_run_log` row. For
/// iMSys MaLos it additionally delivers the free monthly
/// Abrechnungsinformation (§ 40b Abs. 3 EnWG) as a CloudEvent.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillingRunsConfig {
    /// Whether the scheduled billing worker is active. Default: false.
    #[serde(default)]
    pub enabled: bool,
    /// UTC hour (0–23) after which the daily sweep runs. Default: 4.
    #[serde(default = "default_billing_run_hour")]
    pub run_hour_utc: u8,
    /// Emit the § 40b Abs. 3 monthly Abrechnungsinformation for iMSys MaLos.
    /// Default: true (only effective while `enabled`).
    #[serde(default = "default_true")]
    pub abrechnungsinformation: bool,
    /// How many completed periods back the sweep will catch up, per contract.
    ///
    /// A worker that was down for a cycle must still bill the periods it slept
    /// through (§40c EnWG), so the sweep walks the history backwards until it
    /// finds a period that already has a record. The bound caps that walk:
    /// without one, enabling the worker against a portfolio of long-running
    /// annual contracts would issue a decade of back-dated invoices in a single
    /// night. Default 13 — a year of monthly periods plus one.
    #[serde(default = "default_catch_up_periods")]
    pub catch_up_periods: usize,
    /// Bill settling cadences (`JAEHRLICH`) from the scheduled sweep **even
    /// without an advance source**. Default: **false**.
    ///
    /// A Jahresrechnung is a settlement: §40 Abs. 1 EnWG requires it to itemise
    /// the advance payments and deduct them from the Zahlbetrag, and
    /// §14 Abs. 5 Satz 2 UStG requires the tax attributable to each. With
    /// [`accountingd_url`](BillingdConfig::accountingd_url) configured the sweep
    /// reads them from the advance register and settles them, and this flag is
    /// not consulted at all.
    ///
    /// Without one there is no source, so the sweep would state the full year's
    /// gross as `zuZahlen` with zero Vorauszahlungen — a document demanding
    /// money the customer already paid, which looks entirely ordinary. Those
    /// contracts are skipped and billed through
    /// `POST /api/v1/billing/{malo_id}/calculate` with `abschlaege` supplied by
    /// the caller.
    ///
    /// Setting this to `true` opts into emitting them anyway — for a deployment
    /// that genuinely collects no advances, where the deduction is empty because
    /// there is nothing to deduct.
    #[serde(default)]
    pub jahresrechnung: bool,

    /// **Send** each invoice the sweep produces: record it in `outputd` for the
    /// § 147 AO eight years and queue it on the customer's channels.
    /// Default: **true**.
    ///
    /// § 40c Abs. 2 EnWG puts the invoice in the customer's hands within three
    /// weeks of the period end for monthly billing and six otherwise, so a
    /// nightly run that bills and does not send is a deadline nobody is
    /// keeping. Off is for a deployment whose ERP owns delivery and takes the
    /// invoice off the `de.billing.rechnung.erstellt` event instead; the
    /// documents are then never recorded here, and reproducing one is that
    /// ERP's problem.
    ///
    /// A **held** invoice is never sent whatever this says: the risk gate
    /// withheld its issuance, so no receivable stands behind it.
    #[serde(default = "default_true")]
    pub versand: bool,
}

fn default_billing_run_hour() -> u8 {
    4
}
const fn default_catch_up_periods() -> usize {
    13
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
            catch_up_periods: default_catch_up_periods(),
            jahresrechnung: false,
            versand: true,
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
                .unwrap_or(dec!(1.17906516)),
            mwst_rate: r.and_then(|r| r.mwst_rate).unwrap_or(dec!(0.19)),
            mwst_rate_reduced: r.and_then(|r| r.mwst_rate_reduced).unwrap_or(dec!(0.07)),
        }
    }
}

/// Period-aware statutory rate resolution.
///
/// A correction re-opens an old period, and that period is billed under its own
/// rates: 2021 BEHG was 25 EUR/t, the second half of 2020 had 16 % VAT, and
/// gas/Fernwärme carried **7 % USt from 01.10.2022 to 31.03.2024**
/// (§28 Abs. 5/6 UStG) — which is why the product `category` is part of the
/// lookup: the VAT history of gas differs from electricity.
///
/// An explicitly configured rate still wins — configuration is the operator
/// saying "I know better" — but the *defaults* come from the year tables. A
/// period straddling a boundary has **no** correct single rate, so
/// [`BillingdConfig::try_regulatory_rates_for_period`] refuses it rather than
/// choosing one; [`BillingdConfig::steuer_stichtage`] names the split dates.
impl BillingdConfig {
    /// The `[rates] behg_gas_ct_per_kwh` override, when the operator pinned one.
    ///
    /// An explicit override always wins over the nEHS market-price series.
    pub fn behg_override(&self) -> Option<rust_decimal::Decimal> {
        self.rates.as_ref().and_then(|r| r.behg_gas_ct_per_kwh)
    }

    /// The operator's default § 14 Abs. 5 Satz 2 UStG settlement form.
    ///
    /// `Endrechnung` unless `[billing] settlement_form = "RESTRECHNUNG"` — the
    /// German paper norm is to state the whole supply and deduct the advances
    /// with their tax, and switching that silently would change every
    /// Jahresrechnung an operator has ever issued.
    pub fn settlement_form(&self) -> energy_billing::SettlementForm {
        self.billing
            .as_ref()
            .and_then(|b| b.settlement_form)
            .unwrap_or_default()
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
            mwst_rate_reduced: configured
                .and_then(|r| r.mwst_rate_reduced)
                .unwrap_or(defaults.mwst_rate_reduced),
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
            "productd_url": "http://localhost:9080",
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

#[cfg(test)]
mod seller_identity_tests {
    use super::*;

    /// Build a config from JSON, optionally naming each tax identifier.
    fn cfg(vat_id: Option<&str>, tax_number: Option<&str>) -> BillingdConfig {
        let mut v = serde_json::json!({
            "database": { "url": "postgres://localhost/x" },
            "tenant": "9900357000004",
            "productd_url": "http://localhost:9080",
            "edmd_url": "http://localhost:8380",
            "marktd_url": "http://localhost:8080"
        });
        if let Some(id) = vat_id {
            v["seller_vat_id"] = serde_json::json!(id);
        }
        if let Some(nr) = tax_number {
            v["seller_tax_number"] = serde_json::json!(nr);
        }
        serde_json::from_value(v).expect("config parses")
    }

    /// § 14 Abs. 4 Nr. 2 UStG names two identifiers and requires one, so a
    /// § 19 UStG Kleinunternehmer configures the Steuernummer alone.
    #[test]
    fn either_identifier_alone_satisfies_nr_2() {
        assert_eq!(
            cfg(Some("DE123456789"), None).seller_tax_identifier(),
            Some("DE123456789"),
        );
        assert_eq!(
            cfg(None, Some("123/456/78901")).seller_tax_identifier(),
            Some("123/456/78901"),
        );
    }

    /// With both, the USt-IdNr. is the one a counterparty can verify (VIES).
    #[test]
    fn the_vat_id_is_preferred_when_both_exist() {
        assert_eq!(
            cfg(Some("DE123456789"), Some("123/456/78901")).seller_tax_identifier(),
            Some("DE123456789"),
        );
    }

    /// A deployment with neither cannot issue a lawful invoice, so it does not
    /// start. Failing here beats discovering it in a customer's post.
    #[test]
    fn a_seller_with_neither_identifier_is_refused_at_startup() {
        let err = cfg(None, None)
            .validate()
            .expect_err("no tax identifier means no lawful Rechnung");
        let msg = err.to_string();
        assert!(msg.contains("§ 14 Abs. 4 Nr. 2 UStG"), "{msg}");
        assert!(msg.contains("seller_tax_number"), "{msg}");
    }

    /// The ordinary case still starts.
    #[test]
    fn a_configured_seller_validates() {
        assert!(cfg(Some("DE123456789"), None).validate().is_ok());
    }
}

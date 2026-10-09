//! Configuration for `billingd`.

use rust_decimal::Decimal;
use serde::Deserialize;

/// Statutory rate configuration — `[rates]` in `billingd.toml`.
///
/// The Stromsteuer, the Energiesteuer Erdgas and the § 10 BEHG prices come
/// from dated series in `energy-billing`, each ending at the last year its
/// statute was read for. What is configurable here is a **dated** row for a
/// year those series do not cover, with its source; never a year-less rate,
/// which would stand in for every year nobody has read the law for.
#[derive(Debug, Clone, Default, Deserialize)]
// A mistyped rate key is a wrong tax on every invoice, so unknown keys are
// refused at startup rather than ignored.
#[serde(deny_unknown_fields)]
pub struct RatesConfig {
    /// MwSt rate as decimal fraction (default 0.19).
    pub mwst_rate: Option<Decimal>,
    /// Reduced MwSt rate as decimal fraction (§ 12 Abs. 2 UStG, default 0.07).
    /// Applies to the Anlage-2 supplies this platform bills — Trinkwasser.
    pub mwst_rate_reduced: Option<Decimal>,
    /// Which law prices nEHS certificates in a year with no § 10 BEHG price —
    /// `"rvo_bezugspreis"` (default; § 10 Abs. 3 Satz 2 Nr. 4/Nr. 5 BEHG, in
    /// force) or `"korridor"` (55–65 EUR/t, 3. BEHG-ÄndG, BT-Drs. 21/7869).
    #[serde(default)]
    pub behg_regime: energy_billing::BehgRegime,
    /// Supplied nEHS prices (EUR/t) for periods with no § 10 BEHG price,
    /// each checked against [`Self::behg_regime`].
    #[serde(default)]
    pub behg_eur_per_t: Vec<DatierterSatz>,
    /// § 3 StromStG rates (ct/kWh) for years the statutory series does not cover.
    #[serde(default)]
    pub stromsteuer_ct_per_kwh: Vec<DatierterSatz>,
    /// § 2 Abs. 3 Satz 1 Nr. 4 EnergieStG rates (ct/kWh_Hs) for years the
    /// statutory series does not cover.
    #[serde(default)]
    pub energiesteuer_gas_ct_per_kwh: Vec<DatierterSatz>,
}

/// One supplied rate: the dates it covers, the value, and where it was read.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatierterSatz {
    /// First day covered.
    pub von: time::Date,
    /// Last day covered.
    pub bis: time::Date,
    /// The rate — EUR/t for BEHG, ct/kWh for the taxes.
    pub wert: Decimal,
    /// The provision, Fassung or publication the value was read from.
    pub quelle: String,
}

impl RatesConfig {
    /// Refuse a supplied row that cannot be right.
    ///
    /// A row must name its source, cover a non-empty range inside one
    /// calendar year, carry a positive value, overlap no other row of its
    /// kind, and fill a gap: a row for a year the statutory series already
    /// covers would contradict the statute, and is refused rather than
    /// silently ignored or preferred. A BEHG row is checked against the
    /// configured regime.
    ///
    /// # Errors
    ///
    /// Names the key, the row and what is wrong with it.
    pub fn validate(&self) -> anyhow::Result<()> {
        let statutory_strom = |y| energy_billing::stromsteuer_for_year(y).is_some();
        let statutory_gas = |y| energy_billing::energiesteuer_gas_for_year(y).is_some();
        let statutory_behg = |y| {
            energy_billing::RegulatoryRates::for_year(y)
                .behg_gas_ct_per_kwh
                .is_ok()
        };
        for (key, rows, statutory) in [
            (
                "stromsteuer_ct_per_kwh",
                &self.stromsteuer_ct_per_kwh,
                &statutory_strom as &dyn Fn(i32) -> bool,
            ),
            (
                "energiesteuer_gas_ct_per_kwh",
                &self.energiesteuer_gas_ct_per_kwh,
                &statutory_gas,
            ),
            ("behg_eur_per_t", &self.behg_eur_per_t, &statutory_behg),
        ] {
            for (i, z) in rows.iter().enumerate() {
                let at = format!("[rates] {key}[{i}] ({}..{})", z.von, z.bis);
                anyhow::ensure!(
                    !z.quelle.trim().is_empty(),
                    "{at}: `quelle` must name the provision or publication the value was read from"
                );
                anyhow::ensure!(
                    z.von <= z.bis && z.von.year() == z.bis.year(),
                    "{at}: `von`..`bis` must be a non-empty range inside one calendar year"
                );
                anyhow::ensure!(z.wert > Decimal::ZERO, "{at}: `wert` must be positive");
                anyhow::ensure!(
                    !statutory(z.von.year()),
                    "{at}: {} is covered by the statutory series; a supplied row may only \
                     fill a year the series does not cover",
                    z.von.year()
                );
                if key == "behg_eur_per_t" {
                    self.behg_regime
                        .pruefe(z.wert, z.von.year())
                        .map_err(|e| anyhow::anyhow!("{at}: {e}"))?;
                }
                for (j, other) in rows.iter().enumerate().skip(i + 1) {
                    anyhow::ensure!(
                        z.bis < other.von || other.bis < z.von,
                        "{at} overlaps [rates] {key}[{j}] ({}..{})",
                        other.von,
                        other.bis
                    );
                }
            }
        }
        Ok(())
    }
}

impl DatierterSatz {
    /// Whether this row covers the whole of `[from, to]`.
    fn deckt(&self, from: time::Date, to: time::Date) -> bool {
        self.von <= from && to <= self.bis
    }
}

/// The supplied row covering `[from, to]`, if any — rows never overlap
/// ([`BillingdConfig::validate`]).
fn deckende_zeile(
    rows: &[DatierterSatz],
    from: time::Date,
    to: time::Date,
) -> Option<&DatierterSatz> {
    rows.iter().find(|z| z.deckt(from, to))
}

/// An nEHS certificate price point from productd's dated series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NehsPreis {
    /// The date the price was set.
    pub datum: time::Date,
    /// EUR per tonne CO₂.
    pub eur_per_t: Decimal,
}

/// `[e_rechnung]` — the Rechnungsaussteller's facts for § 27 Abs. 38 UStG.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ERechnungConfig {
    /// The issuer's Gesamtumsatz (§ 19 Abs. 2 UStG) per calendar year.
    /// § 27 Abs. 38 Nr. 2 UStG admits a sonstige Rechnung for a 2027 supply
    /// only where the 2026 figure is „nicht mehr als 800 000 Euro"; an
    /// unrecorded year never admits it.
    #[serde(default)]
    pub gesamtumsatz: Vec<Gesamtumsatz>,
}

/// One year's Gesamtumsatz of the Rechnungsaussteller.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gesamtumsatz {
    /// The calendar year.
    pub jahr: i32,
    /// EUR, nach vereinnahmten Entgelten (§ 19 Abs. 2 Satz 1 UStG).
    pub eur: Decimal,
}

impl ERechnungConfig {
    /// The recorded Gesamtumsatz of `jahr`, if any.
    #[must_use]
    pub fn gesamtumsatz_im(&self, jahr: i32) -> Option<Decimal> {
        self.gesamtumsatz
            .iter()
            .find(|g| g.jahr == jahr)
            .map(|g| g.eur)
    }
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

    /// The share of a billing period that must be covered by billable readings
    /// before `billingd` will invoice it, 0–100.
    ///
    /// **Unset by default, and then nothing is refused.** § 40a Abs. 2 EnWG
    /// makes a partially covered period billable: where the supplier cannot
    /// determine the actual consumption for reasons it does not answer for,
    /// the invoice „darf … auf einer Verbrauchsschätzung beruhen", stated on
    /// the document „unter ausdrücklichem und optisch besonders
    /// hervorgehobenem Hinweis" together with its ground and its factors. The
    /// engine carries that duty as the `MENGE_UNVOLLSTAENDIG` finding on every
    /// period below full coverage. Refusing instead would trade a lawful
    /// labelled estimate for a certain § 40c Abs. 2 EnWG deadline breach — one
    /// late MSCONS day in a 31-day month reports 96.77 %.
    ///
    /// Set it where an operator wants a floor below which it would rather
    /// chase the readings than estimate; a period under it is then refused
    /// with `INCOMPLETE_METER_DATA`.
    ///
    /// Read through [`BillingdConfig::min_meter_coverage_pct`].
    #[serde(default)]
    pub min_meter_coverage_pct: Option<Decimal>,

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

    /// `[e_rechnung]` — the facts § 27 Abs. 38 UStG decides the paper path on.
    #[serde(default)]
    pub e_rechnung: ERechnungConfig,

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
    /// The coverage floor a period must reach before it is invoiced, 0–100, or
    /// `None` where the operator set none and § 40a Abs. 2 EnWG governs alone.
    #[must_use]
    pub fn min_meter_coverage_pct(&self) -> Option<Decimal> {
        self.min_meter_coverage_pct
    }

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
        if let Some(rates) = &self.rates {
            rates.validate()?;
        }
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
    /// The configured standard MwSt rate, 19 % unless `[rates] mwst_rate` pins one.
    pub fn mwst_rate(&self) -> Decimal {
        self.rates
            .as_ref()
            .and_then(|r| r.mwst_rate)
            .unwrap_or(rust_decimal::dec!(0.19))
    }

    /// The `[rates]` block, or the empty one.
    fn rates_config(&self) -> std::borrow::Cow<'_, RatesConfig> {
        match &self.rates {
            Some(r) => std::borrow::Cow::Borrowed(r),
            None => std::borrow::Cow::Owned(RatesConfig::default()),
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
/// The per-kWh taxes come from the statutory series, then from a supplied
/// dated row; a period neither covers carries the refusal, which the provider
/// billing that tax raises. A period straddling a boundary has **no** correct
/// single rate, so [`BillingdConfig::try_regulatory_rates_for_period`] refuses
/// it rather than choosing one; [`BillingdConfig::steuer_stichtage`] names the
/// split dates.
impl BillingdConfig {
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
        let stichtage =
            energy_billing::steuer_stichtage_im_zeitraum(category, period_from, period_to);
        // An operator who pinned an explicit VAT rate has taken the VAT
        // decision themselves; splitting at a VAT Stichtag would contradict
        // it. A tax or levy boundary is not theirs to waive.
        if self.rates.as_ref().and_then(|r| r.mwst_rate).is_none() {
            return stichtage;
        }
        stichtage
            .into_iter()
            .filter(|d| d.month() == time::Month::January && d.day() == 1)
            .filter(|d| {
                // Keep a 1 January only where a per-year rate changes there.
                let y = d.year();
                energy_billing::behg_ct_per_kwh_for_year(y - 1)
                    != energy_billing::behg_ct_per_kwh_for_year(y)
                    || energy_billing::stromsteuer_for_year(y - 1)
                        != energy_billing::stromsteuer_for_year(y)
                    || energy_billing::energiesteuer_gas_for_year(y - 1)
                        != energy_billing::energiesteuer_gas_for_year(y)
            })
            .collect()
    }

    /// Resolve the statutory rates for a period.
    ///
    /// `nehs` is productd's latest nEHS price at or before the period start;
    /// it prices the CO₂ component of a `GAS`/`WAERME` period only when it was
    /// set in the period's own year.
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
        nehs: Option<NehsPreis>,
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
        Ok(self.regulatory_rates_for_period(category, period_from, period_to, nehs))
    }

    /// Resolve the statutory rates for a period, assuming it is uniform.
    ///
    /// Prefer [`Self::try_regulatory_rates_for_period`] on any path that bills:
    /// this one reads the rates at the period start where a period straddles a
    /// boundary, which is only safe for previews and estimates. A tax with no
    /// source for the period is carried as its refusal either way.
    pub fn regulatory_rates_for_period(
        &self,
        category: &str,
        period_from: time::Date,
        period_to: time::Date,
        nehs: Option<NehsPreis>,
    ) -> energy_billing::RegulatoryRates {
        use energy_billing::{
            ENERGIESTEUER_GAS_SERIE, RateRefusal, RegulatoryRates, STROMSTEUER_SERIE,
        };
        let year = period_from.year();
        let cfg = self.rates_config();
        let statutory = RegulatoryRates::for_year(year);

        let tax = |statutory: energy_billing::ResolvedRate,
                   rows: &[DatierterSatz],
                   satz: &str,
                   serie: &[energy_billing::SatzZeile]| {
            statutory.or_else(|_| {
                deckende_zeile(rows, period_from, period_to)
                    .map(|z| z.wert)
                    .ok_or_else(|| RateRefusal::ausserhalb_der_serie(satz, serie, year))
            })
        };
        let stromsteuer_ct_per_kwh = tax(
            statutory.stromsteuer_ct_per_kwh,
            &cfg.stromsteuer_ct_per_kwh,
            "Stromsteuer",
            STROMSTEUER_SERIE,
        );
        let energiesteuer_gas_ct_per_kwh = tax(
            statutory.energiesteuer_gas_ct_per_kwh,
            &cfg.energiesteuer_gas_ct_per_kwh,
            "Energiesteuer Erdgas",
            ENERGIESTEUER_GAS_SERIE,
        );
        let (behg_gas_ct_per_kwh, behg_rechtsgrundlage) =
            self.behg_for_period(category, period_from, period_to, nehs);

        let mwst_rate = cfg
            .mwst_rate
            .or_else(|| {
                if matches!(category, "GAS" | "WAERME") {
                    energy_billing::mwst_rate_for_gas_waerme_period(period_from, period_to)
                } else {
                    energy_billing::mwst_rate_for_period(period_from, period_to)
                }
            })
            .unwrap_or(rust_decimal::dec!(0.19));
        RegulatoryRates {
            stromsteuer_ct_per_kwh,
            energiesteuer_gas_ct_per_kwh,
            behg_gas_ct_per_kwh,
            behg_rechtsgrundlage,
            mwst_rate,
            mwst_rate_reduced: cfg.mwst_rate_reduced.unwrap_or(rust_decimal::dec!(0.07)),
        }
    }

    /// The BEHG CO₂ component of a period and the provision that priced it.
    ///
    /// Precedence: productd's nEHS series, when the point was set in the
    /// period's own year (a 2025 Festpreis never prices 2026, a 2026 auction
    /// never prices 2027); then the § 10 BEHG table; then a supplied dated
    /// row. A year with no tabled price takes a price only through the
    /// configured [`energy_billing::BehgRegime`], which checks it; with none,
    /// the period carries the regime's refusal.
    fn behg_for_period(
        &self,
        category: &str,
        period_from: time::Date,
        period_to: time::Date,
        nehs: Option<NehsPreis>,
    ) -> (energy_billing::ResolvedRate, String) {
        let year = period_from.year();
        let cfg = self.rates_config();
        let regime = cfg.behg_regime;
        let statutory = energy_billing::RegulatoryRates::for_year(year);
        let tabled = statutory.behg_gas_ct_per_kwh.as_ref().ok().copied();
        let to_ct = |eur_per_t: Decimal| {
            energy_billing::behg_ct_per_kwh_from_price(eur_per_t, year).ok_or_else(|| {
                energy_billing::RateRefusal::new(
                    "BEHG",
                    year,
                    "kein EBeV-Emissionsfaktor für Erdgas in diesem Jahr",
                )
            })
        };
        let series =
            nehs.filter(|p| matches!(category, "GAS" | "WAERME") && p.datum.year() == year);

        if let Some(p) = series {
            let grundlage = format!(
                "nEHS-Zertifikatspreis vom {} ({})",
                p.datum,
                energy_billing::behg_rechtsgrundlage_for_year(year)
                    .unwrap_or(regime.rechtsgrundlage())
            );
            let checked = if tabled.is_some() {
                Ok(())
            } else {
                regime.pruefe(p.eur_per_t, year)
            };
            return (checked.and_then(|()| to_ct(p.eur_per_t)), grundlage);
        }
        if let Some(ct) = tabled {
            return (Ok(ct), statutory.behg_rechtsgrundlage);
        }
        let grundlage = regime.rechtsgrundlage().to_owned();
        let resolved = match deckende_zeile(&cfg.behg_eur_per_t, period_from, period_to) {
            Some(z) => regime.pruefe(z.wert, year).and_then(|()| to_ct(z.wert)),
            None => Err(regime.fehlender_preis(year)),
        };
        (resolved, grundlage)
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
            .try_regulatory_rates_for_period(
                "GAS",
                date!(2024 - 03 - 01),
                date!(2024 - 04 - 30),
                None,
            )
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
                date!(2024 - 04 - 30),
                None
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
            .try_regulatory_rates_for_period(
                "GAS",
                date!(2023 - 12 - 01),
                date!(2024 - 01 - 31),
                None,
            )
            .expect_err("the BEHG year boundary must be refused");
        assert!(err.stichtage.contains(&date!(2024 - 01 - 01)), "{err:?}");
    }

    /// An operator who pinned an explicit rate has taken the decision; splitting
    /// would contradict it, so the refusal stands down.
    #[test]
    fn an_explicitly_configured_rate_wins_over_the_split() {
        let c = cfg(Some("0.19"));
        let rates = c
            .try_regulatory_rates_for_period(
                "GAS",
                date!(2024 - 03 - 01),
                date!(2024 - 04 - 30),
                None,
            )
            .expect("a pinned rate suppresses the refusal");
        assert_eq!(rates.mwst_rate, rust_decimal::dec!(0.19));
    }

    /// A uniform period resolves normally.
    #[test]
    fn a_uniform_gas_period_resolves() {
        let c = cfg(None);
        assert!(
            c.try_regulatory_rates_for_period(
                "GAS",
                date!(2026 - 05 - 01),
                date!(2026 - 05 - 31),
                None
            )
            .is_ok()
        );
    }
}

#[cfg(test)]
mod dated_rate_tests {
    use super::*;
    use rust_decimal::dec;
    use time::macros::date;

    fn cfg(rates: serde_json::Value) -> BillingdConfig {
        serde_json::from_value(serde_json::json!({
            "database": { "url": "postgres://localhost/x" },
            "tenant": "9900357000004",
            "seller_vat_id": "DE123456789",
            "productd_url": "http://localhost:9080",
            "edmd_url": "http://localhost:8380",
            "marktd_url": "http://localhost:8080",
            "rates": rates
        }))
        .expect("config parses")
    }

    fn gas_jan(year: i32) -> (time::Date, time::Date) {
        (
            time::Date::from_calendar_date(year, time::Month::January, 1).unwrap(),
            time::Date::from_calendar_date(year, time::Month::January, 31).unwrap(),
        )
    }

    /// A Gas invoice for January 2027 with no supplied price is refused by
    /// the engine, naming the year — never billed at the 2026 figure of
    /// 1.08836784 ct/kWh.
    #[test]
    fn a_january_2027_gas_period_with_no_price_is_refused() {
        let c = cfg(serde_json::Value::Null);
        let (from, to) = gas_jan(2027);
        let rates = c
            .try_regulatory_rates_for_period("GAS", from, to, None)
            .expect("a uniform period");
        let err = rates
            .behg_gas_ct_per_kwh
            .clone()
            .expect_err("2027 has no lawful price");
        assert_eq!(err.jahr, 2027);
        assert!(
            err.grund.contains("vorletzten vorangegangenen Quartals"),
            "{err}"
        );

        let product: energy_billing::Product = serde_json::from_value(serde_json::json!({
            "category": "GAS",
            "gas_arbeitspreis_ct_per_kwh_hs": "8.00"
        }))
        .unwrap();
        let ctx = energy_billing::BillingContext {
            period: energy_billing::BillingPeriod::new(from, to).unwrap(),
            regulatory_rates: rates.clone(),
            ..Default::default()
        };
        let quantities = energy_billing::Quantities {
            gas: Some(energy_billing::GasMeterInput {
                kwh_hs: Some(dec!(1000)),
                ..Default::default()
            }),
            ..Default::default()
        };
        let billed = product
            .build_engine(&energy_billing::GridInput::default(), &rates)
            .bill(ctx, &quantities);
        let err = billed.expect_err("the engine refuses the period");
        assert_eq!(err.code(), "RATE_REFUSED", "{err}");
        assert!(err.to_string().contains("2027"), "{err}");
    }

    /// A year-less BEHG rate is not a configuration key: it would stand in
    /// for every year nobody has priced.
    #[test]
    fn a_year_less_behg_rate_is_refused_at_parse() {
        let parsed = serde_json::from_value::<BillingdConfig>(serde_json::json!({
            "database": { "url": "postgres://localhost/x" },
            "tenant": "9900357000004",
            "productd_url": "http://localhost:9080",
            "edmd_url": "http://localhost:8380",
            "marktd_url": "http://localhost:8080",
            "rates": { "behg_gas_ct_per_kwh": "1.08836784" }
        }));
        assert!(parsed.is_err());
    }

    fn row(von: &str, bis: &str, wert: &str) -> serde_json::Value {
        serde_json::json!({ "von": von, "bis": bis, "wert": wert, "quelle": "UBA, Veröffentlichung nach § 4 Abs. 2 CO2KostAufG" })
    }

    /// Switching the regime is one configuration line, and changes what a
    /// supplied 2027 price must satisfy and what the trace names.
    #[test]
    fn the_regime_is_one_setting_and_the_trace_names_it() {
        let (from, to) = gas_jan(2027);
        let rvo = cfg(serde_json::json!({
            "behg_eur_per_t": [row("2027-01-01", "2027-03-31", "70")]
        }));
        rvo.validate().expect("the RVO regime has no corridor");
        let r = rvo.regulatory_rates_for_period("GAS", from, to, None);
        assert_eq!(
            r.behg_gas_ct_per_kwh.unwrap(),
            dec!(70) * dec!(0.18139464) / dec!(10)
        );
        assert!(
            r.behg_rechtsgrundlage
                .contains("§ 10 Abs. 3 Satz 2 Nr. 4/Nr. 5 BEHG")
        );

        let korridor = cfg(serde_json::json!({
            "behg_regime": "korridor",
            "behg_eur_per_t": [row("2027-01-01", "2027-03-31", "70")]
        }));
        assert!(korridor.validate().is_err(), "70 EUR/t is outside 55–65");
        assert!(
            korridor
                .regulatory_rates_for_period("GAS", from, to, None)
                .behg_gas_ct_per_kwh
                .is_err()
        );

        let ok = cfg(serde_json::json!({
            "behg_regime": "korridor",
            "behg_eur_per_t": [row("2027-01-01", "2027-12-31", "60")]
        }));
        ok.validate().expect("60 lies inside the corridor");
        let r = ok.regulatory_rates_for_period("GAS", from, to, None);
        assert!(r.behg_gas_ct_per_kwh.is_ok());
        assert!(r.behg_rechtsgrundlage.contains("BT-Drs. 21/7869"));

        // The CO₂ line itself names the regime.
        let product: energy_billing::Product = serde_json::from_value(serde_json::json!({
            "category": "GAS",
            "gas_arbeitspreis_ct_per_kwh_hs": "8.00"
        }))
        .unwrap();
        let ctx = energy_billing::BillingContext {
            period: energy_billing::BillingPeriod::new(from, to).unwrap(),
            regulatory_rates: r.clone(),
            ..Default::default()
        };
        let quantities = energy_billing::Quantities {
            gas: Some(energy_billing::GasMeterInput {
                kwh_hs: Some(dec!(1000)),
                ..Default::default()
            }),
            ..Default::default()
        };
        let invoice = product
            .build_engine(&energy_billing::GridInput::default(), &r)
            .bill(ctx, &quantities)
            .expect("a 2027 period with a corridor price bills");
        let co2 = invoice
            .positions
            .iter()
            .find(|p| p.tags.iter().any(|t| t == "behg"))
            .expect("a CO₂ line");
        assert!(
            co2.legal_basis
                .as_deref()
                .is_some_and(|b| b.contains("BT-Drs. 21/7869")),
            "{:?}",
            co2.legal_basis
        );
    }

    /// A series point prices only its own year: a 2026 auction never prices
    /// 2027, and a 2025 Festpreis never prices 2026.
    #[test]
    fn a_series_point_prices_only_its_own_year() {
        let c = cfg(serde_json::Value::Null);
        let (from, to) = gas_jan(2027);
        let dec_2026 = NehsPreis {
            datum: date!(2026 - 12 - 16),
            eur_per_t: dec!(63),
        };
        assert!(
            c.regulatory_rates_for_period("GAS", from, to, Some(dec_2026))
                .behg_gas_ct_per_kwh
                .is_err()
        );
        let (from, to) = gas_jan(2026);
        let dec_2025 = NehsPreis {
            datum: date!(2025 - 12 - 10),
            eur_per_t: dec!(55),
        };
        assert_eq!(
            c.regulatory_rates_for_period("GAS", from, to, Some(dec_2025))
                .behg_gas_ct_per_kwh
                .unwrap(),
            dec!(1.08836784),
            "the 2026 table price, not the 2025 point"
        );
        let jan_2026 = NehsPreis {
            datum: date!(2026 - 01 - 07),
            eur_per_t: dec!(62),
        };
        assert_eq!(
            c.regulatory_rates_for_period("GAS", from, to, Some(jan_2026))
                .behg_gas_ct_per_kwh
                .unwrap(),
            dec!(62) * dec!(0.18139464) / dec!(10)
        );
    }

    /// A Strom period in a year past the § 3 StromStG series is refused,
    /// naming the tax and the year; a supplied row with its source fills it.
    #[test]
    fn a_2028_strom_period_refuses_until_a_sourced_row_fills_it() {
        let (from, to) = gas_jan(2028);
        let r = cfg(serde_json::Value::Null).regulatory_rates_for_period("STROM", from, to, None);
        let err = r.effective_stromsteuer(None).expect_err("2028 is unread");
        let msg = err.to_string();
        assert!(msg.contains("Stromsteuer") && msg.contains("2028"), "{msg}");

        let filled = cfg(serde_json::json!({
            "stromsteuer_ct_per_kwh": [{
                "von": "2028-01-01", "bis": "2028-12-31", "wert": "2.05",
                "quelle": "§ 3 StromStG, gelesen 2027-12-01"
            }]
        }));
        filled.validate().expect("a gap-filling row with a source");
        assert_eq!(
            filled
                .regulatory_rates_for_period("STROM", from, to, None)
                .effective_stromsteuer(None)
                .unwrap(),
            dec!(2.05)
        );
    }

    /// A supplied row may fill a gap, never contradict the statute.
    #[test]
    fn a_row_for_a_statutory_year_is_refused_at_startup() {
        let c = cfg(serde_json::json!({
            "stromsteuer_ct_per_kwh": [{
                "von": "2026-01-01", "bis": "2026-12-31", "wert": "1.00",
                "quelle": "x"
            }]
        }));
        let err = c.validate().expect_err("2026 is statutory").to_string();
        assert!(err.contains("statutory series"), "{err}");
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

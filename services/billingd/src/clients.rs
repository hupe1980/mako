//! HTTP clients for external service calls in `billingd`.

use anyhow::{Context as _, Result};
use rust_decimal::Decimal;
use std::collections::HashMap;

use energy_billing::{DynamicInterval, MeterInput, Product};

// ── TarifbdClient ─────────────────────────────────────────────────────────────

pub struct TarifbdClient {
    base_url: String,
    client: reqwest::Client,
}

impl TarifbdClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            client: reqwest::Client::new(),
        }
    }

    /// `GET /api/v1/customer/{malo_id}/product?lf_mp_id={lf_mp_id}`
    ///
    /// Returns the active `TariffInput` for a MaLo, extracted from the
    /// `Tarifpreisblatt` JSONB stored in `tarifbd`.
    pub async fn get_customer_product(
        &self,
        malo_id: &str,
        lf_mp_id: &str,
    ) -> Result<Option<Product>> {
        let url = format!(
            "{}/api/v1/customer/{}/product?lf_mp_id={}",
            self.base_url, malo_id, lf_mp_id
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("tarifbd GET customer/product")?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        resp.error_for_status_ref()
            .map_err(|e| anyhow::anyhow!("tarifbd {e}"))?;

        let body: serde_json::Value = resp.json().await.context("parse customer product")?;
        // Extract pricing fields from the nested product.data (Tarifpreisblatt JSONB).
        let data = body.get("product").and_then(|p| p.get("data"));
        let tariff = extract_tariff_from_product_data(data, body.get("product"))?;
        Ok(Some(tariff))
    }

    #[allow(dead_code)]
    pub async fn get_hourly_epex_prices(
        &self,
        period_from: time::Date,
        period_to: time::Date,
    ) -> Result<HashMap<(i32, u8, u8, u8), Decimal>> {
        let mut map = HashMap::new();
        let mut day = period_from;
        while day <= period_to {
            let url = format!("{}/api/v1/epex-prices/{}/hourly", self.base_url, day,);
            let resp = self
                .client
                .get(&url)
                .send()
                .await
                .context("tarifbd GET epex-prices hourly")?;

            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                tracing::warn!(date = %day, "billingd: EPEX hourly prices not found for date");
                day = day.next_day().unwrap_or(day);
                continue;
            }
            resp.error_for_status_ref()
                .map_err(|e| anyhow::anyhow!("tarifbd epex {e}"))?;

            let prices: Vec<serde_json::Value> = resp.json().await.context("parse epex hourly")?;

            for entry in &prices {
                let hour = entry.get("hour").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
                let price_ct = entry
                    .get("price_ct_kwh")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Decimal>().ok())
                    .unwrap_or(Decimal::ZERO);
                map.insert((day.year(), day.month() as u8, day.day(), hour), price_ct);
            }
            day = day.next_day().unwrap_or(day);
        }
        Ok(map)
    }
}

/// Decode a JSON value as `rust_decimal::Decimal`.
///
/// Accepts both string (`"25.5"`) and JSON number (`25.5`) representations.
/// Rejects nested objects — the old non-BO4E `{"wert": "25.5"}` form is no
/// longer accepted after the `tarifbd` hard-cut.
fn decimal_from_json(v: Option<&serde_json::Value>) -> Option<Decimal> {
    match v? {
        serde_json::Value::String(s) => s.parse().ok(),
        serde_json::Value::Number(n) => n.to_string().parse().ok(),
        _ => None,
    }
}

/// Extract `TariffInput` from a `ProductRow.data` JSONB and the product metadata.
///
/// ## Preistyp — canonical ALLCAPS (hard-cut)
///
/// `tarifbd` normalises all `preistyp` values to canonical ALLCAPS on PUT
/// (enforced by `normalize_tarifpreisblatt()`).  Commodity disambiguation uses
/// the product-level `category` field so that a single `GRUNDPREIS` position
/// maps to the correct `TariffInput` field for STROM, GAS, and WAERME.
///
/// | preistyp | category | TariffInput field |
/// |---|---|---|
/// | `GRUNDPREIS` | `GAS` | `gas_grundpreis_ct_per_day` |
/// | `GRUNDPREIS` | `WAERME` | `waerme_grundpreis_eur_per_month` |
/// | `GRUNDPREIS` | any other | `grundpreis_ct_per_day` |
/// | `ARBEITSPREIS_EINTARIF` | `GAS` | `gas_arbeitspreis_ct_per_kwh_hs` |
/// | `ARBEITSPREIS_EINTARIF` | `WAERME` | `waerme_arbeitspreis_ct_per_kwh` |
/// | `ARBEITSPREIS_EINTARIF` | `SOLAR` | `solar_arbeitspreis_ct_per_kwh` |
/// | `ARBEITSPREIS_EINTARIF` | any other | `arbeitspreis_ct_per_kwh` |
/// | `ARBEITSPREIS_HT` / `ARBEITSPREIS_NT` | — | HT/NT fields |
/// | `LEISTUNGSPREIS` | `WAERME` | `waerme_leistungspreis_eur_per_kw_month` |
/// | mako extensions | — | see constants in `tarifbd::handlers` |
///
/// ## Price extraction
///
/// `preisstaffeln[0].preis` is a scalar `Decimal` (string or JSON number) after
/// `tarifbd` normalisation.  The first staffel is the base price.
///
/// Regulatory overrides (`stromsteuer_ct_per_kwh_override`, etc.) may be stored
/// as top-level keys in `data`.
fn extract_tariff_from_product_data(
    data: Option<&serde_json::Value>,
    product: Option<&serde_json::Value>,
) -> Result<Product> {
    let category = product
        .and_then(|p| p.get("category"))
        .and_then(|v| v.as_str())
        .unwrap_or("STROM")
        .to_owned();
    let register_count = product.and_then(|p| p.get("register_count")).cloned();
    let dynamic_epex = product
        .and_then(|p| p.get("dyn_source"))
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    let preispositionen = data
        .and_then(|d| {
            d.get("tarifpreispositionen")
                .or_else(|| d.get("preispositionen"))
        })
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Regulatory overrides stored as top-level keys in data (optional).
    let get_decimal = |key: &str| -> Option<Decimal> {
        data.and_then(|d| d.get(key))
            .and_then(|v| decimal_from_json(Some(v)))
    };

    let mut grundpreis_ct_per_day: Option<Decimal> = None;
    let mut arbeitspreis_ct_per_kwh: Option<Decimal> = None;
    let mut arbeitspreis_ht_ct_per_kwh: Option<Decimal> = None;
    let mut arbeitspreis_nt_ct_per_kwh: Option<Decimal> = None;
    let mut steuerungsrabatt_modul1_eur_per_kw_year: Option<Decimal> = None;
    let mut steuerungsrabatt_modul3_eur_per_kw_year: Option<Decimal> = None;
    let mut gas_grundpreis_ct_per_day: Option<Decimal> = None;
    let mut gas_arbeitspreis_ct_per_kwh_hs: Option<Decimal> = None;
    let mut waerme_grundpreis_eur_per_month: Option<Decimal> = None;
    let mut waerme_arbeitspreis_ct_per_kwh: Option<Decimal> = None;
    let mut waerme_leistungspreis_eur_per_kw_month: Option<Decimal> = None;
    let mut solar_arbeitspreis_ct_per_kwh: Option<Decimal> = None;
    let mut mieterstrom_aufschlag_ct_per_kwh: Option<Decimal> = None;
    let mut gemeinschaft_rabatt_ct_per_kwh: Option<Decimal> = None;
    let mut eeg_verguetungssatz_ct_per_kwh: Option<Decimal> = None;
    let mut eeg_marktpraemie_ct_per_kwh: Option<Decimal> = None;
    let mut eeg_managementpraemie_ct_per_kwh: Option<Decimal> = None;
    let mut kwkg_zuschlag_ct_per_kwh: Option<Decimal> = None;
    let mut marktwert_ct_per_kwh: Option<Decimal> = None;
    let mut vermarktungsgebuehr_ct_per_kwh: Option<Decimal> = None;
    let mut hems_subscription_eur_per_month_from_code: Option<Decimal> = None;
    let mut hems_optimization_event_eur: Option<Decimal> = None;
    let mut hems_readout_event_eur: Option<Decimal> = None;
    let mut emobility_service_fee_eur_from_code: Option<Decimal> = None;
    let mut emobility_kwh_price_ct_from_code: Option<Decimal> = None;
    let mut emobility_session_fee_eur: Option<Decimal> = None;
    let mut emobility_roaming_fee_eur: Option<Decimal> = None;
    let mut service_fee_eur: Option<Decimal> = None;
    let mut service_event_price_eur: Option<Decimal> = None;

    for pp in &preispositionen {
        // preistyp is stored in ALLCAPS after tarifbd normalisation.
        let pt = pp.get("preistyp").and_then(|v| v.as_str()).unwrap_or("");

        // preisstaffeln[0].preis is a scalar Decimal (string or number) —
        // the old nested {"wert": "..."} form was non-BO4E and is no longer stored.
        let preis = pp
            .get("preisstaffeln")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|s| decimal_from_json(s.get("preis")));

        match (pt, category.as_str()) {
            ("GRUNDPREIS", "GAS") => gas_grundpreis_ct_per_day = preis,
            ("GRUNDPREIS", "WAERME") => waerme_grundpreis_eur_per_month = preis,
            ("GRUNDPREIS", _) => grundpreis_ct_per_day = preis,

            ("ARBEITSPREIS_EINTARIF", "GAS") => gas_arbeitspreis_ct_per_kwh_hs = preis,
            ("ARBEITSPREIS_EINTARIF", "WAERME") => waerme_arbeitspreis_ct_per_kwh = preis,
            ("ARBEITSPREIS_EINTARIF", "SOLAR") => solar_arbeitspreis_ct_per_kwh = preis,
            ("ARBEITSPREIS_EINTARIF", _) => arbeitspreis_ct_per_kwh = preis,

            ("ARBEITSPREIS_HT", _) => arbeitspreis_ht_ct_per_kwh = preis,
            ("ARBEITSPREIS_NT", _) => arbeitspreis_nt_ct_per_kwh = preis,

            ("LEISTUNGSPREIS", "WAERME") => waerme_leistungspreis_eur_per_kw_month = preis,
            ("LEISTUNGSPREIS", _) => {} // not mapped outside Wärme

            ("SOLAR_ARBEITSPREIS", _) => solar_arbeitspreis_ct_per_kwh = preis,
            ("MIETERSTROM_AUFSCHLAG", _) => mieterstrom_aufschlag_ct_per_kwh = preis,
            ("GEMEINSCHAFT_RABATT", _) => gemeinschaft_rabatt_ct_per_kwh = preis,
            ("EEG_VERGUETUNG", _) => eeg_verguetungssatz_ct_per_kwh = preis,
            ("EEG_MARKTPRAEMIE", _) => eeg_marktpraemie_ct_per_kwh = preis,
            ("EEG_MANAGEMENTPRAEMIE", _) => eeg_managementpraemie_ct_per_kwh = preis,
            ("KWKG_ZUSCHLAG", _) => kwkg_zuschlag_ct_per_kwh = preis,
            ("MARKTWERT", _) => marktwert_ct_per_kwh = preis,
            ("VERMARKTUNGSGEBUEHR", _) => vermarktungsgebuehr_ct_per_kwh = preis,
            ("STEUERUNGSRABATT_MODUL1", _) => steuerungsrabatt_modul1_eur_per_kw_year = preis,
            ("STEUERUNGSRABATT_MODUL3", _) => steuerungsrabatt_modul3_eur_per_kw_year = preis,
            ("HEMS_PLATTFORMGEBUEHR", _) => hems_subscription_eur_per_month_from_code = preis,
            ("HEMS_OPTIMIERUNGSEVENT", _) => hems_optimization_event_eur = preis,
            ("HEMS_AUSLESUNG", _) => hems_readout_event_eur = preis,
            ("EMOBILITY_SERVICEGEBUEHR", _) => emobility_service_fee_eur_from_code = preis,
            ("EMOBILITY_ARBEITSPREIS", _) => emobility_kwh_price_ct_from_code = preis,
            ("EMOBILITY_SESSION", _) => emobility_session_fee_eur = preis,
            ("EMOBILITY_ROAMING", _) => emobility_roaming_fee_eur = preis,
            ("SERVICE_GEBUEHR", _) => service_fee_eur = preis,
            ("SERVICE_EVENT", _) => service_event_price_eur = preis,
            _ => {}
        }
    }

    let gas_indexed_price: Option<energy_billing::IndexedPriceConfig> = product
        .and_then(|p| p.get("gas_indexed_price"))
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    let stromsteuer_befreiung: energy_billing::StromsteuerBefreiung = product
        .and_then(|p| p.get("stromsteuer_befreiung"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let energiequellen: Option<energy_billing::EnergieQuellen> = product
        .and_then(|p| p.get("energiequellen"))
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    // Build a flat JSON map and deserialize to the typed Product enum.
    // Product uses #[serde(tag = "category")] so the flat map works directly.
    let flat = serde_json::json!({
        "category": category,
        "product_code": product.and_then(|p| p.get("product_code")).and_then(|v| v.as_str()),
        "register_count": register_count,
        "grundpreis_ct_per_day": grundpreis_ct_per_day,
        "arbeitspreis_ct_per_kwh": arbeitspreis_ct_per_kwh,
        "arbeitspreis_ht_ct_per_kwh": arbeitspreis_ht_ct_per_kwh,
        "arbeitspreis_nt_ct_per_kwh": arbeitspreis_nt_ct_per_kwh,
        "leistungspreis_strom_ct_per_kw_month": get_decimal("leistungspreis_strom_ct_per_kw_month"),
        "sect14a_modul1_nne_reduktion_ct_per_kwh": get_decimal("sect14a_modul1_nne_reduktion_ct_per_kwh"),
        "sect14a_modul3_entschaedigung_ct_per_kwh": get_decimal("sect14a_modul3_entschaedigung_ct_per_kwh"),
        "steuerungsrabatt_modul1_eur_per_kw_year": steuerungsrabatt_modul1_eur_per_kw_year,
        "steuerungsrabatt_modul3_eur_per_kw_year": steuerungsrabatt_modul3_eur_per_kw_year,
        "gas_grundpreis_ct_per_day": gas_grundpreis_ct_per_day,
        "gas_arbeitspreis_ct_per_kwh_hs": gas_arbeitspreis_ct_per_kwh_hs,
        "gas_leistungspreis_ct_per_kw_month": get_decimal("gas_leistungspreis_ct_per_kw_month"),
        "gas_indexed_price": gas_indexed_price,
        "gas_energiesteuer_befreiung": product.and_then(|p| p.get("gas_energiesteuer_befreiung")).and_then(|v| v.as_bool()).unwrap_or(false),
        "waerme_grundpreis_eur_per_month": waerme_grundpreis_eur_per_month,
        "waerme_arbeitspreis_ct_per_kwh": waerme_arbeitspreis_ct_per_kwh,
        "waerme_leistungspreis_eur_per_kw_month": waerme_leistungspreis_eur_per_kw_month,
        "waerme_leistungspreis_eur_per_kw_year": get_decimal("waerme_leistungspreis_eur_per_kw_year"),
        "waerme_is_renewable": product.and_then(|p| p.get("waerme_is_renewable")).and_then(|v| v.as_bool()).unwrap_or(false),
        "waerme_erneuerbar_anteil_pct": get_decimal("waerme_erneuerbar_anteil_pct"),
        "solar_arbeitspreis_ct_per_kwh": solar_arbeitspreis_ct_per_kwh,
        "mieterstrom_aufschlag_ct_per_kwh": mieterstrom_aufschlag_ct_per_kwh,
        "gemeinschaft_rabatt_ct_per_kwh": gemeinschaft_rabatt_ct_per_kwh,
        "solar_include_stromsteuer": false,
        "eeg_verguetungssatz_ct_per_kwh": eeg_verguetungssatz_ct_per_kwh,
        "eeg_marktpraemie_ct_per_kwh": eeg_marktpraemie_ct_per_kwh,
        "eeg_managementpraemie_ct_per_kwh": eeg_managementpraemie_ct_per_kwh,
        "kwkg_zuschlag_ct_per_kwh": kwkg_zuschlag_ct_per_kwh,
        "marktwert_ct_per_kwh": marktwert_ct_per_kwh,
        "vermarktungsgebuehr_ct_per_kwh": vermarktungsgebuehr_ct_per_kwh,
        "hems_subscription_eur_per_month": get_decimal("hems_subscription_eur_per_month").or(hems_subscription_eur_per_month_from_code),
        "hems_optimization_event_eur": hems_optimization_event_eur,
        "hems_readout_event_eur": hems_readout_event_eur,
        "emobility_service_fee_eur": get_decimal("emobility_service_fee_eur").or(emobility_service_fee_eur_from_code),
        "emobility_kwh_price_ct": get_decimal("emobility_kwh_price_ct").or(emobility_kwh_price_ct_from_code),
        "emobility_session_fee_eur": emobility_session_fee_eur,
        "emobility_roaming_fee_eur": emobility_roaming_fee_eur,
        "service_fee_eur": service_fee_eur,
        "service_event_price_eur": service_event_price_eur,
        "dynamic_epex": dynamic_epex,
        "dynamic_epex_floor_ct_kwh": get_decimal("dynamic_epex_floor_ct_kwh"),
        "auf_abschlag_ct_per_kwh": get_decimal("auf_abschlag_ct_per_kwh"),
        "auf_abschlag_eur_per_month": get_decimal("auf_abschlag_eur_per_month"),
        "msb_gebuehr_ct_per_day": get_decimal("msb_gebuehr_ct_per_day"),
        "block_tiers": product.and_then(|p| p.get("block_tiers")).cloned(),
        "minimum_invoice_eur_brutto": get_decimal("minimum_invoice_eur_brutto"),
        "indexed_price": product.and_then(|p| p.get("indexed_price")).cloned(),
        "seasonal_prices": product.and_then(|p| p.get("seasonal_prices")).cloned(),
        "anlage_kwp": get_decimal("anlage_kwp"),
        "industrie_stromsteuer_befreiung": product.and_then(|p| p.get("industrie_stromsteuer_befreiung")).and_then(|v| v.as_bool()).unwrap_or(false),
        "stromsteuer_befreiung": stromsteuer_befreiung,
        "preisgarantie_bis": product.and_then(|p| p.get("preisgarantie_bis")).and_then(|v| v.as_str()),
        "stromsteuer_ct_per_kwh_override": get_decimal("stromsteuer_ct_per_kwh_override"),
        "energiesteuer_gas_ct_per_kwh_override": get_decimal("energiesteuer_gas_ct_per_kwh_override"),
        "behg_gas_ct_per_kwh_override": get_decimal("behg_gas_ct_per_kwh_override"),
        "mwst_rate_override": get_decimal("mwst_rate_override"),
        "sharing_credit_ct_per_kwh": get_decimal("sharing_credit_ct_per_kwh"),
        "sharing_description": product.and_then(|p| p.get("sharing_description")).and_then(|v| v.as_str()),
        "energiequellen": energiequellen,
    });
    serde_json::from_value::<Product>(flat)
        .map_err(|e| anyhow::anyhow!("product deserialization from tarifbd JSONB: {e}"))
}

// ── EdmdClient ────────────────────────────────────────────────────────────────

pub struct EdmdClient {
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl EdmdClient {
    pub fn new(base_url: &str, api_key: Option<String>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key,
            client: reqwest::Client::new(),
        }
    }

    /// `GET /api/v1/billing-period/{malo_id}?from=…&to=…`
    pub async fn get_billing_period(
        &self,
        malo_id: &str,
        period_from: time::Date,
        period_to: time::Date,
    ) -> Result<Option<MeterInput>> {
        let url = format!(
            "{}/api/v1/billing-period/{}?from={}&to={}",
            self.base_url, malo_id, period_from, period_to
        );
        let mut req = self.client.get(&url);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await.context("edmd GET billing-period")?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        resp.error_for_status_ref()
            .map_err(|e| anyhow::anyhow!("edmd {e}"))?;

        let body: serde_json::Value = resp.json().await.context("parse billing period")?;
        let meter = MeterInput {
            arbeitsmenge_kwh: body
                .get("arbeitsmenge_kwh")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::ZERO),
            arbeitsmenge_ht_kwh: body
                .get("arbeitsmenge_ht_kwh")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok()),
            arbeitsmenge_nt_kwh: body
                .get("arbeitsmenge_nt_kwh")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok()),
            spitzenleistung_kw: body
                .get("spitzenleistung_kw")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok()),
            steuerung_stunden: body
                .get("steuerung_stunden")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok()),
            ..Default::default()
        };
        Ok(Some(meter))
    }

    /// Fetch Lastgang intervals for §41a dynamic billing.
    ///
    /// Calls `GET /api/v1/lastgang/{malo_id}?from={from}&to={to}` and
    /// returns `Vec<DynamicInterval>` (one entry per timestamp).
    /// Returns an empty Vec when the MaLo has no Lastgang data.
    pub async fn get_lastgang(
        &self,
        malo_id: &str,
        period_from: time::Date,
        period_to: time::Date,
    ) -> Result<Vec<DynamicInterval>> {
        let url = format!(
            "{}/api/v1/lastgang/{}?from={}&to={}",
            self.base_url, malo_id, period_from, period_to,
        );
        let mut req = self.client.get(&url);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await.context("edmd GET lastgang")?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        resp.error_for_status_ref()
            .map_err(|e| anyhow::anyhow!("edmd lastgang {e}"))?;

        let body: serde_json::Value = resp.json().await.context("parse lastgang")?;
        let intervals = body
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|v| {
                let ts_str = v
                    .get("timestamp_utc")
                    .or_else(|| v.get("zeitstempel"))
                    .and_then(|s| s.as_str())?;
                let kwh_str = v
                    .get("wert")
                    .or_else(|| v.get("kwh"))
                    .and_then(|s| s.as_str())?;
                let ts = time::OffsetDateTime::parse(
                    ts_str,
                    &time::format_description::well_known::Rfc3339,
                )
                .ok()?;
                let kwh: Decimal = kwh_str.parse().ok()?;
                Some(DynamicInterval {
                    timestamp_utc: ts,
                    kwh,
                })
            })
            .collect();
        Ok(intervals)
    }

    /// `GET /api/v1/billing-period/{malo_id}?from=&to=` — Gas billing period.
    ///
    /// Extracts gas-specific fields from the `MeterBillingPeriod` response:
    /// `messung_qm3`, `brennwert_kwh_per_m3`, `zustandszahl`, `spitzenleistung_kw`.
    ///
    /// Returns `None` when the endpoint returns 404 or the response does not contain
    /// a `messung_qm3` field (i.e. the MaLo is not a gas meter).
    ///
    /// The same `/api/v1/billing-period/{malo_id}` endpoint is used for both Strom
    /// (`get_billing_period`) and Gas — the two methods extract different fields from
    /// the same response schema.
    pub async fn get_gas_billing_period(
        &self,
        malo_id: &str,
        period_from: time::Date,
        period_to: time::Date,
    ) -> Result<Option<GasBillingPeriod>> {
        let url = format!(
            "{}/api/v1/billing-period/{}?from={}&to={}",
            self.base_url, malo_id, period_from, period_to
        );
        let mut req = self.client.get(&url);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await.context("edmd GET billing-period (gas)")?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        resp.error_for_status_ref()
            .map_err(|e| anyhow::anyhow!("edmd gas billing-period: {e}"))?;

        let body: serde_json::Value = resp.json().await.context("parse gas billing period")?;

        // `messung_qm3` (or legacy `arbeitsmenge_m3`) signals this is a gas MaLo.
        // If absent, the MaLo is likely an electricity meter — return None.
        let messung_qm3 = body
            .get("messung_qm3")
            .and_then(|v| decimal_from_json(Some(v)))
            .or_else(|| {
                body.get("arbeitsmenge_m3")
                    .and_then(|v| decimal_from_json(Some(v)))
            });

        let Some(messung_qm3) = messung_qm3 else {
            return Ok(None);
        };

        Ok(Some(GasBillingPeriod {
            messung_qm3,
            brennwert_kwh_per_qm3: body
                .get("brennwert_kwh_per_m3")
                .and_then(|v| decimal_from_json(Some(v))),
            zustandszahl: body
                .get("zustandszahl")
                .and_then(|v| decimal_from_json(Some(v))),
            spitzenleistung_kw: body
                .get("spitzenleistung_kw")
                .and_then(|v| decimal_from_json(Some(v))),
        }))
    }

    /// `GET /api/v1/gas-quality/{malo_id}` — MSCONS PID 13007 gas quality data.
    ///
    /// Returns the DSO-published Abrechnungsbrennwert and Zustandszahl for all
    /// billing periods stored for this MaLo.  These values convert gas volume
    /// (m³) to energy (kWh_Hs) per §25 Nr. 4 MessEV / DVGW G 685:
    ///
    /// `kWh_Hs = m³ × brennwert_kwh_per_m3 × zustandszahl`
    ///
    /// Returns `None` on 404 (MaLo has no gas quality data yet).
    /// Returns `Ok(Some(vec![]))` when the response is empty.
    pub async fn get_gas_quality(&self, malo_id: &str) -> Result<Option<Vec<GasQualityRecord>>> {
        let url = format!("{}/api/v1/gas-quality/{}", self.base_url, malo_id);
        let mut req = self.client.get(&url);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await.context("edmd GET gas-quality")?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        resp.error_for_status_ref()
            .map_err(|e| anyhow::anyhow!("edmd gas-quality: {e}"))?;

        let body: serde_json::Value = resp.json().await.context("parse gas quality")?;
        let records: Vec<GasQualityRecord> = body
            .get("gas_quality")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|r| {
                        use time::format_description::well_known::Iso8601;
                        let period_from = r
                            .get("period_from")
                            .and_then(|v| v.as_str())
                            .and_then(|s| time::Date::parse(s, &Iso8601::DEFAULT).ok())?;
                        let period_to = r
                            .get("period_to")
                            .and_then(|v| v.as_str())
                            .and_then(|s| time::Date::parse(s, &Iso8601::DEFAULT).ok())?;
                        let brennwert = decimal_from_json(r.get("brennwert_kwh_per_m3"))?;
                        let zustandszahl = decimal_from_json(r.get("zustandszahl"))?;
                        Some(GasQualityRecord {
                            period_from,
                            period_to,
                            brennwert_kwh_per_m3: brennwert,
                            zustandszahl,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Some(records))
    }
}

/// Gas meter data extracted from `edmd GET /api/v1/billing-period/{malo_id}`.
///
/// Contains the gas-specific fields of a `MeterBillingPeriod`: the volume reading
/// in m³ plus the DSO-supplied conversion factors for m³ → kWh_Hs.
#[derive(Debug, Clone)]
pub struct GasBillingPeriod {
    /// Volume at meter conditions (m³).
    pub messung_qm3: Decimal,
    /// Abrechnungsbrennwert in kWh/m³ (from edmd, sourced from MSCONS PID 13007).
    pub brennwert_kwh_per_qm3: Option<Decimal>,
    /// Zustandszahl — dimensionless volume conversion factor.
    pub zustandszahl: Option<Decimal>,
    /// Peak demand in kW (Spitzenleistung) for Gas RLM billing.
    pub spitzenleistung_kw: Option<Decimal>,
}

/// One gas quality record from `edmd GET /api/v1/gas-quality/{malo_id}`.
///
/// Represents one row of MSCONS PID 13007 data (Gasbeschaffenheitsdaten)
/// published by the DSO.  The `brennwert_kwh_per_m3 × zustandszahl` product
/// gives the kWh content per m³ of gas for a specific billing period.
#[derive(Debug, Clone)]
pub struct GasQualityRecord {
    /// Billing period covered by this quality record.
    pub period_from: time::Date,
    /// Billing period end (inclusive).
    pub period_to: time::Date,
    /// Abrechnungsbrennwert in kWh/m³ (MSCONS QTY+Z08).
    pub brennwert_kwh_per_m3: Decimal,
    /// Zustandszahl — dimensionless compressibility/temperature factor (MSCONS QTY+Z10).
    pub zustandszahl: Decimal,
}

/// Minimal HTTP client for querying `vertragd` contract data.
///
/// Used by `billingd` to:
/// - List active MaLo IDs for a Rahmenvertrag (Sammelrechnung, L2)
pub struct VertragdClient {
    base_url: String,
    client: reqwest::Client,
}

impl VertragdClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            client: reqwest::Client::new(),
        }
    }

    /// `GET /api/v1/rahmenvertraege/{id}/malos`
    ///
    /// Returns the list of active MaLo IDs and their active product codes
    /// for a Rahmenvertrag.  Used by the Sammelrechnung endpoint to enumerate
    /// the sites to consolidate.
    pub async fn get_rahmenvertrag_malos(
        &self,
        rahmenvertrag_id: &str,
    ) -> Result<Vec<RahmenvertragMaloEntry>> {
        let url = format!(
            "{}/api/v1/rahmenvertraege/{}/malos",
            self.base_url, rahmenvertrag_id
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("vertragd GET rahmenvertrag malos")?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(vec![]);
        }
        resp.error_for_status_ref()
            .map_err(|e| anyhow::anyhow!("vertragd {e}"))?;
        resp.json().await.context("parse rahmenvertrag malos")
    }

    /// `GET /api/v1/vertraege/by-malo/{malo_id}`
    ///
    /// The active Versorgungsvertrag behind a MaLo — the source of the §40
    /// Abs. 1 EnWG contract facts (Vertragsdauer, Kündigungsfrist, next
    /// possible Kündigungstermin) the invoice must state. `Ok(None)` when the
    /// MaLo has no active contract in vertragd.
    pub async fn get_vertrag_by_malo(&self, malo_id: &str) -> Result<Option<VertragByMalo>> {
        let url = format!("{}/api/v1/vertraege/by-malo/{}", self.base_url, malo_id);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("vertragd GET vertrag by malo")?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        resp.error_for_status_ref()
            .map_err(|e| anyhow::anyhow!("vertragd {e}"))?;
        resp.json().await.context("parse vertrag by malo")
    }
}

/// Response of `GET /api/v1/vertraege/by-malo/{malo_id}`.
#[derive(Debug, serde::Deserialize)]
pub struct VertragByMalo {
    /// The contract row — only the fields billing needs are read.
    pub vertrag: VertragFacts,
    /// Next possible Kündigungstermin, computed by vertragd as of today.
    pub naechstmoeglicher_kuendigungstermin: Option<time::Date>,
}

/// The contract facts billingd puts on the invoice (§40 Abs. 1 EnWG).
#[derive(Debug, serde::Deserialize)]
pub struct VertragFacts {
    /// vertragd's contract UUID — the invoice's `contract_id` fallback.
    pub id: String,
    /// Human-readable contract number, preferred for `contract_id`.
    pub vertrags_nr: Option<String>,
    /// Contract start — enables §41 pro-rata clipping on first invoices.
    pub vertragsbeginn: time::Date,
    /// Contract end when befristet; `None` for unbefristete Verträge.
    pub vertragsende: Option<time::Date>,
    /// Notice period in months.
    pub kuendigungsfrist_monate: i32,
}

/// One active supply site within a Rahmenvertrag.
#[derive(Debug, serde::Deserialize)]
pub struct RahmenvertragMaloEntry {
    pub malo_id: String,
    #[allow(dead_code)]
    pub product_code: Option<String>,
    #[allow(dead_code)]
    pub kundentyp: Option<String>,
}

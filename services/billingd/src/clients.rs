//! HTTP clients for external service calls in `billingd`.

use anyhow::{Context as _, Result};
use mako_service::http::Upstream;
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::sync::Arc;

use energy_billing::{DynamicInterval, MeterInput, Product};

/// Address a peer service, sharing the daemon's HTTP client.
///
/// Every catalogue, contract and metering route these clients call is
/// authenticated, so a deployment without a credential gets 401s on the lookups
/// every invoice depends on rather than a diagnosable failure.
fn upstream(name: &'static str, base_url: &str, api_key: Option<String>) -> Upstream {
    Upstream::new(
        name,
        base_url,
        api_key.map(secrecy::SecretString::from),
        mako_service::http::default_client(),
    )
}

// ── ProductdClient ─────────────────────────────────────────────────────────────

pub struct ProductdClient {
    up: Upstream,
}

impl ProductdClient {
    pub fn new(base_url: &str, api_key: Option<String>) -> Self {
        Self {
            up: upstream("productd", base_url, api_key),
        }
    }

    /// `POST /api/v1/products/{lf_mp_id}/resolve`
    ///
    /// The product definitions for a list of (code, date) pairs, in one round
    /// trip. A period split by a Tarifwechsel needs one version per leg, each
    /// valid on that leg's own dates; asking per leg is an N+1 on every invoice.
    ///
    /// A code with no version valid on its date comes back as `None` **in
    /// place**, so the caller can name which leg is unpriceable rather than
    /// getting a shorter list than it asked for.
    pub async fn resolve_products(
        &self,
        lf_mp_id: &str,
        anfragen: &[(String, time::Date)],
    ) -> Result<Vec<Option<Product>>> {
        if anfragen.is_empty() {
            return Ok(Vec::new());
        }
        let path = format!("/api/v1/products/{lf_mp_id}/resolve");
        let body = serde_json::json!({
            "anfragen": anfragen
                .iter()
                .map(|(code, as_of)| serde_json::json!({
                    "product_code": code,
                    "as_of": as_of.to_string(),
                }))
                .collect::<Vec<_>>(),
        });
        let payload: serde_json::Value = self
            .up
            .json(self.up.post(&path).json(&body))
            .await
            .context("productd POST products/resolve")?
            .context("productd knows no products for this Lieferant")?;
        let mut out = Vec::with_capacity(anfragen.len());
        for entry in payload
            .get("produkte")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            let product = entry.get("product").filter(|p| !p.is_null());
            out.push(match product {
                Some(p) => Some(extract_tariff_from_product_data(p.get("data"), Some(p))?),
                None => None,
            });
        }
        Ok(out)
    }

    /// `GET /api/v1/nehs-prices/latest?date=…` — most recent nEHS certificate
    /// price (EUR/t CO₂) at or before `date`. Since 2026 the nEHS price is
    /// auction-formed (§10 Abs. 1 BEHG); billingd derives the Gas CO₂
    /// component from this series when no explicit override is configured.
    pub async fn get_latest_nehs_price(&self, date: time::Date) -> Result<Option<Decimal>> {
        let request = self
            .up
            .get("/api/v1/nehs-prices/latest")
            .query(&[("date", date.to_string())]);
        let Some(body) = self
            .up
            .json::<serde_json::Value>(request)
            .await
            .context("productd GET nehs-prices latest")?
        else {
            return Ok(None);
        };
        Ok(body
            .get("eur_per_t")
            .and_then(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .or_else(|| v.as_f64().map(|f| f.to_string()))
            })
            .and_then(|s| s.parse::<Decimal>().ok()))
    }

    /// Fetch §41a EPEX spot prices for `[period_from, period_to]`, keyed on the
    /// UTC start instant of each 15-minute market time unit (the DST-safe key
    /// used by `energy-billing`). productd returns 15-min points (legacy hourly
    /// data is expanded server-side).
    pub async fn get_epex_prices(
        &self,
        period_from: time::Date,
        period_to: time::Date,
    ) -> Result<HashMap<time::OffsetDateTime, Decimal>> {
        use time::format_description::well_known::Rfc3339;
        let mut map = HashMap::new();
        let mut day = period_from;
        while day <= period_to {
            let path = format!("/api/v1/epex-prices/{day}/quarter-hourly");
            let body: serde_json::Value = match self
                .up
                .json(self.up.get(&path))
                .await
                .context("productd GET epex-prices quarter-hourly")?
            {
                Some(b) => b,
                None => {
                    tracing::warn!(date = %day, "billingd: EPEX prices not found for date");
                    day = day.next_day().unwrap_or(day);
                    continue;
                }
            };
            let entries = body
                .get("prices")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            for entry in &entries {
                let Some(mtu_start) = entry
                    .get("mtu_start")
                    .and_then(|v| v.as_str())
                    .and_then(|s| time::OffsetDateTime::parse(s, &Rfc3339).ok())
                else {
                    continue;
                };
                // A missing or unparsable price is an error, not 0 ct/kWh: a
                // zero silently bills the interval free of charge. The §41a path
                // refuses to bill an interval it has no price for, so surfacing
                // the failure here keeps that guard reachable.
                let price_ct = decimal_from_json(entry.get("price_ct_kwh")).ok_or_else(|| {
                    anyhow::anyhow!(
                        "productd epex: price_ct_kwh missing or not a decimal for MTU {mtu_start}"
                    )
                })?;
                map.insert(mtu_start, price_ct);
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
/// longer accepted after the `productd` hard-cut.
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
/// `productd` normalises all `preistyp` values to canonical ALLCAPS on PUT
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
/// | mako extensions | — | see constants in `productd::handlers` |
///
/// ## Price extraction
///
/// `preisstaffeln[0].preis` is a scalar `Decimal` (string or JSON number) after
/// `productd` normalisation.  The first staffel is the base price.
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
    let mut sect14a_modul1_pauschale_eur_per_kw_year: Option<Decimal> = None;
    let mut sect14a_steuerungsentschaedigung_eur_per_kw_year: Option<Decimal> = None;
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
    // Priced positions this mapper has no field for. A silently dropped
    // position is money the customer is not charged (or not credited), and it
    // looks exactly like a product that never had one — the same failure shape
    // as `KEIN_ARBEITSPREIS`, which is why that one is an Error-severity engine
    // finding. This mapper cannot refuse (a catalog may legitimately carry
    // positions billingd does not model), so it says so instead.
    let mut dropped: Vec<String> = Vec::new();

    for pp in &preispositionen {
        // ALLCAPS after productd normalisation. A BO4E-defined type sits in
        // `preistyp`; a mako extension (EEG_MARKTPRAEMIE, HEMS_*, EMOBILITY_*)
        // sits in the `mako:preistyp` ZusatzAttribut, because writing it into
        // BO4E's own enum field made the stored Tarifpreisblatt invalid BO4E.
        let pt = mako_markt::bo4e::position_preistyp(pp);

        // preisstaffeln[0].preis is a scalar Decimal (string or number); a
        // nested {"wert": "..."} object is not BO4E and is never stored.
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
            // Strom and Gas take their demand charge from the typed `data`
            // keys (`leistungspreis_strom_ct_per_kw_month`,
            // `gas_leistungspreis_ct_per_kw_month`), which state the unit.
            // A `LEISTUNGSPREIS` *position* cannot be mapped there: productd
            // normalises `preis` to a bare scalar, so the BO4E `einheit` is
            // gone by the time it reaches here — and Wärme's field is
            // EUR/kW/month while Strom's is ct/kW/month. Guessing would risk a
            // hundredfold error on a demand charge, so it is reported instead.
            ("LEISTUNGSPREIS", other) => dropped.push(format!("LEISTUNGSPREIS ({other})")),

            ("SOLAR_ARBEITSPREIS", _) => solar_arbeitspreis_ct_per_kwh = preis,
            ("MIETERSTROM_AUFSCHLAG", _) => mieterstrom_aufschlag_ct_per_kwh = preis,
            ("GEMEINSCHAFT_RABATT", _) => gemeinschaft_rabatt_ct_per_kwh = preis,
            ("EEG_VERGUETUNG", _) => eeg_verguetungssatz_ct_per_kwh = preis,
            ("EEG_MARKTPRAEMIE", _) => eeg_marktpraemie_ct_per_kwh = preis,
            ("EEG_MANAGEMENTPRAEMIE", _) => eeg_managementpraemie_ct_per_kwh = preis,
            ("KWKG_ZUSCHLAG", _) => kwkg_zuschlag_ct_per_kwh = preis,
            ("MARKTWERT", _) => marktwert_ct_per_kwh = preis,
            ("VERMARKTUNGSGEBUEHR", _) => vermarktungsgebuehr_ct_per_kwh = preis,
            ("STEUERUNGSRABATT_MODUL1", _) => sect14a_modul1_pauschale_eur_per_kw_year = preis,
            ("STEUERUNGSRABATT_MODUL3", _) => {
                sect14a_steuerungsentschaedigung_eur_per_kw_year = preis
            }
            ("HEMS_PLATTFORMGEBUEHR", _) => hems_subscription_eur_per_month_from_code = preis,
            ("HEMS_OPTIMIERUNGSEVENT", _) => hems_optimization_event_eur = preis,
            ("HEMS_AUSLESUNG", _) => hems_readout_event_eur = preis,
            ("EMOBILITY_SERVICEGEBUEHR", _) => emobility_service_fee_eur_from_code = preis,
            ("EMOBILITY_ARBEITSPREIS", _) => emobility_kwh_price_ct_from_code = preis,
            ("EMOBILITY_SESSION", _) => emobility_session_fee_eur = preis,
            ("EMOBILITY_ROAMING", _) => emobility_roaming_fee_eur = preis,
            ("SERVICE_GEBUEHR", _) => service_fee_eur = preis,
            ("SERVICE_EVENT", _) => service_event_price_eur = preis,
            ("", _) => {}
            (other, _) => dropped.push((*other).to_owned()),
        }
    }

    if !dropped.is_empty() {
        tracing::warn!(
            category = %category,
            product_code = product
                .and_then(|p| p.get("product_code"))
                .and_then(|v| v.as_str())
                .unwrap_or("<unnamed>"),
            dropped = %dropped.join(", "),
            "billingd: productd product carries priced positions this mapper has no field for — \
             they are absent from every invoice for this product. Either the preistyp is \
             misspelled in the catalog, or billingd needs a mapping for it."
        );
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
        "sect14a_modul2_nne_reduktion_ct_per_kwh": get_decimal("sect14a_modul2_nne_reduktion_ct_per_kwh"),
        "sect14a_steuerungsentschaedigung_ct_per_kwh": get_decimal("sect14a_steuerungsentschaedigung_ct_per_kwh"),
        "sect14a_modul1_pauschale_eur_per_kw_year": sect14a_modul1_pauschale_eur_per_kw_year,
        "sect14a_steuerungsentschaedigung_eur_per_kw_year": sect14a_steuerungsentschaedigung_eur_per_kw_year,
        "gas_grundpreis_ct_per_day": gas_grundpreis_ct_per_day,
        "gas_arbeitspreis_ct_per_kwh_hs": gas_arbeitspreis_ct_per_kwh_hs,
        "gas_leistungspreis_ct_per_kw_month": get_decimal("gas_leistungspreis_ct_per_kw_month"),
        "gas_indexed_price": gas_indexed_price,
        "gas_energiesteuer_befreiung": product.and_then(|p| p.get("gas_energiesteuer_befreiung")).and_then(|v| v.as_bool()).unwrap_or(false),
        "waerme_grundpreis_eur_per_month": waerme_grundpreis_eur_per_month,
        "waerme_arbeitspreis_ct_per_kwh": waerme_arbeitspreis_ct_per_kwh,
        "waerme_leistungspreis_eur_per_kw_month": waerme_leistungspreis_eur_per_kw_month,
        "waerme_leistungspreis_eur_per_kw_year": get_decimal("waerme_leistungspreis_eur_per_kw_year"),
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
        .map_err(|e| anyhow::anyhow!("product deserialization from productd JSONB: {e}"))
}

/// One valid-time slice of a MaLo's product assignment, clipped to the period.
///
/// Comes from `vertragd`: which product a customer is on is a contract fact,
/// agreed under § 41 Abs. 5 EnWG, so it lives with the contract. The product's
/// *prices* come from `productd`, resolved by code and date.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ProductSlice {
    pub product_code: String,
    pub gueltig_von: time::Date,
    /// Exclusive end; `None` when the slice runs past the requested period.
    #[serde(default)]
    pub gueltig_bis: Option<time::Date>,
}

impl ProductSlice {
    /// The last day this slice covers. `gueltig_bis` is exclusive.
    #[must_use]
    pub fn last_day(&self, period_to: time::Date) -> time::Date {
        self.gueltig_bis
            .and_then(|to| to.previous_day())
            .map_or(period_to, |d| d.min(period_to))
    }
}

#[derive(Debug, Default, serde::Deserialize)]
struct SliceResponse {
    #[serde(default)]
    slices: Vec<ProductSlice>,
}

#[cfg(test)]
mod slice_tests {
    use super::ProductSlice;
    use time::macros::date;

    fn slice(from: time::Date, to: Option<time::Date>) -> ProductSlice {
        ProductSlice {
            product_code: "P".into(),
            gueltig_von: from,
            gueltig_bis: to,
        }
    }

    #[test]
    fn an_exclusive_end_becomes_an_inclusive_last_day() {
        // vertragd reports [01-03, 15-03); the leg it bills ends on the 14th.
        // Reading the exclusive bound as the last day billed the 15th twice —
        // once under each tariff.
        let s = slice(date!(2026 - 03 - 01), Some(date!(2026 - 03 - 15)));
        assert_eq!(s.last_day(date!(2026 - 03 - 31)), date!(2026 - 03 - 14));
    }

    #[test]
    fn an_open_slice_ends_with_the_period() {
        let s = slice(date!(2026 - 03 - 01), None);
        assert_eq!(s.last_day(date!(2026 - 03 - 31)), date!(2026 - 03 - 31));
    }

    #[test]
    fn a_slice_running_past_the_period_is_clipped_to_it() {
        let s = slice(date!(2026 - 03 - 01), Some(date!(2026 - 06 - 01)));
        assert_eq!(s.last_day(date!(2026 - 03 - 31)), date!(2026 - 03 - 31));
    }

    #[test]
    fn consecutive_slices_tile_the_period_without_a_gap_or_an_overlap() {
        let period_to = date!(2026 - 03 - 31);
        let a = slice(date!(2026 - 03 - 01), Some(date!(2026 - 03 - 15)));
        let b = slice(date!(2026 - 03 - 15), Some(date!(2026 - 04 - 01)));
        assert_eq!(a.last_day(period_to), date!(2026 - 03 - 14));
        assert_eq!(
            b.gueltig_von,
            a.last_day(period_to).next_day().unwrap(),
            "the second leg starts the day after the first ends"
        );
        assert_eq!(b.last_day(period_to), period_to);
    }
}

// ── EdmdClient ────────────────────────────────────────────────────────────────

pub struct EdmdClient {
    up: Upstream,
}

impl EdmdClient {
    pub fn new(base_url: &str, api_key: Option<String>) -> Self {
        Self {
            up: upstream("edmd", base_url, api_key),
        }
    }

    /// `GET /api/v1/billing-period/{malo_id}?from=…&to=…`
    pub async fn get_billing_period(
        &self,
        malo_id: &str,
        period_from: time::Date,
        period_to: time::Date,
    ) -> Result<Option<MeterInput>> {
        let path = format!("/api/v1/billing-period/{malo_id}");
        let request = self.up.get(&path).query(&[
            ("from", period_from.to_string()),
            ("to", period_to.to_string()),
        ]);
        let Some(body) = self
            .up
            .json::<serde_json::Value>(request)
            .await
            .context("edmd GET billing-period")?
        else {
            return Ok(None);
        };
        let meter = MeterInput {
            arbeitsmenge_kwh: decimal_from_json(body.get("arbeitsmenge_kwh"))
                .unwrap_or(Decimal::ZERO),
            arbeitsmenge_ht_kwh: decimal_from_json(body.get("arbeitsmenge_ht_kwh")),
            arbeitsmenge_nt_kwh: decimal_from_json(body.get("arbeitsmenge_nt_kwh")),
            spitzenleistung_kw: decimal_from_json(body.get("spitzenleistung_kw")),
            steuerung_stunden: decimal_from_json(body.get("steuerung_stunden")),
            // §40 Abs. 2 Nr. 6 EnWG — start/end register readings for the bill.
            zaehlerstand_von: decimal_from_json(body.get("zaehlerstand_anfang")),
            zaehlerstand_bis: decimal_from_json(body.get("zaehlerstand_ende")),
            // §40a EnWG — how the readings were determined: edmd collapses the
            // contributing reads to a worst-quality flag; anything that is not
            // a real measurement must be labeled as estimated on the bill.
            is_estimated: quality_is_estimated(&body),
            metering_mode: metering_mode_from_messtyp(&body),
            ..Default::default()
        };
        Ok(Some(meter))
    }

    /// Fetch Lastgang intervals for §41a dynamic billing.
    ///
    /// Calls `GET /api/v1/lastgang/{malo_id}?from={from}&to={to}` with
    /// **RFC3339** bounds (edmd rejects bare dates: an unparsable bound
    /// silently widened the window to epoch..now) and parses the **BO4E
    /// `Lastgang`** response shape edmd actually emits — one `Lastgang`
    /// object per OBIS code, each with `werte[]` of
    /// `{wert, status, zeitraum{startdatum, startuhrzeit}}`. The previous
    /// flat `{timestamp_utc, wert}` parser matched nothing, so every §41a
    /// dynamic run saw zero intervals and hard-blocked.
    ///
    /// Returns an empty Vec when the MaLo has no Lastgang data.
    pub async fn get_lastgang(
        &self,
        malo_id: &str,
        period_from: time::Date,
        period_to: time::Date,
    ) -> Result<Vec<DynamicInterval>> {
        // Whole-day window: [from 00:00Z, day-after-to 00:00Z).
        let from_dt = period_from.midnight().assume_utc();
        let to_dt = (period_to + time::Duration::days(1))
            .midnight()
            .assume_utc();
        let rfc3339 = time::format_description::well_known::Rfc3339;
        let path = format!("/api/v1/lastgang/{malo_id}");
        let request = self.up.get(&path).query(&[
            ("from", from_dt.format(&rfc3339).context("format from")?),
            ("to", to_dt.format(&rfc3339).context("format to")?),
        ]);
        let Some(body) = self
            .up
            .json::<serde_json::Value>(request)
            .await
            .context("edmd GET lastgang")?
        else {
            return Ok(Vec::new());
        };
        let mut intervals: Vec<DynamicInterval> = body
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .flat_map(|lastgang| {
                lastgang
                    .get("werte")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            })
            .filter_map(|w| {
                let kwh = decimal_from_json(w.get("wert"))?;
                let zeitraum = w.get("zeitraum")?;
                let datum = zeitraum.get("startdatum")?.as_str()?;
                // `startuhrzeit` is `HH:MM:SS+00:00`; missing → midnight.
                let uhrzeit = zeitraum
                    .get("startuhrzeit")
                    .and_then(|v| v.as_str())
                    .unwrap_or("00:00:00+00:00");
                let ts = time::OffsetDateTime::parse(
                    &format!("{datum}T{uhrzeit}"),
                    &time::format_description::well_known::Rfc3339,
                )
                .ok()?;
                Some(DynamicInterval {
                    timestamp_utc: ts,
                    kwh,
                })
            })
            .collect();
        // Multiple OBIS Lastgänge arrive concatenated — price lookup needs
        // chronological order.
        intervals.sort_by_key(|i| i.timestamp_utc);
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
        let path = format!("/api/v1/billing-period/{malo_id}");
        let request = self.up.get(&path).query(&[
            ("from", period_from.to_string()),
            ("to", period_to.to_string()),
        ]);
        let Some(body) = self
            .up
            .json::<serde_json::Value>(request)
            .await
            .context("edmd GET billing-period (gas)")?
        else {
            return Ok(None);
        };

        // edmd's `MeterBillingPeriod` identifies the commodity via `sparte` and
        // reports gas energy as `arbeitsmenge_kwh` (kWh_Hs) with the applied
        // `brennwert_kwh_per_m3`/`zustandszahl` alongside — it never emits a
        // raw `messung_qm3` volume. The old volume-based detection therefore
        // matched nothing, and gas MaLos silently fell back to defaults.
        let sparte = body.get("sparte").and_then(|v| v.as_str()).unwrap_or("");
        if sparte != "GAS" {
            return Ok(None);
        }

        Ok(Some(GasBillingPeriod {
            kwh_hs: decimal_from_json(body.get("arbeitsmenge_kwh")),
            brennwert_kwh_per_qm3: decimal_from_json(body.get("brennwert_kwh_per_m3")),
            zustandszahl: decimal_from_json(body.get("zustandszahl")),
            spitzenleistung_kw: decimal_from_json(body.get("spitzenleistung_kw")),
            // §40 Abs. 2 Nr. 6 EnWG — register readings (m³ at the gas meter).
            zaehlerstand_von: decimal_from_json(body.get("zaehlerstand_anfang")),
            zaehlerstand_bis: decimal_from_json(body.get("zaehlerstand_ende")),
            is_estimated: quality_is_estimated(&body),
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
        let path = format!("/api/v1/gas-quality/{malo_id}");
        let Some(body) = self
            .up
            .json::<serde_json::Value>(self.up.get(&path))
            .await
            .context("edmd GET gas-quality")?
        else {
            return Ok(None);
        };
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
    /// Gas energy in kWh_Hs — edmd's `arbeitsmenge_kwh` for `sparte = GAS`
    /// (the Brennwert conversion is already applied by the DSO's MSCONS data).
    pub kwh_hs: Option<Decimal>,
    /// Abrechnungsbrennwert in kWh/m³ (from edmd, sourced from MSCONS PID 13007).
    pub brennwert_kwh_per_qm3: Option<Decimal>,
    /// Zustandszahl — dimensionless volume conversion factor.
    pub zustandszahl: Option<Decimal>,
    /// Peak demand in kW (Spitzenleistung) for Gas RLM billing.
    pub spitzenleistung_kw: Option<Decimal>,
    /// Register reading at period start, m³ (§40 Abs. 2 Nr. 6 EnWG).
    pub zaehlerstand_von: Option<Decimal>,
    /// Register reading at period end, m³ (§40 Abs. 2 Nr. 6 EnWG).
    pub zaehlerstand_bis: Option<Decimal>,
    /// The period's worst quality flag was an estimate/Ersatzwert (§40a EnWG).
    pub is_estimated: bool,
}

/// `true` when the period's collapsed quality flag means the value was not a
/// real measurement (§40a EnWG: estimation must be labeled on the bill).
fn quality_is_estimated(body: &serde_json::Value) -> bool {
    matches!(
        body.get("quality").and_then(|v| v.as_str()),
        Some("ESTIMATED" | "SUBSTITUTED" | "CALCULATED" | "PRELIMINARY")
    )
}

/// Map edmd's `messtyp` (SLP/RLM/IMSYS) onto the engine's `MeteringMode`.
fn metering_mode_from_messtyp(body: &serde_json::Value) -> energy_billing::MeteringMode {
    match body.get("messtyp").and_then(|v| v.as_str()) {
        Some("RLM") => energy_billing::MeteringMode::Rlm,
        Some("IMSYS") => energy_billing::MeteringMode::Imsys,
        _ => energy_billing::MeteringMode::Slp,
    }
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
/// - List active MaLo IDs for a Rahmenvertrag (Sammelrechnung)
pub struct VertragdClient {
    /// Every contract route there is authenticated — the responses carry
    /// customer master data and the buyer terms an invoice is addressed to — so
    /// a deployment without a credential gets 401s rather than silently
    /// unaddressed invoices.
    up: Upstream,
}

impl VertragdClient {
    pub fn new(base_url: &str, api_key: Option<String>) -> Self {
        Self {
            up: upstream("vertragd", base_url, api_key),
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
    ) -> Result<RahmenvertragSites> {
        let path = format!("/api/v1/rahmenvertraege/{rahmenvertrag_id}/malos");
        Ok(self
            .up
            .json(self.up.get(&path))
            .await
            .context("vertragd GET rahmenvertrag malos")?
            .unwrap_or_default())
    }

    /// `GET /api/v1/malo/{malo_id}/produkte?from=…&to=…`
    ///
    /// The product-assignment slices covering a billing period, in order.
    ///
    /// An invoice covers a period and a Tarifwechsel inside it splits that
    /// period; asking only for "the current product" billed the whole period at
    /// whichever tariff happened to be in force on the day the run executed.
    /// More than one slice means the period contains a price change.
    ///
    /// The mapping lives in `vertragd` because agreeing it *is* a contract act
    /// (§ 41 Abs. 5 EnWG); `productd` then prices each code.
    pub async fn get_product_slices(
        &self,
        malo_id: &str,
        from: time::Date,
        to: time::Date,
    ) -> Result<Vec<ProductSlice>> {
        let path = format!("/api/v1/malo/{malo_id}/produkte");
        let request = self
            .up
            .get(&path)
            .query(&[("from", from.to_string()), ("to", to.to_string())]);
        Ok(self
            .up
            .json::<SliceResponse>(request)
            .await
            .context("vertragd GET malo produkte")?
            .map(|b| b.slices)
            .unwrap_or_default())
    }

    /// `GET /api/v1/vertraege/by-malo/{malo_id}`
    ///
    /// The active Versorgungsvertrag behind a MaLo — the source of the §40
    /// Abs. 1 EnWG contract facts (Vertragsdauer, Kündigungsfrist, next
    /// possible Kündigungstermin) the invoice must state. `Ok(None)` when the
    /// MaLo has no active contract in vertragd.
    pub async fn get_vertrag_by_malo(&self, malo_id: &str) -> Result<Option<VertragByMalo>> {
        let path = format!("/api/v1/vertraege/by-malo/{malo_id}");
        self.up
            .json(self.up.get(&path))
            .await
            .context("vertragd GET vertrag by malo")
    }

    /// `GET /api/v1/ggv/{ggv_id}/betreiber`
    ///
    /// The § 42b GGV operator behind the community id — the BG-7 buyer of the
    /// bundled GGV Sammelrechnung. A Kunde in vertragd (the operator has no
    /// MP-ID and never appears in MaKo), so this is the same buyer master
    /// every other e-invoice path resolves from. `Ok(None)` until a Betreiber
    /// is recorded; the bundle then ships with its buyer findings, exactly as
    /// an unconfigured retail buyer does.
    pub async fn get_ggv_betreiber(&self, ggv_id: &str) -> Result<Option<Rechnungsempfaenger>> {
        #[derive(serde::Deserialize)]
        struct Answer {
            rechnungsempfaenger: Option<Rechnungsempfaenger>,
        }
        let path = format!("/api/v1/ggv/{ggv_id}/betreiber");
        Ok(self
            .up
            .json::<Answer>(self.up.get(&path))
            .await
            .context("vertragd GET ggv betreiber")?
            .and_then(|a| a.rechnungsempfaenger))
    }

    /// `GET /api/v1/aggregatorvertraege?sr_id={sr_id}&on={date}`
    ///
    /// The §41e EnWG Aggregatorvertrag in force for a SteuerbareRessource on
    /// `on_date`. `billingd` keeps no copy of this contract — it is Contract-
    /// context master data owned by `vertragd`.
    ///
    /// The date is the day the dispatch was *executed*, not the day the webhook
    /// is processed, so a replayed or delayed event still bills under the
    /// contract that was in force when the flexibility was delivered.
    /// `Ok(None)` when no contract was in force.
    pub async fn get_aggregatorvertrag(
        &self,
        sr_id: &str,
        on_date: time::Date,
    ) -> Result<Option<Aggregatorvertrag>> {
        let request = self
            .up
            .get("/api/v1/aggregatorvertraege")
            .query(&[("sr_id", sr_id.to_owned()), ("on", on_date.to_string())]);
        self.up
            .json(request)
            .await
            .context("vertragd GET aggregatorvertrag")
    }

    /// `GET /api/v1/vertraege/billing-candidates`
    ///
    /// Active supply components with their §40b EnWG billing cadence — the
    /// work list for the scheduled billing-run worker.
    pub async fn get_billing_candidates(&self) -> Result<Vec<BillingCandidate>> {
        let body: serde_json::Value = self
            .up
            .json(self.up.get("/api/v1/vertraege/billing-candidates"))
            .await
            .context("vertragd GET billing-candidates")?
            .unwrap_or_default();
        let candidates = body
            .get("candidates")
            .cloned()
            .unwrap_or(serde_json::Value::Array(vec![]));
        serde_json::from_value(candidates).context("deserialize billing candidates")
    }
}

/// One entry of `GET /api/v1/vertraege/billing-candidates`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct BillingCandidate {
    pub malo_id: String,
    pub lf_mp_id: String,
    #[serde(default)]
    pub nb_mp_id: Option<String>,
    pub sparte: String,
    /// §40b EnWG cadence: MONATLICH / VIERTELJAEHRLICH / HALBJAEHRLICH / JAEHRLICH.
    pub abrechnungszyklus: String,
    pub vertragsbeginn: time::Date,
    #[serde(default)]
    pub vertragsende: Option<time::Date>,
    pub lieferbeginn: time::Date,
    #[serde(default)]
    pub lieferende: Option<time::Date>,
}

/// Response of `GET /api/v1/vertraege/by-malo/{malo_id}`.
#[derive(Debug, serde::Deserialize)]
pub struct VertragByMalo {
    /// The contract row — only the fields billing needs are read.
    pub vertrag: VertragFacts,
    /// The BG-7 buyer behind the MaLo, when vertragd has a Kunde on file.
    ///
    /// `billingd` keeps no customer master. Without this the e-invoice buyer is
    /// synthesised from the MaLo-ID and the document fails XRechnung on its
    /// address terms — see `einvoice::buyer_party`.
    #[serde(default)]
    pub rechnungsempfaenger: Option<Rechnungsempfaenger>,
    /// Next possible Kündigungstermin, computed by vertragd as of today.
    pub naechstmoeglicher_kuendigungstermin: Option<time::Date>,
}

/// BG-7 BUYER terms owned by `vertragd`, mirrored here for e-invoicing.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Rechnungsempfaenger {
    /// BT-44 buyer name.
    pub name: Option<String>,
    /// BT-50 address line.
    pub line1: Option<String>,
    /// BT-53 post code.
    pub post_code: Option<String>,
    /// BT-52 city.
    pub city: Option<String>,
    /// BT-55 country code.
    pub country: Option<String>,
    /// BT-48 buyer VAT identifier (B2B only).
    pub vat_id: Option<String>,
    /// § 13b Abs. 2 Nr. 5 lit. b UStG — the buyer is a Stromwiederverkäufer;
    /// billingd derives reverse charge from this master-data flag.
    #[serde(default)]
    pub stromwiederverkaeufer: bool,
    /// Where an electronic document reaches them. No EN 16931 BT carries it;
    /// it is here because the party an invoice is addressed to is the party it
    /// is sent to, and resolving those separately is how a document ends up
    /// addressed to one person and delivered to another.
    #[serde(default)]
    pub email: Option<String>,
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

/// The sites under a Rahmenvertrag plus the holder the bundled invoice addresses.
///
/// A Sammelrechnung bills the **framework-contract holder**, not any one site's
/// supply customer, so the BG-7 buyer cannot be derived from the MaLo list and
/// travels with it.
#[derive(Debug, Default, serde::Deserialize)]
pub struct RahmenvertragSites {
    #[serde(default)]
    pub malos: Vec<RahmenvertragMaloEntry>,
    #[serde(default)]
    pub rechnungsempfaenger: Option<Rechnungsempfaenger>,
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

/// A §41e EnWG Aggregatorvertrag as returned by `vertragd`.
///
/// Mirrors `vertragd::pg::AggregatorvertragRow`; only the fields `billingd`
/// needs to settle a dispatch are modelled.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Aggregatorvertrag {
    pub sr_id: String,
    pub vpp_id: String,
    pub malo_id: String,
    /// Aggregator market-partner ID — the invoicing party.
    pub aggregator_mp_id: String,
    pub capacity_price_eur_per_kwh: rust_decimal::Decimal,
    /// `None` = use the billingd default MwSt rate.
    #[serde(default)]
    pub mwst_rate_override: Option<rust_decimal::Decimal>,
}

// ── OutputdClient ─────────────────────────────────────────────────────────────

/// What `outputd` answered: the ZUGFeRD PDF and the template that rendered it.
pub struct RenderedDocument {
    pub pdf: Vec<u8>,
    /// `X-Mako-Template-Hash` — pin this next to the record so the document is
    /// reproducible for as long as it must be kept (§ 147 AO / GoBD).
    pub template_hash: String,
}

/// Why a render did not come back — split so the HTTP layer can answer with a
/// status that names the right party. outputd's deterministic refusals (no
/// template rolled out, a payload it cannot carry) are the *request's* fault
/// and re-occur on every retry: reporting them as a gateway error points the
/// operator at infrastructure and invites retries that cannot succeed.
#[derive(Debug, thiserror::Error)]
pub enum OutputdError {
    /// outputd answered 4xx: the request (or the record behind it) is at
    /// fault, and the message says what to fix. Relay as `422`.
    #[error("outputd refused the render ({status}): {message}")]
    Refused {
        status: reqwest::StatusCode,
        message: String,
    },
    /// Transport failure or an outputd 5xx — the renderer, not the request.
    /// Report as `502`.
    #[error(transparent)]
    Unavailable(#[from] anyhow::Error),
}

/// Client for `outputd`, the customer-document renderer.
///
/// billingd owns what an invoice *says* (the stored EN 16931 model and the CII
/// rendered from it); outputd owns what it *looks like* (the operator's Typst
/// template, the PDF/A-3 carrier, the publish gate) — and, because it is the
/// side the gate runs on, the projection of the model onto what a template may
/// print. This client carries the model and the payload across that boundary
/// and brings back the PDF plus the template hash to pin.
pub struct OutputdClient {
    up: Upstream,
}

impl OutputdClient {
    pub fn new(base_url: &str, api_key: Option<String>) -> Self {
        // The shared 30 s request timeout stays above outputd's 20 s render
        // budget (which already includes queueing for a render slot), so a slow
        // render surfaces as outputd's own diagnostic rather than a transport
        // error here.
        Self {
            up: upstream("outputd", base_url, api_key),
        }
    }

    /// `POST /api/v1/render/INVOICE`.
    ///
    /// `template_hash = None` renders with the tenant's current template —
    /// outputd resolves it and names it in the response, which is what the
    /// caller then pins. `Some(hash)` reproduces a pinned document.
    ///
    /// Errors carry outputd's own message (a `Refused` names the missing
    /// template or the payload defect), prefixed so a log line says which hop
    /// failed.
    pub async fn render_invoice(
        &self,
        model: &en16931::Invoice,
        xml: String,
        specification_id: &str,
        template_hash: Option<&str>,
        date: time::Date,
        ident: &str,
    ) -> Result<RenderedDocument, OutputdError> {
        // The **semantic model**, not a projected view. outputd owns the
        // projection onto what a template may print, and it owns it once: the
        // publish gate proves every operator template against that projection,
        // so a second copy on this side is a contract the gate cannot see.
        let body = serde_json::json!({
            "model": model,
            "template_hash": template_hash,
            "attachment": { "xml": xml, "specification_id": specification_id },
            "date": date.to_string(),
            "ident": ident,
        });
        // Not `Upstream::send`: it maps a 404 to absence, and here a 404 *is*
        // the answer — outputd saying the pinned template does not exist, which
        // is a refusal the caller must see with its message.
        let resp = self
            .up
            .post("/api/v1/render/INVOICE")
            .json(&body)
            .send()
            .await
            .context("outputd POST render")?;
        let status = resp.status();
        if status.is_client_error() {
            let message = resp.text().await.unwrap_or_default();
            return Err(OutputdError::Refused { status, message });
        }
        if !status.is_success() {
            let msg = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("outputd render failed ({status}): {msg}").into());
        }
        let template_hash = resp
            .headers()
            .get("X-Mako-Template-Hash")
            .and_then(|v| v.to_str().ok())
            .map(ToOwned::to_owned)
            .context("outputd response is missing X-Mako-Template-Hash")?;
        let pdf = resp.bytes().await.context("outputd render body")?.to_vec();
        Ok(RenderedDocument { pdf, template_hash })
    }

    /// `POST /api/v1/documents/INVOICE` — render, **record** and queue for
    /// delivery.
    ///
    /// The difference from [`Self::render_invoice`] is not the bytes but that
    /// they are kept and sent — which is what makes "reproduce exactly what was
    /// issued" (§ 14 Abs. 1 UStG, § 147 AO) and "did the customer receive it"
    /// answerable. Idempotent on the Rechnungsnummer.
    ///
    /// # Errors
    ///
    /// Propagates transport failures and outputd's own refusals.
    pub async fn issue_invoice(&self, req: &IssueInvoiceRequest<'_>) -> Result<IssuedDocument> {
        let body = serde_json::json!({
            "model":       req.model,
            "attachment":  { "xml": req.xml, "specification_id": req.specification_id },
            "template_hash": req.template_hash,
            "date":        req.date.to_string(),
            "ident":       req.ident,
            "subject_ref": req.rechnungsnummer,
            "malo_id":     req.malo_id,
            "kunden_nr":   req.kunden_nr,
            "recipient":   req.recipient,
            "channels":    req.channels,
        });
        self.up
            .json(self.up.post("/api/v1/documents/INVOICE").json(&body))
            .await
            .context("outputd POST document INVOICE")?
            .context("outputd answered 404 for the document endpoint — is it on this version?")
    }
}

/// An invoice to issue and deliver.
#[derive(Debug)]
pub struct IssueInvoiceRequest<'a> {
    pub model: &'a en16931::Invoice,
    pub xml: String,
    pub specification_id: &'a str,
    /// The template the record already pins, so a re-issue reproduces it.
    pub template_hash: Option<&'a str>,
    pub date: time::Date,
    pub ident: String,
    /// The § 14 Abs. 4 Nr. 4 UStG Rechnungsnummer — outputd's idempotency key.
    pub rechnungsnummer: &'a str,
    pub malo_id: &'a str,
    pub kunden_nr: Option<&'a str>,
    pub recipient: serde_json::Value,
    pub channels: Vec<String>,
}

/// What outputd recorded for an issued document.
#[derive(Debug, serde::Deserialize)]
pub struct IssuedDocument {
    pub document_id: uuid::Uuid,
    pub template_hash: String,
}

impl serde::Serialize for IssuedDocument {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct as _;
        let mut st = s.serialize_struct("IssuedDocument", 2)?;
        st.serialize_field("document_id", &self.document_id)?;
        st.serialize_field("template_hash", &self.template_hash)?;
        st.end()
    }
}

// ── BillingDeps ───────────────────────────────────────────────────────────────

/// Everything a calculation needs from outside this process.
///
/// One `Extension` instead of six. Taking `cfg`, `productd`, `edmd`, `marktd`,
/// `vertragd` and `outputd` as separate extractors pushes nearly every function
/// in the service past clippy's argument limit, and the `allow` for that is a
/// workaround for a lint that is right.
pub struct BillingDeps {
    pub cfg: Arc<crate::config::BillingdConfig>,
    pub productd: Arc<ProductdClient>,
    pub edmd: Arc<EdmdClient>,
    pub marktd: Arc<mako_markt::marktd_client::MarktdClient>,
    pub vertragd: Arc<VertragdClient>,
    pub outputd: Arc<OutputdClient>,
    /// The Massenkontokorrent. `None` when no `accountingd_url` is configured —
    /// a deployment that runs its own receivables elsewhere. The §40b sweep
    /// then cannot settle advances and says so instead of guessing zero.
    pub accountingd: Option<Arc<AccountingdClient>>,
}

impl BillingDeps {
    /// The operator's tenant — the resource identity every Cedar check and
    /// every query scopes on.
    #[must_use]
    pub fn tenant(&self) -> &str {
        &self.cfg.tenant
    }
}

// ── AccountingdClient ─────────────────────────────────────────────────────────

/// Client for `accountingd`, the Massenkontokorrent.
///
/// One read, and it does not invert the service graph: `accountingd` is
/// downstream of `billingd` for *events*, and billingd never asks it to compute
/// anything. What it asks for is the advance-payment register — which Abschläge
/// this Marktlokation paid and at which VAT rate, which only the ledger knows
/// and which § 40 Abs. 1 EnWG / § 14 Abs. 5 Satz 2 UStG make part of a lawful
/// settling invoice. The same read SAP IS-U billing makes against FI-CA.
pub struct AccountingdClient {
    up: Upstream,
}

impl AccountingdClient {
    pub fn new(base_url: &str, api_key: Option<String>) -> Self {
        Self {
            up: upstream("accountingd", base_url, api_key),
        }
    }

    /// `GET /api/v1/accounts/{malo_id}/abschlaege?from=&to=`
    ///
    /// The advances a settling invoice for `[from, to]` may deduct: received
    /// (§ 14 Abs. 5 Satz 2 UStG „vereinnahmte Teilentgelte"), not already
    /// absorbed by an earlier settlement, oldest first, each with its rate.
    ///
    /// # Errors
    ///
    /// Propagates transport and deserialisation failures. A `404` — the MaLo
    /// has no Kundenkonto — is `Ok(None)`, which is a different fact from
    /// accountingd being unreachable.
    pub async fn get_abschlaege(
        &self,
        malo_id: &str,
        from: time::Date,
        to: time::Date,
    ) -> Result<Option<Vec<energy_billing::AbschlagDeduction>>> {
        let path = format!("/api/v1/accounts/{malo_id}/abschlaege");
        let request = self
            .up
            .get(&path)
            .query(&[("from", from.to_string()), ("to", to.to_string())]);
        self.up
            .json(request)
            .await
            .context("accountingd GET abschlaege")
    }
}

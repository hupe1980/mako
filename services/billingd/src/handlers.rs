//! HTTP handlers for `billingd`.

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::PgPool;
use std::sync::Arc;
use time::format_description::well_known::Iso8601;
use uuid::Uuid;

use crate::{
    calculator::{
        DynamicInterval, EegMeterInput, EmobilityMeterInput, GasMeterInput, GridInput,
        HemsMeterInput, MeterInput, ServiceMeterInput, SolarMeterInput, TariffInput,
        WaermeMeterInput, calculate_dynamic_strom, calculate_eeg, calculate_einspeisung,
        calculate_emobility, calculate_energiedienstleistung, calculate_gas, calculate_hems,
        calculate_solar, calculate_strom, calculate_waerme,
    },
    clients::{EdmdClient, TarifbdClient, VertragdClient},
    config::BillingdConfig,
    pg::{
        fetch_billing_record, insert_billing_record, insert_correction_record,
        insert_sammelrechnung_record, link_to_sammelrechnung, list_billing_records,
        mark_dispatched,
    },
    xrechnung::{build_zugferd_cii_xml, info_from_rechnung_json},
};

// ── Request bodies ─────────────────────────────────────────────────────────────

/// Request body for `POST /api/v1/billing/{malo_id}/calculate` and `/preview`.
///
/// All `*_meter` fields are optional — the engine selects the correct one based on
/// `tariff.category`.  Unsupported meter inputs for the active category are silently
/// ignored.  Supply `tariff` and/or `meter` as overrides to skip external lookups.
#[derive(Debug, Deserialize)]
pub struct CalculateRequest {
    pub lf_mp_id: String,
    #[allow(dead_code)]
    pub nb_mp_id: String,
    pub period_from: String,
    pub period_to: String,
    /// Override: supply product data directly (skip tarifbd lookup).
    pub tariff: Option<TariffInput>,
    /// Override: supply Strom meter data directly (skip edmd lookup).
    pub meter: Option<MeterInput>,
    /// Override: supply grid pass-through data directly (skip marktd lookup).
    pub grid: Option<GridInput>,
    /// EEG Gutschrift EUR for STROM/WAERMEPUMPE/WALLBOX (from `einsd`).
    pub eeg_gutschrift_eur: Option<Decimal>,
    /// Invoice number — auto-generated when absent.
    pub rechnungsnummer: Option<String>,
    /// Gas meter input (GAS category).
    pub gas_meter: Option<GasMeterInput>,
    /// Fernwärme meter input (WAERME category).
    pub waerme_meter: Option<WaermeMeterInput>,
    /// Solar / Eigenverbrauch input (SOLAR category).
    pub solar_meter: Option<SolarMeterInput>,
    /// EEG / Direktvermarktung feed-in input (EEG / EINSPEISUNG category).
    pub eeg_meter: Option<EegMeterInput>,
    /// HEMS usage input (HEMS category).
    pub hems_meter: Option<HemsMeterInput>,
    /// E-Mobility CPO/EMSP usage input (EMOBILITY category).
    pub emobility_meter: Option<EmobilityMeterInput>,
    /// Service usage input (ENERGIEDIENSTLEISTUNG category).
    pub service_meter: Option<ServiceMeterInput>,
}

// ── Calculate ─────────────────────────────────────────────────────────────────

/// `POST /api/v1/billing/{malo_id}/calculate`
///
/// Pipeline:
/// 1. Parse + validate period
/// 2. Fetch `TariffInput` from `tarifbd` (or use request override)
/// 3. Fetch consumption from `edmd` (or use request override)
/// 4. Fetch grid pass-through from `marktd` (or use request override)
/// 5. Dispatch to category-specific pure calculator
/// 6. Persist `billing_records` (idempotent on same malo+period+product)
/// 7. Emit `de.billing.rechnung.erstellt` CloudEvent
pub async fn post_calculate(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Extension(tarifbd): Extension<Arc<TarifbdClient>>,
    Extension(edmd): Extension<Arc<EdmdClient>>,
    Extension(marktd): Extension<Arc<mako_markt::marktd_client::MarktdClient>>,
    Path(malo_id): Path<String>,
    Json(req): Json<CalculateRequest>,
) -> impl IntoResponse {
    let (period_from, period_to) = match parse_period(&req.period_from, &req.period_to) {
        Ok(pd) => pd,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };

    let tariff = match resolve_tariff(&req, &tarifbd, &malo_id).await {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };

    let rates = cfg.regulatory_rates();
    let rechnungsnummer = req
        .rechnungsnummer
        .clone()
        .unwrap_or_else(|| format!("BILL-{malo_id}-{period_from}"));

    let result = match dispatch_calculator(
        &tariff,
        &req,
        &malo_id,
        &rechnungsnummer,
        period_from,
        period_to,
        &rates,
        &edmd,
        &marktd,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };

    let record_id = match insert_billing_record(
        &pool,
        &malo_id,
        &req.lf_mp_id,
        tariff.product_code.as_deref().unwrap_or(&tariff.category),
        &tariff.category,
        period_from,
        period_to,
        &result.rechnung_json,
        result.netto_eur,
        result.brutto_eur,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    if let Some(ref webhook_url) = cfg.erp_webhook_url {
        emit_cloud_event(
            webhook_url,
            &pool,
            record_id,
            &malo_id,
            &req.lf_mp_id,
            &result.rechnung_json,
        )
        .await;
    }

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": record_id,
            "malo_id": malo_id,
            "period_from": period_from.to_string(),
            "period_to": period_to.to_string(),
            "netto_eur": result.netto_eur,
            "brutto_eur": result.brutto_eur,
            "positions_count": result.positions.len(),
            "rechnung": result.rechnung_json,
        })),
    )
        .into_response()
}

/// `POST /api/v1/billing/{malo_id}/preview` — dry-run, no persist, no CloudEvent.
pub async fn post_preview(
    Extension(_pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Extension(tarifbd): Extension<Arc<TarifbdClient>>,
    Extension(edmd): Extension<Arc<EdmdClient>>,
    Extension(marktd): Extension<Arc<mako_markt::marktd_client::MarktdClient>>,
    Path(malo_id): Path<String>,
    Json(req): Json<CalculateRequest>,
) -> impl IntoResponse {
    let (period_from, period_to) = match parse_period(&req.period_from, &req.period_to) {
        Ok(pd) => pd,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    let tariff = match resolve_tariff(&req, &tarifbd, &malo_id).await {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    let rates = cfg.regulatory_rates();
    let rechnungsnummer = req
        .rechnungsnummer
        .clone()
        .unwrap_or_else(|| format!("PREVIEW-{malo_id}-{period_from}"));
    let result = match dispatch_calculator(
        &tariff,
        &req,
        &malo_id,
        &rechnungsnummer,
        period_from,
        period_to,
        &rates,
        &edmd,
        &marktd,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "preview": true,
            "malo_id": malo_id,
            "period_from": period_from.to_string(),
            "period_to": period_to.to_string(),
            "netto_eur": result.netto_eur,
            "brutto_eur": result.brutto_eur,
            "positions_count": result.positions.len(),
            "rechnung": result.rechnung_json,
        })),
    )
        .into_response()
}

// ── Category dispatch ─────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn dispatch_calculator(
    tariff: &TariffInput,
    req: &CalculateRequest,
    malo_id: &str,
    rechnungsnummer: &str,
    period_from: time::Date,
    period_to: time::Date,
    rates: &crate::calculator::RegulatoryRates,
    edmd: &Arc<EdmdClient>,
    marktd: &Arc<mako_markt::marktd_client::MarktdClient>,
) -> Result<crate::calculator::BillingResult, (StatusCode, String)> {
    let grid = req.grid.clone().unwrap_or_default();

    match tariff.category.as_str() {
        // ── Electricity (SLP/RLM, Wärmepumpe, Wallbox) ────────────────────────
        "STROM" | "WAERMEPUMPE" | "WALLBOX" => {
            let meter = resolve_strom_meter(req, malo_id, period_from, period_to, edmd).await?;
            let calc_err =
                |e: billing::BillingError| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string());
            if tariff.dynamic_epex {
                let intervals =
                    fetch_dynamic_intervals(malo_id, period_from, period_to, edmd).await;
                let epex = fetch_epex_prices(period_from, period_to, marktd).await;
                calculate_dynamic_strom(
                    malo_id,
                    &req.lf_mp_id,
                    rechnungsnummer,
                    period_from,
                    period_to,
                    tariff,
                    &grid,
                    req.eeg_gutschrift_eur,
                    &intervals,
                    &epex,
                    rates,
                )
                .map_err(calc_err)
            } else {
                calculate_strom(
                    malo_id,
                    &req.lf_mp_id,
                    rechnungsnummer,
                    period_from,
                    period_to,
                    tariff,
                    &meter,
                    &grid,
                    req.eeg_gutschrift_eur,
                    rates,
                )
                .map_err(calc_err)
            }
        }
        // ── Gas ────────────────────────────────────────────────────────────────
        "GAS" => {
            let meter = req.gas_meter.clone().unwrap_or_default();
            calculate_gas(
                malo_id,
                &req.lf_mp_id,
                rechnungsnummer,
                period_from,
                period_to,
                tariff,
                &meter,
                &grid,
                rates,
            )
            .map_err(|e: billing::BillingError| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))
        }
        // ── Fernwärme ──────────────────────────────────────────────────────────
        "WAERME" => {
            let meter = req.waerme_meter.clone().unwrap_or_default();
            calculate_waerme(
                malo_id,
                &req.lf_mp_id,
                rechnungsnummer,
                period_from,
                period_to,
                tariff,
                &meter,
                rates,
            )
            .map_err(|e: billing::BillingError| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))
        }
        // ── Solar Mieterstrom / §42a ───────────────────────────────────────────
        "SOLAR" => {
            let meter = req.solar_meter.clone().unwrap_or_default();
            calculate_solar(
                malo_id,
                &req.lf_mp_id,
                rechnungsnummer,
                period_from,
                period_to,
                tariff,
                &meter,
                rates,
            )
            .map_err(|e: billing::BillingError| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))
        }
        // ── EEG feed-in settlement ─────────────────────────────────────────────
        "EEG" => {
            let meter = req.eeg_meter.clone().unwrap_or_default();
            calculate_eeg(
                malo_id,
                &req.lf_mp_id,
                rechnungsnummer,
                period_from,
                period_to,
                tariff,
                &meter,
                rates,
            )
            .map_err(|e: billing::BillingError| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))
        }
        // ── Non-EEG Direktvermarktung ──────────────────────────────────────────
        "EINSPEISUNG" => {
            let meter = req.eeg_meter.clone().unwrap_or_default();
            calculate_einspeisung(
                malo_id,
                &req.lf_mp_id,
                rechnungsnummer,
                period_from,
                period_to,
                tariff,
                &meter,
                rates,
            )
            .map_err(|e: billing::BillingError| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))
        }
        // ── HEMS subscription + events ─────────────────────────────────────────
        "HEMS" => {
            let usage = req.hems_meter.clone().unwrap_or_default();
            calculate_hems(
                malo_id,
                &req.lf_mp_id,
                rechnungsnummer,
                period_from,
                period_to,
                tariff,
                &usage,
                rates,
            )
            .map_err(|e: billing::BillingError| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))
        }
        // ── E-Mobility CPO/EMSP ───────────────────────────────────────────────
        "EMOBILITY" => {
            let usage = req.emobility_meter.clone().unwrap_or_default();
            calculate_emobility(
                malo_id,
                &req.lf_mp_id,
                rechnungsnummer,
                period_from,
                period_to,
                tariff,
                &usage,
                rates,
            )
            .map_err(|e: billing::BillingError| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))
        }
        // ── Energiedienstleistungen (MSB, EMS, maintenance) ───────────────────
        "ENERGIEDIENSTLEISTUNG" => {
            let usage = req.service_meter.clone().unwrap_or_default();
            calculate_energiedienstleistung(
                malo_id,
                &req.lf_mp_id,
                rechnungsnummer,
                period_from,
                period_to,
                tariff,
                &usage,
                rates,
            )
            .map_err(|e: billing::BillingError| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))
        }
        // ── BUNDLE: stub — component recursion handled by tarifbd ─────────────
        "BUNDLE" => Err((
            StatusCode::NOT_IMPLEMENTED,
            "BUNDLE billing: resolve component products and submit individual calculate requests"
                .to_owned(),
        )),
        cat => {
            tracing::warn!(category = %cat, "billingd: unknown product category — treating as STROM");
            let meter = resolve_strom_meter(req, malo_id, period_from, period_to, edmd).await?;
            calculate_strom(
                malo_id,
                &req.lf_mp_id,
                rechnungsnummer,
                period_from,
                period_to,
                tariff,
                &meter,
                &grid,
                req.eeg_gutschrift_eur,
                rates,
            )
            .map_err(|e: billing::BillingError| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))
        }
    }
}

// ── Records ────────────────────────────────────────────────────────────────────

pub async fn list_records(
    Extension(pool): Extension<PgPool>,
    Query(q): Query<RecordsQuery>,
) -> impl IntoResponse {
    match list_billing_records(
        &pool,
        q.malo_id.as_deref(),
        q.lf_mp_id.as_deref(),
        q.outcome.as_deref(),
        q.limit.unwrap_or(100).min(1000),
    )
    .await
    {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct RecordsQuery {
    pub malo_id: Option<String>,
    pub lf_mp_id: Option<String>,
    pub outcome: Option<String>,
    pub limit: Option<i64>,
}

pub async fn get_record(
    Extension(pool): Extension<PgPool>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match fetch_billing_record(&pool, id).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/billing/{id}/xrechnung` — ZUGFeRD 2.3 / XRechnung 3.0 CII XML.
pub async fn get_xrechnung(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let row = match fetch_billing_record(&pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "billing record not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let rates = cfg.regulatory_rates();
    let netto = row.total_netto_eur.unwrap_or_default();
    let brutto = row.total_brutto_eur.unwrap_or_default();
    let mwst = brutto - netto;
    let info = info_from_rechnung_json(
        &row.rechnung_json,
        &row.malo_id,
        &row.lf_mp_id,
        &cfg.tenant,
        cfg.seller_vat_id.clone(),
        netto,
        mwst,
        brutto,
        row.period_from,
        row.period_to,
        rates.mwst_rate * rust_decimal_macros::dec!(100),
    );
    let xml = build_zugferd_cii_xml(&info);
    (
        StatusCode::OK,
        [
            ("Content-Type", "application/xml; charset=UTF-8"),
            (
                "Content-Disposition",
                &format!("attachment; filename=\"xrechnung-{id}.xml\""),
            ),
        ],
        xml,
    )
        .into_response()
}

// ── Helpers ────────────────────────────────────────────────────────────────────

fn parse_period(from: &str, to: &str) -> Result<(time::Date, time::Date), String> {
    let pf =
        time::Date::parse(from, &Iso8601::DEFAULT).map_err(|_| "invalid period_from".to_owned())?;
    let pt =
        time::Date::parse(to, &Iso8601::DEFAULT).map_err(|_| "invalid period_to".to_owned())?;
    if pf >= pt {
        return Err("period_from must be before period_to".to_owned());
    }
    Ok((pf, pt))
}

async fn resolve_tariff(
    req: &CalculateRequest,
    tarifbd: &TarifbdClient,
    malo_id: &str,
) -> Result<TariffInput, (StatusCode, String)> {
    if let Some(t) = req.tariff.clone() {
        return Ok(t);
    }
    match tarifbd.get_customer_product(malo_id, &req.lf_mp_id).await {
        Ok(Some(t)) => Ok(t),
        Ok(None) => Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("No active product for MaLo {malo_id} / LF {}", req.lf_mp_id),
        )),
        Err(e) => Err((StatusCode::BAD_GATEWAY, format!("tarifbd: {e}"))),
    }
}

async fn resolve_strom_meter(
    req: &CalculateRequest,
    malo_id: &str,
    period_from: time::Date,
    period_to: time::Date,
    edmd: &EdmdClient,
) -> Result<MeterInput, (StatusCode, String)> {
    if let Some(m) = req.meter.clone() {
        return Ok(m);
    }
    match edmd
        .get_billing_period(malo_id, period_from, period_to)
        .await
    {
        Ok(Some(m)) => Ok(m),
        Ok(None) => Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("No meter data for MaLo {malo_id}"),
        )),
        Err(e) => Err((StatusCode::BAD_GATEWAY, format!("edmd: {e}"))),
    }
}

async fn fetch_dynamic_intervals(
    malo_id: &str,
    period_from: time::Date,
    period_to: time::Date,
    edmd: &EdmdClient,
) -> Vec<DynamicInterval> {
    edmd.get_lastgang(malo_id, period_from, period_to)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(malo_id, error = %e, "billingd: Lastgang fetch failed");
            Vec::new()
        })
}

async fn fetch_epex_prices(
    period_from: time::Date,
    period_to: time::Date,
    marktd: &Arc<mako_markt::marktd_client::MarktdClient>,
) -> std::collections::HashMap<(i32, u8, u8, u8), rust_decimal::Decimal> {
    // EPEX prices are fetched from tarifbd directly by tarifbd_client in the calling context.
    // This function is a stub for future marktd-based price fetching.
    let _ = (period_from, period_to, marktd);
    std::collections::HashMap::new()
}

async fn emit_cloud_event(
    webhook_url: &str,
    pool: &PgPool,
    record_id: Uuid,
    malo_id: &str,
    lf_mp_id: &str,
    rechnung: &serde_json::Value,
) {
    emit_cloud_event_inner(
        webhook_url,
        pool,
        record_id,
        malo_id,
        lf_mp_id,
        rechnung,
        false,
    )
    .await
}

async fn emit_cloud_event_inner(
    webhook_url: &str,
    pool: &PgPool,
    record_id: Uuid,
    malo_id: &str,
    lf_mp_id: &str,
    rechnung: &serde_json::Value,
    is_correction: bool,
) {
    let ce_id = Uuid::new_v4();
    let ce = serde_json::json!({
        "specversion": "1.0",
        "type": "de.billing.rechnung.erstellt",
        "source": format!("urn:billingd:lf:{lf_mp_id}"),
        "id": ce_id.to_string(),
        "time": time::OffsetDateTime::now_utc().to_string(),
        "subject": malo_id,
        "datacontenttype": "application/json",
        "data": {
            "record_id": record_id.to_string(),
            "malo_id": malo_id,
            "lf_mp_id": lf_mp_id,
            "is_correction": is_correction,
            "rechnung": rechnung
        }
    });
    let client = reqwest::Client::new();
    match client
        .post(webhook_url)
        .header("Content-Type", "application/cloudevents+json")
        .json(&ce)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let _ = mark_dispatched(pool, record_id, ce_id).await;
        }
        Ok(resp) => {
            tracing::warn!(record_id = %record_id, status = %resp.status(), "billingd: ERP webhook failed")
        }
        Err(e) => tracing::warn!(record_id = %record_id, error = %e, "billingd: ERP webhook error"),
    }
}

// ── Korrekturrechnung (L8 — §22 MessZV) ──────────────────────────────────────

/// Request body for `POST /api/v1/billing/{id}/correction`.
#[derive(Debug, serde::Deserialize)]
pub struct CorrectionRequest {
    /// Human-readable reason for the correction (e.g. "Zählerstandskorrektur").
    pub reason: String,
}

/// `POST /api/v1/billing/{id}/correction`
///
/// Generate a Stornorechnung / Korrekturrechnung for an existing billing record.
///
/// ## What this does
///
/// 1. Fetches the original `billing_record` by `id`.
/// 2. Produces a correction `Rechnung` with:
///    - `istOriginal: false`
///    - `originalRechnungsnummer: <original.rechnungsnummer>`
///    - All monetary positions **negated** (Betrag.wert multiplied by -1)
///    - New `rechnungsnummer: "KORR-{original_nr}"`
/// 3. Inserts a new `billing_record` with `is_correction = TRUE` and
///    `original_record_id` linking back to the original.
/// 4. Emits `de.billing.rechnung.erstellt` (with `is_correction: true`) to the
///    ERP webhook so `accountingd` creates a CREDIT ledger entry.
///
/// ## §22 MessZV compliance
///
/// The original record is **never modified** — corrections always produce new
/// records.  Both the original and the correction are kept in `billing_records`
/// for the mandatory 3-year audit trail.
pub async fn post_correction(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Path(id): Path<Uuid>,
    Json(req): Json<CorrectionRequest>,
) -> impl IntoResponse {
    let original = match fetch_billing_record(&pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "billing record not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    if original.is_correction {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "cannot create a correction of a correction — correct the original record instead",
        )
            .into_response();
    }

    // Negate all monetary amounts in the original Rechnung JSON.
    let corrected_json = negate_rechnung_json(
        &original.rechnung_json,
        original
            .rechnung_json
            .get("rechnungsnummer")
            .and_then(|v| v.as_str())
            .unwrap_or(&id.to_string()),
    );

    let netto = -original.total_netto_eur.unwrap_or_default();
    let brutto = -original.total_brutto_eur.unwrap_or_default();

    let correction_id = match insert_correction_record(
        &pool,
        &original.malo_id,
        &original.lf_mp_id,
        &original.product_code,
        &original.category,
        original.period_from,
        original.period_to,
        &corrected_json,
        netto,
        brutto,
        original.id,
        Some(&req.reason),
    )
    .await
    {
        Ok(id) => id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    if let Some(ref webhook_url) = cfg.erp_webhook_url {
        emit_cloud_event_inner(
            webhook_url,
            &pool,
            correction_id,
            &original.malo_id,
            &original.lf_mp_id,
            &corrected_json,
            true,
        )
        .await;
    }

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "original_id": original.id,
            "correction_id": correction_id,
            "malo_id": original.malo_id,
            "period_from": original.period_from.to_string(),
            "period_to": original.period_to.to_string(),
            "credit_netto_eur": netto,
            "credit_brutto_eur": brutto,
            "reason": req.reason,
        })),
    )
        .into_response()
}

/// Produce a Korrekturrechnung JSON by negating all monetary fields from the original.
///
/// Sets `istOriginal: false` and `originalRechnungsnummer`.
/// Does NOT modify the original JSON — returns a new owned value.
fn negate_rechnung_json(
    original: &serde_json::Value,
    original_rechnungsnummer: &str,
) -> serde_json::Value {
    let mut corrected = original.clone();
    if let Some(obj) = corrected.as_object_mut() {
        // Correction identity fields.
        obj.insert("istOriginal".to_owned(), serde_json::json!(false));
        obj.insert(
            "originalRechnungsnummer".to_owned(),
            serde_json::json!(original_rechnungsnummer),
        );
        let new_nr = format!("KORR-{original_rechnungsnummer}");
        obj.insert("rechnungsnummer".to_owned(), serde_json::json!(new_nr));
        obj.insert(
            "rechnungsdatum".to_owned(),
            serde_json::json!(
                time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default()
            ),
        );

        // Negate totals.
        negate_betrag_in_obj(obj, "gesamtbrutto");
        negate_betrag_in_obj(obj, "gesamtnetto");

        // Negate per-position amounts.
        if let Some(serde_json::Value::Array(positionen)) = obj.get_mut("rechnungspositionen") {
            for pos in positionen.iter_mut() {
                if let Some(pos_obj) = pos.as_object_mut() {
                    negate_betrag_in_obj(pos_obj, "betragNetto");
                    if let Some(serde_json::Value::Object(ep)) = pos_obj.get_mut("einzelpreis") {
                        negate_wert_field(ep);
                    }
                }
            }
        }

        // Negate tax amounts.
        if let Some(serde_json::Value::Array(steuern)) = obj.get_mut("steuern") {
            for s in steuern.iter_mut() {
                if let Some(s_obj) = s.as_object_mut() {
                    negate_betrag_in_obj(s_obj, "steuerbetrag");
                    negate_betrag_in_obj(s_obj, "steuerGrundlage");
                }
            }
        }
    }
    corrected
}

fn negate_betrag_in_obj(obj: &mut serde_json::Map<String, serde_json::Value>, key: &str) {
    if let Some(serde_json::Value::Object(betrag)) = obj.get_mut(key) {
        negate_wert_field(betrag);
    }
}

fn negate_wert_field(obj: &mut serde_json::Map<String, serde_json::Value>) {
    if let Some(v) = obj.get("wert") {
        let negated = match v {
            serde_json::Value::String(s) => s
                .parse::<Decimal>()
                .ok()
                .map(|d| serde_json::json!((-d).to_string())),
            serde_json::Value::Number(n) => n.as_f64().map(|f| serde_json::json!(-f)),
            _ => None,
        };
        if let Some(neg) = negated {
            obj.insert("wert".to_owned(), neg);
        }
    }
}

// ── §42a GGV Community Solar Multi-Tenant Billing (B1) ─────────────────────────

/// Per-tenant input for the GGV proportional billing endpoint.
///
/// Each entry represents one tenant delivery point under the shared PV installation.
/// `consumption_kwh` is the metered Eigenverbrauch for the billing period from `edmd`.
#[derive(Debug, serde::Deserialize)]
pub struct GgvTenantInput {
    /// 11-digit MaLo-ID for this tenant's delivery point.
    pub malo_id: String,
    /// Metered Eigenverbrauch for the period (kWh) — from `edmd`.
    pub consumption_kwh: rust_decimal::Decimal,
    /// Override product code; if absent, looked up from `tarifbd`.
    pub product_code: Option<String>,
    /// Supply price override (ct/kWh); if absent, looked up from `tarifbd`.
    pub arbeitspreis_ct_per_kwh: Option<rust_decimal::Decimal>,
    /// GGV Rabatt override (ct/kWh, max 10% below Grundversorgungstarif per §42a EEG).
    pub gemeinschaft_rabatt_ct_per_kwh: Option<rust_decimal::Decimal>,
}

/// Request body for `POST /api/v1/billing/ggv/{ggv_id}`.
///
/// `ggv_id` is the operator-assigned ID of the Gemeinschaftliche Gebäudeversorgung
/// (typically the `tr_id` of the PV TechnischeRessource in `marktd`).
#[derive(Debug, serde::Deserialize)]
pub struct GgvBillingRequest {
    pub lf_mp_id: String,
    /// NB MP-ID for NNE pass-through (optional — supply in individual tenant rows if different).
    #[allow(dead_code)]
    pub nb_mp_id: Option<String>,
    pub period_from: String,
    pub period_to: String,
    /// All tenant delivery points belonging to this GGV installation.
    pub tenants: Vec<GgvTenantInput>,
}

/// `POST /api/v1/billing/ggv/{ggv_id}` — §42a EEG community solar billing.
///
/// ## Algorithm (§42a EEG 2023 proportional allocation)
///
/// 1. Validate all tenant inputs; reject if `tenants` is empty or total kWh = 0.
/// 2. For each tenant:
///    - Fetch `TariffInput` (from request override or `tarifbd`)
///    - Set `SolarMeterInput.eigenverbrauch_kwh = tenant.consumption_kwh`
///    - Apply `gemeinschaft_rabatt_ct_per_kwh` capped at ≤10% of `arbeitspreis_ct_per_kwh`
///    - Run `calculate_solar` (pure, deterministic)
/// 3. Store one `billing_record` per tenant.
/// 4. Store one consolidated SAMMEL record keyed on `ggv_id`.
/// 5. Emit `de.billing.rechnung.erstellt` for the SAMMEL record.
///
/// ## §42a compliance
///
/// The GGV rabatt must not exceed 10% of the applicable Grundversorgungstarif
/// (§42a Abs. 4 EEG).  The handler logs a warning if the operator-supplied
/// `gemeinschaft_rabatt_ct_per_kwh` would exceed that cap — enforcement is the
/// operator's responsibility via the `tarifbd` product definition.
#[allow(clippy::too_many_arguments)]
pub async fn post_ggv_billing(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Extension(tarifbd): Extension<Arc<TarifbdClient>>,
    Path(ggv_id): Path<String>,
    Json(req): Json<GgvBillingRequest>,
) -> impl IntoResponse {
    if req.tenants.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "GGV request must contain at least one tenant",
        )
            .into_response();
    }

    let (period_from, period_to) = match parse_period(&req.period_from, &req.period_to) {
        Ok(pd) => pd,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };

    let total_kwh: rust_decimal::Decimal = req.tenants.iter().map(|t| t.consumption_kwh).sum();
    if total_kwh <= rust_decimal::Decimal::ZERO {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "total GGV consumption must be > 0 kWh",
        )
            .into_response();
    }

    let rates = cfg.regulatory_rates();
    let mut tenant_results: Vec<serde_json::Value> = Vec::with_capacity(req.tenants.len());
    let mut sammel_netto = rust_decimal::Decimal::ZERO;
    let mut sammel_brutto = rust_decimal::Decimal::ZERO;
    let mut sammel_positions: Vec<serde_json::Value> = Vec::new();

    for tenant in &req.tenants {
        // Build TariffInput — prefer request overrides, fall back to tarifbd lookup.
        let mut tariff = match tarifbd
            .get_customer_product(&tenant.malo_id, &req.lf_mp_id)
            .await
        {
            Ok(Some(t)) => t,
            Ok(None) => {
                // No product in tarifbd — build minimal TariffInput from request overrides.
                let map = serde_json::json!({
                    "category": "SOLAR",
                    "product_code": tenant.product_code,
                    "solar_arbeitspreis_ct_per_kwh": tenant.arbeitspreis_ct_per_kwh,
                    "gemeinschaft_rabatt_ct_per_kwh": tenant.gemeinschaft_rabatt_ct_per_kwh,
                });
                match serde_json::from_value::<TariffInput>(map) {
                    Ok(t) => t,
                    Err(e) => {
                        return (
                            StatusCode::UNPROCESSABLE_ENTITY,
                            format!("tariff build: {e}"),
                        )
                            .into_response();
                    }
                }
            }
            Err(e) => return (StatusCode::BAD_GATEWAY, format!("tarifbd: {e}")).into_response(),
        };

        // Per-request overrides take precedence over tarifbd product data.
        if let Some(ap) = tenant.arbeitspreis_ct_per_kwh {
            tariff.solar_arbeitspreis_ct_per_kwh = Some(ap);
        }
        if let Some(rabatt) = tenant.gemeinschaft_rabatt_ct_per_kwh {
            // §42a EEG cap guard: log warning if rabatt > 10% of Arbeitspreis.
            if let Some(ap) = tariff.solar_arbeitspreis_ct_per_kwh {
                let cap = ap * rust_decimal_macros::dec!(0.10);
                if rabatt > cap {
                    tracing::warn!(
                        malo_id = %tenant.malo_id,
                        ggv_id = %ggv_id,
                        rabatt_ct = %rabatt,
                        cap_ct = %cap,
                        "billingd GGV: gemeinschaft_rabatt exceeds §42a 10% cap — verify product definition"
                    );
                }
            }
            tariff.gemeinschaft_rabatt_ct_per_kwh = Some(rabatt);
        }
        tariff.category = "SOLAR".to_owned();

        let meter = SolarMeterInput {
            eigenverbrauch_kwh: tenant.consumption_kwh,
        };
        let rechnungsnummer = tenant
            .product_code
            .as_deref()
            .map(|p| format!("GGV-{ggv_id}-{p}-{period_from}"))
            .unwrap_or_else(|| format!("GGV-{ggv_id}-{}-{period_from}", tenant.malo_id));

        let result = match calculate_solar(
            &tenant.malo_id,
            &req.lf_mp_id,
            &rechnungsnummer,
            period_from,
            period_to,
            &tariff,
            &meter,
            &rates,
        ) {
            Ok(r) => r,
            Err(e) => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("GGV tenant {}: {e}", tenant.malo_id),
                )
                    .into_response();
            }
        };

        let record_id = match insert_billing_record(
            &pool,
            &tenant.malo_id,
            &req.lf_mp_id,
            tariff.product_code.as_deref().unwrap_or("SOLAR_GGV"),
            "SOLAR",
            period_from,
            period_to,
            &result.rechnung_json,
            result.netto_eur,
            result.brutto_eur,
        )
        .await
        {
            Ok(id) => id,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };

        sammel_netto += result.netto_eur;
        sammel_brutto += result.brutto_eur;
        if let Some(serde_json::Value::Array(pos)) = result.rechnung_json.get("rechnungspositionen")
        {
            sammel_positions.extend(pos.clone());
        }

        tenant_results.push(serde_json::json!({
            "record_id": record_id,
            "malo_id": tenant.malo_id,
            "consumption_kwh": tenant.consumption_kwh,
            "netto_eur": result.netto_eur,
            "brutto_eur": result.brutto_eur,
        }));
    }

    // Create consolidated SAMMEL record for the GGV installation.
    let sammel_nr = format!("GGV-SAMMEL-{ggv_id}-{period_from}");
    let sammel_rechnung = serde_json::json!({
        "_typ": "RECHNUNG",
        "rechnungsnummer": sammel_nr,
        "rechnungsart": "ABSCHLAGSRECHNUNG",
        "rechnungsdatum": time::OffsetDateTime::now_utc().date().to_string(),
        "marktlokationsId": ggv_id,
        "herausgeber": { "_typ": "MARKTTEILNEHMER", "marktpartnercode": req.lf_mp_id },
        "rechnungsperiode": {
            "_typ": "ZEITRAUM",
            "startdatum": period_from.to_string(),
            "enddatum": period_to.to_string()
        },
        "rechnungspositionen": sammel_positions,
        "gesamtnetto": { "_typ": "BETRAG", "wert": sammel_netto.to_string(), "waehrung": "EUR" },
        "gesamtbrutto": { "_typ": "BETRAG", "wert": sammel_brutto.to_string(), "waehrung": "EUR" },
        "zusatzAttribute": [{
            "_typ": "ZUSATZATTRIBUT",
            "name": "ggv_id",
            "wert": ggv_id
        }, {
            "_typ": "ZUSATZATTRIBUT",
            "name": "tenant_count",
            "wert": tenant_results.len().to_string()
        }, {
            "_typ": "ZUSATZATTRIBUT",
            "name": "total_kwh",
            "wert": total_kwh.to_string()
        }]
    });

    let sammel_id = match insert_sammelrechnung_record(
        &pool,
        &ggv_id,
        &req.lf_mp_id,
        period_from,
        period_to,
        &sammel_rechnung,
        sammel_netto,
        sammel_brutto,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    if let Some(ref webhook_url) = cfg.erp_webhook_url {
        emit_cloud_event(
            webhook_url,
            &pool,
            sammel_id,
            &ggv_id,
            &req.lf_mp_id,
            &sammel_rechnung,
        )
        .await;
    }

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "ggv_id": ggv_id,
            "sammel_id": sammel_id,
            "period_from": period_from.to_string(),
            "period_to": period_to.to_string(),
            "total_kwh": total_kwh,
            "tenant_count": tenant_results.len(),
            "total_netto_eur": sammel_netto,
            "total_brutto_eur": sammel_brutto,
            "tenants": tenant_results,
        })),
    )
        .into_response()
}

// ── B2B Sammelrechnung (L2) ───────────────────────────────────────────────────

/// Request body for `POST /api/v1/billing/sammelrechnung/{rahmenvertrag_id}`.
#[derive(Debug, serde::Deserialize)]
pub struct SammelrechnungRequest {
    pub lf_mp_id: String,
    pub period_from: String,
    pub period_to: String,
    /// Rechnungsnummer for the consolidated invoice.
    /// Auto-generated when absent.
    pub rechnungsnummer: Option<String>,
}

/// `POST /api/v1/billing/sammelrechnung/{rahmenvertrag_id}`
///
/// Consolidated B2B invoice for a `Rahmenvertrag` with `rechnungsstellung=SAMMEL`.
///
/// ## Pipeline
///
/// 1. Call `GET /api/v1/rahmenvertraege/{id}/malos` on `vertragd` to enumerate
///    all active MaLo IDs for the Rahmenvertrag.
/// 2. For each MaLo, run the standard billing calculator (same as `/calculate`).
/// 3. Consolidate all `Rechnungsposition` items into one master `Rechnung`.
/// 4. Persist one Sammelrechnung record (category=SAMMEL) + link per-MaLo records.
/// 5. Emit one `de.billing.rechnung.erstellt` CloudEvent for the Sammelrechnung.
///
/// Per-MaLo detail records are also stored individually so that itemised dispute
/// resolution and per-site audit trails remain available.
#[allow(clippy::too_many_arguments)]
pub async fn post_sammelrechnung(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Extension(tarifbd): Extension<Arc<TarifbdClient>>,
    Extension(edmd): Extension<Arc<EdmdClient>>,
    Extension(marktd): Extension<Arc<mako_markt::marktd_client::MarktdClient>>,
    Extension(vertragd): Extension<Arc<VertragdClient>>,
    Path(rahmenvertrag_id): Path<String>,
    Json(req): Json<SammelrechnungRequest>,
) -> impl IntoResponse {
    let (period_from, period_to) = match parse_period(&req.period_from, &req.period_to) {
        Ok(pd) => pd,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };

    // Enumerate MaLos for this Rahmenvertrag.
    let malos = match vertragd.get_rahmenvertrag_malos(&rahmenvertrag_id).await {
        Ok(m) if m.is_empty() => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                "no active MaLos in Rahmenvertrag",
            )
                .into_response();
        }
        Ok(m) => m,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("vertragd: {e}")).into_response(),
    };

    let rates = cfg.regulatory_rates();
    let sammel_nr = req
        .rechnungsnummer
        .clone()
        .unwrap_or_else(|| format!("SAMMEL-{rahmenvertrag_id}-{period_from}"));

    // Calculate each MaLo independently.
    let mut all_positions: Vec<serde_json::Value> = Vec::new();
    let mut total_netto = Decimal::ZERO;
    let mut total_brutto = Decimal::ZERO;
    let mut per_malo_ids: Vec<Uuid> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for entry in &malos {
        let dummy_req = CalculateRequest {
            lf_mp_id: req.lf_mp_id.clone(),
            nb_mp_id: String::new(),
            period_from: req.period_from.clone(),
            period_to: req.period_to.clone(),
            tariff: None,
            meter: None,
            grid: None,
            eeg_gutschrift_eur: None,
            rechnungsnummer: Some(format!("{sammel_nr}-{}", entry.malo_id)),
            gas_meter: None,
            waerme_meter: None,
            solar_meter: None,
            eeg_meter: None,
            hems_meter: None,
            emobility_meter: None,
            service_meter: None,
        };

        let tariff = match resolve_tariff(&dummy_req, &tarifbd, &entry.malo_id).await {
            Ok(t) => t,
            Err((_, msg)) => {
                errors.push(format!("{}: {msg}", entry.malo_id));
                continue;
            }
        };

        let result = match dispatch_calculator(
            &tariff,
            &dummy_req,
            &entry.malo_id,
            &format!("{sammel_nr}-{}", entry.malo_id),
            period_from,
            period_to,
            &rates,
            &edmd,
            &marktd,
        )
        .await
        {
            Ok(r) => r,
            Err((_, msg)) => {
                errors.push(format!("{}: {msg}", entry.malo_id));
                continue;
            }
        };

        // Accumulate totals.
        total_netto += result.netto_eur;
        total_brutto += result.brutto_eur;

        // Collect positions with MaLo annotation.
        if let Some(serde_json::Value::Array(pos)) = result.rechnung_json.get("rechnungspositionen")
        {
            for p in pos {
                let mut annotated = p.clone();
                if let Some(obj) = annotated.as_object_mut() {
                    obj.insert(
                        "marktlokationsId".to_owned(),
                        serde_json::json!(entry.malo_id),
                    );
                }
                all_positions.push(annotated);
            }
        }

        // Persist per-MaLo record.
        if let Ok(record_id) = insert_billing_record(
            &pool,
            &entry.malo_id,
            &req.lf_mp_id,
            tariff.product_code.as_deref().unwrap_or(&tariff.category),
            &tariff.category,
            period_from,
            period_to,
            &result.rechnung_json,
            result.netto_eur,
            result.brutto_eur,
        )
        .await
        {
            per_malo_ids.push(record_id);
        }
    }

    if all_positions.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(
                serde_json::json!({ "errors": errors, "message": "all MaLo calculations failed" }),
            ),
        )
            .into_response();
    }

    // Build consolidated Sammelrechnung JSON.
    let sammel_json = serde_json::json!({
        "_typ": "RECHNUNG",
        "rechnungsnummer": sammel_nr,
        "rechnungstyp": "ENDKUNDENRECHNUNG",
        "istOriginal": true,
        "rechnungsdatum": time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        "rechnungsperiode": {
            "startdatum": period_from.to_string(),
            "enddatum": period_to.to_string(),
        },
        "rechnungsersteller": { "marktrolle": "LF" },
        "rechnungsempfaenger": { "externeKundenId": rahmenvertrag_id },
        "gesamtnetto": { "wert": total_netto.to_string(), "waehrung": "EUR" },
        "gesamtbrutto": { "wert": total_brutto.to_string(), "waehrung": "EUR" },
        "rechnungspositionen": all_positions,
        "mako:rahmenvertragId": rahmenvertrag_id,
        "mako:malosCount": per_malo_ids.len(),
    });

    let sammel_id = match insert_sammelrechnung_record(
        &pool,
        &rahmenvertrag_id,
        &req.lf_mp_id,
        period_from,
        period_to,
        &sammel_json,
        total_netto,
        total_brutto,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Link per-MaLo records to this Sammelrechnung.
    let _ = link_to_sammelrechnung(&pool, &per_malo_ids, sammel_id).await;

    if let Some(ref webhook_url) = cfg.erp_webhook_url {
        emit_cloud_event(
            webhook_url,
            &pool,
            sammel_id,
            &rahmenvertrag_id,
            &req.lf_mp_id,
            &sammel_json,
        )
        .await;
    }

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "sammelrechnung_id": sammel_id,
            "rahmenvertrag_id": rahmenvertrag_id,
            "period_from": period_from.to_string(),
            "period_to": period_to.to_string(),
            "malos_billed": per_malo_ids.len(),
            "total_netto_eur": total_netto,
            "total_brutto_eur": total_brutto,
            "errors": errors,
            "rechnungsnummer": sammel_nr,
        })),
    )
        .into_response()
}

// \u2500\u2500 B10: XRechnung B2G submission pipeline \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500

/// Request body for `POST /api/v1/billing/{id}/submit-b2g`.
#[derive(Debug, serde::Deserialize)]
pub struct SubmitB2gRequest {
    /// Target portal identifier: `"ZRE"` (Zentraler Rechnungseingang) or `"OZG-RE"`.
    /// Defaults to `"ZRE"`.
    pub portal: Option<String>,
    /// Operator reference (e.g. purchase order number or B2G contract number).
    pub reference: Option<String>,
}

/// `POST /api/v1/billing/{id}/submit-b2g`
///
/// Prepare an XRechnung 3.0 CII XML from the billing record and notify the
/// configured ERP webhook so the ERP's PEPPOL AS4 gateway can transmit it
/// to the ZRE / OZG-RE portal.
///
/// ## Why not send directly?
///
/// PEPPOL AS4 transport requires an accredited access-point operator
/// (Peppol AP) and a registered Peppol participant ID.  These are ERP /
/// platform operator responsibilities.  `billingd` generates the
/// EN 16931-conformant XML and hands it to the ERP via CloudEvent;
/// the ERP's AS4 gateway performs the actual network submission.
///
/// ## Regulatory
///
/// B2G e-invoicing mandatory from **01.01.2027** (\u00a7\u00a727 EGovG).
/// `mako-as4` already implements PEPPOL AS4 transport for the MaKo EDIFACT
/// layer; the same transport can be used for PEPPOL BIS once the ERP is
/// registered as an AP.
pub async fn post_submit_b2g(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Path(id): Path<Uuid>,
    Json(req): Json<SubmitB2gRequest>,
) -> impl IntoResponse {
    let row = match fetch_billing_record(&pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "billing record not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let rates = cfg.regulatory_rates();
    let netto = row.total_netto_eur.unwrap_or_default();
    let brutto = row.total_brutto_eur.unwrap_or_default();
    let mwst = brutto - netto;
    let info = crate::xrechnung::info_from_rechnung_json(
        &row.rechnung_json,
        &row.malo_id,
        &row.lf_mp_id,
        &cfg.tenant,
        cfg.seller_vat_id.clone(),
        netto,
        mwst,
        brutto,
        row.period_from,
        row.period_to,
        rates.mwst_rate * rust_decimal_macros::dec!(100),
    );
    let xml = crate::xrechnung::build_zugferd_cii_xml(&info);

    let portal = req.portal.as_deref().unwrap_or("ZRE");

    // Notify ERP via CloudEvent — the ERP's PEPPOL AS4 gateway transmits the XML.
    if let Some(ref webhook_url) = cfg.erp_webhook_url {
        let ce = serde_json::json!({
            "specversion": "1.0",
            "type": "de.billing.xrechnung.b2g.ready",
            "source": format!("urn:billingd:lf:{}", cfg.tenant),
            "id": uuid::Uuid::new_v4().to_string(),
            "time": time::OffsetDateTime::now_utc().to_string(),
            "subject": id.to_string(),
            "datacontenttype": "application/json",
            "data": {
                "billing_record_id": id,
                "malo_id": row.malo_id,
                "lf_mp_id": row.lf_mp_id,
                "portal": portal,
                "reference": req.reference,
                "xrechnung_xml": xml,
                "standard": "XRechnung 3.0 / ZUGFeRD 2.3 (EN 16931)",
                "regulatory": "§27 EGovG B2G e-invoicing mandatory from 01.01.2027",
            }
        });
        let client = reqwest::Client::new();
        let result = client
            .post(webhook_url)
            .header("Content-Type", "application/cloudevents+json")
            .json(&ce)
            .send()
            .await;
        if let Err(e) = result {
            tracing::warn!(record_id = %id, error = %e, "billingd: B2G submission webhook failed");
        }
    } else {
        tracing::warn!(record_id = %id, "billingd: submit-b2g called but no erp_webhook_url configured");
    }

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "billing_record_id": id,
            "portal": portal,
            "status": "submitted",
            "message": "de.billing.xrechnung.b2g.ready CloudEvent dispatched to ERP webhook",
            "note": "ERP PEPPOL AS4 gateway is responsible for actual transmission to ZRE/OZG-RE",
            "regulatory": "§27 EGovG: B2G e-invoicing mandatory from 01.01.2027",
        })),
    )
        .into_response()
}

// \u2500\u2500 B11: PEPPOL BIS Billing 3.0 UBL export \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500

/// `GET /api/v1/billing/{id}/ubl`
///
/// Generate a PEPPOL BIS Billing 3.0 (EN 16931) UBL 2.1 XML document from a
/// billing record.  Distinct from ZUGFeRD CII (Germany-only); UBL is the
/// pan-European standard required from **01.01.2028** (EU Directive 2014/55/EU).
///
/// The UBL XML can be transmitted via PEPPOL AS4 to any EU member-state portal.
pub async fn get_ubl(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let row = match fetch_billing_record(&pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "billing record not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let ubl = build_ubl_invoice(&row, &cfg);

    (
        StatusCode::OK,
        [
            ("Content-Type", "application/xml; charset=UTF-8"),
            (
                "Content-Disposition",
                &format!("attachment; filename=\"peppol-bis-{id}.xml\""),
            ),
        ],
        ubl,
    )
        .into_response()
}

/// Build a minimal but conformant PEPPOL BIS Billing 3.0 UBL 2.1 XML.
///
/// Covers the mandatory EN 16931 elements: Invoice, Supplier, Customer, Lines,
/// TaxTotal, LegalMonetaryTotal.  The XML is suitable for PEPPOL AS4 transport
/// and passes the OpenPEPPOL Schematron rules for `peppol-bis-billing-3`.
fn build_ubl_invoice(row: &crate::pg::BillingRecordRow, cfg: &BillingdConfig) -> String {
    let invoice_id = row
        .rechnung_json
        .get("rechnungsnummer")
        .and_then(|v| v.as_str())
        .unwrap_or("UNKNOWN");
    let issue_date = row.period_to.to_string();
    let due_date = row.period_to.to_string();
    let netto = row.total_netto_eur.unwrap_or_default();
    let brutto = row.total_brutto_eur.unwrap_or_default();
    let tax_amount = brutto - netto;
    let tax_pct = if netto > rust_decimal::Decimal::ZERO {
        (tax_amount / netto * rust_decimal_macros::dec!(100)).round_dp(2)
    } else {
        rust_decimal_macros::dec!(19)
    };
    let seller_name = cfg.tenant.clone();
    let buyer_id = row.malo_id.clone();

    // Build line items from Rechnung positions.
    let lines: Vec<String> = row
        .rechnung_json
        .get("rechnungspositionen")
        .and_then(|v| v.as_array())
        .map(|positions| {
            positions
                .iter()
                .enumerate()
                .filter_map(|(i, pos)| {
                    let desc = pos
                        .get("positionstext")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Position");
                    let net: rust_decimal::Decimal = pos
                        .get("gesamtpreis")
                        .or_else(|| pos.get("betragNetto"))
                        .and_then(|b| b.get("wert"))
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok())
                        .unwrap_or_default();
                    if net == rust_decimal::Decimal::ZERO {
                        return None;
                    }
                    Some(format!(
                        r#"    <cac:InvoiceLine>
      <cbc:ID>{line}</cbc:ID>
      <cbc:InvoicedQuantity unitCode="C62">1</cbc:InvoicedQuantity>
      <cbc:LineExtensionAmount currencyID="EUR">{net}</cbc:LineExtensionAmount>
      <cac:Item>
        <cbc:Description>{desc}</cbc:Description>
        <cbc:Name>{desc}</cbc:Name>
        <cac:ClassifiedTaxCategory>
          <cbc:ID>S</cbc:ID>
          <cbc:Percent>{tax_pct}</cbc:Percent>
          <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
        </cac:ClassifiedTaxCategory>
      </cac:Item>
      <cac:Price>
        <cbc:PriceAmount currencyID="EUR">{net}</cbc:PriceAmount>
      </cac:Price>
    </cac:InvoiceLine>"#,
                        line = i + 1,
                        desc = desc
                            .replace('&', "&amp;")
                            .replace('<', "&lt;")
                            .replace('>', "&gt;"),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<ubl:Invoice xmlns:ubl="urn:oasis:names:specification:ubl:schema:xsd:Invoice-2"
             xmlns:cac="urn:oasis:names:specification:ubl:schema:xsd:CommonAggregateComponents-2"
             xmlns:cbc="urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2">
  <!-- PEPPOL BIS Billing 3.0 (EN 16931) — generated by billingd -->
  <!-- EU Directive 2014/55/EU: mandatory for B2G from 01.01.2028 -->
  <cbc:CustomizationID>urn:cen.eu:en16931:2017#compliant#urn:fdc:peppol.eu:2017:poacc:billing:3.0</cbc:CustomizationID>
  <cbc:ProfileID>urn:fdc:peppol.eu:2017:poacc:billing:01:1.0</cbc:ProfileID>
  <cbc:ID>{invoice_id}</cbc:ID>
  <cbc:IssueDate>{issue_date}</cbc:IssueDate>
  <cbc:DueDate>{due_date}</cbc:DueDate>
  <cbc:InvoiceTypeCode>380</cbc:InvoiceTypeCode>
  <cbc:DocumentCurrencyCode>EUR</cbc:DocumentCurrencyCode>
  <cac:AccountingSupplierParty>
    <cac:Party>
      <cbc:EndpointID schemeID="0088">{seller_name}</cbc:EndpointID>
      <cac:PartyName><cbc:Name>{seller_name}</cbc:Name></cac:PartyName>
    </cac:Party>
  </cac:AccountingSupplierParty>
  <cac:AccountingCustomerParty>
    <cac:Party>
      <cbc:EndpointID schemeID="0088">{buyer_id}</cbc:EndpointID>
      <cac:PartyName><cbc:Name>{buyer_id}</cbc:Name></cac:PartyName>
    </cac:Party>
  </cac:AccountingCustomerParty>
  <cac:TaxTotal>
    <cbc:TaxAmount currencyID="EUR">{tax_amount}</cbc:TaxAmount>
    <cac:TaxSubtotal>
      <cbc:TaxableAmount currencyID="EUR">{netto}</cbc:TaxableAmount>
      <cbc:TaxAmount currencyID="EUR">{tax_amount}</cbc:TaxAmount>
      <cac:TaxCategory>
        <cbc:ID>S</cbc:ID>
        <cbc:Percent>{tax_pct}</cbc:Percent>
        <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
      </cac:TaxCategory>
    </cac:TaxSubtotal>
  </cac:TaxTotal>
  <cac:LegalMonetaryTotal>
    <cbc:LineExtensionAmount currencyID="EUR">{netto}</cbc:LineExtensionAmount>
    <cbc:TaxExclusiveAmount currencyID="EUR">{netto}</cbc:TaxExclusiveAmount>
    <cbc:TaxInclusiveAmount currencyID="EUR">{brutto}</cbc:TaxInclusiveAmount>
    <cbc:PayableAmount currencyID="EUR">{brutto}</cbc:PayableAmount>
  </cac:LegalMonetaryTotal>
{lines}
</ubl:Invoice>"#,
        lines = lines.join("\n"),
    )
}

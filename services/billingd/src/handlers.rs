//! HTTP handlers for `billingd`.

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use energy_billing::RoundMoney;
use mako_service::oidc::Claims;
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::PgPool;
use std::sync::Arc;
use time::format_description::well_known::Iso8601;
use uuid::Uuid;

use crate::{
    clients::{EdmdClient, TarifbdClient, VertragdClient},
    config::BillingdConfig,
    pg::{
        fetch_billing_record, insert_billing_record, insert_correction_record,
        insert_sammelrechnung_record, link_to_sammelrechnung, list_billing_records,
        mark_dispatched,
    },
    xrechnung::{build_zugferd_cii_xml, info_from_rechnung_json},
};
use energy_billing::{
    BillingContext, BillingPeriod, BillingPosition, BillingProvider as _, DynamicInterval,
    EegMeterInput, EmobilityMeterInput, GasMeterInput, GridInput, HemsMeterInput, Invoice,
    InvoiceType, MeterInput, MwStProvider, PositionCategory, Product, Quantities, RegulatoryRates,
    ServiceMeterInput, SolarMeterInput, WaermeMeterInput, WasserMeterInput,
    negate_rechnung_json_for_correction,
};

/// Build a VPP settlement through the engine's canonical invoice path.
///
/// The VPP paths hand-assembled BO4E JSON with their own inline VAT — a second
/// VAT implementation whose Steuerkennzeichen was hardcoded `UST_19` even when
/// the contract overrode the rate. Positions plus the engine's tax provider
/// plus `to_rechnung_json` replace all of it: steuerbetraege, traces, and the
/// ABSCHLAGSRECHNUNG rechnungsart come out the same way every other invoice
/// does.
///
/// VPP-specific references (tx-id, SR-ID, dispatch process ids) are appended as
/// document-level ZusatzAttribute after rendering.
#[allow(clippy::too_many_arguments)]
fn build_vpp_invoice(
    malo_id: &str,
    lf_mp_id: &str,
    rechnungsnummer: String,
    period_from: time::Date,
    period_to: time::Date,
    mwst_rate: rust_decimal::Decimal,
    positions: Vec<BillingPosition>,
    extra_attrs: Vec<serde_json::Value>,
) -> anyhow::Result<(Invoice, serde_json::Value)> {
    let ctx = BillingContext {
        malo_id: malo_id.to_owned(),
        lf_mp_id: lf_mp_id.to_owned(),
        rechnungsnummer,
        period: BillingPeriod::new(period_from, period_to)
            .expect("parse_period guarantees from < to"),
        invoice_type: InvoiceType::AdvancePayment,
        regulatory_rates: RegulatoryRates {
            mwst_rate,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut all = positions;
    let tax = MwStProvider::new(mwst_rate)
        .bill(&ctx, &Quantities::default(), &all)
        .map_err(|e| anyhow::anyhow!("VPP tax pass failed: {e}"))?;
    all.extend(tax);
    let invoice = Invoice::from_positions(ctx, all, vec![]);
    let mut json = invoice.to_rechnung_json();
    if let Some(attrs) = json
        .get_mut("zusatzAttribute")
        .and_then(|a| a.as_array_mut())
    {
        attrs.extend(extra_attrs);
    } else if !extra_attrs.is_empty() {
        json["zusatzAttribute"] = serde_json::Value::Array(extra_attrs);
    }
    Ok((invoice, json))
}

/// Assemble a consolidated document (Sammelrechnung, GGV-Sammel) from per-MaLo
/// engine invoices.
///
/// The per-MaLo runs stay stored as calculation records; the consolidated
/// document is the invoice the counterparty receives, so its VAT is computed
/// **once** over the combined base per rate — not summed from the per-MaLo
/// roundings, which can drift from a single consistent tax document by cents.
/// Concretely: the sub-invoices' Tax positions are stripped, the engine's tax
/// provider re-runs over the concatenated base (grouping by each position's
/// effective rate), and `to_rechnung_json` renders totals, steuerbetraege and
/// rechnungsdatum the same way every other invoice gets them.
///
/// Each rendered position carries the `marktlokationsId` it came from; the
/// document-level tax positions carry none, because they belong to the whole
/// document.
#[allow(clippy::too_many_arguments)]
fn build_aggregate_invoice(
    subject_id: &str,
    lf_mp_id: &str,
    rechnungsnummer: String,
    period_from: time::Date,
    period_to: time::Date,
    rates: RegulatoryRates,
    parts: Vec<(String, Invoice)>,
    extra_attrs: Vec<serde_json::Value>,
) -> anyhow::Result<(Invoice, serde_json::Value)> {
    let ctx = BillingContext {
        malo_id: subject_id.to_owned(),
        lf_mp_id: lf_mp_id.to_owned(),
        rechnungsnummer,
        period: BillingPeriod::new(period_from, period_to)
            .expect("parse_period guarantees from < to"),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates,
        ..Default::default()
    };

    let mut base: Vec<BillingPosition> = Vec::new();
    let mut warnings = Vec::new();
    // (malo_id, number of positions contributed) — for the JSON annotation.
    let mut slices: Vec<(String, usize)> = Vec::with_capacity(parts.len());
    for (malo_id, invoice) in parts {
        let non_tax: Vec<BillingPosition> = invoice
            .positions
            .into_iter()
            .filter(|p| p.category != PositionCategory::Tax)
            .collect();
        slices.push((malo_id, non_tax.len()));
        base.extend(non_tax);
        warnings.extend(invoice.warnings);
    }

    let tax = MwStProvider::new(ctx.regulatory_rates.mwst_rate)
        .bill(&ctx, &Quantities::default(), &base)
        .map_err(|e| anyhow::anyhow!("aggregate tax pass failed: {e}"))?;
    base.extend(tax);
    let aggregate = Invoice::from_positions(ctx, base, warnings);

    let mut json = aggregate.to_rechnung_json();
    if let Some(pos) = json
        .get_mut("rechnungspositionen")
        .and_then(|p| p.as_array_mut())
    {
        let mut idx = 0usize;
        for (malo_id, count) in &slices {
            for p in pos.iter_mut().skip(idx).take(*count) {
                if let Some(obj) = p.as_object_mut() {
                    obj.insert("marktlokationsId".to_owned(), serde_json::json!(malo_id));
                }
            }
            idx += count;
        }
    }
    if let Some(attrs) = json
        .get_mut("zusatzAttribute")
        .and_then(|a| a.as_array_mut())
    {
        attrs.extend(extra_attrs);
    } else if !extra_attrs.is_empty() {
        json["zusatzAttribute"] = serde_json::Value::Array(extra_attrs);
    }
    Ok((aggregate, json))
}

/// Structured JSON error body for a typed engine error.
///
/// Carries the stable machine-readable code, the display message, and — for a
/// blocked validation — every warning the engine collected, so a caller can
/// act on `MODUL2_AND_FLAT_NNE` without parsing prose.
fn engine_error_body(context: &str, e: &energy_billing::EngineError) -> String {
    serde_json::json!({
        "error": {
            "code": e.code(),
            "context": context,
            "message": e.to_string(),
            "warnings": e.blocking_warnings(),
        }
    })
    .to_string()
}

// ── Request bodies ─────────────────────────────────────────────────────────────

/// Request body for `POST /api/v1/billing/{malo_id}/calculate` and `/preview`.
///
/// All `*_meter` fields are optional — the engine selects the correct one based on
/// `tariff.category`.  Unsupported meter inputs for the active category are silently
/// ignored.  Supply `tariff` and/or `meter` as overrides to skip external lookups.
#[derive(Debug, Default, Deserialize)]
pub struct CalculateRequest {
    pub lf_mp_id: String,
    /// §41 Abs. 1 Nr. 5 EnWG — Netzbetreiber identification on the invoice.
    ///
    /// When set, propagated to `BillingContext.nb_mp_id`. When absent, `billingd`
    /// looks up the NB from `marktd` via the MaLo's grid assignment.
    #[serde(default)]
    pub nb_mp_id: Option<String>,
    pub period_from: String,
    pub period_to: String,
    /// Override: supply product data directly (skip tarifbd lookup).
    pub tariff: Option<Product>,
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
    /// Wasser/Abwasser meter + property input (WASSER category).
    #[serde(default)]
    pub wasser_meter: Option<WasserMeterInput>,
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
    /// Issue a Schlussrechnung (§40c EnWG: end of supply — move-out or
    /// supplier switch). Sets `rechnungsart = SCHLUSSRECHNUNG` and settles
    /// the paid `abschlaege` against the consumption bill.
    #[serde(default)]
    pub schlussrechnung: bool,
    /// Paid advance payments to settle on this invoice (§40c Abs. 2 EnWG:
    /// credits are offset with the next Abschlag or refunded within two
    /// weeks). Each entry carries the VAT rate it was invoiced at.
    #[serde(default)]
    pub abschlaege: Vec<energy_billing::AbschlagDeduction>,
}

// ── Calculate ─────────────────────────────────────────────────────────────────

/// `POST /api/v1/billing/{malo_id}/calculate`
///
/// Pipeline:
/// 1. Parse + validate period
/// 2. Fetch `Product` from `tarifbd` (or use request override)
/// 3. Fetch consumption from `edmd` (or use request override)
/// 4. Fetch grid pass-through from `marktd` (or use request override)
/// 5. Dispatch to category-specific pure calculator
/// 6. Persist `billing_records` (idempotent on same malo+period+product)
/// 7. Emit `de.billing.rechnung.erstellt` CloudEvent
#[allow(clippy::too_many_arguments)]
pub async fn post_calculate(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Extension(tarifbd): Extension<Arc<TarifbdClient>>,
    Extension(edmd): Extension<Arc<EdmdClient>>,
    Extension(marktd): Extension<Arc<mako_markt::marktd_client::MarktdClient>>,
    Extension(vertragd): Extension<Arc<VertragdClient>>,
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

    let rates = cfg.regulatory_rates_for_period(tariff.category_str(), period_from, period_to);
    // §14 Abs. 4 Nr. 4 UStG: the Rechnungsnummer must be einmalig. The DB
    // uniqueness spans (malo, period, product, tenant) — two products billed
    // for the same MaLo and period are distinct invoices, so the product code
    // is part of the number series.
    let rechnungsnummer = req.rechnungsnummer.clone().unwrap_or_else(|| {
        format!(
            "BILL-{malo_id}-{}-{period_from}",
            tariff.product_code().unwrap_or(tariff.category_str())
        )
    });

    let result = match dispatch_calculator(
        &cfg,
        &tariff,
        &req,
        &malo_id,
        &rechnungsnummer,
        period_from,
        period_to,
        &rates,
        &edmd,
        &marktd,
        &tarifbd,
        &vertragd,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };

    let record_id = match insert_billing_record(
        &pool,
        &cfg.tenant,
        &malo_id,
        &req.lf_mp_id,
        tariff.product_code().unwrap_or(tariff.category_str()),
        tariff.category_str(),
        period_from,
        period_to,
        &result.to_rechnung_json(),
        result.netto_eur,
        result.brutto_eur,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Deterministic risk gate: score, persist the findings, and hold
    // dispatch when the band demands an analyst.
    let assessment = assess_and_persist_risk(
        &pool,
        &cfg,
        record_id,
        &malo_id,
        &result,
        &rates,
        period_from,
        period_to,
    )
    .await;
    let held = assessment
        .as_ref()
        .is_some_and(|a| cfg.risk.hold_dispatch && a.band == crate::risk::RiskBand::Held);

    if held {
        tracing::warn!(
            %record_id, %malo_id,
            score = assessment.as_ref().map(|a| a.score),
            "billingd: invoice HELD by risk gate — dispatch requires POST …/release"
        );
    } else if let Some(ref webhook_url) = cfg.erp_webhook_url {
        emit_cloud_event(
            webhook_url,
            cfg.erp_hmac_secret.as_deref(),
            &pool,
            record_id,
            &malo_id,
            &req.lf_mp_id,
            &result.to_rechnung_json(),
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
            "risk": assessment,
            "held": held,
            "rechnung": result.to_rechnung_json(),
        })),
    )
        .into_response()
}

/// Score a freshly calculated invoice, persist the assessment, and return it.
///
/// Failures degrade to `None` (unscored) rather than failing the billing run —
/// a broken history query must not block invoice creation; the record simply
/// stays without a band and dispatches as before.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn assess_and_persist_risk(
    pool: &PgPool,
    cfg: &BillingdConfig,
    record_id: Uuid,
    malo_id: &str,
    invoice: &Invoice,
    rates: &RegulatoryRates,
    period_from: time::Date,
    period_to: time::Date,
) -> Option<crate::risk::RiskAssessment> {
    if !cfg.risk.enabled {
        return None;
    }
    let ctx = match crate::pg::risk_context(pool, &cfg.tenant, malo_id, period_from).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(%malo_id, error = %e, "billingd: risk context unavailable — record unscored");
            return None;
        }
    };
    let assessment = crate::risk::assess(
        &cfg.risk,
        invoice,
        rates.mwst_rate,
        period_from,
        period_to,
        &ctx,
    );
    if let Err(e) = crate::pg::set_risk(pool, record_id, &assessment).await {
        tracing::warn!(%record_id, error = %e, "billingd: risk persistence failed");
    }
    Some(assessment)
}

/// `GET /api/v1/billing/review-queue?band=&limit=`
///
/// The analyst work list: REVIEW and HELD records, highest risk first, each
/// carrying its coded findings.
pub async fn get_review_queue(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Query(q): Query<ReviewQueueQuery>,
) -> impl IntoResponse {
    match crate::pg::list_review_queue(
        &pool,
        &cfg.tenant,
        q.band.as_deref(),
        q.limit.unwrap_or(100).clamp(1, 1000),
    )
    .await
    {
        Ok(rows) => {
            Json(serde_json::json!({ "count": rows.len(), "records": rows })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct ReviewQueueQuery {
    pub band: Option<String>,
    pub limit: Option<i64>,
}

/// `POST /api/v1/billing/{id}/release`
///
/// Analyst release of a HELD record: stamps who released it and dispatches
/// the CloudEvent that the risk gate withheld. 409 when the record is not
/// currently held.
pub async fn post_release(
    claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match crate::pg::release_held_record(&pool, &cfg.tenant, id, claims.sub()).await {
        Ok(Some(row)) => {
            if let Some(ref webhook_url) = cfg.erp_webhook_url {
                emit_cloud_event(
                    webhook_url,
                    cfg.erp_hmac_secret.as_deref(),
                    &pool,
                    row.id,
                    &row.malo_id,
                    &row.lf_mp_id,
                    &row.rechnung_json,
                )
                .await;
            }
            Json(serde_json::json!({
                "id": row.id,
                "released_by": claims.sub(),
                "dispatched": cfg.erp_webhook_url.is_some(),
            }))
            .into_response()
        }
        Ok(None) => (
            StatusCode::CONFLICT,
            "record is not HELD (already released, dispatched, or unscored)",
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `POST /api/v1/billing/{malo_id}/preview` — dry-run, no persist, no CloudEvent.
#[allow(clippy::too_many_arguments)]
pub async fn post_preview(
    _claims: Claims,
    Extension(_pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Extension(tarifbd): Extension<Arc<TarifbdClient>>,
    Extension(edmd): Extension<Arc<EdmdClient>>,
    Extension(marktd): Extension<Arc<mako_markt::marktd_client::MarktdClient>>,
    Extension(vertragd): Extension<Arc<VertragdClient>>,
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
    let rates = cfg.regulatory_rates_for_period(tariff.category_str(), period_from, period_to);
    let rechnungsnummer = req
        .rechnungsnummer
        .clone()
        .unwrap_or_else(|| format!("PREVIEW-{malo_id}-{period_from}"));
    let result = match dispatch_calculator(
        &cfg,
        &tariff,
        &req,
        &malo_id,
        &rechnungsnummer,
        period_from,
        period_to,
        &rates,
        &edmd,
        &marktd,
        &tarifbd,
        &vertragd,
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
            "rechnung": result.to_rechnung_json(),
        })),
    )
        .into_response()
}

// ── Category dispatch via BillingEngine ──────────────────────────────────────

/// Build the `Quantities` for a billing request by resolving meter data.
#[allow(clippy::too_many_arguments)]
async fn build_quantities(
    tariff: &Product,
    req: &CalculateRequest,
    malo_id: &str,
    period_from: time::Date,
    period_to: time::Date,
    edmd: &Arc<EdmdClient>,
    marktd: &Arc<mako_markt::marktd_client::MarktdClient>,
    tarifbd: &Arc<TarifbdClient>,
) -> Result<Quantities, (StatusCode, String)> {
    let mut q = Quantities {
        eeg_gutschrift_eur: req.eeg_gutschrift_eur,
        ..Default::default()
    };

    match tariff.category_str() {
        "STROM" | "WAERMEPUMPE" | "WALLBOX" => {
            let is_dynamic = match tariff {
                Product::Strom(p) => p.dynamic_epex,
                Product::Waermepumpe(p) | Product::Wallbox(p) => p.base.dynamic_epex,
                _ => false,
            };
            if is_dynamic {
                q.dynamic_intervals =
                    fetch_dynamic_intervals(malo_id, period_from, period_to, edmd).await;
                q.dynamic_epex_prices = fetch_epex_prices(period_from, period_to, tarifbd).await;
            } else {
                q.electricity =
                    Some(resolve_strom_meter(req, malo_id, period_from, period_to, edmd).await?);
            }
        }
        "GAS" => {
            let mut meter = req.gas_meter.clone().unwrap_or_default();
            enrich_gas_meter(&mut meter, malo_id, period_from, period_to, edmd, marktd).await;
            q.gas = Some(meter);
        }
        "WAERME" => {
            q.heat = Some(req.waerme_meter.clone().unwrap_or_default());
        }
        "WASSER" => {
            q.wasser = Some(req.wasser_meter.clone().unwrap_or_default());
        }
        "SOLAR" => {
            q.solar = Some(req.solar_meter.clone().unwrap_or_default());
        }
        "EEG" | "EINSPEISUNG" => {
            q.eeg = Some(req.eeg_meter.clone().unwrap_or_default());
        }
        "HEMS" => {
            q.hems = Some(req.hems_meter.clone().unwrap_or_default());
        }
        "EMOBILITY" => {
            q.emobility = Some(req.emobility_meter.clone().unwrap_or_default());
        }
        "ENERGIEDIENSTLEISTUNG" => {
            q.service = Some(req.service_meter.clone().unwrap_or_default());
        }
        _ => {
            // Unknown category: try electricity as fallback
            q.electricity =
                Some(resolve_strom_meter(req, malo_id, period_from, period_to, edmd).await?);
        }
    }
    Ok(q)
}

/// Dispatch a billing request using the new `BillingEngine` architecture.
///
/// Replaces the old `dispatch_calculator` function.
/// Returns an `Invoice` instead of a `BillingResult`.
#[allow(clippy::too_many_arguments)]
async fn dispatch_invoice(
    cfg: &BillingdConfig,
    tariff: &Product,
    req: &CalculateRequest,
    malo_id: &str,
    rechnungsnummer: &str,
    period_from: time::Date,
    period_to: time::Date,
    rates: &RegulatoryRates,
    edmd: &Arc<EdmdClient>,
    marktd: &Arc<mako_markt::marktd_client::MarktdClient>,
    tarifbd: &Arc<TarifbdClient>,
    vertragd: &Arc<VertragdClient>,
) -> Result<Invoice, (StatusCode, String)> {
    let grid = req.grid.clone().unwrap_or_default();
    let quantities = build_quantities(
        tariff,
        req,
        malo_id,
        period_from,
        period_to,
        edmd,
        marktd,
        tarifbd,
    )
    .await?;

    // Generate a unique billing run ID for audit trail and duplicate detection.
    // Stored on the Invoice and propagated to the billing_records table.
    let run_id = Uuid::new_v4().to_string();

    // §40 Abs. 1 EnWG — the contract facts the invoice must state live in
    // vertragd, not in the tariff. Soft dependency: an unreachable vertragd
    // or an uncontracted MaLo degrades to an invoice without them, logged.
    let vertrag = match vertragd.get_vertrag_by_malo(malo_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(%malo_id, error = %e, "billingd: vertragd lookup failed — invoice will lack §40 contract facts");
            None
        }
    };

    // §40 Abs. 2 Nr. 6 EnWG — meter identity. The device registry (marktd)
    // is the authority: MaLo → Lokationszuordnung → MeLo → Zähler. Soft
    // dependency, logged when missing.
    let zaehler_id = resolve_zaehlernummer(marktd, malo_id).await;
    if zaehler_id.is_none() {
        tracing::warn!(%malo_id, "billingd: no Zählernummer resolvable via marktd — invoice will lack §40 Abs. 2 Nr. 6 meter identity");
    }

    // §40 Abs. 2 Nr. 7/8 EnWG — consumption comparison. Prior-year kWh from
    // edmd (same window one year earlier); the comparable-customer-group
    // value comes from operator config (Stromspiegel/BDEW reference data),
    // pro-rated to the billing period's length.
    let verbrauchshistorie =
        resolve_verbrauchshistorie(cfg, edmd, malo_id, period_from, period_to).await;
    let vertragsinformationen = vertrag.as_ref().map(|v| {
        energy_billing::Vertragsinformationen {
            vertragsdauer: Some(match v.vertrag.vertragsende {
                Some(ende) => format!("{} bis {ende}", v.vertrag.vertragsbeginn),
                None => format!("unbefristet seit {}", v.vertrag.vertragsbeginn),
            }),
            kuendigungsfrist: Some(match v.vertrag.kuendigungsfrist_monate {
                1 => "1 Monat".to_owned(),
                n => format!("{n} Monate"),
            }),
            naechstmoeglicher_kuendigungstermin: v.naechstmoeglicher_kuendigungstermin,
            // The next settlement follows the cadence of this one: a period of
            // the same length, starting the day after this one ends.
            naechster_abrechnungstermin: period_to.checked_add(time::Duration::days(
                (period_to - period_from).whole_days() + 1,
            )),
        }
    });

    let ctx = BillingContext {
        malo_id: malo_id.to_owned(),
        lf_mp_id: req.lf_mp_id.clone(),
        rechnungsnummer: rechnungsnummer.to_owned(),
        period: BillingPeriod::new(period_from, period_to)
            .expect("parse_period guarantees from < to"),
        // §40c EnWG: a Schlussrechnung (end of supply) settles the account;
        // the engine renders rechnungsart = SCHLUSSRECHNUNG and deducts the
        // paid Abschläge below from the Zahlbetrag.
        invoice_type: if req.schlussrechnung {
            InvoiceType::Final
        } else {
            InvoiceType::Initial
        },
        abschlage: req.abschlaege.clone(),
        regulatory_rates: rates.clone(),
        contract_id: vertrag.as_ref().map(|v| {
            v.vertrag
                .vertrags_nr
                .clone()
                .unwrap_or_else(|| v.vertrag.id.clone())
        }),
        // §41 EnWG pro-rata: clip the billable days to the contract term.
        vertragsbeginn: vertrag.as_ref().map(|v| v.vertrag.vertragsbeginn),
        vertragsende: vertrag.as_ref().and_then(|v| v.vertrag.vertragsende),
        vertragsinformationen,
        // §40 Abs. 2 Nr. 6 EnWG — Zählernummer from the marktd device registry.
        zaehler_id,
        // §40 Abs. 2 Nr. 7/8 EnWG — Vorjahresverbrauch + Vergleichsgruppe.
        verbrauchshistorie,
        // §40 Abs. 2 Nr. 1/9/10/11/12 EnWG — supplier contact from config;
        // the statutory Schlichtungsstelle/BNetzA/Beratung hints come from
        // the engine defaults.
        verbraucherinformationen: Some(energy_billing::Verbraucherinformationen {
            lieferant_name: Some(
                cfg.seller_name
                    .clone()
                    .unwrap_or_else(|| cfg.tenant.clone()),
            ),
            lieferant_anschrift: cfg.seller_address.clone(),
            lieferant_kontakt: cfg.seller_contact.clone(),
            ..Default::default()
        }),
        // Propagate minimum invoice from product definition (tarifbd) to billing context.
        minimum_invoice_eur_brutto: tariff.minimum_invoice_eur_brutto(),
        // §42 EnWG — the product's Stromkennzeichnung, structured, so the
        // invoice can state the fuel mix and the mandatory CO₂ figure.
        energiequellen: tariff.energiequellen().cloned(),
        // §41 Abs. 1 Nr. 5 EnWG — Netzbetreiber identification on invoice.
        nb_mp_id: req.nb_mp_id.clone(),
        // Audit trail: unique run ID links DB record to calculation output.
        billing_run_id: Some(run_id),
        ..Default::default()
    };

    let engine = tariff.build_engine(&grid, rates);

    let mut invoice = engine.bill(ctx, &quantities).map_err(|e| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            engine_error_body(malo_id, &e),
        )
    })?;

    // §40c EnWG — Abrechnungen must reach the customer within six weeks of
    // the end of the Abrechnungszeitraum (Schlussrechnungen: of the end of
    // supply); **three weeks** for monthly billing. The engine is clock-free
    // by design, so the deadline is checked here, where a clock legitimately
    // exists: generation time is what the law measures.
    let deadline_weeks = if (period_to - period_from).whole_days() <= 32 {
        3 // §40c Abs. 1 S. 2: monatliche Abrechnung → drei Wochen
    } else {
        6
    };
    let deadline = period_to + time::Duration::weeks(deadline_weeks);
    let today = time::OffsetDateTime::now_utc().date();
    if today > deadline {
        tracing::warn!(
            %malo_id,
            %period_to,
            %deadline,
            "billingd: invoice issued after the §40c EnWG deadline"
        );
        invoice.warnings.push(energy_billing::BillingWarning {
            code: "SECT40C_DEADLINE_EXCEEDED",
            severity: energy_billing::WarningSeverity::Warning,
            message: format!(
                "issued {today}, after the §40c EnWG deadline of {deadline} \
                 ({deadline_weeks} weeks past the period end {period_to})"
            ),
        });
    }
    Ok(invoice)
}

/// Resolve the Zählernummer serving a MaLo via the marktd device registry:
/// MaLo → Lokationszuordnung (B5 graph) → MeLo → Zähler.
///
/// Returns the first registered Zähler of the first linked MeLo — the common
/// single-meter case. Multi-meter locations carry their per-meter identity in
/// `MeterInput::zaehlernummer` from the caller instead.
async fn resolve_zaehlernummer(
    marktd: &Arc<mako_markt::marktd_client::MarktdClient>,
    malo_id: &str,
) -> Option<String> {
    let edges = marktd.get_lokationen(malo_id, "malo", None).await.ok()?;
    let melo_id = edges
        .iter()
        .find(|e| e.nach_typ == "melo")
        .map(|e| e.nach_id.clone())
        .or_else(|| {
            edges
                .iter()
                .find(|e| e.von_typ == "melo")
                .map(|e| e.von_id.clone())
        })?;
    marktd
        .list_zaehler_ids(&melo_id)
        .await
        .ok()?
        .into_iter()
        .next()
}

/// The same calendar window one year earlier, Feb 29 clamped to Feb 28.
fn year_earlier(d: time::Date) -> time::Date {
    d.replace_year(d.year() - 1)
        .unwrap_or_else(|_| d - time::Duration::days(365))
}

/// §40 Abs. 2 Nr. 7/8 EnWG — assemble the consumption comparison.
///
/// Prior-year consumption comes from edmd (soft dependency); the
/// comparable-customer-group annual value comes from operator config and is
/// pro-rated to the billing period. Returns `None` when neither source
/// yields a figure, so the engine omits the comparison positions instead of
/// rendering empty ones.
async fn resolve_verbrauchshistorie(
    cfg: &BillingdConfig,
    edmd: &Arc<EdmdClient>,
    malo_id: &str,
    period_from: time::Date,
    period_to: time::Date,
) -> Option<energy_billing::Verbrauchshistorie> {
    let vorjahr_kwh = match edmd
        .get_billing_period(malo_id, year_earlier(period_from), year_earlier(period_to))
        .await
    {
        Ok(Some(m)) if m.arbeitsmenge_kwh > Decimal::ZERO => Some(m.arbeitsmenge_kwh),
        Ok(_) => None,
        Err(e) => {
            tracing::debug!(%malo_id, error = %e, "billingd: no prior-year consumption from edmd");
            None
        }
    };

    let bundesdurchschnitt_kwh = cfg.vergleichsgruppe_kwh_pro_jahr.map(|annual| {
        let days = Decimal::from((period_to - period_from).whole_days() + 1);
        let year_days = Decimal::from(time::util::days_in_year(period_from.year()));
        energy_billing::round_money(annual * days / year_days, 0)
    });

    if vorjahr_kwh.is_none() && bundesdurchschnitt_kwh.is_none() {
        return None;
    }
    Some(energy_billing::Verbrauchshistorie {
        vorjahr_kwh,
        bundesdurchschnitt_kwh,
        kundengruppe: cfg.vergleichsgruppe_label.clone(),
    })
}

/// Backward-compat shim: dispatch and return Invoice.
///
/// Called by existing HTTP handlers.
/// New callers should use `dispatch_invoice` directly.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch_calculator(
    cfg: &BillingdConfig,
    tariff: &Product,
    req: &CalculateRequest,
    malo_id: &str,
    rechnungsnummer: &str,
    period_from: time::Date,
    period_to: time::Date,
    rates: &RegulatoryRates,
    edmd: &Arc<EdmdClient>,
    marktd: &Arc<mako_markt::marktd_client::MarktdClient>,
    tarifbd: &Arc<TarifbdClient>,
    vertragd: &Arc<VertragdClient>,
) -> Result<Invoice, (StatusCode, String)> {
    dispatch_invoice(
        cfg,
        tariff,
        req,
        malo_id,
        rechnungsnummer,
        period_from,
        period_to,
        rates,
        edmd,
        marktd,
        tarifbd,
        vertragd,
    )
    .await
}

// ── Records ────────────────────────────────────────────────────────────────────

pub async fn list_records(
    _claims: Claims,
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
    _claims: Claims,
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
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let row = match fetch_billing_record(&pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "billing record not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    // The stored record's own period decides its rates — an XRechnung rendered
    // for an old record must state the VAT rate that period was billed under.
    let rates = cfg.regulatory_rates_for_period(&row.category, row.period_from, row.period_to);
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
        rates.mwst_rate * rust_decimal::dec!(100),
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

pub(crate) async fn resolve_tariff(
    req: &CalculateRequest,
    tarifbd: &TarifbdClient,
    malo_id: &str,
) -> Result<Product, (StatusCode, String)> {
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

// ── Gas meter auto-enrichment ─────────────────────────────────────────────────

/// Normalize a raw `gasqualitaet` string to a canonical BO4E / BNetzA MaStR form.
///
/// ## Canonical values
///
/// | Canonical | Aliases accepted |
/// |---|---|
/// | `H_GAS` | `HGas`, `H-Gas`, `H-gas`, `HGAS`, `HIGH_CALORIFIC` |
/// | `L_GAS` | `LGas`, `L-Gas`, `L-gas`, `LGAS`, `LOW_CALORIFIC` |
/// | `H2_BLEND` | `H2Blend`, `H2-Blend`, `HYDROGEN_BLEND` |
/// | `BIOGAS` | `BioGas`, `Bio-Gas` |
/// | `FLUESSIGGAS` | `LPG`, `FlüssigGas` |
///
/// Unknown values are returned as-is (upper-case, underscores).
///
/// ## Why normalization matters
///
/// `marktd` stores `gasqualitaet` as extracted from the UTILMD G `STS+E01+Z12`
/// qualifier — typically `"HGas"` or `"LGas"` (legacy German abbreviations).
/// The BO4E schema (`rubo4e::GasQualitaet`) and BNetzA MaStR use `"H_GAS"` /
/// `"L_GAS"` / `"H2_BLEND"`.  Billing invoices, comparison portals, and AI agents
/// all benefit from a single canonical form.
pub(crate) fn normalize_gasqualitaet(raw: &str) -> String {
    // Normalize to UPPER_SNAKE_CASE first for uniform matching.
    let norm = raw.trim().to_uppercase().replace(['-', ' '], "_");
    match norm.as_str() {
        "HGAS" | "H_GAS" | "HIGH_CALORIFIC" | "HOCHKALORISCH" | "ERDGAS_H" => "H_GAS".to_owned(),
        "LGAS" | "L_GAS" | "LOW_CALORIFIC" | "NIEDERKALORISCH" | "ERDGAS_L" => "L_GAS".to_owned(),
        "H2_BLEND" | "H2BLEND" | "HYDROGEN_BLEND" | "HYDROGEN_GAS" | "H2_GAS" => {
            "H2_BLEND".to_owned()
        }
        "BIOGAS" | "BIO_GAS" | "BIOMETHANE" | "BIOMETHAN" => "BIOGAS".to_owned(),
        "FLUESSIGGAS" | "FLUSSIGGAS" | "LPG" | "LIQUID_GAS" => "FLUESSIGGAS".to_owned(),
        other => other.to_owned(),
    }
}

/// Auto-enrich a `GasMeterInput` with data from `edmd` and `marktd`.
///
/// This is the **Gas billing data pipeline** for `billingd`.  It fills in
/// missing fields using the priority order below, without overriding anything
/// the caller already supplied.
///
/// ## Priority order (highest to lowest)
///
/// | Field | 1st source | 2nd source | Fallback |
/// |---|---|---|---|
/// | `messung_qm3` | caller (`req.gas_meter`) | `edmd` billing-period | `0` (engine rejects) |
/// | `brennwert_kwh_per_qm3` | caller | edmd **gas-quality** (PID 13007) | edmd billing-period | `None` (engine applies default 10.55) |
/// | `zustandszahl` | caller | edmd gas-quality (PID 13007) | edmd billing-period | `None` (engine applies default 1.0) |
/// | `spitzenleistung_kw` | caller | edmd billing-period | `None` (no RLM demand charge) |
/// | `gasqualitaet` | caller | marktd MaLo fields | `None` (no audit annotation) |
///
/// ## Non-blocking
///
/// All external fetches are best-effort.  Failures are logged as `WARN` and
/// billing proceeds with the data available.  This prevents an edmd or marktd
/// outage from blocking all gas invoicing.
///
/// ## DVGW G 685 / §25 Nr. 4 MessEV compliance
///
/// `brennwert_kwh_per_qm3` × `zustandszahl` converts m³ → kWh_Hs.  The
/// energy-billing engine applies DVGW defaults when both are absent:
/// - brennwert: 10.55 kWh/m³ (German-average H-Gas per DVGW G 685 §5.3)
/// - zustandszahl: 1.0 (pressure/temperature ≈ reference conditions)
///
/// To suppress the engine default and ensure the DSO-published values are
/// always used, operators should verify that MSCONS PID 13007 data is flowing
/// into `edmd` before running billing.
async fn enrich_gas_meter(
    meter: &mut GasMeterInput,
    malo_id: &str,
    period_from: time::Date,
    period_to: time::Date,
    edmd: &EdmdClient,
    marktd: &Arc<mako_markt::marktd_client::MarktdClient>,
) {
    use crate::clients::{GasBillingPeriod, GasQualityRecord};

    // Track which fields were enriched for structured logging.
    let mut enriched_from_edmd_period = false;
    let mut enriched_bw_from_edmd_quality = false;
    let mut enriched_gq_from_marktd = false;

    // ── Step 1: Energy + conversion factors from edmd billing period ──────────
    // Fetch only when the caller supplied neither a volume nor an energy value.
    // edmd reports gas energy as kWh_Hs (`arbeitsmenge_kwh`, Brennwert already
    // applied by the DSO's MSCONS data) plus the applied conversion factors and
    // the §40 Abs. 2 Nr. 6 register readings.
    if meter.messung_qm3 == rust_decimal::Decimal::ZERO && meter.kwh_hs.is_none() {
        match edmd
            .get_gas_billing_period(malo_id, period_from, period_to)
            .await
        {
            Ok(Some(GasBillingPeriod {
                kwh_hs,
                brennwert_kwh_per_qm3,
                zustandszahl,
                spitzenleistung_kw,
                zaehlerstand_von,
                zaehlerstand_bis,
                is_estimated,
            })) => {
                meter.kwh_hs = kwh_hs;
                if meter.brennwert_kwh_per_qm3.is_none() {
                    meter.brennwert_kwh_per_qm3 = brennwert_kwh_per_qm3;
                }
                if meter.zustandszahl.is_none() {
                    meter.zustandszahl = zustandszahl;
                }
                if meter.spitzenleistung_kw.is_none() {
                    meter.spitzenleistung_kw = spitzenleistung_kw;
                }
                if meter.zaehlerstand_von.is_none() {
                    meter.zaehlerstand_von = zaehlerstand_von;
                }
                if meter.zaehlerstand_bis.is_none() {
                    meter.zaehlerstand_bis = zaehlerstand_bis;
                }
                meter.is_estimated |= is_estimated;
                enriched_from_edmd_period = true;
            }
            Ok(None) => {
                tracing::debug!(malo_id, "billingd GAS: no billing period in edmd");
            }
            Err(e) => {
                tracing::warn!(
                    malo_id,
                    error = %e,
                    "billingd GAS: edmd billing-period fetch failed — proceeding without"
                );
            }
        }
    }

    // ── Step 2: Abrechnungsbrennwert + Zustandszahl from edmd gas-quality ─────
    // MSCONS PID 13007 (Gasbeschaffenheitsdaten) carries the DSO-published
    // monthly Brennwert and Zustandszahl — more precise than the billing-period
    // summary because it covers the exact billing window.
    // Only fetch when at least one conversion factor is still missing.
    if meter.brennwert_kwh_per_qm3.is_none() || meter.zustandszahl.is_none() {
        match edmd.get_gas_quality(malo_id).await {
            Ok(Some(records)) => {
                // Find the record whose period best covers the billing period.
                // "Best" = latest period_from that still starts ≤ billing period end,
                // ensuring we pick the most recent DSO-published Brennwert.
                let best: Option<&GasQualityRecord> = records
                    .iter()
                    .filter(|q| q.period_from <= period_to && q.period_to >= period_from)
                    .max_by_key(|q| q.period_from);

                if let Some(q) = best {
                    if meter.brennwert_kwh_per_qm3.is_none() {
                        meter.brennwert_kwh_per_qm3 = Some(q.brennwert_kwh_per_m3);
                        enriched_bw_from_edmd_quality = true;
                    }
                    if meter.zustandszahl.is_none() {
                        meter.zustandszahl = Some(q.zustandszahl);
                        enriched_bw_from_edmd_quality = true;
                    }
                } else if !records.is_empty() {
                    tracing::debug!(
                        malo_id,
                        period_from = %period_from,
                        period_to   = %period_to,
                        "billingd GAS: edmd gas-quality records exist but none cover billing period"
                    );
                }
            }
            Ok(None) => {
                tracing::debug!(
                    malo_id,
                    "billingd GAS: no gas-quality data in edmd (PID 13007 not yet received)"
                );
            }
            Err(e) => {
                tracing::warn!(
                    malo_id,
                    error = %e,
                    "billingd GAS: edmd gas-quality fetch failed — proceeding without"
                );
            }
        }
    }

    // ── Step 3: gasqualitaet annotation from marktd MaLo ──────────────────────
    // Informational only — billing always uses the measured Brennwert.
    // Annotated on the invoice as `ZusatzAttribut` for § 147 AO / GoBD audit trail
    // and for H2-blend detection in downstream AI agents (eeg-compliance-agent).
    if meter.gasqualitaet.is_none() {
        match marktd.get_malo(malo_id).await {
            Ok(Some(malo_fields)) => {
                if let Some(raw_gq) = malo_fields.gasqualitaet {
                    let canonical = normalize_gasqualitaet(&raw_gq);
                    meter.gasqualitaet = Some(canonical);
                    enriched_gq_from_marktd = true;
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    malo_id,
                    error = %e,
                    "billingd GAS: marktd get_malo failed — proceeding without gasqualitaet"
                );
            }
        }
    }

    // ── Structured enrichment summary ─────────────────────────────────────────
    // Logged at DEBUG level so billing operators can verify auto-enrichment
    // without flooding production logs.
    if enriched_from_edmd_period || enriched_bw_from_edmd_quality || enriched_gq_from_marktd {
        tracing::debug!(
            malo_id,
            messung_qm3                   = %meter.messung_qm3,
            brennwert_kwh_per_qm3         = ?meter.brennwert_kwh_per_qm3,
            zustandszahl                  = ?meter.zustandszahl,
            spitzenleistung_kw            = ?meter.spitzenleistung_kw,
            gasqualitaet                  = ?meter.gasqualitaet,
            enriched_from_edmd_period,
            enriched_bw_from_edmd_quality,
            enriched_gq_from_marktd,
            "billingd GAS: meter enrichment complete"
        );
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
    tarifbd: &Arc<TarifbdClient>,
) -> std::collections::HashMap<(i32, u8, u8, u8), rust_decimal::Decimal> {
    // tarifbd owns the imported EPEX day-ahead series. This was a stub returning
    // an empty map, which made every §41a dynamic calculate run price all
    // intervals at nothing — the client function existed the whole time and was
    // dead code.
    match tarifbd.get_hourly_epex_prices(period_from, period_to).await {
        Ok(map) => map,
        Err(e) => {
            tracing::warn!(error = %e, "billingd: EPEX price fetch failed; dynamic intervals will lack prices");
            std::collections::HashMap::new()
        }
    }
}

pub(crate) async fn emit_cloud_event(
    webhook_url: &str,
    hmac_secret: Option<&str>,
    pool: &PgPool,
    record_id: Uuid,
    malo_id: &str,
    lf_mp_id: &str,
    rechnung: &serde_json::Value,
) {
    emit_cloud_event_inner(
        webhook_url,
        hmac_secret,
        pool,
        record_id,
        malo_id,
        lf_mp_id,
        rechnung,
        false,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn emit_cloud_event_inner(
    webhook_url: &str,
    hmac_secret: Option<&str>,
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
    let body = serde_json::to_vec(&ce).unwrap_or_default();
    let client = reqwest::Client::new();
    let mut req = client
        .post(webhook_url)
        .header("Content-Type", "application/cloudevents+json")
        .body(body.clone());
    // The config documents `erp_hmac_secret` as signing outbound events —
    // an unsigned emit would be the recurring divergent-worker-event defect.
    if let Some(secret) = hmac_secret {
        let sig = mako_markt::cloudevents::compute_signature(secret.as_bytes(), &body);
        req = req.header("X-Mako-Signature", sig);
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            let _ = mark_dispatched(pool, record_id, ce_id).await;
        }
        Ok(resp) => {
            tracing::warn!(record_id = %record_id, status = %resp.status(), "billingd: ERP webhook failed")
        }
        Err(e) => tracing::warn!(record_id = %record_id, error = %e, "billingd: ERP webhook error"),
    }
}

// ── Korrekturrechnung (L8 — § 147 AO / GoBD) ──────────────────────────────────────

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
/// ## § 147 AO / GoBD compliance
///
/// The original record is **never modified** — corrections always produce new
/// records.  Both the original and the correction are kept in `billing_records`
/// for the mandatory 3-year audit trail.
pub async fn post_correction(
    _claims: Claims,
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

    // §14 Abs. 4 Nr. 4 UStG: `KORR-{original_nr}` must stay einmalig — a
    // second correction of the same original would duplicate the number and
    // double-negate the amounts in accounting.
    match sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM billing_records WHERE original_record_id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    {
        Ok(0) => {}
        Ok(_) => {
            return (
                StatusCode::CONFLICT,
                "a correction for this record already exists — bill the corrected \
                 amounts as a new invoice instead of correcting twice",
            )
                .into_response();
        }
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }

    // Produce a Korrekturrechnung JSON by negating the original via the library function.
    // The library owns all sign-negation logic for consistency with the engine's
    // negate_positions() path used for fresh Cancellation calculations.
    let id_str = id.to_string();
    let original_nr = original
        .rechnung_json
        .get("rechnungsnummer")
        .and_then(|v| v.as_str())
        .unwrap_or(&id_str);
    let new_nr = format!("KORR-{original_nr}");
    let corrected_json =
        negate_rechnung_json_for_correction(&original.rechnung_json, original_nr, &new_nr);

    let netto = -original.total_netto_eur.unwrap_or_default();
    let brutto = -original.total_brutto_eur.unwrap_or_default();

    let correction_id = match insert_correction_record(
        &pool,
        &cfg.tenant,
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
            cfg.erp_hmac_secret.as_deref(),
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

// ── §42b EEG 2023 (Solarpaket I) GGV Community Solar Multi-Tenant Billing ─────

/// Per-tenant input for the GGV proportional billing endpoint.
///
/// Each entry represents one tenant delivery point under the shared PV installation.
/// `consumption_kwh` is the metered actual consumption for the billing period from `edmd`.
#[derive(Debug, serde::Deserialize)]
pub struct GgvTenantInput {
    /// 11-digit MaLo-ID for this tenant's delivery point.
    pub malo_id: String,
    /// Metered actual consumption for the period (kWh) — from `edmd`.
    ///
    /// When `nutzungsplan` is set in `GgvBillingRequest`, billing is split into
    /// PV portion (allocated from plant generation) + residual grid portion.
    /// Without `nutzungsplan`, the full amount is billed as solar eigenverbrauch.
    pub consumption_kwh: rust_decimal::Decimal,
    /// Override product code; if absent, looked up from `tarifbd`.
    pub product_code: Option<String>,
    /// Supply price override (ct/kWh); if absent, looked up from `tarifbd`.
    pub arbeitspreis_ct_per_kwh: Option<rust_decimal::Decimal>,
    /// Standard grid electricity rate for residual consumption (ct/kWh).
    ///
    /// Required when `pv_generation_kwh` is set and some tenants have consumption
    /// exceeding their PV allocation (grid fallback billing).
    pub grid_arbeitspreis_ct_per_kwh: Option<rust_decimal::Decimal>,
    /// GGV Rabatt on the PV portion (ct/kWh, §42b Abs. 3 EEG 2023).
    ///
    /// The discount reduces the net price of the PV portion below the standard
    /// electricity rate. Per §42b Abs. 3 EEG 2023 the LF must pass on savings from
    /// reduced grid charges for locally consumed PV electricity.
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
    #[serde(default)]
    pub nb_mp_id: Option<String>,
    pub period_from: String,
    pub period_to: String,
    /// Total PV generation of the GGV plant for the billing period (kWh).
    ///
    /// When supplied together with `nutzungsplan`, enables the full §42b billing model:
    /// - PV generation is allocated per tenant via the Nutzungsplan fractions
    /// - Each tenant invoice shows both the PV portion and the grid fallback portion
    ///
    /// When `None`, each tenant's full `consumption_kwh` is billed as solar eigenverbrauch
    /// (the previous simplified model — valid only when consumption ≤ plant output).
    pub pv_generation_kwh: Option<rust_decimal::Decimal>,
    /// GGV allocation plan: tenant fractions that sum to 1.0.
    ///
    /// Required when `pv_generation_kwh` is supplied. The fractions determine how much
    /// of the plant's generation each tenant is entitled to (§42b Abs. 2 EEG 2023).
    /// Entries must match the `malo_id` values in `tenants`.
    pub nutzungsplan: Option<Vec<NutzungsplanInput>>,
    /// All tenant delivery points belonging to this GGV installation.
    pub tenants: Vec<GgvTenantInput>,
}

/// One entry in the GGV Nutzungsplan submitted via the billing API.
#[derive(Debug, serde::Deserialize)]
pub struct NutzungsplanInput {
    pub malo_id: String,
    pub fraction: rust_decimal::Decimal,
}

/// `POST /api/v1/billing/ggv/{ggv_id}` — §42b EEG 2023 community solar billing.
///
/// ## Algorithm (§42a EEG 2023 proportional allocation)
///
/// 1. Validate all tenant inputs; reject if `tenants` is empty or total kWh = 0.
/// ## §42b EEG 2023 (Solarpaket I) compliance
///
/// The handler supports two billing models:
///
/// **Model A — Nutzungsplan-based (recommended)**: Supply `pv_generation_kwh` +
/// `nutzungsplan`. Each tenant is allocated a proportional share of plant generation.
/// Request body for `POST /api/v1/billing/{malo_id}/tarifwechsel`.
///
/// Calculates a combined invoice when a price change occurs within the billing
/// period. Uses `billing::merge_period_documents` semantics via `Invoice::merge()`.
#[derive(Debug, serde::Deserialize)]
pub struct TarifwechselRequest {
    /// Lieferant MP-ID.
    pub lf_mp_id: String,
    /// §41 Abs. 1 Nr. 5 EnWG — Netzbetreiber identification.
    #[serde(default)]
    pub nb_mp_id: Option<String>,
    /// Start of the billing period (inclusive, YYYY-MM-DD).
    pub period_from: String,
    /// End of the billing period (inclusive, YYYY-MM-DD).
    pub period_to: String,
    /// Date when the new tariff takes effect (YYYY-MM-DD, must be within the period).
    pub switch_date: String,
    /// Old tariff (applies from `period_from` to `switch_date - 1`).
    pub old_tariff: Product,
    /// New tariff (applies from `switch_date` to `period_to`).
    pub new_tariff: Product,
    /// Meter data for the old sub-period.
    #[serde(default)]
    pub old_meter: Option<MeterInput>,
    /// Meter data for the new sub-period.
    #[serde(default)]
    pub new_meter: Option<MeterInput>,
    /// Optional grid pass-through data.
    #[serde(default)]
    pub grid: Option<GridInput>,
}

/// `POST /api/v1/billing/{malo_id}/tarifwechsel`
///
/// Calculates a combined invoice for a billing period containing a price change
/// (Tarifwechsel). The period is split at `switch_date`:
///
/// - **Sub-period A**: `period_from` → `switch_date - 1` at `old_tariff`
/// - **Sub-period B**: `switch_date` → `period_to` at `new_tariff`
///
/// The two invoices are merged via [`Invoice::merge()`] using the same logic
/// as `billing::merge_period_documents`: positions are concatenated, totals
/// re-summed. Tax is applied **independently** per sub-period (correct for
/// mid-month rate changes per §41 EnWG).
///
/// ## Legal basis
///
/// §41 Abs. 1 Nr. 4 EnWG: every price change requires transparent itemisation
/// on the next invoice showing the old and new price with their respective
/// applicable periods.
pub async fn post_tarifwechsel(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Path(malo_id): Path<String>,
    Json(req): Json<TarifwechselRequest>,
) -> impl IntoResponse {
    // Parse all three date boundaries
    let (period_from, period_to) = match parse_period(&req.period_from, &req.period_to) {
        Ok(d) => d,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("period: {e}")).into_response(),
    };
    let switch_date = match parse_period(&req.switch_date, &req.switch_date) {
        Ok((d, _)) => d,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("switch_date: {e}")).into_response(),
    };
    if switch_date <= period_from || switch_date > period_to {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "switch_date {switch_date} must be strictly inside [{period_from}, {period_to}]"
            ),
        )
            .into_response();
    }

    let grid = req.grid.clone().unwrap_or_default();

    // Build the rechnungsnummer prefix — use timestamp for uniqueness
    let base_nr = format!("TW-{malo_id}-{period_from}",);

    // ── Sub-period A: period_from → switch_date - 1 ───────────────────────────
    // Each leg is billed under the statutory rates of *its own* dates and
    // commodity — that is the point of the split (§41 Abs. 5 EnWG price
    // change; a leg inside a VAT window carries that window's rate).
    let period_a_to = switch_date - time::Duration::days(1);
    let rates_a =
        cfg.regulatory_rates_for_period(req.old_tariff.category_str(), period_from, period_a_to);
    let run_id_a = Uuid::new_v4().to_string();
    let ctx_a = BillingContext {
        malo_id: malo_id.clone(),
        lf_mp_id: req.lf_mp_id.clone(),
        rechnungsnummer: format!("{base_nr}-A"),
        period: BillingPeriod::new(period_from, period_a_to)
            .expect("switch date is validated inside the period"),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates_a.clone(),
        nb_mp_id: req.nb_mp_id.clone(),
        billing_run_id: Some(run_id_a),
        ..Default::default()
    };
    let quantities_a = Quantities {
        electricity: req.old_meter.clone(),
        ..Default::default()
    };
    let engine_a = req.old_tariff.build_engine(&grid, &rates_a);
    let inv_a = match engine_a.bill(ctx_a, &quantities_a) {
        Ok(i) => i,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                engine_error_body("tarifwechsel period A", &e),
            )
                .into_response();
        }
    };

    // ── Sub-period B: switch_date → period_to ─────────────────────────────────
    let rates_b =
        cfg.regulatory_rates_for_period(req.new_tariff.category_str(), switch_date, period_to);
    let run_id_b = Uuid::new_v4().to_string();
    let ctx_b = BillingContext {
        malo_id: malo_id.clone(),
        lf_mp_id: req.lf_mp_id.clone(),
        rechnungsnummer: format!("{base_nr}-B"),
        period: BillingPeriod::new(switch_date, period_to)
            .expect("switch date is validated inside the period"),
        invoice_type: InvoiceType::Initial,
        regulatory_rates: rates_b.clone(),
        nb_mp_id: req.nb_mp_id.clone(),
        billing_run_id: Some(run_id_b),
        ..Default::default()
    };
    let quantities_b = Quantities {
        electricity: req.new_meter.clone(),
        ..Default::default()
    };
    let engine_b = req.new_tariff.build_engine(&grid, &rates_b);
    let inv_b = match engine_b.bill(ctx_b, &quantities_b) {
        Ok(i) => i,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                engine_error_body("tarifwechsel period B", &e),
            )
                .into_response();
        }
    };

    // ── Merge via billing::merge_period_documents semantics ───────────────────
    let merged = inv_a.merge(inv_b);
    merged.assert_valid();

    let rechnung_json = merged.to_rechnung_json();
    let netto = merged.netto_eur;
    let brutto = merged.brutto_eur;

    let product_code = format!(
        "{}-{}",
        req.old_tariff.category_str(),
        req.new_tariff.category_str()
    );
    let record_id = match insert_billing_record(
        &pool,
        &cfg.tenant,
        &malo_id,
        &req.lf_mp_id,
        &product_code,
        "TARIFWECHSEL",
        period_from,
        period_to,
        &rechnung_json,
        netto,
        brutto,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    if let Some(ref webhook_url) = cfg.erp_webhook_url {
        emit_cloud_event_inner(
            webhook_url,
            cfg.erp_hmac_secret.as_deref(),
            &pool,
            record_id,
            &malo_id,
            &req.lf_mp_id,
            &rechnung_json,
            false,
        )
        .await;
    }

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "record_id": record_id,
            "malo_id": malo_id,
            "period_from": period_from.to_string(),
            "switch_date": switch_date.to_string(),
            "period_to": period_to.to_string(),
            "netto_eur": netto,
            "brutto_eur": brutto,
            "old_category": req.old_tariff.category_str(),
            "new_category": req.new_tariff.category_str(),
        })),
    )
        .into_response()
}

/// Per-tenant invoices show both the PV portion (at GGV rate) and the residual grid
/// electricity (at standard rate). This correctly implements §42b Abs. 2 EEG 2023.
///
/// **Model B — Direct consumption (legacy)**: Omit `pv_generation_kwh`. Each
/// tenant's full `consumption_kwh` is billed as solar eigenverbrauch. Only valid
/// when consumption ≤ plant output for all tenants.
///
/// The GGV Rabatt (§42b Abs. 3 EEG 2023) must reflect the savings from reduced
/// network charges for locally consumed PV electricity.
#[allow(clippy::too_many_arguments)]
pub async fn post_ggv_billing(
    _claims: Claims,
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

    // ── §42b Model A: Nutzungsplan-based PV allocation ─────────────────────────
    // Build a MaloId → allocated_pv_kwh map when pv_generation_kwh is supplied.
    let pv_allocations: std::collections::HashMap<String, rust_decimal::Decimal> =
        if let (Some(pv_gen_kwh), Some(np)) = (req.pv_generation_kwh, req.nutzungsplan.as_ref()) {
            use energy_billing::{GgvNutzungsplan, GgvNutzungsplanEntry};
            let plan = GgvNutzungsplan(
                np.iter()
                    .map(|e| GgvNutzungsplanEntry {
                        malo_id: e.malo_id.clone(),
                        fraction: e.fraction,
                    })
                    .collect(),
            );
            if let Err(e) = plan.validate() {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("nutzungsplan: {e}"),
                )
                    .into_response();
            }
            plan.allocate(pv_gen_kwh).into_iter().collect()
        } else {
            std::collections::HashMap::new()
        };

    let rates = cfg.regulatory_rates_for_period("SOLAR", period_from, period_to);
    let mut tenant_results: Vec<serde_json::Value> = Vec::with_capacity(req.tenants.len());
    let mut parts: Vec<(String, Invoice)> = Vec::with_capacity(req.tenants.len());

    for tenant in &req.tenants {
        // Build Product — prefer request overrides, fall back to tarifbd lookup.
        let tariff = match tarifbd
            .get_customer_product(&tenant.malo_id, &req.lf_mp_id)
            .await
        {
            Ok(Some(t)) => t,
            Ok(None) => {
                // No product in tarifbd — build minimal Product from request overrides.
                let map = serde_json::json!({
                    "category": "SOLAR",
                    "product_code": tenant.product_code,
                    "solar_arbeitspreis_ct_per_kwh": tenant.arbeitspreis_ct_per_kwh,
                    "gemeinschaft_rabatt_ct_per_kwh": tenant.gemeinschaft_rabatt_ct_per_kwh,
                    "arbeitspreis_ct_per_kwh": tenant.grid_arbeitspreis_ct_per_kwh,
                });
                match serde_json::from_value::<Product>(map) {
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
        // Product is an enum — apply overrides by rebuilding the Solar/Sharing variant.
        let tariff = match tariff {
            Product::Solar(mut p) => {
                if let Some(ap) = tenant.arbeitspreis_ct_per_kwh {
                    p.solar_arbeitspreis_ct_per_kwh = Some(ap);
                }
                // solar_arbeitspreis is also used for grid remainder in SolarProvider
                if let Some(rabatt) = tenant.gemeinschaft_rabatt_ct_per_kwh {
                    if let Some(ap) = p.solar_arbeitspreis_ct_per_kwh {
                        let cap = ap * rust_decimal::dec!(0.10);
                        if rabatt > cap {
                            tracing::warn!(
                                malo_id = %tenant.malo_id,
                                ggv_id = %ggv_id,
                                rabatt_ct = %rabatt,
                                cap_diagnostic = %cap,
                                "billingd GGV: gemeinschaft_rabatt > 10% of Arbeitspreis — \
                                 verify §42b Abs. 3 EEG 2023 compliance against local Grundversorgungstarif"
                            );
                        }
                    }
                    p.gemeinschaft_rabatt_ct_per_kwh = Some(rabatt);
                }
                Product::Solar(p)
            }
            Product::Sharing(mut p) => {
                if let Some(ap) = tenant.arbeitspreis_ct_per_kwh {
                    p.electricity.solar_include_stromsteuer = false; // GGV shares are Stromsteuer-free
                    p.electricity.arbeitspreis_ct_per_kwh = Some(ap);
                }
                if let Some(rabatt) = tenant.gemeinschaft_rabatt_ct_per_kwh {
                    p.sharing_credit_ct_per_kwh = Some(rabatt);
                }
                Product::Sharing(p)
            }
            other => other,
        };

        // ── Build Quantities: Model A (GgvSolarInput) or Model B (SolarMeterInput) ──
        let quantities = if let Some(&pv_allocated) = pv_allocations.get(&tenant.malo_id) {
            // Model A: proportional allocation — hybrid PV + grid billing
            Quantities {
                ggv_solar: Some(energy_billing::GgvSolarInput {
                    pv_allocated_kwh: pv_allocated,
                    actual_consumption_kwh: tenant.consumption_kwh,
                }),
                ..Default::default()
            }
        } else {
            // Model B: direct consumption as solar eigenverbrauch
            Quantities {
                solar: Some(SolarMeterInput {
                    eigenverbrauch_kwh: tenant.consumption_kwh,
                }),
                ..Default::default()
            }
        };

        let rechnungsnummer = tenant
            .product_code
            .as_deref()
            .map(|p| format!("GGV-{ggv_id}-{p}-{period_from}"))
            .unwrap_or_else(|| format!("GGV-{ggv_id}-{}-{period_from}", tenant.malo_id));

        let ctx = BillingContext {
            malo_id: tenant.malo_id.clone(),
            lf_mp_id: req.lf_mp_id.clone(),
            rechnungsnummer: rechnungsnummer.clone(),
            period: BillingPeriod::new(period_from, period_to)
                .expect("parse_period guarantees from < to"),
            invoice_type: InvoiceType::Initial,
            regulatory_rates: rates.clone(),
            contract_id: None,
            ..Default::default()
        };
        let engine = tariff.build_engine(&GridInput::default(), &rates);

        let result = match engine.bill(ctx, &quantities) {
            Ok(r) => r,
            Err(e) => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    engine_error_body(&format!("GGV tenant {}", tenant.malo_id), &e),
                )
                    .into_response();
            }
        };

        let record_id = match insert_billing_record(
            &pool,
            &cfg.tenant,
            &tenant.malo_id,
            &req.lf_mp_id,
            tariff.product_code().unwrap_or("SOLAR_GGV"),
            "SOLAR",
            period_from,
            period_to,
            &result.to_rechnung_json(),
            result.netto_eur,
            result.brutto_eur,
        )
        .await
        {
            Ok(id) => id,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };

        tenant_results.push(serde_json::json!({
            "record_id": record_id,
            "malo_id": tenant.malo_id,
            "consumption_kwh": tenant.consumption_kwh,
            "netto_eur": result.netto_eur,
            "brutto_eur": result.brutto_eur,
        }));
        parts.push((tenant.malo_id.clone(), result));
    }

    // Consolidated SAMMEL document for the GGV installation — through the
    // engine, like every other invoice: derived totals, per-rate VAT over the
    // combined base, deterministic rechnungsdatum.
    let sammel_nr = format!("GGV-SAMMEL-{ggv_id}-{period_from}");
    let (sammel_invoice, sammel_rechnung) = match build_aggregate_invoice(
        &ggv_id,
        &req.lf_mp_id,
        sammel_nr,
        period_from,
        period_to,
        rates,
        parts,
        vec![
            serde_json::json!({
                "_typ": "ZUSATZ_ATTRIBUT",
                "name": "ggv_id",
                "wert": ggv_id
            }),
            serde_json::json!({
                "_typ": "ZUSATZ_ATTRIBUT",
                "name": "tenant_count",
                "wert": tenant_results.len().to_string()
            }),
            serde_json::json!({
                "_typ": "ZUSATZ_ATTRIBUT",
                "name": "total_kwh",
                "wert": total_kwh.to_string()
            }),
        ],
    ) {
        Ok(x) => x,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let (sammel_netto, sammel_brutto) = (sammel_invoice.netto_eur, sammel_invoice.brutto_eur);

    let sammel_id = match insert_sammelrechnung_record(
        &pool,
        &cfg.tenant,
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
            cfg.erp_hmac_secret.as_deref(),
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

// ── VPP Aggregation Billing (B12 — RED III Article 17) ───────────────────────

/// One confirmed dispatch event for VPP settlement billing.
///
/// Source: WiM Steuerungsauftrag IFTSTA confirmation (PID 21039) or equivalent
/// VPP aggregator dispatch confirmation.
#[derive(Debug, serde::Deserialize)]
pub struct VppDispatchEvent {
    /// UTC dispatch start — ISO-8601 e.g. `"2026-01-15T10:00:00Z"`.
    pub start_utc: String,
    /// UTC dispatch end — ISO-8601 e.g. `"2026-01-15T10:15:00Z"`.
    pub end_utc: String,
    /// Actual flexibility delivered in kWh (positive = load reduction; negative = load increase).
    pub flexibility_kwh: rust_decimal::Decimal,
    /// IFTSTA process UUID from makod (for §20 audit trail).
    pub process_id: Option<String>,
}

/// Request body for `POST /api/v1/billing/vpp/{vpp_id}`.
///
/// `vpp_id` is the operator-assigned virtual power plant identifier
/// (typically the SR-ID of the `SteuerbareRessource` portfolio in `marktd`).
#[derive(Debug, serde::Deserialize)]
pub struct VppBillingRequest {
    /// LF/Aggregator MP-ID (invoice issuer).
    pub lf_mp_id: String,
    /// MaLo-ID of the VPP aggregation point (or primary resource).
    pub malo_id: String,
    /// Billing period start (`YYYY-MM-DD`).
    pub period_from: String,
    /// Billing period end (`YYYY-MM-DD`).
    pub period_to: String,
    /// Capacity price EUR/kWh (agreed in VPP contract or dynamic market price).
    pub capacity_price_eur_per_kwh: rust_decimal::Decimal,
    /// All confirmed dispatch events in the billing period.
    pub dispatch_events: Vec<VppDispatchEvent>,
    /// Optional invoice number prefix.
    pub rechnungsnummer_prefix: Option<String>,
    /// MwSt rate override (default from billingd config, typically 0.19).
    pub mwst_rate_override: Option<rust_decimal::Decimal>,
}

/// `POST /api/v1/billing/vpp/{vpp_id}`
///
/// **B12 — VPP Aggregation Settlement (RED III Article 17).**
///
/// Generates a settlement `Rechnung` for a Virtual Power Plant aggregator.
/// Each dispatch event becomes one `Rechnungsposition`.
///
/// ## Calculation
///
/// ```text
/// DispatchPosition_eur = flexibility_kwh * capacity_price_eur_per_kwh
/// Total_netto          = sum(DispatchPosition_eur)
/// MwSt                 = Total_netto * mwst_rate
/// Total_brutto         = Total_netto + MwSt
/// ```
///
/// ## CloudEvent emitted
///
/// `de.vpp.settlement.berechnet` (type) — consumed by ERP/DSO settlement systems.
///
/// ## Regulatory basis
///
/// RED III Article 17 (§ 41b EnWG transposition, expected 2026):
/// Aggregators must provide transparent settlement invoices per dispatch event.
pub async fn post_vpp_billing(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Path(vpp_id): Path<String>,
    Json(req): Json<VppBillingRequest>,
) -> impl IntoResponse {
    use rust_decimal::Decimal;

    if req.dispatch_events.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "VPP billing requires at least one dispatch event",
        )
            .into_response();
    }

    let (period_from, period_to) = match parse_period(&req.period_from, &req.period_to) {
        Ok(pd) => pd,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };

    let mwst_rate = req
        .mwst_rate_override
        .unwrap_or_else(|| cfg.regulatory_rates().mwst_rate);

    // ── Positions from dispatch events, through the engine ────────────────────
    // Every event becomes a BillingPosition; VAT, steuerbetraege and traces come
    // from the same machinery as every other invoice instead of an inline block
    // whose Steuerkennzeichen said UST_19 whatever the override rate was.
    let mut positions: Vec<BillingPosition> = Vec::with_capacity(req.dispatch_events.len());
    let mut total_flex_kwh = Decimal::ZERO;
    for ev in &req.dispatch_events {
        if ev.flexibility_kwh <= Decimal::ZERO {
            continue;
        }
        total_flex_kwh += ev.flexibility_kwh;
        let mut pos = BillingPosition::debit(
            format!("VPP Dispatch {} bis {}", ev.start_utc, ev.end_utc),
            ev.flexibility_kwh,
            "kWh",
            req.capacity_price_eur_per_kwh,
            PositionCategory::Fee,
        )
        .with_legal_basis("RED III Art. 17, VPP-Vertrag")
        .with_tag("vpp_dispatch");
        pos.trace = energy_billing::PositionTrace::commodity(
            ev.flexibility_kwh,
            "kWh",
            req.capacity_price_eur_per_kwh,
            "RED III Art. 17, VPP-Vertrag",
        );
        positions.push(pos);
    }

    if positions.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "all dispatch events have zero or negative flexibility — no billing generated",
        )
            .into_response();
    }

    let rechnungsnummer = req
        .rechnungsnummer_prefix
        .as_deref()
        .map(|p| format!("{p}-{period_from}"))
        .unwrap_or_else(|| format!("VPP-{vpp_id}-{period_from}"));

    let attrs = vec![
        serde_json::json!({ "_typ": "ZUSATZ_ATTRIBUT", "name": "vpp_id", "wert": vpp_id }),
        serde_json::json!({ "_typ": "ZUSATZ_ATTRIBUT", "name": "total_flexibility_kwh", "wert": total_flex_kwh.to_string() }),
        serde_json::json!({ "_typ": "ZUSATZ_ATTRIBUT", "name": "dispatch_event_count", "wert": req.dispatch_events.len().to_string() }),
        serde_json::json!({
            "_typ": "ZUSATZ_ATTRIBUT",
            "name": "dispatch_process_ids",
            "wert": req
                .dispatch_events
                .iter()
                .filter_map(|ev| ev.process_id.as_deref())
                .collect::<Vec<_>>(),
        }),
    ];
    let (invoice, rechnung_json) = match build_vpp_invoice(
        &req.malo_id,
        &req.lf_mp_id,
        rechnungsnummer,
        period_from,
        period_to,
        mwst_rate,
        positions,
        attrs,
    ) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };
    let total_netto = invoice.netto_eur;
    let total_brutto = invoice.brutto_eur;

    // Persist billing record.
    let record_id = match insert_billing_record(
        &pool,
        &cfg.tenant,
        &req.malo_id,
        &req.lf_mp_id,
        &format!("VPP_{vpp_id}"),
        "VPP",
        period_from,
        period_to,
        &rechnung_json,
        total_netto,
        total_brutto,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Emit de.vpp.settlement.berechnet CloudEvent.
    if let Some(ref webhook_url) = cfg.erp_webhook_url {
        let ce_id = uuid::Uuid::new_v4();
        let ce = serde_json::json!({
            "specversion": "1.0",
            "type": "de.vpp.settlement.berechnet",
            "source": format!("urn:billingd:vpp:{vpp_id}"),
            "id": ce_id.to_string(),
            "time": time::OffsetDateTime::now_utc().to_string(),
            "subject": vpp_id,
            "datacontenttype": "application/json",
            "data": {
                "record_id": record_id.to_string(),
                "vpp_id": vpp_id,
                "malo_id": req.malo_id,
                "lf_mp_id": req.lf_mp_id,
                "total_flexibility_kwh": total_flex_kwh.to_string(),
                "total_netto_eur": total_netto.to_string(),
                "total_brutto_eur": total_brutto.to_string(),
                "dispatch_count": req.dispatch_events.len(),
                "rechnung": rechnung_json,
            }
        });
        let client = reqwest::Client::new();
        if let Ok(resp) = client
            .post(webhook_url)
            .header("Content-Type", "application/cloudevents+json")
            .json(&ce)
            .send()
            .await
            && resp.status().is_success()
        {
            let _ = mark_dispatched(&pool, record_id, ce_id).await;
        }
    }

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "record_id": record_id,
            "vpp_id": vpp_id,
            "malo_id": req.malo_id,
            "period_from": period_from.to_string(),
            "period_to": period_to.to_string(),
            "dispatch_count": req.dispatch_events.len(),
            "total_flexibility_kwh": total_flex_kwh.to_string(),
            "total_netto_eur": total_netto.to_string(),
            "total_brutto_eur": total_brutto.to_string(),
            "mwst_eur": invoice.mwst_eur.to_string(),
            "rechnung": rechnung_json,
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
    _claims: Claims,
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

    let rates = cfg.regulatory_rates_for_period("STROM", period_from, period_to);
    let sammel_nr = req
        .rechnungsnummer
        .clone()
        .unwrap_or_else(|| format!("SAMMEL-{rahmenvertrag_id}-{period_from}"));

    // Calculate each MaLo independently.
    let mut parts: Vec<(String, Invoice)> = Vec::with_capacity(malos.len());
    let mut per_malo_ids: Vec<Uuid> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for entry in &malos {
        let dummy_req = CalculateRequest {
            schlussrechnung: false,
            abschlaege: Vec::new(),
            lf_mp_id: req.lf_mp_id.clone(),
            nb_mp_id: None,
            period_from: req.period_from.clone(),
            period_to: req.period_to.clone(),
            tariff: None,
            meter: None,
            grid: None,
            eeg_gutschrift_eur: None,
            rechnungsnummer: Some(format!("{sammel_nr}-{}", entry.malo_id)),
            gas_meter: None,
            waerme_meter: None,
            wasser_meter: None,
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
            &cfg,
            &tariff,
            &dummy_req,
            &entry.malo_id,
            &format!("{sammel_nr}-{}", entry.malo_id),
            period_from,
            period_to,
            &rates,
            &edmd,
            &marktd,
            &tarifbd,
            &vertragd,
        )
        .await
        {
            Ok(r) => r,
            Err((_, msg)) => {
                errors.push(format!("{}: {msg}", entry.malo_id));
                continue;
            }
        };

        // Persist per-MaLo record.
        if let Ok(record_id) = insert_billing_record(
            &pool,
            &cfg.tenant,
            &entry.malo_id,
            &req.lf_mp_id,
            tariff.product_code().unwrap_or(tariff.category_str()),
            tariff.category_str(),
            period_from,
            period_to,
            &result.to_rechnung_json(),
            result.netto_eur,
            result.brutto_eur,
        )
        .await
        {
            per_malo_ids.push(record_id);
        }
        parts.push((entry.malo_id.clone(), result));
    }

    if parts.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(
                serde_json::json!({ "errors": errors, "message": "all MaLo calculations failed" }),
            ),
        )
            .into_response();
    }

    // Consolidated Sammelrechnung — through the engine: per-rate VAT over the
    // combined base, derived totals, deterministic rechnungsdatum. The per-MaLo
    // runs stay stored as calculation records linked below.
    let malos_count = parts.len();
    let (sammel_invoice, sammel_json) = match build_aggregate_invoice(
        &rahmenvertrag_id,
        &req.lf_mp_id,
        sammel_nr.clone(),
        period_from,
        period_to,
        rates,
        parts,
        vec![
            serde_json::json!({
                "_typ": "ZUSATZ_ATTRIBUT",
                "name": "rahmenvertragId",
                "wert": rahmenvertrag_id
            }),
            serde_json::json!({
                "_typ": "ZUSATZ_ATTRIBUT",
                "name": "malosCount",
                "wert": malos_count.to_string()
            }),
        ],
    ) {
        Ok(x) => x,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let (total_netto, total_brutto) = (sammel_invoice.netto_eur, sammel_invoice.brutto_eur);

    let sammel_id = match insert_sammelrechnung_record(
        &pool,
        &cfg.tenant,
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
            cfg.erp_hmac_secret.as_deref(),
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
    _claims: Claims,
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

    let rates = cfg.regulatory_rates_for_period(&row.category, row.period_from, row.period_to);
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
        rates.mwst_rate * rust_decimal::dec!(100),
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
    _claims: Claims,
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
    // §40c EnWG: payment due at the earliest two weeks after receipt of the
    // payment request — use the engine-stamped zahlungsziel (issue + 14 d).
    let due_date = row
        .rechnung_json
        .get("zahlungsziel")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| (row.period_to + time::Duration::days(14)).to_string());
    let netto = row.total_netto_eur.unwrap_or_default();
    let brutto = row.total_brutto_eur.unwrap_or_default();
    let tax_amount = brutto - netto;
    let tax_pct = if netto > rust_decimal::Decimal::ZERO {
        (tax_amount / netto * rust_decimal::dec!(100)).round_kfm(2)
    } else {
        rust_decimal::dec!(19)
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

// ── VPP Contract Registry (B12) ──────────────────────────────────────────────

/// `PUT /api/v1/billing/vpp-contracts/{sr_id}`
///
/// Upsert a VPP contract for a `SteuerbareRessource`.
///
/// Idempotent on `(sr_id, tenant, valid_from)`.
/// Used to configure the capacity price and billing identifiers that `billingd`
/// needs when auto-settling a `de.vpp.dispatch.confirmed` dispatch event.
pub async fn put_vpp_contract(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    Path(sr_id): Path<String>,
    Json(mut row): Json<crate::pg::VppContractRow>,
) -> impl IntoResponse {
    row.sr_id = sr_id;
    row.tenant = cfg.tenant.clone();
    row.updated_at = time::OffsetDateTime::now_utc();
    if row.id.is_nil() {
        row.id = Uuid::new_v4();
    }
    match crate::pg::upsert_vpp_contract(&pool, &row).await {
        Ok(id) => (StatusCode::OK, Json(serde_json::json!({ "id": id }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/billing/vpp-contracts`
///
/// List all VPP contracts for this tenant.
pub async fn list_vpp_contracts(
    _claims: Claims,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
) -> impl IntoResponse {
    match crate::pg::list_vpp_contracts(&pool, &cfg.tenant).await {
        Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── VPP Auto-Billing Webhook (B12 — RED III Article 17) ──────────────────────

/// `POST /api/v1/webhooks/vpp-dispatch`
///
/// **VPP Dispatch Confirmed auto-billing trigger.**
///
/// Receives `de.vpp.dispatch.confirmed` CloudEvents emitted by `makod` when
/// the MSB sends a positive `EndantwortPositiv` for a WiM Steuerungsauftrag
/// (PID 55168).  Auto-generates a VPP settlement `Rechnung` using the
/// pre-configured `VppContractRow` for the dispatched SR-ID.
///
/// ## Idempotency
///
/// Each `tx_id` is recorded in `vpp_dispatch_ledger`.  Repeated delivery
/// (outbox retry) returns `202 Accepted` without re-billing.
///
/// ## HMAC verification
///
/// When `[inbound_webhook_secret]` is configured in `billingd.toml`, the
/// `X-Mako-Signature: sha256=<hex>` header is verified.  Requests with
/// invalid or missing signatures are rejected with `401 Unauthorized`.
///
/// ## Auto-billing disabled
///
/// When `vpp_auto_billing = false` in config (the default), the webhook accepts
/// events and records them in `vpp_dispatch_ledger` but does **not** generate a
/// `Rechnung`.  The manual `POST /api/v1/billing/vpp/{vpp_id}` endpoint remains
/// available.
///
/// ## CloudEvent data schema
///
/// ```json
/// {
///   "tx_id":               "abc123",
///   "location_id":         "C0001234567890",
///   "location_type":       "sr",
///   "execution_time_from": "2026-01-15T10:00:00Z",
///   "execution_time_until": "2026-01-15T10:15:00Z",
///   "max_power_kw":        "11.0",
///   "command_type":        "Konfiguration",
///   "sender_mp_id":        "9900123456789",
///   "produkt_code":        "TX-MODUL2-HT"
/// }
/// ```
pub async fn post_vpp_webhook(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<BillingdConfig>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    // ── 1. HMAC signature verification ────────────────────────────────────────
    if let Some(ref secret) = cfg.inbound_webhook_secret {
        let sig = headers
            .get("x-mako-signature")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.strip_prefix("sha256=").unwrap_or(v));
        match sig {
            Some(hex)
                if mako_markt::cloudevents::verify_signature(secret.as_bytes(), &body, hex) => {}
            Some(_) => {
                tracing::warn!("billingd: vpp-dispatch webhook — invalid HMAC signature");
                return StatusCode::UNAUTHORIZED.into_response();
            }
            None => {
                tracing::warn!("billingd: vpp-dispatch webhook — missing X-Mako-Signature");
                return StatusCode::UNAUTHORIZED.into_response();
            }
        }
    }

    // ── 2. Parse CloudEvent ───────────────────────────────────────────────────
    let event: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("invalid JSON: {e}")).into_response();
        }
    };
    let data = event
        .get("data")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let tx_id = data
        .get("tx_id")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| {
            event
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
        })
        .to_owned();

    // ── 3. Idempotency check ───────────────────────────────────────────────────
    match crate::pg::is_vpp_dispatch_processed(&pool, &tx_id, &cfg.tenant).await {
        Ok(true) => {
            tracing::debug!(tx_id, "billingd: vpp-dispatch already processed — skipping");
            return StatusCode::ACCEPTED.into_response();
        }
        Ok(false) => {}
        Err(e) => {
            // Fail closed: proceeding without the ledger answer risks billing
            // the same dispatch twice. The sender retries on 5xx.
            tracing::error!(tx_id, error = %e, "billingd: vpp_dispatch_ledger check failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "idempotency ledger unavailable — retry later",
            )
                .into_response();
        }
    }

    // ── 4. Extract dispatch metadata ──────────────────────────────────────────
    let location_id = data
        .get("location_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let location_type = data
        .get("location_type")
        .and_then(|v| v.as_str())
        .unwrap_or("sr");
    let execution_time_from = data
        .get("execution_time_from")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let execution_time_until = data
        .get("execution_time_until")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let max_power_kw: rust_decimal::Decimal = data
        .get("max_power_kw")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .unwrap_or(rust_decimal::Decimal::ZERO);

    // Only SR-IDs are currently supported for VPP contract lookup.
    // NeLo-IDs (grid constraint redispatch) use a different billing flow.
    if location_type != "sr" {
        tracing::debug!(
            tx_id,
            location_type,
            "billingd: vpp-dispatch webhook — skipping non-SR location"
        );
        let _ = crate::pg::record_vpp_dispatch(&pool, &tx_id, &cfg.tenant, None).await;
        return StatusCode::ACCEPTED.into_response();
    }

    // ── 5. Look up active VPP contract ────────────────────────────────────────
    // The contract is selected by the day the dispatch was *executed*, not the
    // day this webhook happens to be processed. Selecting by "today" meant a
    // replayed or delayed event could bill under a different contract version
    // than the one in force when the flexibility was actually delivered.
    let dispatch_date = parse_dispatch_date(&execution_time_from)
        .unwrap_or_else(|| time::OffsetDateTime::now_utc().date());
    let contract =
        match crate::pg::find_active_vpp_contract(&pool, &location_id, &cfg.tenant, dispatch_date)
            .await
        {
            Ok(Some(c)) => c,
            Ok(None) => {
                tracing::warn!(
                    tx_id,
                    sr_id = %location_id,
                    "billingd: vpp-dispatch — no active VPP contract found; cannot auto-bill"
                );
                let _ = crate::pg::record_vpp_dispatch(&pool, &tx_id, &cfg.tenant, None).await;
                return StatusCode::ACCEPTED.into_response();
            }
            Err(e) => {
                tracing::error!(tx_id, error = %e, "billingd: vpp_contract lookup failed");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };

    // ── 6. Check vpp_auto_billing flag ────────────────────────────────────────
    if !cfg.vpp_auto_billing {
        tracing::info!(
            tx_id,
            vpp_id = %contract.vpp_id,
            "billingd: vpp-dispatch — auto-billing disabled; recording dispatch only"
        );
        let _ = crate::pg::record_vpp_dispatch(&pool, &tx_id, &cfg.tenant, None).await;
        return StatusCode::ACCEPTED.into_response();
    }

    // ── 7. Compute flexibility_kwh from dispatch window ────────────────────────
    // flexibility_kwh = max_power_kw × duration_hours
    // Duration is derived from execution_time_until − execution_time_from.
    // Falls back to 15 minutes (standard §14a dispatch window) if no end time.
    let flexibility_kwh = compute_dispatch_flexibility_kwh(
        max_power_kw,
        &execution_time_from,
        execution_time_until.as_deref(),
    );

    if flexibility_kwh <= rust_decimal::Decimal::ZERO {
        tracing::warn!(
            tx_id,
            "billingd: vpp-dispatch — zero flexibility; no billing"
        );
        let _ = crate::pg::record_vpp_dispatch(&pool, &tx_id, &cfg.tenant, None).await;
        return StatusCode::ACCEPTED.into_response();
    }

    // ── 8. Build and run VPP billing ──────────────────────────────────────────
    // Billing period = calendar day of dispatch_from — the same day the
    // contract above was selected by.
    let period_from = dispatch_date;
    let period_to = period_from; // single-day billing record per dispatch

    let mwst_rate = contract
        .mwst_rate_override
        .unwrap_or_else(|| cfg.regulatory_rates().mwst_rate);

    let rechnungsnummer = format!(
        "VPP-{}-{}-{}",
        contract.vpp_id,
        period_from,
        tx_id.get(..8).unwrap_or(&tx_id)
    );

    // One position through the engine's canonical path — VAT, steuerbetraege
    // and the trace come from the same machinery as every other invoice.
    let mut pos = BillingPosition::debit(
        format!(
            "VPP Dispatch {} bis {} (SR: {})",
            execution_time_from,
            execution_time_until.as_deref().unwrap_or("open"),
            location_id
        ),
        flexibility_kwh,
        "kWh",
        contract.capacity_price_eur_per_kwh,
        PositionCategory::Fee,
    )
    .with_legal_basis("RED III Art. 17, VPP-Vertrag")
    .with_tag("vpp_dispatch");
    pos.trace = energy_billing::PositionTrace::commodity(
        flexibility_kwh,
        "kWh",
        contract.capacity_price_eur_per_kwh,
        "RED III Art. 17, VPP-Vertrag",
    );

    let attrs = vec![
        serde_json::json!({ "_typ": "ZUSATZ_ATTRIBUT", "name": "vpp_id", "wert": contract.vpp_id.clone() }),
        serde_json::json!({ "_typ": "ZUSATZ_ATTRIBUT", "name": "tx_id", "wert": tx_id.clone() }),
        serde_json::json!({ "_typ": "ZUSATZ_ATTRIBUT", "name": "sr_id", "wert": location_id.clone() }),
        serde_json::json!({ "_typ": "ZUSATZ_ATTRIBUT", "name": "flexibility_kwh", "wert": flexibility_kwh.to_string() }),
    ];
    let (invoice, rechnung_json) = match build_vpp_invoice(
        &contract.malo_id,
        &contract.lf_mp_id,
        rechnungsnummer,
        period_from,
        period_to,
        mwst_rate,
        vec![pos],
        attrs,
    ) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(tx_id, error = %e, "billingd: vpp invoice build failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let position_netto = invoice.netto_eur;
    let total_brutto = invoice.brutto_eur;

    let record_id = match insert_billing_record(
        &pool,
        &cfg.tenant,
        &contract.malo_id,
        &contract.lf_mp_id,
        &format!("VPP_{}", contract.vpp_id),
        "VPP",
        period_from,
        period_to,
        &rechnung_json,
        position_netto,
        total_brutto,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            tracing::error!(tx_id, error = %e, "billingd: vpp auto-billing insert failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // Record for idempotency.
    let _ = crate::pg::record_vpp_dispatch(&pool, &tx_id, &cfg.tenant, Some(record_id)).await;

    tracing::info!(
        tx_id,
        %record_id,
        vpp_id = %contract.vpp_id,
        malo_id = %contract.malo_id,
        flexibility_kwh = %flexibility_kwh,
        total_brutto = %total_brutto,
        "billingd: VPP dispatch auto-billed"
    );

    // ── 9. Emit de.vpp.settlement.berechnet ───────────────────────────────────
    if let Some(ref webhook_url) = cfg.erp_webhook_url {
        let ce_id = Uuid::new_v4();
        let ce = serde_json::json!({
            "specversion": "1.0",
            "type": "de.vpp.settlement.berechnet",
            "source": format!("urn:billingd:vpp:{}", contract.vpp_id),
            "id": ce_id.to_string(),
            "time": time::OffsetDateTime::now_utc().to_string(),
            "subject": contract.vpp_id,
            "datacontenttype": "application/json",
            "data": {
                "record_id":          record_id.to_string(),
                "vpp_id":             contract.vpp_id,
                "malo_id":            contract.malo_id,
                "lf_mp_id":           contract.lf_mp_id,
                "tx_id":              tx_id,
                "sr_id":              location_id,
                "flexibility_kwh":    flexibility_kwh.to_string(),
                "total_netto_eur":    position_netto.to_string(),
                "total_brutto_eur":   total_brutto.to_string(),
                "trigger":            "auto",
                "rechnung":           rechnung_json,
            }
        });
        emit_cloud_event(
            webhook_url,
            cfg.erp_hmac_secret.as_deref(),
            &pool,
            record_id,
            &contract.malo_id,
            &contract.lf_mp_id,
            &ce["data"],
        )
        .await;
        let _ = mark_dispatched(&pool, record_id, ce_id).await;
    }

    StatusCode::ACCEPTED.into_response()
}

/// Compute delivered flexibility in kWh from dispatch parameters.
///
/// `flexibility_kwh = max_power_kw × duration_hours`
///
/// Duration is parsed from ISO-8601 UTC timestamps.  Falls back to 15 minutes
/// (the standard BNetzA §14a dispatch window minimum) when `time_until` is
/// absent or parsing fails.
fn compute_dispatch_flexibility_kwh(
    max_power_kw: rust_decimal::Decimal,
    time_from: &str,
    time_until: Option<&str>,
) -> rust_decimal::Decimal {
    use rust_decimal::dec;

    let duration_hours = time_until
        .and_then(|tu| {
            let f = time::OffsetDateTime::parse(
                time_from,
                &time::format_description::well_known::Rfc3339,
            )
            .ok()?;
            let u = time::OffsetDateTime::parse(tu, &time::format_description::well_known::Rfc3339)
                .ok()?;
            let secs = (u - f).whole_seconds();
            if secs > 0 {
                Some(rust_decimal::Decimal::from(secs) / dec!(3600))
            } else {
                None
            }
        })
        .unwrap_or(dec!(0.25)); // 15-minute default

    (max_power_kw * duration_hours).round_kfm(6)
}

/// Extract the calendar date (UTC) from an ISO-8601 timestamp string.
fn parse_dispatch_date(ts: &str) -> Option<time::Date> {
    time::OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|dt| dt.date())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod gas_enrichment_tests {
    use super::{build_aggregate_invoice, engine_error_body, normalize_gasqualitaet};
    use energy_billing::{
        BillingContext, BillingPeriod, BillingPosition, BillingProvider as _, Invoice, InvoiceType,
        MwStProvider, Quantities, RegulatoryRates,
    };

    // ── normalize_gasqualitaet ────────────────────────────────────────────────

    #[test]
    fn normalize_hgas_variants() {
        // All aliases for H-Gas must map to "H_GAS"
        for raw in &[
            "HGas",
            "H-Gas",
            "H-gas",
            "HGAS",
            "H_GAS",
            "HIGH_CALORIFIC",
            "ERDGAS_H",
        ] {
            assert_eq!(
                normalize_gasqualitaet(raw),
                "H_GAS",
                "expected H_GAS for input {raw:?}"
            );
        }
    }

    #[test]
    fn normalize_lgas_variants() {
        for raw in &[
            "LGas",
            "L-Gas",
            "L-gas",
            "LGAS",
            "L_GAS",
            "LOW_CALORIFIC",
            "ERDGAS_L",
        ] {
            assert_eq!(
                normalize_gasqualitaet(raw),
                "L_GAS",
                "expected L_GAS for input {raw:?}"
            );
        }
    }

    #[test]
    fn normalize_h2_blend_variants() {
        for raw in &[
            "H2_BLEND",
            "H2Blend",
            "H2-Blend",
            "HYDROGEN_BLEND",
            "H2BLEND",
        ] {
            assert_eq!(
                normalize_gasqualitaet(raw),
                "H2_BLEND",
                "expected H2_BLEND for input {raw:?}"
            );
        }
    }

    #[test]
    fn normalize_biogas_variants() {
        for raw in &["BIOGAS", "BioGas", "Bio-Gas", "BIOMETHANE", "BIOMETHAN"] {
            assert_eq!(
                normalize_gasqualitaet(raw),
                "BIOGAS",
                "expected BIOGAS for input {raw:?}"
            );
        }
    }

    #[test]
    fn normalize_fluessiggas_variants() {
        for raw in &["FLUESSIGGAS", "LPG", "LIQUID_GAS"] {
            assert_eq!(
                normalize_gasqualitaet(raw),
                "FLUESSIGGAS",
                "expected FLUESSIGGAS for input {raw:?}"
            );
        }
    }

    #[test]
    fn normalize_unknown_returns_uppercase_underscored() {
        // Unknown values are normalized to UPPER_SNAKE_CASE but preserved.
        assert_eq!(normalize_gasqualitaet("syngas"), "SYNGAS");
        assert_eq!(
            normalize_gasqualitaet("Compressed Natural Gas"),
            "COMPRESSED_NATURAL_GAS"
        );
    }

    #[test]
    fn normalize_already_canonical_is_idempotent() {
        for canonical in &["H_GAS", "L_GAS", "H2_BLEND", "BIOGAS", "FLUESSIGGAS"] {
            let result = normalize_gasqualitaet(canonical);
            assert_eq!(
                &result, canonical,
                "normalize_gasqualitaet should be idempotent on canonical value {canonical}"
            );
        }
    }

    #[test]
    fn normalize_trims_whitespace() {
        assert_eq!(normalize_gasqualitaet("  HGas  "), "H_GAS");
        assert_eq!(normalize_gasqualitaet("\tLGas\n"), "L_GAS");
    }

    // ── build_aggregate_invoice ───────────────────────────────────────────────

    fn sub_invoice(malo: &str, netto_ct: rust_decimal::Decimal) -> (String, Invoice) {
        use energy_billing::PositionCategory;
        let ctx = BillingContext {
            malo_id: malo.to_owned(),
            lf_mp_id: "9900000000001".to_owned(),
            rechnungsnummer: format!("SUB-{malo}"),
            period: BillingPeriod::new(
                time::macros::date!(2026 - 01 - 01),
                time::macros::date!(2026 - 01 - 31),
            )
            .unwrap(),
            invoice_type: InvoiceType::Initial,
            regulatory_rates: RegulatoryRates::default(),
            ..Default::default()
        };
        let base = vec![BillingPosition::debit(
            "Arbeitspreis".to_owned(),
            rust_decimal::Decimal::ONE,
            "kWh",
            netto_ct,
            PositionCategory::Commodity,
        )];
        let mut all = base.clone();
        all.extend(
            MwStProvider::new(rust_decimal::dec!(0.19))
                .bill(&ctx, &Quantities::default(), &base)
                .unwrap(),
        );
        (malo.to_owned(), Invoice::from_positions(ctx, all, vec![]))
    }

    /// The consolidated document strips the sub-invoices' tax positions and
    /// recomputes VAT once over the combined base per rate — steuerbetraege
    /// and totals agree by construction, not by hoping the parts add up.
    #[test]
    fn aggregate_recomputes_vat_over_the_combined_base() {
        use rust_decimal::dec;
        let parts: Vec<(String, Invoice)> = ["11111111111", "22222222222", "33333333333"]
            .iter()
            .map(|m| sub_invoice(m, dec!(10.01)))
            .collect();
        let (agg, json) = build_aggregate_invoice(
            "RV-1",
            "9900000000001",
            "SAMMEL-RV-1".to_owned(),
            time::macros::date!(2026 - 01 - 01),
            time::macros::date!(2026 - 01 - 31),
            RegulatoryRates::default(),
            parts,
            vec![],
        )
        .unwrap();
        assert_eq!(agg.netto_eur, dec!(30.03));
        // 30.03 × 0.19, at the engine's 5-dp precision — cent rounding is a
        // display concern, not a calculation one.
        assert_eq!(agg.mwst_eur, dec!(5.7057), "VAT over the combined base");
        assert_eq!(agg.brutto_eur, dec!(35.7357));
        // The BG-23 breakdown rounds to cents (BT-117) over the combined
        // base per rate — 30.03 × 0.19 = 5.7057 → 5.71. Had the aggregate
        // summed the per-MaLo breakdowns instead, it would show 3 × 1.90.
        let steuer: rust_decimal::Decimal = json["steuerbetraege"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                s["steuerwert"]
                    .as_str()
                    .map(|v| v.parse::<rust_decimal::Decimal>().unwrap())
                    .unwrap_or_else(|| {
                        rust_decimal::Decimal::try_from(s["steuerwert"].as_f64().unwrap()).unwrap()
                    })
            })
            .sum();
        assert_eq!(steuer, dec!(5.71));
    }

    /// Every rendered position names the MaLo it came from; the document-level
    /// tax position names none.
    #[test]
    fn aggregate_annotates_positions_with_their_malo() {
        use rust_decimal::dec;
        let parts = vec![
            sub_invoice("11111111111", dec!(50)),
            sub_invoice("22222222222", dec!(70)),
        ];
        let (_, json) = build_aggregate_invoice(
            "RV-2",
            "9900000000001",
            "SAMMEL-RV-2".to_owned(),
            time::macros::date!(2026 - 01 - 01),
            time::macros::date!(2026 - 01 - 31),
            RegulatoryRates::default(),
            parts,
            vec![],
        )
        .unwrap();
        let pos = json["rechnungspositionen"].as_array().unwrap();
        // 2 commodity positions annotated + 1 aggregate tax position without.
        assert_eq!(pos[0]["marktlokationsId"], "11111111111");
        assert_eq!(pos[1]["marktlokationsId"], "22222222222");
        let tax = pos
            .iter()
            .find(|p| p["kategorie"] == "Tax")
            .expect("aggregate tax position");
        assert!(tax.get("marktlokationsId").is_none());
        // Deterministic rechnungsdatum — no wall clock in the document.
        assert_eq!(json["rechnungsdatum"], "2026-01-31");
    }

    /// The engine-error body is machine-readable: code, context, warnings.
    #[test]
    fn engine_error_body_is_structured() {
        let e = energy_billing::EngineError::ValidationBlocked {
            warnings: vec![energy_billing::BillingWarning {
                code: "MODUL2_AND_FLAT_NNE",
                severity: energy_billing::WarningSeverity::Error,
                message: "both configured".to_owned(),
            }],
        };
        let body: serde_json::Value =
            serde_json::from_str(&engine_error_body("51238696781", &e)).unwrap();
        assert_eq!(body["error"]["code"], "VALIDATION_BLOCKED");
        assert_eq!(body["error"]["context"], "51238696781");
        assert_eq!(body["error"]["warnings"][0]["code"], "MODUL2_AND_FLAT_NNE");
    }
}

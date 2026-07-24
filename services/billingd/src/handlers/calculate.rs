//! Calculate / preview / review-queue / release endpoints.

#[allow(unused_imports)]
use super::*;

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

    let mut rates = cfg.regulatory_rates_for_period(tariff.category_str(), period_from, period_to);
    apply_nehs_market_price(
        &mut rates,
        tariff.category_str(),
        period_from,
        &cfg,
        &tarifbd,
    )
    .await;
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
    let mut rates = cfg.regulatory_rates_for_period(tariff.category_str(), period_from, period_to);
    apply_nehs_market_price(
        &mut rates,
        tariff.category_str(),
        period_from,
        &cfg,
        &tarifbd,
    )
    .await;
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

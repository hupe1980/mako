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
    /// §13b UStG reverse charge — set `true` when the customer is a
    /// Stromwiederverkäufer (electricity/gas reseller, §13b Abs. 2 Nr. 5 lit. b
    /// UStG). The whole supply is then invoiced net (no VAT); the recipient owes
    /// the Umsatzsteuer, and the EN 16931 tax breakdown carries an `AE` subtotal.
    #[serde(default)]
    pub reverse_charge: bool,
    /// §40b Abs. 1 EnWG — this contract is billed **monthly**.
    ///
    /// Drives the §40c Abs. 1 deadline: monthly billing must reach the customer
    /// within three weeks of the period end, everything else within six. The
    /// trigger is the agreed cadence, not the length of this particular period —
    /// a 30-day Teilrechnung for a move-out is not monthly billing.
    #[serde(default)]
    pub monatliche_abrechnung: bool,
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

    let mut rates =
        match cfg.try_regulatory_rates_for_period(tariff.category_str(), period_from, period_to) {
            Ok(r) => r,
            // 422: the request is well-formed but cannot be billed as one period.
            // The error names the Stichtage so the caller can split and retry.
            Err(e) => {
                return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": e.to_string(),
                    "stichtage": e.stichtage.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    "legal_basis": "§28 Abs. 5/6 UStG (Gas/Fernwärme), §10 BEHG",
                })),
            )
                .into_response();
            }
        };
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

    // Deterministic risk gate: scored read-only *before* the outbox tx, because
    // a HELD band withholds the dispatch enqueue. The record it scores does not
    // exist yet — `assess_risk` reads only strictly-earlier periods, so the
    // result is unchanged.
    let assessment = assess_risk(
        &pool,
        &cfg,
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

    // Business write + dispatch event commit atomically: the invoice row and its
    // `de.billing.rechnung.erstellt` outbox row live in one transaction, so a
    // crash can never leave a billed period without its ERP event. A HELD invoice
    // commits without the event (dispatch waits for POST …/release).
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let record_id = match insert_billing_record(
        &mut *tx,
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
    if held {
        tracing::warn!(
            %record_id, %malo_id,
            score = assessment.as_ref().map(|a| a.score),
            "billingd: invoice HELD by risk gate — dispatch requires POST …/release"
        );
    } else if cfg.erp_webhook_url.is_some() {
        let ce = rechnung_erstellt_ce(
            record_id,
            &malo_id,
            &req.lf_mp_id,
            &result.to_rechnung_json(),
            false,
        );
        if let Err(e) = mako_service::outbox::enqueue(&mut tx, &ce).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
        if let Err(e) = mark_dispatched_tx(&mut *tx, record_id).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }
    // Attach the EN 16931 semantic model (the XRechnung/CII/UBL render source),
    // mapped from the invoice with full per-line VAT — not from BO4E.
    if let Err(e) = crate::einvoice::store(&mut *tx, record_id, &result, &cfg, &malo_id).await {
        tracing::warn!(%record_id, error = %e, "billingd: attach en16931 model failed");
    }
    if let Err(e) = tx.commit().await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    // Persist the risk findings on the now-committed record (best effort).
    persist_risk(&pool, record_id, assessment.as_ref()).await;

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

/// Score a freshly calculated invoice against its history (read-only).
///
/// Pure of any write: the caller needs the band *before* opening the outbox
/// transaction (a HELD band withholds the dispatch enqueue), and the record it
/// scores does not exist yet when this runs. `risk_context` queries strictly
/// earlier periods (`period_from < …`), so the result is identical whether the
/// current record is inserted or not. Failures degrade to `None` (unscored)
/// rather than failing the billing run — a broken history query must not block
/// invoice creation. Persist the returned assessment with
/// [`persist_risk`] after the business transaction commits.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn assess_risk(
    pool: &PgPool,
    cfg: &BillingdConfig,
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
    Some(crate::risk::assess(
        &cfg.risk,
        invoice,
        rates.mwst_rate,
        period_from,
        period_to,
        &ctx,
    ))
}

/// Best-effort persistence of a risk assessment on its committed record.
///
/// Runs *after* the outbox transaction: a persistence failure leaves the record
/// unscored but never rolls back the invoice or its already-enqueued dispatch
/// event — exactly the pre-outbox degradation.
pub(crate) async fn persist_risk(
    pool: &PgPool,
    record_id: Uuid,
    assessment: Option<&crate::risk::RiskAssessment>,
) {
    let Some(a) = assessment else {
        return;
    };
    if let Err(e) = crate::pg::set_risk(pool, record_id, a).await {
        tracing::warn!(%record_id, error = %e, "billingd: risk persistence failed");
    }
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
    // Release stamp + withheld dispatch event commit atomically: the record is
    // marked released and its `de.billing.rechnung.erstellt` outbox row is
    // written in one transaction, so a crash cannot release a record without
    // (eventually) dispatching the invoice the risk gate had held back.
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let row = match crate::pg::release_held_record(&mut *tx, &cfg.tenant, id, claims.sub()).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            return (
                StatusCode::CONFLICT,
                "record is not HELD (already released, dispatched, or unscored)",
            )
                .into_response();
        }
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    if cfg.erp_webhook_url.is_some() {
        let ce = rechnung_erstellt_ce(
            row.id,
            &row.malo_id,
            &row.lf_mp_id,
            &row.rechnung_json,
            false,
        );
        if let Err(e) = mako_service::outbox::enqueue(&mut tx, &ce).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
        if let Err(e) = mark_dispatched_tx(&mut *tx, row.id).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }
    if let Err(e) = tx.commit().await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    Json(serde_json::json!({
        "id": row.id,
        "released_by": claims.sub(),
        "dispatched": cfg.erp_webhook_url.is_some(),
    }))
    .into_response()
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
    let mut rates =
        match cfg.try_regulatory_rates_for_period(tariff.category_str(), period_from, period_to) {
            Ok(r) => r,
            // 422: the request is well-formed but cannot be billed as one period.
            // The error names the Stichtage so the caller can split and retry.
            Err(e) => {
                return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": e.to_string(),
                    "stichtage": e.stichtage.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    "legal_basis": "§28 Abs. 5/6 UStG (Gas/Fernwärme), §10 BEHG",
                })),
            )
                .into_response();
            }
        };
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

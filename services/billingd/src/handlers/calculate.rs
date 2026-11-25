//! Calculate / preview / review-queue / release endpoints.

use super::*;

// ── Request bodies ─────────────────────────────────────────────────────────────

/// Request body for `POST /api/v1/billing/{malo_id}/calculate` and `/preview`.
///
/// All `*_meter` fields are optional — the engine selects the correct one based on
/// `tariff.category`.  Unsupported meter inputs for the active category are silently
/// ignored.  Supply `tariff` and/or `meter` as overrides to skip external lookups.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalculateRequest {
    pub lf_mp_id: String,
    /// §41 Abs. 1 Nr. 5 EnWG — Netzbetreiber identification on the invoice.
    ///
    /// Propagated to `BillingContext.nb_mp_id`. `billingd` does not look the NB
    /// up from `marktd` when it is absent.
    #[serde(default)]
    pub nb_mp_id: Option<String>,
    pub period_from: String,
    pub period_to: String,
    /// Override: supply product data directly (skip productd lookup).
    pub tariff: Option<Product>,
    /// Override: supply Strom meter data directly (skip edmd lookup).
    pub meter: Option<MeterInput>,
    /// Grid pass-through data. `billingd` does not fetch it from `marktd`; the
    /// caller supplies it.
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
    /// §13b UStG reverse charge — the supply is invoiced net (no VAT); the
    /// recipient owes the Umsatzsteuer and the EN 16931 tax breakdown carries
    /// an `AE` subtotal.
    ///
    /// **Derived automatically** from the customer master
    /// (`vertragd kunden.stromwiederverkaeufer`, §13b Abs. 2 Nr. 5 lit. b
    /// UStG) — this flag ORs with it: a caller can assert reverse charge for
    /// a customer not yet flagged, but cannot switch it off for one that is,
    /// because §13b is mandatory when its conditions are met.
    #[serde(default)]
    pub reverse_charge: bool,
    /// §40b Abs. 1 EnWG — this contract is billed **monthly**.
    ///
    /// Drives the **§ 40c Abs. 2** deadline: monthly billing must reach the
    /// customer within three weeks of the period end, everything else within
    /// six. (Abs. 1 is the two-week Fälligkeit, a different rule.) The trigger
    /// is the agreed cadence, not the length of this particular period — a
    /// 30-day Teilrechnung for a move-out is not monthly billing.
    #[serde(default)]
    pub monatliche_abrechnung: bool,
    /// Paid advance payments to settle on this invoice.
    ///
    /// § 40 Abs. 1 EnWG requires the settling invoice to itemise and deduct
    /// them. A resulting credit is then governed by **§ 40c Abs. 3**: offset in
    /// full against the next Abschlag, or paid out within two weeks — and from
    /// an Abschlussrechnung, paid out within two weeks either way. Each entry
    /// carries the VAT rate it was invoiced at.
    #[serde(default)]
    pub abschlaege: Vec<energy_billing::AbschlagDeduction>,
    /// § 14 Abs. 5 Satz 2 UStG — how the settling invoice presents the advances.
    ///
    /// `ENDRECHNUNG` invoices the whole supply and deducts the advances **and
    /// the tax contained in them**; `RESTRECHNUNG` invoices only the remainder
    /// and lists no advances. Both are lawful and the customer pays the same
    /// amount; they differ in what the document shows.
    ///
    /// The BMF recommends the Restrechnung for e-invoices (Schreiben v.
    /// 15.10.2024, Rn. 48), because EN 16931's core profiles carry no
    /// per-advance tax — mako renders it as one BG-20 document-level allowance
    /// per `(VAT category, rate)` group, so the residual base per rate is
    /// exact even when an advance was invoiced at a different rate.
    ///
    /// Absent, the operator's `[policy] settlement_form` default applies.
    #[serde(default)]
    pub settlement_form: Option<energy_billing::SettlementForm>,
}

// ── Calculate ─────────────────────────────────────────────────────────────────

/// `POST /api/v1/billing/{malo_id}/calculate`
///
/// Pipeline:
/// 1. Parse + validate period
/// 2. Fetch `Product` from `productd` (or use request override)
/// 3. Fetch consumption from `edmd` (or use request override)
/// 4. Take grid pass-through from the request's `grid` (nothing fetches it from `marktd`)
/// 5. Dispatch to category-specific pure calculator
/// 6. Persist `billing_records` (idempotent on same malo+period+product)
/// 7. Emit `de.mako.billing.rechnung.erstellt` CloudEvent
#[utoipa::path(
    post,
    path = "/api/v1/billing/{malo_id}/calculate",
    tag = "billing",
    params(
        ("malo_id" = String, Path, description = "11-digit Marktlokations-ID"),
    ),
    request_body(content = serde_json::Value, description = "`CalculateRequest`: the period, the Lieferant and optional inputs that override what billingd reads"),
    responses(
        (status = 201, description = "Calculated, persisted and announced", body = serde_json::Value),
        (status = 400, description = "Malformed period or input", body = crate::openapi::Problem),
        (status = 403, description = "Cedar `run-billing` refused", body = crate::openapi::Problem),
        (status = 409, description = "The period already carries an issued document, or the number is in use", body = crate::openapi::Problem),
        (status = 422, description = "Well-formed, but it cannot be billed as asked — the code names the rule", body = crate::openapi::Problem),
        (status = 502, description = "An upstream service the calculation reads did not answer", body = crate::openapi::Problem),
    )
)]
pub async fn post_calculate(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Path(malo_id): Path<String>,
    Json(req): Json<CalculateRequest>,
) -> BillingResult<impl IntoResponse> {
    let cfg = Arc::clone(&deps.cfg);
    authorize(&cedar, &claims, "run-billing", &cfg.tenant)?;
    let (period_from, period_to) = parse_period(&req.period_from, &req.period_to)?;
    // The product assignments covering the period, each split further at every
    // statutory rate boundary inside it. A price change mid-period is billed as
    // the parts it consists of; priced at the product in force on the period's
    // last day, a 25→35 ct switch on 15 March bills all of March at 35 ct.
    let legs = resolve_legs(&req, &deps, &malo_id, period_from, period_to).await?;
    let summary = LegSummary::of(&legs);

    // § 14 Abs. 4 Nr. 4 UStG: a fortlaufende Nummer from the tenant's series,
    // unless the caller stated one. Allocated before the engine runs because it
    // is the document's BT-1.
    let rechnungsnummer = next_rechnungsnummer(
        &pool,
        &cfg.tenant,
        series::INVOICE,
        req.rechnungsnummer.as_deref(),
        period_from,
    )
    .await?;

    // The BG-7 buyer travels with the priced invoice: both come from the one
    // vertragd answer the billing pipeline already needs for the §40 Abs. 1
    // contract facts. billingd holds no customer master, and a model built
    // without it carries a synthesised buyer that fails XRechnung on BR-DE-8/9.
    let Billed { invoice: result } = dispatch_invoice_multi(
        &deps,
        &legs,
        &req,
        &malo_id,
        &rechnungsnummer,
        // A single on-demand calculation belongs to no run.
        RunId::NONE,
    )
    .await?;

    // A public-law Gebühr is issued as a Bescheid beside the supply invoice,
    // under one `billingRunId` — BR-O-11 ff. forbid the two on one e-invoice.
    let mut belege = crate::gebuehrenbescheid::trennen(result);
    crate::gebuehrenbescheid::eine_lauf_id(&mut belege);
    let bescheid = match belege.gebuehr.take() {
        Some(mut g) => {
            g.context.rechnungsnummer =
                next_rechnungsnummer(&pool, &cfg.tenant, series::BESCHEID, None, period_from)
                    .await?;
            let view = crate::gebuehrenbescheid::view(&g, &cfg)?;
            Some((g, view))
        }
        None => None,
    };
    let Some(result) = belege.lieferung else {
        // The period carries nothing but Gebühren: the Bescheid is its only
        // document.
        let (g, view) = bescheid.ok_or_else(|| {
            BillingError::Internal(anyhow::anyhow!("a billed period yielded no document"))
        })?;
        let mut tx = pool.begin().await?;
        let id = match crate::gebuehrenbescheid::speichern(
            &mut tx,
            &cfg,
            &g,
            &view,
            &summary.product_code,
            &summary.category,
            false,
        )
        .await
        {
            Ok(id) => id,
            Err(e) => return Err(period_conflict(&mut tx, &cfg.tenant, e).await),
        };
        tx.commit().await?;
        return Ok((
            StatusCode::CREATED,
            Json(serde_json::json!({
                "id": id,
                "dokumentart": crate::gebuehrenbescheid::GEBUEHRENBESCHEID,
                "malo_id": malo_id,
                "rechnungsnummer": g.context.rechnungsnummer,
                "period_from": period_from.to_string(),
                "period_to": period_to.to_string(),
                "netto_eur": g.netto_eur,
                "brutto_eur": g.brutto_eur,
                "billing_run_id": g.billing_run_id,
                "bescheid": view,
            })),
        ));
    };

    // The rates the risk gate scores the document against. Each leg was billed
    // under its own; a split period is scored against its **last** leg's, which
    // are the ones in force when the document is issued and the ones an anomaly
    // in the total would be measured against.
    let last_leg = legs.last().expect("resolve_legs yields at least one leg");
    let nehs = fetch_nehs_price(
        last_leg.tariff.category_str(),
        last_leg.from,
        &deps.productd,
    )
    .await;
    let rates = cfg.try_regulatory_rates_for_period(
        last_leg.tariff.category_str(),
        last_leg.from,
        last_leg.to,
        nehs,
    )?;

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
    // `de.mako.billing.rechnung.erstellt` outbox row live in one transaction, so a
    // crash can never leave a billed period without its ERP event. A HELD invoice
    // commits without the event (issuance waits for POST …/release).
    let rechnung_json = result.to_rechnung_json();
    let mut tx = pool.begin().await?;
    let record_id = insert_billing_record(
        &mut *tx,
        &crate::pg::NewBillingRecord {
            tenant: &cfg.tenant,
            malo_id: &malo_id,
            lf_mp_id: &req.lf_mp_id,
            product_code: &summary.product_code,
            category: &summary.category,
            rechnungsnummer: &rechnungsnummer,
            period_from,
            period_to,
            rechnung_json: &rechnung_json,
            total_netto_eur: result.netto_eur,
            total_brutto_eur: result.brutto_eur,
            steuerzeilen: &crate::steuerzeilen::aus_rechnung(&result),
            due_on: Some(crate::handlers::sect40c_due_on(&req, period_to)),
        },
    )
    .await;
    let record_id = match record_id {
        Ok(id) => id,
        Err(e) => return Err(period_conflict(&mut tx, &cfg.tenant, e).await),
    };
    if held {
        tracing::warn!(
            %record_id, %malo_id,
            score = assessment.as_ref().map(|a| a.score),
            "billingd: invoice HELD by risk gate — issuance requires POST …/release"
        );
    } else {
        let ce = rechnung_erstellt_ce(
            &cfg.tenant,
            record_id,
            &malo_id,
            &req.lf_mp_id,
            &rechnung_json,
            false,
        );
        issue_record(&mut tx, &cfg, record_id, &ce).await?;
    }
    persist_risk(&mut *tx, record_id, assessment.as_ref()).await?;
    // Attach the EN 16931 semantic model (the XRechnung/CII/UBL render source),
    // mapped from the invoice with full per-line VAT — not from BO4E.
    crate::einvoice::store(&mut tx, record_id, &result, &cfg, &malo_id).await?;
    let bescheid_record = match &bescheid {
        Some((g, view)) => match crate::gebuehrenbescheid::speichern(
            &mut tx,
            &cfg,
            g,
            view,
            &summary.product_code,
            &summary.category,
            held,
        )
        .await
        {
            Ok(id) => Some(serde_json::json!({
                "id": id,
                "rechnungsnummer": g.context.rechnungsnummer,
                "netto_eur": g.netto_eur,
                "brutto_eur": g.brutto_eur,
            })),
            Err(e) => return Err(period_conflict(&mut tx, &cfg.tenant, e).await),
        },
        None => None,
    };
    tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": record_id,
            "malo_id": malo_id,
            "rechnungsnummer": rechnungsnummer,
            "period_from": period_from.to_string(),
            "period_to": period_to.to_string(),
            "netto_eur": result.netto_eur,
            "brutto_eur": result.brutto_eur,
            "positions_count": result.positions.len(),
            "risk": assessment,
            "held": held,
            "billing_run_id": result.billing_run_id,
            "gebuehrenbescheid": bescheid_record,
            "rechnung": rechnung_json,
        })),
    ))
}

/// Turn a refused write into the answer the caller can act on.
///
/// A period that already carries an issued document is a `409` **naming that
/// document**, not a `500` with a database string: a client retrying a request
/// whose response it lost reconciles against the record id, and an operator
/// reading the message knows which invoice to storno. An internal error would
/// read as "try again" for something that can never succeed.
pub(crate) async fn period_conflict(
    executor: &mut sqlx::PgConnection,
    tenant: &str,
    e: crate::pg::InsertError,
) -> crate::error::BillingError {
    let crate::pg::InsertError::PeriodAlreadyIssued {
        ref malo_id,
        ref lf_mp_id,
        ref product_code,
        period_from,
        period_to,
    } = e
    else {
        return e.into();
    };
    let Some((id, nr, outcome)) = crate::pg::find_live_original(
        executor,
        tenant,
        malo_id,
        lf_mp_id,
        product_code,
        period_from,
        period_to,
    )
    .await
    else {
        return e.into();
    };
    crate::error::BillingError::conflict_with(
        "PERIOD_ALREADY_BILLED",
        e.to_string(),
        serde_json::json!({
            "record_id": id,
            "rechnungsnummer": nr,
            "outcome": outcome,
            "malo_id": malo_id,
            "lf_mp_id": lf_mp_id,
            "period_from": period_from.to_string(),
            "period_to": period_to.to_string(),
        }),
    )
}

/// Score a freshly calculated invoice against its history (read-only).
///
/// Pure of any write: the caller needs the band *before* opening the outbox
/// transaction (a HELD band withholds the dispatch enqueue), and the record it
/// scores does not exist yet when this runs. `risk_context` queries strictly
/// earlier periods (`period_from < …`), so the result is identical whether the
/// current record is inserted or not. Failures degrade to `None` (unscored)
/// rather than failing the billing run — a broken history query must not block
/// invoice creation. Persist the returned assessment with [`persist_risk`]
/// **inside** the business transaction.
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

/// Persist a risk assessment on its record, inside the caller's transaction.
///
/// Not best-effort, and not after the commit. A HELD record is withheld from
/// dispatch, and `POST …/release` only finds rows whose stored `risk_band` is
/// `HELD` — so an assessment that failed to persist after the commit left the
/// invoice held, invisible to the review queue and impossible to release. The
/// band and the withheld dispatch are one decision and now commit as one.
pub(crate) async fn persist_risk(
    executor: impl sqlx::PgExecutor<'_>,
    record_id: Uuid,
    assessment: Option<&crate::risk::RiskAssessment>,
) -> anyhow::Result<()> {
    match assessment {
        Some(a) => crate::pg::set_risk(executor, record_id, a).await,
        None => Ok(()),
    }
}

/// `GET /api/v1/billing/review-queue?band=&limit=`
///
/// The analyst work list: REVIEW and HELD records, highest risk first, each
/// carrying its coded findings.
#[utoipa::path(
    get,
    path = "/api/v1/billing/review-queue",
    tag = "risk",
    params(
        ReviewQueueQuery,
    ),
    responses(
        (status = 200, description = "Records the risk gate banded REVIEW or HELD, highest score first", body = serde_json::Value),
        (status = 403, description = "Cedar `read-billing` refused", body = crate::openapi::Problem),
    )
)]
pub async fn get_review_queue(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Query(q): Query<ReviewQueueQuery>,
) -> BillingResult<impl IntoResponse> {
    let cfg = &deps.cfg;
    authorize(&cedar, &claims, "read-billing", &cfg.tenant)?;
    let rows = crate::pg::list_review_queue(
        &pool,
        &cfg.tenant,
        q.band.as_deref(),
        q.limit.unwrap_or(100).clamp(1, 1000),
    )
    .await?;
    Ok(Json(
        serde_json::json!({ "count": rows.len(), "records": rows }),
    ))
}

/// Receipt of a committed manual run. Its evidence is available in the billing watch.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct SweepReceipt {
    pub run_id: Uuid,
}

/// The run uses the installation's clock and policy. Unknown options are refused.
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SweepRequest {}

/// Request the scheduled billing workflow on today's Berlin business date.
#[utoipa::path(
    post, path = "/api/v1/billing/runs/sweep", tag = "deadlines",
    request_body = SweepRequest,
    params(("Idempotency-Key" = Uuid, Header, description = "UUID identifying this attempt; reuse it after an uncertain response")),
    responses(
        (status = 200, description = "Billing run completed or receipt replayed", body = SweepReceipt),
        (status = 400, description = "A single UUID Idempotency-Key is required", body = mako_service::ApiError),
        (status = 403, description = "Cedar `run-billing-sweep` refused", body = mako_service::ApiError),
        (status = 409, description = "Another run is active", body = mako_service::ApiError),
        (status = 422, description = "Unknown sweep option", body = mako_service::ApiError),
        (status = 502, description = "The attempt failed", body = mako_service::ApiError),
    )
)]
pub async fn post_billing_sweep(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    headers: axum::http::HeaderMap,
    Json(_request): Json<SweepRequest>,
) -> mako_service::ApiResult<Json<SweepReceipt>> {
    use mako_service::ApiError;
    authorize(&cedar, &claims, "run-billing-sweep", &deps.cfg.tenant)
        .map_err(|_| ApiError::forbidden("the LF role is required to run the billing sweep"))?;
    let id = mako_service::headers::single_str(&headers, "Idempotency-Key")
        .ok()
        .flatten()
        .and_then(|key| Uuid::parse_str(key).ok())
        .ok_or_else(|| {
            ApiError::bad_request("supply a single UUID Idempotency-Key")
                .with_code("invalid_idempotency_key")
        })?;
    let now = time::OffsetDateTime::now_utc();
    let next = crate::billing_runs::next_scheduled_at(&deps.cfg.billing_runs, now);
    let run_id =
        crate::billing_runs::sweep(&deps, &pool, mako_fristen::berlin_date(now), next, Some(id))
            .await
            .map_err(|error| {
                ApiError::upstream("billing sweep", error).with_code("billing_sweep_failed")
            })?
            .ok_or_else(|| {
                ApiError::conflict("a billing sweep is in progress").with_code("billing_sweep_busy")
            })?;
    Ok(Json(SweepReceipt { run_id }))
}

/// The LF's refused periods, ordered by due date, with the latest discovery evidence.
#[utoipa::path(
    get,
    path = "/api/v1/billing/unbilled",
    tag = "deadlines",
    params(
        UnbilledQuery,
    ),
    responses(
        (status = 200, description = "Refused periods ordered by due date, with the latest discovery evidence", body = crate::discovery::UnbilledPeriods),
        (status = 403, description = "Cedar `read-billing-watch` refused", body = crate::openapi::Problem),
    )
)]
pub async fn get_unbilled_periods(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Query(q): Query<UnbilledQuery>,
) -> BillingResult<impl IntoResponse> {
    let cfg = &deps.cfg;
    authorize(&cedar, &claims, "read-billing-watch", &cfg.tenant)?;
    Ok(Json(
        crate::discovery::UnbilledPeriods::read(
            &pool,
            &cfg.tenant,
            q.open.unwrap_or(true),
            q.limit,
        )
        .await?,
    ))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct UnbilledQuery {
    /// Only periods no invoice has discharged yet (default `true`).
    pub open: Option<bool>,
    pub limit: Option<i64>,
}

/// `GET /api/v1/billing/deadline-summary`
///
/// Per Lieferant: invoices owed, issued on time, issued late and overdue under
/// § 40c Abs. 2 EnWG.
#[utoipa::path(
    get,
    path = "/api/v1/billing/deadline-summary",
    tag = "deadlines",
    responses(
        (status = 200, description = "Per Lieferant: owed, on time, late and overdue under § 40c Abs. 2 EnWG", body = crate::discovery::DeadlineSummary),
        (status = 403, description = "Cedar `read-billing-watch` refused", body = crate::openapi::Problem),
    )
)]
pub async fn get_deadline_summary(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
) -> BillingResult<impl IntoResponse> {
    let cfg = &deps.cfg;
    authorize(&cedar, &claims, "read-billing-watch", &cfg.tenant)?;
    Ok(Json(
        crate::discovery::DeadlineSummary::read(&pool, &cfg.tenant).await?,
    ))
}

/// `GET /api/v1/billing/{id}/nachrechnung`
///
/// Rechnung nachrechnen: every line and total of the stored invoice recomputed
/// to the cent, whether the issue-time seal still holds, and the inputs of
/// each line (quantity and how it was obtained, unit price and its source,
/// the rule).
#[utoipa::path(
    get,
    path = "/api/v1/billing/{id}/nachrechnung",
    tag = "nachrechnen",
    params(
        ("id" = uuid::Uuid, Path, description = "The billing record"),
    ),
    responses(
        (status = 200, description = "The invoice recomputed line by line, with the seal check", body = serde_json::Value),
        (status = 403, description = "Cedar `read-billing` refused", body = crate::openapi::Problem),
        (status = 404, description = "No such billing record in this tenant (`RECORD_NOT_FOUND`)", body = crate::openapi::Problem),
        (status = 422, description = "A Storno- or Korrekturrechnung; recompute its original", body = crate::openapi::Problem),
    )
)]
pub async fn get_nachrechnung(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Path(id): Path<Uuid>,
) -> BillingResult<impl IntoResponse> {
    let cfg = &deps.cfg;
    authorize(&cedar, &claims, "read-billing", &cfg.tenant)?;
    let row = crate::pg::fetch_billing_record(&pool, &cfg.tenant, id)
        .await?
        .ok_or_else(|| BillingError::not_found("RECORD_NOT_FOUND", "billing record not found"))?;
    if row.is_correction {
        return Err(BillingError::unprocessable(
            "NOT_AN_ORIGINAL",
            "a Storno- or Korrekturrechnung is recomputed through its original",
        ));
    }
    let mut view = crate::nachrechnen::nachrechnen(&row)?;
    view.public_link = crate::nachrechnen::public_link(cfg, id);
    Ok(Json(view))
}

/// `POST /api/v1/billing/nachrechnen` `{ "token": "…" }`
///
/// The masked projection the public page (portald) renders: no name, no
/// address, the MaLo-ID masked. An unknown, malformed and a revoked token all
/// answer `404 RECORD_NOT_FOUND`, so the answer says nothing about whether a
/// token ever existed. The token travels in the body, never in a path a
/// request log or a metrics label could keep.
#[utoipa::path(
    post,
    path = "/api/v1/billing/nachrechnen",
    tag = "nachrechnen",
    request_body(content = VerifyTokenBody, description = "The verify token printed on the invoice"),
    responses(
        (status = 200, description = "The masked recomputation the public page shows", body = serde_json::Value),
        (status = 403, description = "Cedar `read-billing-by-token` refused", body = crate::openapi::Problem),
        (status = 404, description = "No invoice carries this token, or it was revoked", body = crate::openapi::Problem),
    )
)]
pub async fn post_public_nachrechnung(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Json(VerifyTokenBody { token }): Json<VerifyTokenBody>,
) -> BillingResult<impl IntoResponse> {
    let cfg = &deps.cfg;
    // portald's service principal alone: a token is a customer's credential,
    // and an operator holding `read-billing` reads invoices by id instead.
    authorize(&cedar, &claims, "read-billing-by-token", &cfg.tenant)?;
    let not_found = || BillingError::not_found("RECORD_NOT_FOUND", "no such invoice");
    if token.len() != 22 {
        return Err(not_found());
    }
    let hash = crate::nachrechnen::token_hash(&token);
    let row = crate::pg::fetch_by_verify_token(&pool, &cfg.tenant, &hash)
        .await?
        .ok_or_else(not_found)?;
    Ok(Json(crate::nachrechnen::public_view(&row)?))
}

/// The body of [`post_public_nachrechnung`].
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VerifyTokenBody {
    /// The verify token printed on the invoice.
    pub token: String,
}

/// `POST /api/v1/billing/{id}/verify-token/revoke`
///
/// Revoke the invoice's verify token: its public page answers 404 from now on.
#[utoipa::path(
    post,
    path = "/api/v1/billing/{id}/verify-token/revoke",
    tag = "nachrechnen",
    params(
        ("id" = uuid::Uuid, Path, description = "The billing record"),
    ),
    responses(
        (status = 200, description = "Revoked; the public page answers 404 from now on", body = serde_json::Value),
        (status = 403, description = "Cedar `correct-billing` refused", body = crate::openapi::Problem),
        (status = 404, description = "No such billing record in this tenant (`RECORD_NOT_FOUND`)", body = crate::openapi::Problem),
    )
)]
pub async fn post_revoke_verify_token(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Path(id): Path<Uuid>,
) -> BillingResult<impl IntoResponse> {
    let cfg = &deps.cfg;
    authorize(&cedar, &claims, "correct-billing", &cfg.tenant)?;
    if !crate::pg::revoke_verify_token(&pool, &cfg.tenant, id, claims.sub()).await? {
        return Err(BillingError::not_found(
            "VERIFY_TOKEN_NOT_FOUND",
            "the record does not exist or carries no verify token",
        ));
    }
    Ok(Json(
        serde_json::json!({ "record_id": id, "revoked": true }),
    ))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ReviewQueueQuery {
    pub band: Option<String>,
    pub limit: Option<i64>,
}

/// `POST /api/v1/billing/{id}/release`
///
/// Analyst release of a HELD record: stamps who released it and dispatches
/// the CloudEvent that the risk gate withheld. 409 when the record is not
/// currently held.
#[utoipa::path(
    post,
    path = "/api/v1/billing/{id}/release",
    tag = "risk",
    params(
        ("id" = uuid::Uuid, Path, description = "The billing record"),
    ),
    responses(
        (status = 200, description = "Released and dispatched", body = serde_json::Value),
        (status = 403, description = "Cedar `release-billing` refused", body = crate::openapi::Problem),
        (status = 404, description = "No such billing record in this tenant (`RECORD_NOT_FOUND`)", body = crate::openapi::Problem),
        (status = 409, description = "The record is not held", body = crate::openapi::Problem),
    )
)]
pub async fn post_release(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Path(id): Path<Uuid>,
) -> BillingResult<impl IntoResponse> {
    let cfg = &deps.cfg;
    authorize(&cedar, &claims, "release-billing", &cfg.tenant)?;
    // Release stamp + withheld dispatch event commit atomically: the record is
    // marked released and its `de.mako.billing.rechnung.erstellt` outbox row is
    // written in one transaction, so a crash cannot release a record without
    // (eventually) issuing the invoice the risk gate had held back.
    let mut tx = pool.begin().await?;
    let row = crate::pg::release_held_record(&mut *tx, &cfg.tenant, id, claims.sub())
        .await?
        .ok_or_else(|| {
            crate::error::BillingError::conflict(
                "NOT_HELD",
                "record is not HELD (already released, issued, or unscored)",
            )
        })?;
    let ce = rechnung_erstellt_ce(
        &cfg.tenant,
        row.id,
        &row.malo_id,
        &row.lf_mp_id,
        &row.rechnung_json,
        false,
    );
    issue_record(&mut tx, cfg, row.id, &ce).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({
        "id": row.id,
        "rechnungsnummer": row.rechnungsnummer,
        "released_by": claims.sub(),
        "outcome": "dispatched",
    })))
}

/// `POST /api/v1/billing/{id}/reviewed`
///
/// An analyst's review of a REVIEW record: stamps who reviewed it and takes it
/// off the work list. The invoice was already issued when the risk gate
/// banded it, so this sends nothing — a finding that holds is answered with a
/// correction (`POST …/correction`), not here. 409 when the record is not an
/// unreviewed REVIEW record.
#[utoipa::path(
    post,
    path = "/api/v1/billing/{id}/reviewed",
    tag = "risk",
    params(
        ("id" = uuid::Uuid, Path, description = "The billing record"),
    ),
    responses(
        (status = 200, description = "The review is recorded; nothing is sent", body = serde_json::Value),
        (status = 403, description = "Cedar `review-billing` refused", body = crate::openapi::Problem),
        (status = 404, description = "No such billing record in this tenant (`RECORD_NOT_FOUND`)", body = crate::openapi::Problem),
        (status = 409, description = "The record is not banded REVIEW, or was reviewed already", body = crate::openapi::Problem),
    )
)]
pub async fn post_reviewed(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Path(id): Path<Uuid>,
) -> BillingResult<impl IntoResponse> {
    let cfg = &deps.cfg;
    authorize(&cedar, &claims, "review-billing", &cfg.tenant)?;
    crate::pg::mark_reviewed(&pool, &cfg.tenant, id, claims.sub())
        .await?
        .ok_or_else(|| {
            crate::error::BillingError::conflict(
                "NOT_IN_REVIEW",
                "record is not awaiting review (already reviewed, held, or never banded REVIEW)",
            )
        })?;
    Ok(Json(serde_json::json!({
        "id": id,
        "reviewed_by": claims.sub(),
    })))
}

/// Everything a dry-run produces: the engine's invoice and the period it billed.
pub struct Preview {
    pub invoice: Invoice,
    pub period_from: time::Date,
    pub period_to: time::Date,
}

/// Run the calculation pipeline without persisting anything.
///
/// The read-only half of `/calculate`: parse the period, resolve the product
/// legs covering it — each under its own statutory rates, refusing a straddle —
/// and bill them as one document. Shared by `POST …/preview` and the
/// `preview_billing` MCP tool so the two can never answer differently.
///
/// # Errors
///
/// The HTTP status and body the caller should relay — 400 for a malformed
/// period, 422 for a straddling period or a blocked validation, 502 when an
/// upstream service is unreachable.
pub async fn compute_preview(
    deps: &BillingDeps,
    malo_id: &str,
    req: &CalculateRequest,
) -> BillingResult<Preview> {
    let (period_from, period_to) = parse_period(&req.period_from, &req.period_to)?;
    // The same legs `/calculate` bills, so a preview can never price a period
    // differently from the invoice it previews.
    let legs = resolve_legs(req, deps, malo_id, period_from, period_to).await?;

    // A dry run issues nothing, so it must not consume a number from the
    // § 14 UStG series — the placeholder makes that visible in the output.
    let rechnungsnummer = req
        .rechnungsnummer
        .clone()
        .unwrap_or_else(|| format!("PREVIEW-{malo_id}-{period_from}"));
    let billed =
        dispatch_invoice_multi(deps, &legs, req, malo_id, &rechnungsnummer, RunId::NONE).await?;
    // The refusal `/calculate` answers for a foreign Rechnungsempfänger.
    crate::einvoice::pruefe_empfaengerland(&billed.invoice)?;
    Ok(Preview {
        invoice: billed.invoice,
        period_from,
        period_to,
    })
}

/// `POST /api/v1/billing/{malo_id}/preview` — dry-run, no persist, no CloudEvent.
#[utoipa::path(
    post,
    path = "/api/v1/billing/{malo_id}/preview",
    tag = "billing",
    params(
        ("malo_id" = String, Path, description = "11-digit Marktlokations-ID"),
    ),
    request_body(content = serde_json::Value, description = "`CalculateRequest`, as for `/calculate`"),
    responses(
        (status = 200, description = "The positions, totals and warnings a run would produce; nothing persisted", body = serde_json::Value),
        (status = 400, description = "Malformed period or input", body = crate::openapi::Problem),
        (status = 403, description = "Cedar `preview-billing` refused", body = crate::openapi::Problem),
        (status = 422, description = "Well-formed, but it cannot be billed as asked — the code names the rule", body = crate::openapi::Problem),
        (status = 502, description = "An upstream service the calculation reads did not answer", body = crate::openapi::Problem),
    )
)]
pub async fn post_preview(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Path(malo_id): Path<String>,
    Json(req): Json<CalculateRequest>,
) -> BillingResult<impl IntoResponse> {
    authorize(&cedar, &claims, "preview-billing", &deps.cfg.tenant)?;
    let preview = compute_preview(&deps, &malo_id, &req).await?;
    Ok(Json(preview_json(&malo_id, &preview)))
}

/// The dry-run answer, shared by the HTTP endpoint and the MCP tool.
pub fn preview_json(malo_id: &str, p: &Preview) -> serde_json::Value {
    serde_json::json!({
        "preview": true,
        "malo_id": malo_id,
        "period_from": p.period_from.to_string(),
        "period_to": p.period_to.to_string(),
        "netto_eur": p.invoice.netto_eur,
        "brutto_eur": p.invoice.brutto_eur,
        "positions_count": p.invoice.positions.len(),
        "warnings": p.invoice.warnings.iter().map(|w| serde_json::json!({
            "code": w.code,
            "severity": format!("{:?}", w.severity),
            "message": w.message,
        })).collect::<Vec<_>>(),
        "rechnung": p.invoice.to_rechnung_json(),
    })
}

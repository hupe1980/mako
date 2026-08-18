//! Calculate / preview / review-queue / release endpoints.

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
    /// §42c EnWG allocated community share (SHARING category).
    ///
    /// The residual supply comes from `edmd` like any other electricity bill;
    /// the allocation is the community's, computed by the sharing settlement
    /// and handed in here, so `EnergyShareProvider` can credit it.
    #[serde(default)]
    pub energy_share: Option<energy_billing::EnergyShareMeterInput>,
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
    let tariff = resolve_tariff(&req, &deps.tarifbd, &malo_id).await?;

    // A period straddling a statutory boundary has no correct single rate; the
    // 422 names the Stichtage so the caller can split and retry.
    let mut rates =
        cfg.try_regulatory_rates_for_period(tariff.category_str(), period_from, period_to)?;
    apply_nehs_market_price(
        &mut rates,
        tariff.category_str(),
        period_from,
        &cfg,
        &deps.tarifbd,
    )
    .await;

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
    // vertragd answer `dispatch_invoice` already needs for the §40 Abs. 1
    // contract facts. billingd holds no customer master, and a model built
    // without it carries a synthesised buyer that fails XRechnung on BR-DE-8/9.
    let Billed {
        invoice: result,
        buyer,
    } = dispatch_invoice(
        &deps,
        &tariff,
        &req,
        &malo_id,
        &rechnungsnummer,
        period_from,
        period_to,
        &rates,
        // A single on-demand calculation belongs to no run.
        RunId::NONE,
    )
    .await?;

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
    // commits without the event (issuance waits for POST …/release).
    let rechnung_json = result.to_rechnung_json();
    let mut tx = pool.begin().await?;
    let record_id = insert_billing_record(
        &mut *tx,
        &crate::pg::NewBillingRecord {
            tenant: &cfg.tenant,
            malo_id: &malo_id,
            lf_mp_id: &req.lf_mp_id,
            product_code: tariff.product_code().unwrap_or(tariff.category_str()),
            category: tariff.category_str(),
            rechnungsnummer: &rechnungsnummer,
            period_from,
            period_to,
            rechnung_json: &rechnung_json,
            total_netto_eur: result.netto_eur,
            total_brutto_eur: result.brutto_eur,
        },
    )
    .await;
    let record_id = match record_id {
        Ok(id) => id,
        Err(e) => return Err(period_conflict(&pool, &cfg.tenant, e).await),
    };
    if held {
        tracing::warn!(
            %record_id, %malo_id,
            score = assessment.as_ref().map(|a| a.score),
            "billingd: invoice HELD by risk gate — issuance requires POST …/release"
        );
    } else {
        let ce = rechnung_erstellt_ce(record_id, &malo_id, &req.lf_mp_id, &rechnung_json, false);
        issue_record(&mut tx, &cfg, record_id, &ce).await?;
    }
    persist_risk(&mut *tx, record_id, assessment.as_ref()).await?;
    // Attach the EN 16931 semantic model (the XRechnung/CII/UBL render source),
    // mapped from the invoice with full per-line VAT — not from BO4E.
    crate::einvoice::store(&mut *tx, record_id, &result, &cfg, &malo_id, buyer.as_ref()).await?;
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
            "rechnung": rechnung_json,
        })),
    ))
}

/// Turn a refused write into the answer the caller can act on.
///
/// A period that already carries an issued document is a `409` **naming that
/// document**, not a `500` with a database string: a client retrying a request
/// whose response it lost reconciles against the record id, and an operator
/// reading the message knows which invoice to storno. The refusal used to
/// surface as an internal error, which reads as "try again" for something that
/// can never succeed.
pub(crate) async fn period_conflict(
    pool: &PgPool,
    tenant: &str,
    e: crate::pg::InsertError,
) -> crate::error::BillingError {
    let crate::pg::InsertError::PeriodAlreadyIssued {
        ref malo_id,
        ref product_code,
        period_from,
        period_to,
    } = e
    else {
        return e.into();
    };
    let Some((id, nr, outcome)) =
        crate::pg::find_live_original(pool, tenant, malo_id, product_code, period_from, period_to)
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
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Path(id): Path<Uuid>,
) -> BillingResult<impl IntoResponse> {
    let cfg = &deps.cfg;
    authorize(&cedar, &claims, "release-billing", &cfg.tenant)?;
    // Release stamp + withheld dispatch event commit atomically: the record is
    // marked released and its `de.billing.rechnung.erstellt` outbox row is
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

/// Everything a dry-run produces: the engine's invoice and the period it billed.
pub struct Preview {
    pub invoice: Invoice,
    pub period_from: time::Date,
    pub period_to: time::Date,
}

/// Run the calculation pipeline without persisting anything.
///
/// The read-only half of `/calculate`: parse the period, resolve the product,
/// resolve the period's statutory rates (refusing a straddle), overlay the nEHS
/// market price, and bill. Shared by `POST …/preview` and the `preview_billing`
/// MCP tool so the two can never answer differently.
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
    let cfg = deps.cfg.as_ref();
    let (period_from, period_to) = parse_period(&req.period_from, &req.period_to)?;
    let tariff = resolve_tariff(req, &deps.tarifbd, malo_id).await?;
    let mut rates =
        cfg.try_regulatory_rates_for_period(tariff.category_str(), period_from, period_to)?;
    apply_nehs_market_price(
        &mut rates,
        tariff.category_str(),
        period_from,
        cfg,
        &deps.tarifbd,
    )
    .await;

    // A dry run issues nothing, so it must not consume a number from the
    // § 14 UStG series — the placeholder makes that visible in the output.
    let rechnungsnummer = req
        .rechnungsnummer
        .clone()
        .unwrap_or_else(|| format!("PREVIEW-{malo_id}-{period_from}"));
    let billed = dispatch_invoice(
        deps,
        &tariff,
        req,
        malo_id,
        &rechnungsnummer,
        period_from,
        period_to,
        &rates,
        RunId::NONE,
    )
    .await?;
    Ok(Preview {
        invoice: billed.invoice,
        period_from,
        period_to,
    })
}

/// `POST /api/v1/billing/{malo_id}/preview` — dry-run, no persist, no CloudEvent.
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

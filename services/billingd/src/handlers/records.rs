//! Billing record read endpoints (list / get / XRechnung download).

#[allow(unused_imports)]
use super::*;

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

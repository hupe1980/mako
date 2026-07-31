//! Korrekturrechnung (L8 — § 147 AO / GoBD).

#[allow(unused_imports)]
use super::*;

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

    // Korrekturrechnung row + its `de.billing.rechnung.erstellt`
    // (`is_correction: true`) event commit atomically, so accountingd's CREDIT
    // ledger entry can never be lost after the correction is persisted.
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let correction_id = match insert_correction_record(
        &mut *tx,
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

    if cfg.erp_webhook_url.is_some() {
        let ce = rechnung_erstellt_ce(
            correction_id,
            &original.malo_id,
            &original.lf_mp_id,
            &corrected_json,
            true,
        );
        if let Err(e) = mako_service::outbox::enqueue(&mut tx, &ce).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }
    if let Err(e) = tx.commit().await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
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

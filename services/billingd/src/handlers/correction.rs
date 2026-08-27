//! Stornorechnung / Korrekturrechnung (§ 147 AO / GoBD).

use super::*;

// ── Stornorechnung / Korrekturrechnung ────────────────────────────────────────

/// Request body for `POST /api/v1/billing/{id}/correction`.
#[derive(Debug, serde::Deserialize)]
pub struct CorrectionRequest {
    /// Human-readable reason for the correction (e.g. "Zählerstandskorrektur").
    pub reason: String,
    /// Override the Stornorechnung's number. Absent — the normal case — takes
    /// the next number of the tenant's `ST` series.
    #[serde(default)]
    pub rechnungsnummer: Option<String>,
}

/// Whether a record may be reversed at all.
///
/// * a **correction** — the original is corrected again, not the Storno.
/// * an **already cancelled** record — its period is free; re-bill and correct
///   that invoice.
/// * a record that was **never issued**. `generated` means the risk gate
///   withheld the document *and its CloudEvent*, so `accountingd` booked no
///   DEBIT for it; crediting it would post an unbalanced CREDIT against
///   nothing. Nor is there anything to reverse — a draft is outside
///   `br_unique_original`'s overwrite guard, so the remedy is to bill the
///   period again ([`crate::pg::insert_billing_record`] replaces it) or to
///   release it and correct the issued invoice.
///
/// # Errors
///
/// A `409`/`422` naming which of the three it is.
fn ensure_correctable(is_correction: bool, outcome: &str) -> BillingResult<()> {
    if is_correction {
        return Err(BillingError::unprocessable(
            "CANNOT_CORRECT_A_CORRECTION",
            "cannot create a correction of a correction — correct the original record instead",
        ));
    }
    match outcome {
        "cancelled" => Err(BillingError::conflict(
            "ALREADY_CANCELLED",
            "this record is already cancelled — re-bill the period via \
             POST /api/v1/billing/{malo_id}/calculate and correct that invoice instead",
        )),
        "generated" => Err(BillingError::conflict(
            "NOT_YET_ISSUED",
            "this record has not been issued — the risk gate is still withholding it, so \
             there is no document to reverse and nothing has been booked for it. Re-bill \
             the period via POST /api/v1/billing/{malo_id}/calculate, which replaces the \
             draft, or release it via POST /api/v1/billing/{id}/release and correct the \
             issued invoice",
        )),
        _ => Ok(()),
    }
}

/// `POST /api/v1/billing/{id}/correction`
///
/// Issue a Stornorechnung for an existing record.
///
/// 1. Fetch the original by `id` (tenant-scoped).
/// 2. Produce a correction `Rechnung`: `istOriginal: false`,
///    `originalRechnungsnummer: <original>`, every monetary position negated,
///    and its own number from the tenant's `ST` series.
/// 3. Insert it with `is_correction = TRUE` and `original_record_id`, and
///    advance the original's `outcome` to `cancelled` — in one transaction.
/// 4. Enqueue `de.billing.rechnung.erstellt` (`is_correction: true`) so
///    `accountingd` books the CREDIT.
///
/// ## Storno und Neuberechnung
///
/// Cancelling the original **releases its period**: `br_unique_original`
/// excludes cancelled rows, so the corrected amounts are re-billed by calling
/// `POST /api/v1/billing/{malo_id}/calculate` again for the same window. That
/// is the flow German accounting practice expects, and the schema used to
/// forbid it while the endpoint's own error message recommended it.
///
/// ## § 147 AO / GoBD
///
/// The original's content is **never** modified — only its outcome — and both
/// documents stay in `billing_records` for the statutory **8 years**
/// (§ 147 Abs. 3 AO for a Buchungsbeleg, reduced from ten by the BEG IV with
/// effect from 01.01.2025).
pub async fn post_correction(
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(deps): Extension<Arc<BillingDeps>>,
    Path(id): Path<Uuid>,
    Json(req): Json<CorrectionRequest>,
) -> BillingResult<impl IntoResponse> {
    let cfg = &deps.cfg;
    authorize(&cedar, &claims, "correct-billing", &cfg.tenant)?;
    let original = fetch_billing_record(&pool, &cfg.tenant, id)
        .await?
        .ok_or_else(|| BillingError::not_found("RECORD_NOT_FOUND", "billing record not found"))?;

    ensure_correctable(original.is_correction, &original.outcome)?;

    // Produce a Korrekturrechnung JSON by negating the original via the library
    // function. The library owns all sign-negation logic, for consistency with
    // the engine's `negate_positions()` path.
    //
    // The Storno takes its **own** number from the `ST` series. Deriving it as
    // `KORR-{original}` was einmalig only by luck: re-billing the released
    // period and correcting *that* invoice produced the same string again, and
    // `br_unique_rechnungsnummer` refused the second Storno.
    let original_nr = original.rechnungsnummer.clone();
    let new_nr = next_rechnungsnummer(
        &pool,
        &cfg.tenant,
        series::CORRECTION,
        req.rechnungsnummer.as_deref(),
        original.period_from,
    )
    .await?;
    let corrected_json =
        negate_rechnung_json_for_correction(&original.rechnung_json, &original_nr, &new_nr);

    // The outbound gate. A Korrekturrechnung is built by negating the stored
    // JSON field by field — string-keyed surgery over totals, the tax
    // breakdown, the advances and every position — so a field the negation
    // misses yields a document whose totals silently disagree. It is persisted
    // and published as `de.billing.rechnung.erstellt`, and it was the one
    // billingd write path that never crossed the gate the ordinary invoice
    // path crosses. A correction that does not add up must not be booked.
    let corrected: rubo4e::current::Rechnung = serde_json::from_value(corrected_json.clone())
        .map_err(|e| anyhow::anyhow!("the correction is not a readable BO4E Rechnung: {e}"))?;
    mako_markt::bo4e::ensure_conformant(&corrected)
        .map_err(|e| anyhow::anyhow!("the correction is not a valid BO4E document: {e}"))?;

    let netto = -original.total_netto_eur.unwrap_or_default();
    let brutto = -original.total_brutto_eur.unwrap_or_default();

    // Korrekturrechnung row + its `de.billing.rechnung.erstellt`
    // (`is_correction: true`) event commit atomically, so accountingd's CREDIT
    // ledger entry can never be lost after the correction is persisted.
    let mut tx = pool.begin().await?;
    let correction_id = insert_correction_record(
        &mut tx,
        &crate::pg::NewBillingRecord {
            tenant: &cfg.tenant,
            malo_id: &original.malo_id,
            lf_mp_id: &original.lf_mp_id,
            product_code: &original.product_code,
            category: &original.category,
            rechnungsnummer: &new_nr,
            period_from: original.period_from,
            period_to: original.period_to,
            rechnung_json: &corrected_json,
            total_netto_eur: netto,
            total_brutto_eur: brutto,
        },
        original.id,
        Some(&req.reason),
    )
    .await?;

    let ce = rechnung_erstellt_ce(
        correction_id,
        &original.malo_id,
        &original.lf_mp_id,
        &corrected_json,
        true,
    );
    issue_record(&mut tx, cfg, correction_id, &ce).await?;

    // The correction credits the original: reuse its EN 16931 model as a credit
    // note (positive amounts, document type 381) so the render endpoints serve a
    // conformant Stornorechnung without re-billing.
    match original
        .en16931_json
        .as_ref()
        .and_then(|v| serde_json::from_value::<en16931::Invoice>(v.clone()).ok())
    {
        Some(model) => match crate::einvoice::to_credit_note(&model, &new_nr) {
            Ok(credit) => crate::einvoice::store_model(&mut *tx, correction_id, &credit).await?,
            Err(e) => {
                tracing::warn!(%correction_id, error = %e, "billingd: credit-note transform failed");
            }
        },
        // Not fatal, but not silent either: the Storno then has no EN 16931
        // model, and /xrechnung, /ubl and /pdf all answer 422 for it.
        None => tracing::warn!(
            %correction_id,
            original_id = %original.id,
            "billingd: the corrected original carries no EN 16931 model — the Stornorechnung \
             will have none either, and its e-invoice endpoints will refuse it"
        ),
    }
    tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "original_id": original.id,
            "original_rechnungsnummer": original_nr,
            "correction_id": correction_id,
            "malo_id": original.malo_id,
            "period_from": original.period_from.to_string(),
            "period_to": original.period_to.to_string(),
            "rechnungsnummer": new_nr,
            "credit_netto_eur": netto,
            "credit_brutto_eur": brutto,
            "reason": req.reason,
            "original_outcome": "cancelled",
            "next": "the period is free again — POST /api/v1/billing/{malo_id}/calculate to re-bill it",
        })),
    ))
}

#[cfg(test)]
mod tests {
    use super::ensure_correctable;

    /// A withheld draft has been booked nowhere, so crediting it would post an
    /// unbalanced CREDIT against nothing.
    #[test]
    fn a_never_issued_draft_cannot_be_reversed() {
        let err = ensure_correctable(false, "generated")
            .expect_err("a withheld draft has not been booked and cannot be credited");
        assert_eq!(err.code(), "NOT_YET_ISSUED");
        assert_eq!(err.status(), axum::http::StatusCode::CONFLICT);
        // The remedy is named, because the caller has two and they differ.
        let msg = err.to_string();
        assert!(
            msg.contains("calculate") && msg.contains("release"),
            "{msg}"
        );
    }

    /// Everything that has been issued may be reversed, whatever happened to it
    /// afterwards — a paid or disputed invoice is exactly what a Storno is for.
    #[test]
    fn an_issued_record_may_be_reversed_in_any_later_state() {
        for outcome in ["dispatched", "paid", "partial", "disputed"] {
            assert!(
                ensure_correctable(false, outcome).is_ok(),
                "{outcome} is an issued document",
            );
        }
    }

    /// A Storno of a Storno, and a second Storno of one original.
    #[test]
    fn a_correction_and_a_cancelled_record_are_still_refused() {
        assert_eq!(
            ensure_correctable(true, "dispatched")
                .expect_err("no Storno of a Storno")
                .code(),
            "CANNOT_CORRECT_A_CORRECTION",
        );
        assert_eq!(
            ensure_correctable(false, "cancelled")
                .expect_err("already reversed")
                .code(),
            "ALREADY_CANCELLED",
        );
    }
}

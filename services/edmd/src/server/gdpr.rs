//! GDPR Art. 17 erasure — hot-tier marking and cold-tier archive erasure.

#[allow(unused_imports)]
use super::*;

// ── GDPR Art. 17 — cold-tier erasure ──────────────────────────────────────────

/// `POST /api/v1/gdpr/erasure/{malo_id}/archive-plan`
///
/// Plan the physical deletion of an erased MaLo's rows from the Iceberg cold
/// tier, and record the affected data files.
///
/// Read-time exclusion already hides these rows from every query. This is about
/// the bytes still on disk, which Art. 17 also reaches.
///
/// iceberg-rust 0.9.1 exposes only `fast_append` on a transaction — no public
/// API removes or rewrites data files — so the rewrite itself is run by an
/// external engine (Spark, Trino) against the returned list. Recording it turns
/// `archive_deletion_pending` from a flag that is never cleared into an
/// obligation with a defined discharge.
///
/// **Cedar action**: `write-gdpr-erasure`
pub(crate) async fn plan_gdpr_archive_erasure(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "write-gdpr-erasure", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let Some(ref olap) = state.olap_engine else {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "archival is not enabled; there is no cold tier to erase from",
            })),
        )
            .into_response();
    };

    // The erasure must already be on record: planning a rewrite for a MaLo
    // nobody asked to erase would delete lawfully-held data.
    let deletion_id: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM gdpr_deletions WHERE malo_id = $1 AND tenant = $2")
            .bind(&malo_id)
            .bind(resource_tenant)
            .fetch_optional(state.repo.pool())
            .await
            .ok()
            .flatten();

    let Some(deletion_id) = deletion_id else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "no erasure request on record for this MaLo",
                "hint": "DELETE /api/v1/gdpr/erasure/{malo_id} first",
            })),
        )
            .into_response();
    };

    let files = match olap.plan_erasure_files(&malo_id).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!(malo_id = %malo_id, error = %e, "edmd: erasure planning failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    for f in &files {
        let _ = sqlx::query(
            r"INSERT INTO gdpr_archive_files
                  (deletion_id, file_path, record_count, file_size_bytes, tenant)
              VALUES ($1,$2,$3,$4,$5)
              ON CONFLICT (deletion_id, file_path) DO NOTHING",
        )
        .bind(deletion_id)
        .bind(&f.file_path)
        .bind(f.record_count.map(|c| i64::try_from(c).unwrap_or(i64::MAX)))
        .bind(i64::try_from(f.file_size_bytes).unwrap_or(i64::MAX))
        .bind(resource_tenant)
        .execute(state.repo.pool())
        .await;
    }

    // No files means nothing of this MaLo reached the cold tier, so the
    // obligation is already discharged there.
    if files.is_empty() {
        let _ = sqlx::query(
            "UPDATE gdpr_deletions
                SET archive_deletion_pending = false, archive_deletion_completed_at = now()
              WHERE id = $1",
        )
        .bind(deletion_id)
        .execute(state.repo.pool())
        .await;
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "malo_id":      malo_id,
            "deletion_id":  deletion_id.to_string(),
            "files":        files,
            "file_count":   files.len(),
            "pending":      !files.is_empty(),
            "next_step": if files.is_empty() {
                "nothing of this MaLo is in the cold tier — the obligation is discharged"
            } else {
                "run the rewrite with an external engine, then POST .../archive-complete"
            },
            "legal_basis":  "DSGVO Art. 17",
        })),
    )
        .into_response()
}

/// `POST /api/v1/gdpr/erasure/{malo_id}/archive-complete`
///
/// Record that the cold-tier rewrite has been carried out, discharging the
/// Art. 17 obligation for the archive.
///
/// **Cedar action**: `write-gdpr-erasure`
pub(crate) async fn complete_gdpr_archive_erasure(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "write-gdpr-erasure", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let res = sqlx::query(
        r"WITH d AS (
              UPDATE gdpr_deletions
                 SET archive_deletion_pending = false,
                     archive_deletion_completed_at = now()
               WHERE malo_id = $1 AND tenant = $2
               RETURNING id
          )
          UPDATE gdpr_archive_files f
             SET rewritten_at = now()
            FROM d
           WHERE f.deletion_id = d.id AND f.rewritten_at IS NULL",
    )
    .bind(&malo_id)
    .bind(resource_tenant)
    .execute(state.repo.pool())
    .await;

    match res {
        Ok(r) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "malo_id":        malo_id,
                "files_marked":   r.rows_affected(),
                "archive_pending": false,
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ── F-17: GDPR §17 DSGVO right to erasure ────────────────────────────────────

/// `DELETE /api/v1/gdpr/erasure/{malo_id}`
///
/// Initiates GDPR Art. 17 right-to-erasure for a MaLo.
///
/// ## What this does
///
/// 1. Inserts a row in `gdpr_deletions` (idempotent on `malo_id + tenant`).
/// 2. **Soft-deletes** all `meter_reads` rows for this MaLo by marking them
///    `quality = 'FAULTY'` and replacing `quantity_kwh` with `'0'`
///    (§ 60 Abs. 6 MsbG: audit trail must be preserved — rows are not physically deleted).
/// 3. Deletes `meter_billing_periods` rows (no audit trail obligation).
/// 4. Deletes `quality_assessments` rows.
/// 5. Hard deletion of Iceberg Parquet data must be done by the operator
///    via the archive rewrite pipeline (out-of-band; noted in `gdpr_deletions`).
///
/// ## Regulatory basis
///
/// DSGVO Art. 17 right to erasure. § 60 Abs. 6 MsbG (3-year audit trail) applies
/// to *billing-relevant* data — once anonymized, the obligation is satisfied.
#[derive(serde::Deserialize)]
pub(crate) struct GdprErasureRequest {
    /// Human-readable reason for erasure (required for audit trail).
    reason: String,
    /// Operator identity who authorized the erasure.
    authorized_by: String,
}

pub(crate) async fn post_gdpr_erasure(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Json(req): Json<GdprErasureRequest>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    // Erasure is irreversible and destroys billing history, so it is gated by
    // its own action rather than by the general write permission every ingest
    // client already holds.
    if let Err(e) = enforcer.check(&claims.principal(), "write-gdpr-erasure", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let pool = state.repo.pool();

    // Every step runs in one transaction. An Art. 17 erasure either completed
    // or it did not: a partial erasure reported as success closes out the
    // request while personal data remains, and the caller has no way to tell
    // that apart from a MaLo that legitimately held no readings.
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(malo_id = %malo_id, error = %e, "edmd: GDPR erasure could not begin");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    /// Abort the erasure, reporting which step failed.
    macro_rules! erasure_step {
        ($step:literal, $expr:expr) => {
            match $expr {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!(
                        malo_id = %malo_id, step = $step, error = %e,
                        "edmd: GDPR Art. 17 erasure failed — rolled back"
                    );
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": e.to_string(),
                            "failed_step": $step,
                            "status": "not_erased",
                        })),
                    )
                        .into_response();
                }
            }
        };
    }

    // 1. Record the erasure request (idempotent).
    erasure_step!(
        "record_request",
        sqlx::query(
            r"INSERT INTO gdpr_deletions
                  (malo_id, tenant, reason, authorized_by, requested_at, archive_deletion_pending)
              VALUES ($1, $2, $3, $4, now(), true)
              ON CONFLICT (malo_id, tenant) DO UPDATE
                  SET reason                  = EXCLUDED.reason,
                      authorized_by           = EXCLUDED.authorized_by,
                      requested_at            = now(),
                      archive_deletion_pending = true",
        )
        .bind(&malo_id)
        .bind(resource_tenant)
        .bind(&req.reason)
        .bind(&req.authorized_by)
        .execute(&mut *tx)
        .await
    );

    // 2. Anonymize meter_reads: zero the value, mark Faulty, preserve row for audit.
    // `archived = false` requeues the row for re-export, so the anonymised
    // version replaces the personal data already sitting in the cold tier.
    let anonymized = erasure_step!(
        "anonymize_reads",
        sqlx::query(
            r"UPDATE meter_reads
              SET quantity_kwh = '0',
                  quality      = 'FAULTY',
                  source       = 'GDPR_ERASURE',
                  push_session = NULL,
                  quality_warnings = NULL,
                  sender_mp_id = NULL,
                  archived     = false
              WHERE malo_id = $1 AND tenant = $2",
        )
        .bind(&malo_id)
        .bind(resource_tenant)
        .execute(&mut *tx)
        .await
    );
    let anonymized_count = anonymized.rows_affected();

    // 3. Delete billing period aggregates (no audit trail required).
    // A MaLo-ID is not unique across tenants, so an erasure request scoped to
    // one tenant must not reach another tenant's aggregates for the same ID.
    erasure_step!(
        "delete_billing_periods",
        sqlx::query("DELETE FROM meter_billing_periods WHERE malo_id = $1 AND tenant = $2")
            .bind(&malo_id)
            .bind(resource_tenant)
            .execute(&mut *tx)
            .await
    );

    // 4. Delete quality assessments.
    erasure_step!(
        "delete_quality_assessments",
        sqlx::query("DELETE FROM quality_assessments WHERE malo_id = $1 AND tenant = $2")
            .bind(&malo_id)
            .bind(resource_tenant)
            .execute(&mut *tx)
            .await
    );

    // 5. Delete substitute value log.
    erasure_step!(
        "delete_substitute_log",
        sqlx::query("DELETE FROM substitute_value_log WHERE malo_id = $1 AND tenant = $2")
            .bind(&malo_id)
            .bind(resource_tenant)
            .execute(&mut *tx)
            .await
    );

    erasure_step!("commit", tx.commit().await);

    tracing::info!(
        malo_id,
        anonymized_count,
        authorized_by = %req.authorized_by,
        "edmd: GDPR Art. 17 erasure completed for MaLo (hot storage anonymized)"
    );

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "malo_id":           malo_id,
            "status":            "anonymized",
            "anonymized_reads":  anonymized_count,
            "archive_pending":   true,
            "legal_basis":       "DSGVO Art. 17 right to erasure",
            "audit_note": "meter_reads rows anonymized (quantity=0, quality=FAULTY) — \
                           § 60 Abs. 6 MsbG audit trail row structure preserved. \
                           Iceberg Parquet deletion is scheduled via archive rewrite pipeline.",
        })),
    )
        .into_response()
}

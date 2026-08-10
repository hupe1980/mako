//! GDPR Art. 17 right to erasure.
//!
//! `meter_reads` lives in meterstore's append-only tiered store (hot PostgreSQL +
//! cold Iceberg), which cannot rewrite settled history in place. Erasure therefore
//! works by destroying the **subject mapping** (pseudonymisation, § 12.4 of the
//! meterstore model): each MaLo is enrolled as an erasure subject at ingest, and
//! Art. 17 deletes that mapping — the readings survive in both tiers but their link
//! to the erased MaLo is gone, so they are unattributable everywhere at once, with
//! no external Parquet-rewrite step to schedule.
//!
//! edmd's own derived tables (billing-period cache, quality assessments,
//! substitute-value log) carry no audit obligation and are deleted outright — in
//! the **same transaction** as the mapping erasure, so an Art. 17 request either
//! completes whole or not at all. A partial erasure reported as success would close
//! the request while personal data survived, indistinguishable from a MaLo that
//! legitimately held nothing.

#[allow(unused_imports)]
use super::*;

/// `DELETE /api/v1/gdpr/erasure/{malo_id}`
///
/// Executes GDPR Art. 17 right-to-erasure for a MaLo.
///
/// ## What this does (one transaction)
///
/// 1. Records the erasure request in `gdpr_deletions` (idempotent on
///    `malo_id + tenant`).
/// 2. Destroys the MaLo's subject mapping in meterstore's registry
///    ([`SubjectRegistry::erase_in`]) — the readings in both tiers become
///    unattributable. Skipped when the MaLo has no mapping (never stored, or
///    already erased), which is recorded rather than treated as an error.
/// 3. Deletes the derived `meter_billing_periods`, `quality_assessments` and
///    `substitute_value_log` rows for the MaLo, tenant-scoped.
///
/// ## Regulatory basis
///
/// DSGVO Art. 17 — and § 60 Abs. 6 MsbG, which is a *deletion* duty, not a
/// retention one: personal Messwerte must be deleted or anonymized (§ 52
/// Abs. 3 Satz 2 MsbG) at latest three years after the end of the calendar
/// year they were collected in. Destroying the subject mapping is exactly that
/// anonymization: the values remain for § 147 AO reconciliation (billed data
/// is a Buchungsbeleg, 8 years) but no longer identify anyone.
///
/// **Cedar action**: `write-gdpr-erasure` — erasure is irreversible, so it is
/// gated by its own action rather than the general write permission.
///
/// [`SubjectRegistry::erase_in`]: meterstore::SubjectRegistry::erase_in
#[derive(serde::Deserialize)]
pub(crate) struct GdprErasureRequest {
    /// Human-readable reason for erasure (required for the audit trail).
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
    if let Err(e) = enforcer.check(&claims.principal(), "write-gdpr-erasure", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    if req.reason.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": "an erasure reason is required for the audit trail" }),
            ),
        )
            .into_response();
    }

    let store = state.repo.store();
    let pool = state.repo.pool();

    // Resolve the subject mapping first (a plain read). `None` means the MaLo was
    // never stored or was already erased — there is nothing to unlink, but the
    // request is still recorded so a repeat stays auditable.
    let natural_id = crate::store::subject_natural_id(resource_tenant, &malo_id);
    let subject = match store.subject_registry() {
        Some(reg) => match reg.lookup(&natural_id).await {
            Ok(subject) => subject,
            Err(e) => {
                tracing::error!(malo_id = %malo_id, error = %e, "edmd: GDPR subject lookup failed");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response();
            }
        },
        None => None,
    };

    // Every step runs in one transaction: the mapping erasure and the derived-row
    // deletes must commit together or roll back together.
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

    /// Abort the erasure, naming the step that failed.
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
            r"INSERT INTO gdpr_deletions (malo_id, tenant, reason, authorized_by, requested_at)
              VALUES ($1, $2, $3, $4, now())
              ON CONFLICT (malo_id, tenant) DO UPDATE
                  SET reason        = EXCLUDED.reason,
                      authorized_by = EXCLUDED.authorized_by,
                      requested_at  = now()",
        )
        .bind(&malo_id)
        .bind(resource_tenant)
        .bind(&req.reason)
        .bind(&req.authorized_by)
        .execute(&mut *tx)
        .await
    );

    // 2. Destroy the subject linkage in the same transaction (pseudonymisation).
    //    `FOR UPDATE` inside `erase_in` holds the mapping row so a concurrent
    //    re-registration cannot interleave.
    let subject_unlinked = if let (Some(reg), Some(subject)) = (store.subject_registry(), &subject)
    {
        erasure_step!(
            "erase_subject",
            reg.erase_in(
                &mut tx,
                subject,
                &req.reason,
                &req.authorized_by,
                OffsetDateTime::now_utc(),
            )
            .await
        );
        true
    } else {
        false
    };

    // 3. Delete derived aggregates (no audit obligation). A MaLo-ID is not unique
    //    across tenants, so every delete is tenant-scoped.
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

    // 5. Delete the substitute-value log.
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
        subject_unlinked,
        authorized_by = %req.authorized_by,
        "edmd: GDPR Art. 17 erasure completed"
    );

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "malo_id":          malo_id,
            "status":           "erased",
            "subject_unlinked": subject_unlinked,
            "mechanism":        "meterstore subject-registry pseudonymisation (append-only tiers)",
            "legal_basis":      "DSGVO Art. 17 right to erasure",
            "audit_note": "Subject mapping destroyed — readings survive in both tiers for \
                           § 60 Abs. 6 MsbG reconciliation but no longer identify the MaLo. \
                           Derived aggregates deleted.",
        })),
    )
        .into_response()
}

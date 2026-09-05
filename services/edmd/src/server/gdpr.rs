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
///    ([`SubjectRegistry::erase_in`]) — the intervals, the ESA Typ-2 values and
///    the Zählerstandsgang become unattributable in both tiers at once. One
///    registry spans the catalog's tables, so this is one destruction rather
///    than three. Skipped when the MaLo has no mapping (never stored, or already
///    erased), which is recorded rather than treated as an error.
/// 3. Rewrites `malo_id` to the (now unmapped) subject reference in every table
///    whose rows are Buchungsbelege — `meter_read_corrections`,
///    `substitute_value_log`, `meter_data_receipts`, `ablese_auftraege`,
///    `gas_quality_data` — and deletes the derived, operational and device
///    tables outright. Tenant-scoped throughout.
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

    // 3. Every other table that keys rows on this MaLo. Unlinking the reading
    //    store alone left the MaLo-ID — and, in four of these, register readings
    //    and corrected kWh values beside it — in plain text, so the "erased"
    //    subject stayed identifiable in edmd\'s own database. Each table is
    //    handled by what it is:
    //
    //    - **Pseudonymised** where the row is a Buchungsbeleg. § 147 Abs. 1 AO
    //      requires it kept and Art. 17 Abs. 3 lit. b DSGVO exempts exactly that;
    //      rewriting `malo_id` to the (now unmapped) subject reference satisfies
    //      both, the same way the readings themselves are handled.
    //    - **Deleted** where the row is derived, operational, or device
    //      administration, and carries no retention duty of its own.
    //
    //    A MaLo-ID is not unique across tenants, so every statement is
    //    tenant-scoped.
    let pseudonym = subject.as_ref().map(|s| s.as_str().to_owned());

    /// Rewrite `malo_id` to the pseudonym, or delete the rows when the MaLo
    /// never had a mapping (nothing to keep them linked to).
    macro_rules! pseudonymise {
        ($step:literal, $table:literal) => {
            match pseudonym.as_deref() {
                Some(p) => erasure_step!(
                    $step,
                    sqlx::query(concat!(
                        "UPDATE ",
                        $table,
                        " SET malo_id = $1 WHERE malo_id = $2 AND tenant = $3"
                    ))
                    .bind(p)
                    .bind(&malo_id)
                    .bind(resource_tenant)
                    .execute(&mut *tx)
                    .await
                ),
                None => erasure_step!(
                    $step,
                    sqlx::query(concat!(
                        "DELETE FROM ",
                        $table,
                        " WHERE malo_id = $1 AND tenant = $2"
                    ))
                    .bind(&malo_id)
                    .bind(resource_tenant)
                    .execute(&mut *tx)
                    .await
                ),
            }
        };
    }

    macro_rules! purge {
        ($step:literal, $table:literal) => {
            erasure_step!(
                $step,
                sqlx::query(concat!(
                    "DELETE FROM ",
                    $table,
                    " WHERE malo_id = $1 AND tenant = $2"
                ))
                .bind(&malo_id)
                .bind(resource_tenant)
                .execute(&mut *tx)
                .await
            )
        };
    }

    // Buchungsbeleg-bearing: the values must survive, the identity must not.
    //
    // The Zählerstandsgang itself is **not** here. It is a meterstore point
    // table with its own `subject_ref`, so destroying the subject mapping
    // unlinks it in both tiers exactly as it unlinks the intervals — the same
    // mechanism, not a second one. `zsg_conversion_log` stays: it says why a
    // given quarter-hour has no measured value, which is the other half of a
    // § 60 Abs. 1 substitution's justification, and it is an edmd audit table
    // rather than a measurement.
    pseudonymise!("pseudonymise_zsg_log", "zsg_conversion_log");
    pseudonymise!("pseudonymise_corrections", "meter_read_corrections");
    pseudonymise!("pseudonymise_substitute_log", "substitute_value_log");
    pseudonymise!("pseudonymise_receipts", "meter_data_receipts");
    pseudonymise!("pseudonymise_reading_orders", "ablese_auftraege");
    pseudonymise!("pseudonymise_gas_quality", "gas_quality_data");

    // Derived, operational, or device administration — no retention duty.
    purge!("delete_billing_periods", "meter_billing_periods");
    purge!("delete_quality_assessments", "quality_assessments");
    purge!("delete_confirmations", "estimated_read_confirmations");
    purge!("delete_push_sessions", "direct_push_sessions");
    purge!("delete_smgw_sessions", "smgw_sessions");
    purge!("delete_cls_compliance_issues", "cls_compliance_issues");
    purge!("delete_delivery_surveillance", "delivery_surveillance");
    purge!("delete_cert_expiry_alerts", "smgw_cert_expiry_alerts");

    // Virtual meter configuration names MaLos in two places and neither is a
    // `malo_id` column, so it survived an erasure that covered every other table:
    // `virtual_malo_id` is the derived point's own ID, and `rule_json` carries the
    // **source** MaLo-IDs of the aggregation. A community member erased under
    // Art. 17 stayed named, in clear text, inside the § 42b rule of every virtual
    // meter that drew on their meter.
    //
    // Configuration is derived and operational — no retention duty — so both go.
    // Deleting a rule that referenced the subject is also the only coherent
    // outcome: its readings are unattributable after the mapping is destroyed, so
    // the virtual meter can no longer be computed either way.
    //
    // The source match is `jsonb_path_exists` with the ID as a **bound variable**,
    // not a `LIKE '%…%'` over the serialised JSON: the rule variants nest their
    // IDs under different keys (`subtract_malo_ids`, `generation_malo_id`,
    // `plant_melo_id`), so the recursive wildcard is what makes one statement
    // cover all of them — while still comparing whole values, so an 11-digit ID
    // cannot match as a substring of some other field and delete a stranger's
    // community.
    erasure_step!(
        "delete_virtual_meter_configs",
        sqlx::query(
            r"DELETE FROM virtual_meter_configs
               WHERE tenant = $2
                 AND (virtual_malo_id = $1
                      OR jsonb_path_exists(
                             rule_json,
                             '$.** ? (@ == $mid)',
                             jsonb_build_object('mid', $1::text)
                         ))",
        )
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
            "legal_basis":      "DSGVO Art. 17; Art. 17 Abs. 3 lit. b for the retained \
                                 Buchungsbelege (§ 147 Abs. 1 AO)",
            "pseudonymised": [
                "zsg_conversion_log", "meter_read_corrections",
                "substitute_value_log", "meter_data_receipts", "ablese_auftraege",
                "gas_quality_data",
            ],
            "deleted": [
                "meter_billing_periods", "quality_assessments",
                "estimated_read_confirmations", "direct_push_sessions",
                "smgw_sessions", "cls_compliance_issues", "delivery_surveillance",
                "smgw_cert_expiry_alerts", "virtual_meter_configs",
            ],
            "audit_note": "Subject mapping destroyed — readings survive in both tiers, in the \
                           authoritative store and the ESA Typ-2 one alike, and the § 147 AO \
                           trail survives in edmd, but none of them identifies the MaLo. \
                           Derived and operational rows deleted outright.",
        })),
    )
        .into_response()
}

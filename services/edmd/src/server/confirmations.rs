//! § 60 Abs. 2 MsbG estimated-reading confirmation obligations.

#[allow(unused_imports)]
use super::*;

// ── § 60 Abs. 2 MsbG confirmations ────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub(crate) struct ConfirmationListParams {
    /// Filter: OFFEN | BESTAETIGT | UEBERFAELLIG. Default: all open kinds.
    status: Option<String>,
    limit: Option<i64>,
}

/// `GET /api/v1/confirmations?status=&limit=`
///
/// Open/overdue obligations to replace estimated or substituted intervals
/// with plausibilised real values (§ 60 Abs. 2 MsbG). Resolution happens
/// automatically on ingest of a MEASURED/CORRECTED value for the same slot.
pub(crate) async fn list_confirmations(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Query(params): Query<ConfirmationListParams>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(
        &claims.principal(),
        "read-timeseries",
        state.tenant.as_str(),
    ) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    let limit = params.limit.unwrap_or(200).clamp(1, 2000);
    // An unrecognised status is refused rather than returning an empty list: an
    // operator filtering on a typo would otherwise read "no open obligations"
    // off a § 60 Abs. 2 queue that is not empty.
    const STATUSES: [&str; 3] = ["OFFEN", "BESTAETIGT", "UEBERFAELLIG"];
    let status = params.status.as_deref().map(str::trim);
    if let Some(s) = status.filter(|s| !STATUSES.contains(s)) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!("unknown status `{s}`"),
                "expected": STATUSES,
            })),
        )
            .into_response();
    }
    let rows = sqlx::query(
        // The parentheses are load-bearing: `AND` binds tighter than `OR`, so
        // without them the default branch and the filter branch read as one
        // condition.
        r"SELECT malo_id, dtm_from, dtm_to, obis_code_norm, quality,
                 created_at, status, resolved_at, resolved_by
          FROM estimated_read_confirmations
          WHERE tenant = $1
            AND (($2::text IS NULL AND status IN ('OFFEN','UEBERFAELLIG'))
                 OR status = $2)
          ORDER BY created_at ASC
          LIMIT $3",
    )
    .bind(&state.tenant)
    .bind(status)
    .bind(limit)
    .fetch_all(state.repo.pool())
    .await;

    match rows {
        Ok(rows) => {
            use sqlx::Row as _;
            use time::format_description::well_known::Rfc3339;
            let items: Vec<serde_json::Value> = rows
                .iter()
                .map(|r| {
                    let fmt = |c: &str| {
                        r.try_get::<OffsetDateTime, _>(c)
                            .ok()
                            .and_then(|t| t.format(&Rfc3339).ok())
                    };
                    serde_json::json!({
                        "malo_id": r.try_get::<String, _>("malo_id").unwrap_or_default(),
                        "dtm_from": fmt("dtm_from"),
                        "dtm_to": fmt("dtm_to"),
                        "obis_code_norm": r.try_get::<String, _>("obis_code_norm").unwrap_or_default(),
                        "quality": r.try_get::<String, _>("quality").unwrap_or_default(),
                        "created_at": fmt("created_at"),
                        "status": r.try_get::<String, _>("status").unwrap_or_default(),
                        "resolved_at": fmt("resolved_at"),
                        "resolved_by": r.try_get::<Option<String>, _>("resolved_by").unwrap_or(None),
                    })
                })
                .collect();
            Json(serde_json::json!({
                "count": items.len(),
                "confirmations": items,
                "legal_basis": "§ 60 Abs. 2 MsbG (Plausibilisierung und Ersatzwertbildung)",
            }))
            .into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "edmd: confirmations query failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

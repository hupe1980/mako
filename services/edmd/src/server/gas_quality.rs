//! Gasbeschaffenheitsdaten endpoint (PID 13007).

#[allow(unused_imports)]
use super::*;

// ── Gas quality endpoint ────────────────────────────────────────────────

/// `GET /api/v1/gas-quality/{malo_id}`
///
/// Returns Gasbeschaffenheitsdaten (Brennwert + Zustandszahl) received via PID 13007.
/// Used for Gas m³ → kWh_Hs conversion per §25 Nr. 4 MessEV / DVGW G 685.
pub(crate) async fn get_gas_quality(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<SimpleTimeParams>,
) -> impl IntoResponse {
    use time::format_description::well_known::Rfc3339;

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
    let from = params
        .from
        .as_deref()
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok().map(|t| t.date()))
        .unwrap_or(time::Date::MIN);
    let to = params
        .to
        .as_deref()
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok().map(|t| t.date()))
        .unwrap_or(time::Date::MAX);

    match sqlx::query(
        "SELECT period_from, period_to, brennwert_kwh_per_m3, zustandszahl, pid, received_at
           FROM gas_quality_data
          WHERE malo_id = $1 AND period_from >= $2 AND period_to <= $3
            AND tenant = $4
          ORDER BY period_from DESC LIMIT 50",
    )
    .bind(&malo_id)
    .bind(from)
    .bind(to)
    .bind(state.tenant.as_str())
    .fetch_all(state.repo.pool())
    .await
    {
        Ok(rows) => {
            use sqlx::Row;
            let records: Vec<serde_json::Value> = rows.iter().map(|r| serde_json::json!({
                "period_from": r.try_get::<time::Date, _>("period_from").ok().map(|d| d.to_string()),
                "period_to": r.try_get::<time::Date, _>("period_to").ok().map(|d| d.to_string()),
                "brennwert_kwh_per_m3": r.try_get::<String, _>("brennwert_kwh_per_m3").unwrap_or_default(),
                "zustandszahl": r.try_get::<String, _>("zustandszahl").unwrap_or_default(),
                "pid": r.try_get::<i32, _>("pid").unwrap_or(13007),
                "received_at": r.try_get::<OffsetDateTime, _>("received_at").ok().map(|t| t.to_string()),
                "legal_basis": "§25 Nr. 4 MessEV / DVGW G 685",
            })).collect();
            if records.is_empty() {
                (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": "no gas quality data for this MaLo in requested period"
                    })),
                )
                    .into_response()
            } else {
                Json(serde_json::json!({
                    "malo_id": malo_id,
                    "count": records.len(),
                    "gas_quality": records,
                }))
                .into_response()
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: get_gas_quality failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

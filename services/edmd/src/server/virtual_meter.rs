//! Virtual meter endpoints — derived time series from `AggregationRule`.

#[allow(unused_imports)]
use super::*;

// ── Virtual meter endpoints ──────────────────────────────────────────────

/// `GET /api/v1/virtual/{virtual_malo_id}/lastgang`
///
/// Computes the virtual meter time series by fetching all source MaLo time
/// series and applying the stored `AggregationRule`. The result is NOT stored
/// in `meter_reads` — it is computed on demand.
///
/// Use `?from=` / `?to=` (RFC3339 UTC) to bound the query window.
pub(crate) async fn get_virtual_lastgang(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(virtual_malo_id): Path<String>,
    Query(params): Query<SimpleTimeParams>,
) -> impl IntoResponse {
    use metering::{AggregationRule, compute_virtual_meter};
    use std::collections::HashMap;
    use time::format_description::well_known::Rfc3339;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-timeseries", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // Load virtual meter config from DB
    let config_row = match sqlx::query(
        "SELECT rule_type, rule_json FROM virtual_meter_configs WHERE virtual_malo_id = $1 AND tenant = $2 LIMIT 1"
    )
    .bind(&virtual_malo_id)
    .bind(&state.tenant)
    .fetch_optional(state.repo.pool())
    .await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": format!("virtual meter {virtual_malo_id:?} not found")
        }))).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, virtual_malo_id, "edmd: virtual meter config query failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let rule_json: serde_json::Value = match sqlx::Row::try_get(&config_row, "rule_json") {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "edmd: failed to decode virtual meter rule_json");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let rule: AggregationRule = match serde_json::from_value(rule_json) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "edmd: failed to deserialise AggregationRule");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("invalid rule_json: {e}")
                })),
            )
                .into_response();
        }
    };

    let from = params
        .from
        .as_deref()
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        .unwrap_or(OffsetDateTime::UNIX_EPOCH);
    let to = params
        .to
        .as_deref()
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        .unwrap_or_else(OffsetDateTime::now_utc);

    // Fetch source series for all referenced MaLos
    let mut sources: HashMap<String, Vec<metering::MeterInterval>> = HashMap::new();
    for malo_id in rule.source_malo_ids() {
        let q = TimeSeriesQuery {
            malo_id: malo_id.to_owned(),
            from,
            to,
            sparte: None,
            tenant: state.tenant.clone(),
        };
        match state.repo.query(&q).await {
            Ok(reads) => {
                let intervals: Vec<metering::MeterInterval> = reads
                    .iter()
                    .map(|r| metering::MeterInterval {
                        from: r.dtm_from,
                        to: r.dtm_to,
                        value: r.quantity_kwh,
                        quality: r.quality,
                        obis_code: r.obis_code.as_deref().and_then(|s| s.parse().ok()),
                    })
                    .collect();
                sources.insert(malo_id.to_owned(), intervals);
            }
            Err(e) => {
                tracing::warn!(error = %e, malo_id, "edmd: virtual meter source query failed");
            }
        }
    }

    match compute_virtual_meter(&rule, &sources) {
        Ok(intervals) => {
            let result: Vec<serde_json::Value> = intervals
                .iter()
                .map(|iv| {
                    serde_json::json!({
                        "from": iv.from, "to": iv.to,
                        "value": iv.value,
                        "quality": format!("{:?}", iv.quality),
                    })
                })
                .collect();
            Json(serde_json::json!({
                "virtual_malo_id": virtual_malo_id,
                "from": from, "to": to,
                "interval_count": result.len(),
                "intervals": result,
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": e.to_string()
            })),
        )
            .into_response(),
    }
}

/// `GET /api/v1/virtual` — list all virtual meter configurations for this tenant.
pub(crate) async fn list_virtual_meters(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
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
    match sqlx::query("SELECT virtual_malo_id, display_name, rule_type, legal_basis, sparte, valid_from, valid_to, created_at FROM virtual_meter_configs WHERE tenant = $1 ORDER BY virtual_malo_id")
        .bind(&state.tenant)
        .fetch_all(state.repo.pool())
        .await {
        Ok(rows) => {
            let configs: Vec<serde_json::Value> = rows.iter().map(|r| {
                use sqlx::Row;
                serde_json::json!({
                    "virtual_malo_id": r.try_get::<String, _>("virtual_malo_id").unwrap_or_default(),
                    "display_name": r.try_get::<String, _>("display_name").unwrap_or_default(),
                    "rule_type": r.try_get::<String, _>("rule_type").unwrap_or_default(),
                    "legal_basis": r.try_get::<Option<String>, _>("legal_basis").unwrap_or_default(),
                    "sparte": r.try_get::<String, _>("sparte").unwrap_or_default(),
                    "valid_from": r.try_get::<time::Date, _>("valid_from").ok().map(|d| d.to_string()),
                    "valid_to": r.try_get::<Option<time::Date>, _>("valid_to").ok().flatten().map(|d| d.to_string()),
                    "created_at": r.try_get::<OffsetDateTime, _>("created_at").ok().map(|t| t.to_string()),
                })
            }).collect();
            Json(serde_json::json!({ "virtual_meters": configs, "count": configs.len() })).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "edmd: list_virtual_meters failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/v1/virtual` — create a virtual meter configuration.
pub(crate) async fn create_virtual_meter(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(
        &claims.principal(),
        "write-meter-reads",
        state.tenant.as_str(),
    ) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    // Validate that rule_json deserialises to a known AggregationRule
    if let Err(e) = serde_json::from_value::<metering::AggregationRule>(
        body.get("rule_json")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    ) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!("invalid rule_json: {e}")
            })),
        )
            .into_response();
    }
    let virtual_malo_id = body
        .get("virtual_malo_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let display_name = body
        .get("display_name")
        .and_then(|v| v.as_str())
        .unwrap_or(virtual_malo_id);
    let rule_type = body.get("rule_type").and_then(|v| v.as_str()).unwrap_or("");
    let sparte = body
        .get("sparte")
        .and_then(|v| v.as_str())
        .unwrap_or("STROM");
    let rule_json = body
        .get("rule_json")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let legal_basis: Option<&str> = body.get("legal_basis").and_then(|v| v.as_str());

    if virtual_malo_id.is_empty() || rule_type.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "virtual_malo_id and rule_type are required"
            })),
        )
            .into_response();
    }

    match sqlx::query(
        "INSERT INTO virtual_meter_configs (virtual_malo_id, display_name, rule_type, rule_json, legal_basis, sparte, valid_from, tenant)
         VALUES ($1, $2, $3, $4, $5, $6, CURRENT_DATE, $7)
         ON CONFLICT (virtual_malo_id, tenant) DO UPDATE
            SET display_name = EXCLUDED.display_name,
                rule_type = EXCLUDED.rule_type,
                rule_json = EXCLUDED.rule_json,
                legal_basis = EXCLUDED.legal_basis,
                sparte = EXCLUDED.sparte,
                updated_at = now()
         RETURNING id"
    )
    .bind(virtual_malo_id)
    .bind(display_name)
    .bind(rule_type)
    .bind(rule_json)
    .bind(legal_basis)
    .bind(sparte)
    .bind(&state.tenant)
    .fetch_one(state.repo.pool())
    .await {
        Ok(_) => (StatusCode::CREATED, Json(serde_json::json!({
            "virtual_malo_id": virtual_malo_id, "status": "created"
        }))).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "edmd: create_virtual_meter insert failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `GET /api/v1/virtual/{virtual_malo_id}` — get one virtual meter config.
pub(crate) async fn get_virtual_meter(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(virtual_malo_id): Path<String>,
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
    match sqlx::query("SELECT virtual_malo_id, display_name, rule_type, rule_json, legal_basis, sparte, valid_from, valid_to, created_at FROM virtual_meter_configs WHERE virtual_malo_id = $1 AND tenant = $2")
        .bind(&virtual_malo_id)
        .bind(&state.tenant)
        .fetch_optional(state.repo.pool())
        .await {
        Ok(Some(r)) => {
            use sqlx::Row;
            Json(serde_json::json!({
                "virtual_malo_id": r.try_get::<String, _>("virtual_malo_id").unwrap_or_default(),
                "display_name": r.try_get::<String, _>("display_name").unwrap_or_default(),
                "rule_type": r.try_get::<String, _>("rule_type").unwrap_or_default(),
                "rule_json": r.try_get::<serde_json::Value, _>("rule_json").ok(),
                "legal_basis": r.try_get::<Option<String>, _>("legal_basis").unwrap_or_default(),
                "sparte": r.try_get::<String, _>("sparte").unwrap_or_default(),
            })).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "not found" }))).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "edmd: get_virtual_meter failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `DELETE /api/v1/virtual/{virtual_malo_id}` — remove a virtual meter configuration.
pub(crate) async fn delete_virtual_meter(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(virtual_malo_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(
        &claims.principal(),
        "write-meter-reads",
        state.tenant.as_str(),
    ) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    match sqlx::query(
        "DELETE FROM virtual_meter_configs WHERE virtual_malo_id = $1 AND tenant = $2",
    )
    .bind(&virtual_malo_id)
    .bind(&state.tenant)
    .execute(state.repo.pool())
    .await
    {
        Ok(res) if res.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "not found" })),
        )
            .into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "edmd: delete_virtual_meter failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

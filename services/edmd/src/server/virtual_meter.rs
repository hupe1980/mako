//! Virtual meter endpoints — derived time series from `AggregationRule`.

#[allow(unused_imports)]
use super::*;

// ── Virtual meter endpoints ──────────────────────────────────────────────

/// Project one source's resolved reads onto the register that matches its role
/// in the aggregation.
///
/// A MaLo/MeLo delivers *all* its OBIS registers (1.8.x Bezug, 2.8.x
/// Einspeisung, HT/NT splits), but `metering`'s virtual-meter engine indexes
/// each source by interval start alone — feeding it every register makes
/// same-slot registers collide and an arbitrary one win. So each source is put
/// through the canonical projection in [`crate::domain::register`], which is the
/// same one every aggregate uses: non-billable qualities dropped, the wrong
/// direction dropped, reactive and fault registers refused, and the total
/// register preferred over its own HT/NT decomposition.
///
/// Sharing that projection fixed a defect specific to this path: it used to keep
/// **one** register per slot, so a dual-tariff source reporting only HT and NT —
/// with no total register to prefer — had its NT consumption discarded rather
/// than added, and every derived series ran low by that amount.
pub(crate) fn source_intervals(
    reads: &[MeterRead],
    generation: bool,
) -> Vec<metering::MeterInterval> {
    let direction = if generation {
        crate::domain::EnergyDirection::Einspeisung
    } else {
        crate::domain::EnergyDirection::Bezug
    };
    crate::domain::energy_intervals(reads, direction)
}

/// The source IDs that carry generation (Einspeisung) in this rule; every other
/// source is a consumption series.
///
/// `Residual` is the one judgement call: the crate documents it as arithmetic
/// ("which series to subtract from which is the contract's to say"), and its
/// stated common application is building load minus PV, so the subtrahends are
/// read as generation.
pub(crate) fn generation_source_ids(rule: &metering::AggregationRule) -> Vec<&str> {
    use metering::AggregationRule as R;
    match rule {
        R::Sum { .. } => Vec::new(),
        R::Residual {
            subtract_malo_ids, ..
        } => subtract_malo_ids.iter().map(String::as_str).collect(),
        R::PvSelfConsumption {
            generation_malo_id, ..
        } => vec![generation_malo_id.as_str()],
        R::GgvConstantAllocation { plant_melo_id, .. }
        | R::GgvProportionalAllocation { plant_melo_id, .. } => vec![plant_melo_id.as_str()],
    }
}

/// The `rule_type` discriminator for a rule, matching the DDL\'s CHECK list.
///
/// The single source of truth is the enum variant, so the stored discriminator
/// and the stored rule can never disagree.
pub(crate) fn rule_type_of(rule: &metering::AggregationRule) -> &'static str {
    use metering::AggregationRule as R;
    match rule {
        R::Sum { .. } => "Sum",
        R::Residual { .. } => "Residual",
        R::PvSelfConsumption { .. } => "PvSelfConsumption",
        R::GgvConstantAllocation { .. } => "GgvConstantAllocation",
        R::GgvProportionalAllocation { .. } => "GgvProportionalAllocation",
    }
}

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

    let (from, to) = match read_window(params.from.as_deref(), params.to.as_deref()) {
        Ok(w) => w,
        Err(refusal) => return refusal.into_response(),
    };
    // Fetch source series for all referenced MaLos, projected per source onto
    // the register matching its role in the rule (see `source_intervals`).
    let generation_ids = generation_source_ids(&rule);
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
                let intervals = source_intervals(&reads, generation_ids.contains(&malo_id));
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
    // `rule_type` is **derived** from the rule, never taken from the caller
    // beside it. Two fields describing the same thing drift: a body claiming
    // `rule_type: "Sum"` while `rule_json` held a `GgvConstantAllocation` was
    // accepted, and then `/virtual/{id}/lastgang` (which reads `rule_json`)
    // computed a GGV allocation while `/sharing/{id}/allocation` (which filters
    // on `rule_type`) could not find the community at all.
    let rule: metering::AggregationRule = match serde_json::from_value(
        body.get("rule_json")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    ) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("invalid rule_json: {e}")
                })),
            )
                .into_response();
        }
    };
    let rule_type = rule_type_of(&rule);
    let virtual_malo_id = body
        .get("virtual_malo_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let display_name = body
        .get("display_name")
        .and_then(|v| v.as_str())
        .unwrap_or(virtual_malo_id);
    let sparte = body
        .get("sparte")
        .and_then(|v| v.as_str())
        .unwrap_or("STROM");
    let rule_json = body
        .get("rule_json")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let legal_basis: Option<&str> = body.get("legal_basis").and_then(|v| v.as_str());

    if virtual_malo_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "virtual_malo_id is required" })),
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
            "virtual_malo_id": virtual_malo_id, "rule_type": rule_type, "status": "created"
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{IngestionSource, Sparte};
    use time::macros::datetime;

    fn read(from: OffsetDateTime, value: &str, obis: &str, quality: QualityFlag) -> MeterRead {
        MeterRead {
            malo_id: "51238696012".to_owned(),
            melo_id: None,
            dtm_from: from,
            dtm_to: from + time::Duration::minutes(15),
            quantity_kwh: value.parse().expect("decimal"),
            quality,
            pid: 13025,
            sparte: Sparte::Strom,
            obis_code: Some(obis.to_owned()),
            tenant: "9910000000001".to_owned(),
            source: IngestionSource::Mscons,
            push_session: None,
            quality_warnings: None,
            sender_mp_id: None,
            allocation_version: "INITIAL".to_owned(),
            valid_from_tx: None,
            mscons_version: None,
        }
    }

    /// A MeLo carrying both directions is the ordinary prosumer case, and
    /// `metering`'s virtual-meter engine keys its sources by interval start
    /// alone — so handing it both registers made the two collide and let
    /// whichever sorted last win. A tenant's "consumption" could silently be
    /// its export series.
    #[test]
    fn a_bidirectional_melo_projects_onto_the_register_its_role_needs() {
        let slot = datetime!(2026-07-01 10:00 UTC);
        let reads = vec![
            read(slot, "5.0", "1-0:1.8.0", QualityFlag::Measured),
            read(slot, "3.0", "1-0:2.8.0", QualityFlag::Measured),
        ];

        let consumption = source_intervals(&reads, false);
        assert_eq!(consumption.len(), 1, "one slot, one value");
        assert_eq!(
            consumption[0].value.to_string(),
            "5.0",
            "a consumption source must read Bezug (1.8.x), never Einspeisung"
        );

        let generation = source_intervals(&reads, true);
        assert_eq!(generation.len(), 1);
        assert_eq!(
            generation[0].value.to_string(),
            "3.0",
            "a generation source must read Einspeisung (2.8.x), never Bezug"
        );
    }

    /// The total register (E = 0) beats its own HT/NT split, so a meter
    /// delivering both does not double-book or pick by sort order. Faulty,
    /// reactive and Fehlerregister rows never enter a derived series at all.
    #[test]
    fn same_direction_collisions_and_non_billable_rows_are_resolved() {
        let slot = datetime!(2026-07-01 10:00 UTC);
        let later = slot + time::Duration::minutes(15);
        let reads = vec![
            read(slot, "6.0", "1-0:1.8.1", QualityFlag::Measured),
            read(slot, "10.0", "1-0:1.8.0", QualityFlag::Measured),
            read(slot, "99.0", "1-0:3.8.0", QualityFlag::Measured),
            read(later, "7.0", "1-0:1.8.0", QualityFlag::Faulty),
        ];

        let out = source_intervals(&reads, false);
        assert_eq!(out.len(), 1, "the faulty slot must not contribute: {out:?}");
        assert_eq!(
            out[0].value.to_string(),
            "10.0",
            "the total register must win over its HT split, and Blindarbeit \
             must not be summed as Wirkarbeit"
        );
    }
}

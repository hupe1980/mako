//! Virtual meter endpoints — derived time series from a stored
//! [`VirtualMeterRule`](crate::domain::virtual_meter::VirtualMeterRule).

#[allow(
    unused_imports,
    reason = "used only in some feature or target combinations"
)]
use super::*;

use crate::domain::virtual_meter::VirtualMeterRule;

// ── Virtual meter endpoints ──────────────────────────────────────────────

/// One Messlokation register as a series, read from the Marktlokation its
/// readings are filed under.
///
/// A Marktlokation delivers *all* its OBIS registers (1.8.x Bezug, 2.8.x
/// Einspeisung, HT/NT splits), and a formula operand names one register by its
/// Energieflussrichtung. So the MaLo's readings are put through the canonical
/// projection in [`crate::domain::register`] — non-billable qualities dropped,
/// the other direction dropped, reactive and fault registers refused, the total
/// register preferred over its own HT/NT decomposition — the one every
/// aggregate uses.
///
/// Readings that name a **different** Messlokation are left out; readings that
/// name none are the MaLo's own and stay.
///
/// # Errors
///
/// A ready response: `503` when the read failed — a read that failed is not a
/// source that is missing — and `422` when the Messlokation has no series in
/// the window, which a formula cannot be evaluated without.
pub(crate) async fn melo_series(
    state: &HandlerState,
    malo_id: &metering::MaloId,
    melo_id: &metering::MeloId,
    direction: metering::Direction,
    from: OffsetDateTime,
    to: OffsetDateTime,
) -> Result<metering::Series, axum::response::Response> {
    let reads = state
        .repo
        .query(&TimeSeriesQuery {
            malo_id: malo_id.as_str().to_owned(),
            from,
            to,
            sparte: None,
            tenant: state.tenant.clone(),
        })
        .await
        .map_err(|e| {
            tracing::error!(error = %e, malo = %malo_id, "edmd: virtual meter source query failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": format!("source series {malo_id} could not be read: {e}"),
                })),
            )
                .into_response()
        })?;
    let reads: Vec<MeterRead> = reads
        .into_iter()
        .filter(|r| r.melo_id.as_deref().is_none_or(|m| m == melo_id.as_str()))
        .collect();
    let sparte = reads.first().map_or(EdmSparte::Strom, |r| r.sparte);
    let projected = crate::domain::energy_intervals(
        &reads,
        match direction {
            metering::Direction::Export => crate::domain::EnergyDirection::Einspeisung,
            _ => crate::domain::EnergyDirection::Bezug,
        },
    );
    crate::domain::vee::energy_series(sparte, &projected).map_err(|defect| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!(
                    "Messlokation {melo_id} ({} via MaLo {malo_id}) has no series in the window: {defect}",
                    direction.as_str()
                ),
            })),
        )
            .into_response()
    })
}

/// Load a stored rule.
async fn load_rule(
    state: &HandlerState,
    virtual_malo_id: &str,
) -> Result<VirtualMeterRule, axum::response::Response> {
    let row = sqlx::query(
        "SELECT rule_json FROM virtual_meter_configs WHERE virtual_malo_id = $1 AND tenant = $2 LIMIT 1",
    )
    .bind(virtual_malo_id)
    .bind(&state.tenant)
    .fetch_optional(state.repo.pool())
    .await
    .map_err(|e| {
        tracing::warn!(error = %e, virtual_malo_id, "edmd: virtual meter config query failed");
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    })?
    .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("virtual meter {virtual_malo_id:?} not found")
            })),
        )
            .into_response()
    })?;
    let rule_json: serde_json::Value = sqlx::Row::try_get(&row, "rule_json").map_err(|e| {
        tracing::warn!(error = %e, "edmd: failed to decode virtual meter rule_json");
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    })?;
    // A stored rule that no longer deserialises is a row edmd did not write.
    VirtualMeterRule::from_json(&rule_json).map_err(|e| {
        tracing::warn!(error = %e, "edmd: failed to deserialise VirtualMeterRule");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("invalid rule_json: {e}") })),
        )
            .into_response()
    })
}

/// `GET /api/v1/virtual/{virtual_malo_id}/lastgang`
///
/// Computes the virtual meter time series from the stored rule: a
/// Berechnungsformel is evaluated over its Messlokationen, a community share is
/// the tenant's grid draw after the community allocation. The result is NOT
/// stored in `meter_reads` — it is computed on demand.
///
/// Use `?from=` / `?to=` (RFC3339 UTC) to bound the query window.
pub(crate) async fn get_virtual_lastgang(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(virtual_malo_id): Path<String>,
    Query(params): Query<SimpleTimeParams>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-timeseries", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let rule = match load_rule(&state, &virtual_malo_id).await {
        Ok(rule) => rule,
        Err(response) => return response,
    };
    let (from, to) = match read_window(params.from.as_deref(), params.to.as_deref()) {
        Ok(w) => w,
        Err(refusal) => return refusal.into_response(),
    };

    let unprocessable = |error: String| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": error, "virtual_malo_id": virtual_malo_id })),
        )
            .into_response()
    };
    let series = match &rule {
        VirtualMeterRule::Formula { formula, sources } => {
            // Each input register read once, then evaluated in one pass.
            let mut inputs: BTreeMap<(metering::MeloId, metering::Direction), metering::Series> =
                BTreeMap::new();
            for (melo, flow) in formula.inputs() {
                let Some(malo) = sources.get(&melo) else {
                    return unprocessable(format!(
                        "the rule names no MaLo for Messlokation {melo}"
                    ));
                };
                let direction = flow.direction();
                match melo_series(&state, malo, &melo, direction, from, to).await {
                    Ok(series) => {
                        inputs.insert((melo, direction), series);
                    }
                    Err(response) => return response,
                }
            }
            match formula.eval(|melo, direction| inputs.get(&(*melo, direction))) {
                Ok(evaluation) => evaluation.series,
                Err(e) => return unprocessable(e.to_string()),
            }
        }
        VirtualMeterRule::CommunityShare { plant, tenant, .. } => {
            match community_allocation(&state, plant, from, to).await {
                Ok(community) => {
                    let draw: Vec<metering::MeterInterval> = community
                        .rows
                        .iter()
                        .filter_map(|row| {
                            let share = row.share(tenant)?;
                            metering::MeterInterval::new(
                                row.from,
                                row.to,
                                share.net_grid_draw(),
                                row.quality,
                            )
                            .ok()
                        })
                        .collect();
                    match metering::Series::new(
                        community.plant.resolution(),
                        community.plant.boundary(),
                        draw,
                    ) {
                        Ok(series) => series,
                        Err(e) => return unprocessable(e.to_string()),
                    }
                }
                Err(response) => return response,
            }
        }
    };

    let result: Vec<serde_json::Value> = series
        .iter()
        .map(|iv| {
            serde_json::json!({
                "from": iv.from(), "to": iv.to(),
                "value": iv.value(),
                "quality": crate::store::quality_to_str(iv.quality()),
            })
        })
        .collect();
    Json(serde_json::json!({
        "virtual_malo_id": virtual_malo_id,
        "rule_type": rule.kind(),
        "from": from, "to": to,
        "resolution": series.resolution().to_string(),
        "interval_count": result.len(),
        "intervals": result,
    }))
    .into_response()
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
    // beside it. Two fields describing the same thing drift: a body claiming one
    // kind while `rule_json` held another would be evaluated as one and found by
    // the community endpoint (which filters on `rule_type`) as the other.
    let rule = match VirtualMeterRule::from_json(
        body.get("rule_json").unwrap_or(&serde_json::Value::Null),
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
    if let Some(melo) = rule.unsourced() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!(
                    "rule_json reads Messlokation {melo} but names no MaLo for it in `sources`"
                )
            })),
        )
            .into_response();
    }
    let rule_type = rule.kind();
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
    // The normalised document, so the stored JSON is the one edmd reads back.
    let rule_json = match serde_json::to_value(&rule) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "edmd: could not serialise a VirtualMeterRule");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
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
         VALUES ($1, $2, $3, $4, $5, $6, heute(), $7)
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
            tenant: "9910000000002".to_owned(),
            source: IngestionSource::Mscons,
            push_session: None,
            quality_warnings: None,
            sender_mp_id: None,
            allocation_version: "INITIAL".to_owned(),
            valid_from_tx: None,
            mscons_version: None,
        }
    }

    fn bezug(reads: &[MeterRead]) -> Vec<metering::MeterInterval> {
        crate::domain::energy_intervals(reads, crate::domain::EnergyDirection::Bezug)
    }

    /// A MeLo carrying both directions is the ordinary prosumer case. An
    /// operand names one register by its Energieflussrichtung, so the source
    /// is projected onto that direction — a tenant's "consumption" must never
    /// be its export series.
    #[test]
    fn a_bidirectional_melo_projects_onto_the_register_its_role_needs() {
        let slot = datetime!(2026-07-01 10:00 UTC);
        let reads = vec![
            read(slot, "5.0", "1-0:1.8.0", QualityFlag::Measured),
            read(slot, "3.0", "1-0:2.8.0", QualityFlag::Measured),
        ];

        let consumption = bezug(&reads);
        assert_eq!(consumption.len(), 1, "one slot, one value");
        assert_eq!(
            consumption[0].value().to_string(),
            "5.0",
            "a consumption source must read Bezug (1.8.x), never Einspeisung"
        );

        let generation =
            crate::domain::energy_intervals(&reads, crate::domain::EnergyDirection::Einspeisung);
        assert_eq!(generation.len(), 1);
        assert_eq!(
            generation[0].value().to_string(),
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

        let out = bezug(&reads);
        assert_eq!(out.len(), 1, "the faulty slot must not contribute: {out:?}");
        assert_eq!(
            out[0].value().to_string(),
            "10.0",
            "the total register must win over its HT split, and Blindarbeit \
             must not be summed as Wirkarbeit"
        );
        let series = crate::domain::vee::energy_series(Sparte::Strom, &out).expect("one series");
        assert_eq!(series.len(), 1);
    }
}

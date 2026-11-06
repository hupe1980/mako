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
/// A ready response: `500` when the read failed — a read that failed is not a
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
            ApiError::internal(anyhow::anyhow!(
                "virtual meter source series {malo_id} could not be read: {e}"
            ))
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
        ApiError::unprocessable(format!(
            "Messlokation {melo_id} ({} via MaLo {malo_id}) has no series in the window: {defect}",
            direction.as_str()
        ))
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
        ApiError::internal(anyhow::Error::from(e).context(format!("virtual meter config query failed (virtual_malo_id {virtual_malo_id})"))).into_response()
    })?
    .ok_or_else(|| {
        ApiError::NotFound.into_response()
    })?;
    let rule_json: serde_json::Value = sqlx::Row::try_get(&row, "rule_json").map_err(|e| {
        ApiError::internal(
            anyhow::Error::from(e).context("failed to decode virtual meter rule_json"),
        )
        .into_response()
    })?;
    // A stored rule that no longer deserialises is a row edmd did not write.
    VirtualMeterRule::from_json(&rule_json).map_err(|e| {
        ApiError::internal(anyhow::anyhow!(
            "stored rule_json of {virtual_malo_id} does not deserialise: {e}"
        ))
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
        return ApiError::forbidden(e.to_string()).into_response();
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
        ApiError::unprocessable_with(
            error,
            serde_json::json!({ "virtual_malo_id": virtual_malo_id }),
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
        return ApiError::forbidden(e.to_string()).into_response();
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
            ApiError::internal(anyhow::Error::from(e).context("list_virtual_meters failed")).into_response()
        }
    }
}

/// The Sparte a virtual meter is filed under — the `CHECK` list on
/// `virtual_meter_configs.sparte`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VirtualMeterSparte {
    /// Electricity.
    #[default]
    Strom,
    /// Gas.
    Gas,
    /// Heat.
    Waerme,
    /// Water.
    Wasser,
}

impl VirtualMeterSparte {
    /// The column value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Strom => "STROM",
            Self::Gas => "GAS",
            Self::Waerme => "WAERME",
            Self::Wasser => "WASSER",
        }
    }
}

/// `POST /api/v1/virtual` body.
///
/// Read through `mako_service::Json`, so a key this struct does not declare is
/// a `422` naming it: a misspelt `legal_basis` would otherwise be stored as
/// absent and the community found under no regime. `rule_json` is the rule
/// itself — a [`VirtualMeterRule`], which denies unknown keys of its own.
#[derive(Debug, Deserialize)]
pub struct CreateVirtualMeterRequest {
    /// The virtual Marktlokation's own ID — the row's key. Required.
    pub virtual_malo_id: String,
    /// Label; the `virtual_malo_id` when absent.
    #[serde(default)]
    pub display_name: Option<String>,
    /// `STROM` when absent.
    #[serde(default)]
    pub sparte: VirtualMeterSparte,
    /// Statutory citation the community operates under, free text
    /// (e.g. `§42b EnWG`).
    #[serde(default)]
    pub legal_basis: Option<String>,
    /// The rule; its tag becomes `rule_type`.
    pub rule_json: VirtualMeterRule,
}

/// `POST /api/v1/virtual` — create a virtual meter configuration.
pub(crate) async fn create_virtual_meter(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Json(body): Json<CreateVirtualMeterRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(
        &claims.principal(),
        "write-meter-reads",
        state.tenant.as_str(),
    ) {
        return ApiError::forbidden(e.to_string()).into_response();
    }
    // `rule_type` is **derived** from the rule, never taken from the caller
    // beside it. Two fields describing the same thing drift: a body claiming one
    // kind while `rule_json` held another would be evaluated as one and found by
    // the community endpoint (which filters on `rule_type`) as the other.
    let rule = &body.rule_json;
    if let Some(melo) = rule.unsourced() {
        return ApiError::unprocessable(format!(
            "rule_json reads Messlokation {melo} but names no MaLo for it in `sources`"
        ))
        .into_response();
    }
    let rule_type = rule.kind();
    let virtual_malo_id = body.virtual_malo_id.as_str();
    if virtual_malo_id.is_empty() {
        return ApiError::bad_request("virtual_malo_id is required").into_response();
    }
    let display_name = body.display_name.as_deref().unwrap_or(virtual_malo_id);
    let sparte = body.sparte.as_str();
    // The normalised document, so the stored JSON is the one edmd reads back.
    let rule_json = match serde_json::to_value(rule) {
        Ok(v) => v,
        Err(e) => {
            return ApiError::internal(
                anyhow::Error::from(e).context("could not serialise a VirtualMeterRule"),
            )
            .into_response();
        }
    };
    let legal_basis: Option<&str> = body.legal_basis.as_deref();

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
            ApiError::internal(anyhow::Error::from(e).context("create_virtual_meter insert failed")).into_response()
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
        return ApiError::forbidden(e.to_string()).into_response();
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
        Ok(None) => ApiError::NotFound.into_response(),
        Err(e) => {
            ApiError::internal(anyhow::Error::from(e).context("get_virtual_meter failed")).into_response()
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
        return ApiError::forbidden(e.to_string()).into_response();
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
        Ok(_) => ApiError::NotFound.into_response(),
        Err(e) => ApiError::internal(anyhow::Error::from(e).context("delete_virtual_meter failed"))
            .into_response(),
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

    /// Reads `body` exactly as the route does: through `mako_service::Json`.
    async fn extract(body: &serde_json::Value) -> Result<CreateVirtualMeterRequest, ApiError> {
        use axum::extract::FromRequest as _;
        let request = axum::http::Request::post("/api/v1/virtual")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .expect("request");
        Json::<CreateVirtualMeterRequest>::from_request(request, &())
            .await
            .map(|Json(body)| body)
    }

    /// The documented `FORMULA` example — numbered Rechenschritte as map keys —
    /// reads through the unknown-key refusal intact.
    fn formula_body() -> serde_json::Value {
        serde_json::json!({
            "virtual_malo_id": "10001234558",
            "rule_json": { "FORMULA": {
                "formula": { "result": 3, "steps": {
                    "1": { "PRODUCT": [ { "MELO": { "melo": "DE0001234567890000000000000000001",
                                                    "direction": "Z72", "loss_trafo": null,
                                                    "loss_line": null, "split": "0.5" } } ] },
                    "2": { "SUM": [ [ "Z70", { "STEP": 1 } ],
                                    [ "Z69", { "MELO": { "melo": "DE0001234567890000000000000000002",
                                                         "direction": "Z71", "loss_trafo": null,
                                                         "loss_line": null, "split": null } } ] ] },
                    "3": { "POSITIVE": 2 } } },
                "sources": { "DE0001234567890000000000000000001": "51238696012",
                             "DE0001234567890000000000000000002": "51238696012" } } }
        })
    }

    #[tokio::test]
    async fn a_formula_request_is_read_with_its_defaults() {
        let req = extract(&formula_body()).await.expect("a valid request");
        assert_eq!(req.rule_json.kind(), "FORMULA");
        assert_eq!(req.sparte, VirtualMeterSparte::Strom);
        assert_eq!(req.display_name, None);
        assert_eq!(req.rule_json.unsourced(), None);
    }

    /// A key the request does not declare is refused, not read as absent —
    /// which is what an untyped `serde_json::Value` body did.
    #[tokio::test]
    async fn an_unknown_top_level_key_is_refused() {
        let mut body = formula_body();
        body["legal_bases"] = serde_json::json!("§42b EnWG");
        let err = extract(&body).await.expect_err("an undeclared key");
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// `rule_json` and `virtual_malo_id` are mandatory: absent, the request is
    /// refused rather than stored with a default.
    #[tokio::test]
    async fn a_missing_rule_or_id_is_refused() {
        for key in ["rule_json", "virtual_malo_id"] {
            let mut body = formula_body();
            body.as_object_mut().expect("object").remove(key);
            assert!(extract(&body).await.is_err(), "{key} absent");
        }
    }

    /// A Sparte outside the column's `CHECK` list is a `422` at the door, not a
    /// constraint violation at the insert.
    #[tokio::test]
    async fn an_unknown_sparte_is_refused() {
        let mut body = formula_body();
        body["sparte"] = serde_json::json!("FERNWAERME");
        assert!(extract(&body).await.is_err());
    }
}

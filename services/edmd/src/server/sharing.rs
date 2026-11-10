//! §42c EnWG energy sharing: VZW allocation and readiness report.

use super::*;

// ── §42b / §42c community allocation ──────────────────────────────────────────

/// A community of participants sharing one plant's generation, allocated.
pub(crate) struct CommunityAllocation {
    /// The plant's generation series.
    pub(crate) plant: metering::Series,
    /// One row per interval of the plant's series.
    pub(crate) rows: Vec<metering::allocation::community::AllocationRow>,
    /// `(virtual_malo_id, display_name, tenant MeLo)` per participant.
    pub(crate) participants: Vec<(String, String, metering::MeloId)>,
    /// The key every participant's rule carries.
    pub(crate) key: metering::allocation::community::AllocationKey,
    /// The regime the first participant names.
    pub(crate) legal_basis: Option<String>,
}

/// Allocate the community around `plant` over `[from, to)`.
///
/// The community is every `COMMUNITY_SHARE` rule naming this plant, one per
/// participant. They must agree on the key and on where the plant's readings
/// are filed: two keys for one community are two different allocations, and
/// running either would bill the participants of the other wrongly.
///
/// The arithmetic is `metering::allocation::community::allocate` whole: the
/// § 42b Abs. 5 `Pos()` cap, the residual that fed the grid and
/// `Σ allocated + residual == pool` per interval.
///
/// # Errors
///
/// A ready response: `404` for a plant no rule names, `422` for rules that
/// disagree or series that do not share a grid, `500` for a failed read.
pub(crate) async fn community_allocation(
    state: &HandlerState,
    plant: &metering::MeloId,
    from: OffsetDateTime,
    to: OffsetDateTime,
) -> Result<CommunityAllocation, axum::response::Response> {
    use crate::domain::virtual_meter::VirtualMeterRule;
    use sqlx::Row as _;

    let unprocessable = |error: String| {
        ApiError::unprocessable_with(error, serde_json::json!({ "community_id": plant.as_str() }))
            .into_response()
    };

    let rows = sqlx::query(
        r"SELECT virtual_malo_id, rule_json, display_name, legal_basis
          FROM virtual_meter_configs
          WHERE tenant = $2
            AND rule_type = 'COMMUNITY_SHARE'
            AND rule_json->'COMMUNITY_SHARE'->>'plant' = $1
          ORDER BY virtual_malo_id",
    )
    .bind(plant.as_str())
    .bind(&state.tenant)
    .fetch_all(state.repo.pool())
    .await
    .map_err(|e| {
        ApiError::internal(
            anyhow::Error::from(e).context(format!("community query failed (plant {plant})")),
        )
        .into_response()
    })?;
    if rows.is_empty() {
        return Err(ApiError::not_found(
            "no community participants are configured against this plant",
        )
        .with_fields(serde_json::json!({
            "community_id": plant.as_str(),
            "hint": "`community_id` is the shared plant's MeLo. Create one rule per \
                     participant via POST /api/v1/virtual with a COMMUNITY_SHARE \
                     rule_json naming it as `plant`.",
        }))
        .into_response());
    }

    let legal_basis: Option<String> = rows[0].try_get("legal_basis").unwrap_or(None);
    let mut key = None;
    let mut plant_malo = None;
    let mut participants = Vec::with_capacity(rows.len());
    let mut tenant_malos = Vec::with_capacity(rows.len());
    for row in &rows {
        let rule_json: serde_json::Value = row.try_get("rule_json").unwrap_or_default();
        let rule = VirtualMeterRule::from_json(&rule_json).map_err(|e| {
            // A stored rule that no longer deserialises is a config fault, and
            // silently dropping a participant under-allocates the settlement.
            ApiError::internal(anyhow::anyhow!(
                "a stored community rule of plant {plant} could not be read: {e}"
            ))
            .into_response()
        })?;
        let VirtualMeterRule::CommunityShare {
            tenant,
            key: k,
            sources,
            ..
        } = rule
        else {
            continue;
        };
        if key.as_ref().is_some_and(|held| *held != k) {
            return Err(unprocessable(
                "the participants of this plant carry different allocation keys".to_owned(),
            ));
        }
        key = Some(k);
        let (Some(p), Some(t)) = (sources.get(plant), sources.get(&tenant)) else {
            return Err(unprocessable(format!(
                "a participant's rule names no MaLo for the plant or for {tenant}"
            )));
        };
        if plant_malo.as_ref().is_some_and(|held| held != p) {
            return Err(unprocessable(
                "the participants file the plant's readings under different MaLos".to_owned(),
            ));
        }
        plant_malo = Some(*p);
        tenant_malos.push(*t);
        participants.push((
            row.try_get::<String, _>("virtual_malo_id")
                .unwrap_or_default(),
            row.try_get::<Option<String>, _>("display_name")
                .ok()
                .flatten()
                .unwrap_or_default(),
            tenant,
        ));
    }
    let (Some(key), Some(plant_malo)) = (key, plant_malo) else {
        return Err(unprocessable(
            "no participant's rule could be read as a community share".to_owned(),
        ));
    };

    let plant_series = crate::server::melo_series(
        state,
        &plant_malo,
        plant,
        metering::Direction::Export,
        from,
        to,
    )
    .await?;
    let mut consumption = Vec::with_capacity(participants.len());
    for ((_, _, tenant), malo) in participants.iter().zip(&tenant_malos) {
        consumption.push(
            crate::server::melo_series(state, malo, tenant, metering::Direction::Import, from, to)
                .await?,
        );
    }
    let inputs: Vec<(metering::MeloId, &metering::Series)> = participants
        .iter()
        .zip(&consumption)
        .map(|((_, _, tenant), series)| (*tenant, series))
        .collect();
    let mut allocated = metering::allocation::community::allocate(&plant_series, &inputs, &key)
        .map_err(|e| unprocessable(e.to_string()))?;
    multiply_first(&mut allocated, &key)
        .map_err(|at| unprocessable(format!("the community allocation overflows at {at}")))?;
    Ok(CommunityAllocation {
        plant: plant_series,
        rows: allocated,
        participants,
        key,
        legal_basis,
    })
}

/// Re-divide each interval's pool multiply-first, for the keys whose fraction
/// is a quotient.
///
/// `metering` 0.25's `community::allocate` forms the fraction first —
/// `cut(1 ÷ n)` or `cut(cᵢ ÷ Σ c)` — and multiplies the pool by it, so a
/// three-way equal split of 10 kWh credits 3.333330 kWh each where the share
/// is 3.333333. A billed share is `pool × wᵢ ÷ Σ w`, multiplied before it
/// divides and cut to [`metering::precision::ALLOCATION_DP`] places toward
/// zero; the § 42b Abs. 5 `Pos()` cap and the residual follow from it as
/// before, so `Σ allocated + residual == pool` still holds per interval.
///
/// A [`Constant`](metering::allocation::community::AllocationKey::Constant)
/// key states its fractions and divides nothing; a
/// [`Cascading`](metering::allocation::community::AllocationKey::Cascading)
/// key re-offers what a cap refused and is left as `metering` computed it.
///
/// # Errors
///
/// The interval start at which a value leaves `Decimal`'s range.
pub(crate) fn multiply_first(
    rows: &mut [metering::allocation::community::AllocationRow],
    key: &metering::allocation::community::AllocationKey,
) -> Result<(), OffsetDateTime> {
    use metering::allocation::community::AllocationKey;
    use metering::precision::ALLOCATION_DP;
    use rust_decimal::Decimal;

    let proportional = match key {
        AllocationKey::Proportional => true,
        AllocationKey::EqualShares => false,
        AllocationKey::Constant { .. } | AllocationKey::Cascading { .. } => return Ok(()),
    };
    for row in rows {
        let at = row.from;
        let pool = row.pool();
        let weights: Vec<Decimal> = row
            .shares
            .iter()
            .map(|s| {
                if proportional {
                    s.consumption.max(Decimal::ZERO)
                } else {
                    Decimal::ONE
                }
            })
            .collect();
        let total = weights
            .iter()
            .try_fold(Decimal::ZERO, |acc, w| acc.checked_add(*w))
            .ok_or(at)?;
        let mut taken = Decimal::ZERO;
        for (share, weight) in row.shares.iter_mut().zip(&weights) {
            let nominal = if total.is_zero() {
                Decimal::ZERO
            } else {
                let dividend = pool.checked_mul(*weight).ok_or(at)?;
                let mut q = dividend
                    .checked_div(total)
                    .ok_or(at)?
                    .trunc_with_scale(ALLOCATION_DP);
                // `checked_div` rounds its 28th digit; a quotient rounded up
                // across the cut is one unit too large, and one unit back
                // restores `q × Σ w ≤ pool × wᵢ`.
                if q.checked_mul(total).ok_or(at)? > dividend {
                    q -= Decimal::new(1, ALLOCATION_DP);
                }
                q
            };
            share.share = nominal;
            share.allocated = nominal.min(share.consumption.max(Decimal::ZERO));
            taken = taken.checked_add(share.allocated).ok_or(at)?;
        }
        row.residual = pool - taken;
    }
    Ok(())
}

/// The `(plant, tenant)` Messlokationen of every community whose
/// `COMMUNITY_SHARE` rule files its tenant's readings under `malo` — the
/// communities that MaLo takes part in, each once.
///
/// The match is on the tenant's own source, not on any source: the plant's
/// MaLo is named by every participant's rule and is no participant.
///
/// # Errors
///
/// A failed query, or a stored rule that no longer reads as one.
pub async fn community_participations(
    pool: &sqlx::PgPool,
    tenant: &str,
    malo: &metering::MaloId,
) -> anyhow::Result<Vec<(metering::MeloId, metering::MeloId)>> {
    use crate::domain::virtual_meter::VirtualMeterRule;
    use anyhow::Context as _;
    use sqlx::Row as _;

    let rows = sqlx::query(
        r"SELECT rule_json
          FROM virtual_meter_configs
          WHERE tenant = $2
            AND rule_type = 'COMMUNITY_SHARE'
            AND rule_json->'COMMUNITY_SHARE'->'sources'
                  ->>(rule_json->'COMMUNITY_SHARE'->>'tenant') = $1
          ORDER BY virtual_malo_id",
    )
    .bind(malo.as_str())
    .bind(tenant)
    .fetch_all(pool)
    .await
    .with_context(|| format!("participant query failed (MaLo {malo})"))?;
    let mut found: Vec<(metering::MeloId, metering::MeloId)> = Vec::new();
    for row in &rows {
        let rule_json: serde_json::Value = row.try_get("rule_json")?;
        let rule = VirtualMeterRule::from_json(&rule_json).with_context(|| {
            format!("a stored community rule sourcing MaLo {malo} could not be read")
        })?;
        if let VirtualMeterRule::CommunityShare { plant, tenant, .. } = rule
            && !found.contains(&(plant, tenant))
        {
            found.push((plant, tenant));
        }
    }
    Ok(found)
}

/// `GET /api/v1/sharing/participants/{malo_id}/allocation?from=RFC3339&to=RFC3339`
///
/// One participant's allocation, found by the Marktlokation its consumption is
/// filed under: the community is the `COMMUNITY_SHARE` rule whose tenant
/// Messlokation that MaLo sources, and the figure is the participant's sum of
/// capped interval shares from [`community_allocation`] — the quantity a
/// § 42c invoice credits.
///
/// `404` when no community names the MaLo, and `422` when two do: one invoice
/// credits one community's allocation, and picking either would credit the
/// wrong one.
pub(crate) async fn get_participant_allocation(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<LastgangParams>,
) -> impl IntoResponse {
    use rust_decimal::Decimal;

    if let Err(e) = enforcer.check(
        &claims.principal(),
        "read-timeseries",
        state.tenant.as_str(),
    ) {
        return ApiError::forbidden(e.to_string()).into_response();
    }
    let (from, to) = match read_window(params.from.as_deref(), params.to.as_deref()) {
        Ok(w) => w,
        Err(refusal) => return refusal.into_response(),
    };
    let malo: metering::MaloId = match malo_id.parse() {
        Ok(m) => m,
        Err(e) => return ApiError::bad_request(format!("malo_id: {e}")).into_response(),
    };

    let found = match community_participations(state.repo.pool(), &state.tenant, &malo).await {
        Ok(found) => found,
        Err(e) => return ApiError::internal(e).into_response(),
    };
    let (plant, tenant) = match found.as_slice() {
        [] => {
            return ApiError::not_found("no community names this MaLo as a participant")
                .with_fields(serde_json::json!({ "malo_id": malo.as_str() }))
                .into_response();
        }
        [one] => *one,
        many => {
            return ApiError::unprocessable_with(
                "the MaLo is a participant of more than one community; one invoice credits \
                 one community's allocation"
                    .to_owned(),
                serde_json::json!({
                    "malo_id": malo.as_str(),
                    "communities": many.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
                }),
            )
            .into_response();
        }
    };

    let community = match community_allocation(&state, &plant, from, to).await {
        Ok(c) => c,
        Err(response) => return response,
    };
    let mut allocated_kwh = Decimal::ZERO;
    let mut production_kwh = Decimal::ZERO;
    let mut interval_count = 0_usize;
    for row in &community.rows {
        production_kwh += row.pool();
        if let Some(share) = row.share(&tenant) {
            allocated_kwh += share.allocated;
            interval_count += 1;
        }
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "malo_id":              malo.as_str(),
            "community_id":         plant.as_str(),
            "tenant_melo_id":       tenant.as_str(),
            "key":                  serde_json::to_value(&community.key).unwrap_or_default(),
            "from":                 from,
            "to":                   to,
            "interval_count":       interval_count,
            "allocated_kwh":        allocated_kwh.to_string(),
            "total_production_kwh": production_kwh.to_string(),
        })),
    )
        .into_response()
}

/// `GET /api/v1/sharing/{community_id}/allocation?from=RFC3339&to=RFC3339`
///
/// The quarter-hour allocation for one § 42b / § 42c community: what the
/// shared plant produced in each interval, how much of it each participant
/// was allocated, and what fed the public grid.
///
/// **`community_id` is the plant's MeLo.** A `COMMUNITY_SHARE` rule is written
/// per participant and names the plant, so the community is the set of rules
/// sharing a plant.
///
/// § 42c EnWG defines **no** new MaKo process (BNetzA Mitteilung Nr. 73 vom
/// 07.07.2026, Az. BK6-06-009, endorsing the Dienstleistungsmodell), so this
/// computes the attribution on demand from stored reads and stored rules rather
/// than implementing a mandated exchange.
pub(crate) async fn get_sharing_allocation(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(community_id): Path<String>,
    Query(params): Query<LastgangParams>,
) -> impl IntoResponse {
    use rust_decimal::Decimal;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-timeseries", resource_tenant) {
        return ApiError::forbidden(e.to_string()).into_response();
    }

    let (from, to) = match read_window(params.from.as_deref(), params.to.as_deref()) {
        Ok(w) => w,
        Err(refusal) => return refusal.into_response(),
    };
    let plant: metering::MeloId = match community_id.parse() {
        Ok(m) => m,
        Err(e) => {
            return ApiError::bad_request(format!("community_id is the plant's MeLo: {e}"))
                .into_response();
        }
    };

    let community = match community_allocation(&state, &plant, from, to).await {
        Ok(c) => c,
        Err(response) => return response,
    };

    let mut production_kwh = Decimal::ZERO;
    let mut residual_kwh = Decimal::ZERO;
    for row in &community.rows {
        production_kwh += row.generation;
        residual_kwh += row.residual;
    }

    let mut allocations: Vec<serde_json::Value> = Vec::new();
    let mut allocated_total = Decimal::ZERO;
    for (virtual_malo_id, display_name, tenant) in &community.participants {
        let mut intervals: Vec<serde_json::Value> = Vec::with_capacity(community.rows.len());
        let mut participant_total = Decimal::ZERO;
        for row in &community.rows {
            let Some(share) = row.share(tenant) else {
                continue;
            };
            participant_total += share.allocated;
            intervals.push(serde_json::json!({
                "from": row.from,
                "to": row.to,
                "consumption_kwh": share.consumption.to_string(),
                "generation_kwh": row.generation.to_string(),
                // The nominal share before the cap, and whether the tenant's own
                // consumption bounded it (§ 42b Abs. 5 Satz 4).
                "share_kwh": share.share.to_string(),
                "capped": share.capped(),
                "net_grid_draw_kwh": share.net_grid_draw().to_string(),
                "allocated_kwh": share.allocated.to_string(),
                "quality": crate::store::quality_to_str(row.quality),
            }));
        }
        allocated_total += participant_total;
        allocations.push(serde_json::json!({
            "virtual_malo_id": virtual_malo_id,
            "display_name": display_name,
            "tenant_melo_id": tenant.as_str(),
            "allocated_kwh": participant_total.to_string(),
            "interval_count": intervals.len(),
            "intervals": intervals,
        }));
    }

    let production_intervals: Vec<serde_json::Value> = community
        .rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "from": row.from,
                "to": row.to,
                "total_kwh": row.generation.to_string(),
                "residual_kwh": row.residual.to_string(),
                // The real flag, not a hardcoded MEASURED: a § 42c settlement has
                // to see that a slot of "production" is an Ersatzwert.
                "quality": crate::store::quality_to_str(row.quality),
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "community_id":         community_id,
            "plant_melo_id":        plant.as_str(),
            "legal_basis":          community.legal_basis.as_deref().unwrap_or(
                "§42c EnWG · §42b Abs. 5 EnWG (Pos()-Deckel) · BNetzA Mitteilung Nr. 73"),
            "key":                  serde_json::to_value(&community.key).unwrap_or_default(),
            "from":                 from,
            "to":                   to,
            "resolution":           community.plant.resolution().to_string(),
            "participant_count":    community.participants.len(),
            "total_production_kwh": production_kwh.to_string(),
            // What the plant produced but nobody was allocated: it fed the grid.
            // § 42b Abs. 5 caps each tenant at their own consumption, so this is
            // a real remainder and not a rounding error.
            "unallocated_kwh":      residual_kwh.to_string(),
            "total_allocated_kwh":  allocated_total.to_string(),
            "production":           production_intervals,
            "participants":         allocations,
        })),
    )
        .into_response()
}

// ── §42c EnWG Energy-Sharing readiness report ─────────────────────────────────

/// Query parameters for `GET /api/v1/sharing/readiness`.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SharingReadinessParams {
    /// The Berlin calendar month to assess, `YYYY-MM`. Defaults to the last
    /// complete month.
    month: Option<String>,
    /// Comma-separated MaLo-IDs to assess. Defaults to every MaLo with readings
    /// in the month.
    malo_ids: Option<String>,
    /// Coverage threshold in percent. Defaults to
    /// [`metering::allocation::sharing::DEFAULT_COVERAGE_THRESHOLD_PCT`].
    min_coverage_pct: Option<u8>,
}

/// Per-point delivery verdict in the readiness report.
#[derive(Debug, serde::Serialize)]
pub(crate) struct SharingReadinessItem {
    malo_id: String,
    /// `DELIVERING` · `INSUFFICIENT` · `ABSENT`.
    delivery: String,
    /// The dominant register's observed grid (`PT15M`), when it has one.
    resolution: Option<String>,
    /// Billable quarter-hours of the month delivered, in percent.
    coverage_pct: Option<rust_decimal::Decimal>,
    reading_count: u64,
    /// Why the point is not delivering a conforming series.
    reasons: Vec<String>,
    /// What an operator must do next.
    required_action: String,
}

/// `GET /api/v1/sharing/readiness`
///
/// Fleet report: which delivery points are **actually** producing the
/// quarter-hour series that §42c Abs. 1 EnWG requires.
///
/// This is the delivery half of the §42c readiness question. `marktd`'s
/// `GET /api/v1/melos/{id}/sharing-eligibility` answers the capability half from
/// device master data. Read together they separate the two states an operator
/// must act on differently:
///
/// - **capable but not delivering** — the meter supports Zählerstandsgangmessung
///   but none is configured; order the configuration, not a meter.
/// - **not capable** — needs an iMSys rollout or an RLM conversion.
///
/// The verdict is `metering::allocation::sharing::assess_delivery` over one
/// Berlin calendar month: the dominant energy register as a series on its
/// observed grid, billable quarter-hours against the quarter-hours the month
/// holds (2 972, 2 976 or 2 980 across DST). A register that is not one series
/// is insufficient, with the defect as its reason.
pub(crate) async fn get_sharing_readiness(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Query(params): Query<SharingReadinessParams>,
) -> impl IntoResponse {
    use metering::allocation::sharing::{
        DEFAULT_COVERAGE_THRESHOLD_PCT, Delivery, assess_delivery,
    };
    use rust_decimal::Decimal;
    use time::format_description::well_known::Rfc3339;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-timeseries", resource_tenant) {
        return ApiError::forbidden(e.to_string()).into_response();
    }
    let unprocessable = |error: String| ApiError::unprocessable(error).into_response();

    let now = OffsetDateTime::now_utc();
    let (year, month) = match params.month.as_deref() {
        Some(raw) => {
            let parsed = raw.split_once('-').and_then(|(y, m)| {
                Some((
                    y.parse::<i32>().ok()?,
                    time::Month::try_from(m.parse::<u8>().ok()?).ok()?,
                ))
            });
            match parsed {
                Some(ym) => ym,
                None => return unprocessable(format!("month {raw:?} is not YYYY-MM")),
            }
        }
        None => {
            let today = mako_fristen::berlin_date(now);
            let first = today.replace_day(1).unwrap_or(today);
            let last_month = first.previous_day().unwrap_or(first);
            (last_month.year(), last_month.month())
        }
    };
    let Some(period) = metering::DayBoundary::Strom.month(year, month) else {
        return unprocessable(format!(
            "{year}-{month} lies outside the supported calendar"
        ));
    };
    let (from, to) = period.range();

    let threshold = params
        .min_coverage_pct
        .unwrap_or(DEFAULT_COVERAGE_THRESHOLD_PCT);
    if threshold > 100 {
        return unprocessable(format!("min_coverage_pct {threshold} is above 100"));
    }

    let explicit: Option<Vec<String>> = params.malo_ids.as_deref().map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    });

    // Resolve the candidate set: explicit list, or every MaLo with readings.
    // The "all MaLos with readings" scan is cross-MaLo, so it runs over
    // meterstore's version-resolved relation (Pattern B) rather than edmd's pool.
    let malo_ids: Vec<String> = match explicit {
        Some(ids) => ids,
        None => {
            let store = state.repo.store();
            let sql = format!(
                r#"SELECT DISTINCT "malo_id"
                     FROM "{table}"
                    WHERE "tenant" = $1 AND "from" >= $2 AND "to" <= $3
                    ORDER BY "malo_id""#,
                table = store.resolved_table(),
            );
            match store
                .query_with_params(
                    &sql,
                    vec![
                        datafusion::scalar::ScalarValue::Utf8(Some(resource_tenant.to_owned())),
                        ts_param(from),
                        ts_param(to),
                    ],
                )
                .await
                .and_then(|r| r.to_json())
            {
                Ok(rows) => rows
                    .iter()
                    .filter_map(|row| {
                        row.get("malo_id")
                            .and_then(|v| v.as_str())
                            .map(str::to_owned)
                    })
                    .collect(),
                Err(e) => {
                    tracing::warn!(error = %e, "sharing readiness: candidate scan failed");
                    return ApiError::internal(e).into_response();
                }
            }
        }
    };

    let window_secs = Decimal::from((to - from).whole_seconds().max(1));
    let mut items = Vec::with_capacity(malo_ids.len());

    for malo_id in &malo_ids {
        // Per-MaLo read through the repository (version-resolved,
        // tenant-scoped). A failed read must not abort the fleet report;
        // surface it as an explicit reason instead of silently dropping the
        // point.
        let reads = match state
            .repo
            .query(&TimeSeriesQuery {
                malo_id: malo_id.clone(),
                from,
                to,
                sparte: None,
                tenant: resource_tenant.to_owned(),
            })
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(malo_id = %malo_id, error = %e, "sharing readiness: read query failed");
                items.push(SharingReadinessItem {
                    malo_id: malo_id.clone(),
                    delivery: "ABSENT".to_owned(),
                    resolution: None,
                    coverage_pct: None,
                    reading_count: 0,
                    reasons: vec![format!("Abfrage fehlgeschlagen: {e}")],
                    required_action: "edmd-Log prüfen".to_owned(),
                });
                continue;
            }
        };

        // Split per register before anything is measured: the question §42c
        // asks is whether the point delivers quarter-hour values at all, and a
        // point whose Lastgang is complete delivers them even if a secondary
        // register reports monthly. The dominant energy register — the one
        // carrying the most intervals — decides.
        let groups = crate::domain::register_groups(&reads);
        let reading_count = reads.iter().filter(|r| r.quality.is_billable()).count() as u64;
        let dominant = groups
            .into_iter()
            .filter(|g| {
                g.obis_code
                    .is_none_or(crate::domain::register::is_energy_register)
            })
            .max_by_key(|g| g.intervals.len());
        // A dominant register exists only where readings do, and they name
        // their Sparte: none is assumed for an empty window.
        let sparte = reads.first().map(|r| r.sparte);

        let (delivery, reasons, resolution, coverage_pct) = match dominant
            .zip(sparte)
            .map(|(g, sparte)| crate::domain::vee::series_of(sparte, g.intervals))
        {
            None => (
                Delivery::Absent,
                vec![
                    metering::allocation::sharing::Finding::NoReadings
                        .as_str()
                        .to_owned(),
                ],
                None,
                None,
            ),
            Some(Err(defect)) => (
                Delivery::Insufficient,
                vec![defect.code().to_owned()],
                None,
                None,
            ),
            Some(Ok(series)) => {
                let (delivery, findings) = assess_delivery(&series, period, threshold);
                let covered: i64 = series
                    .iter()
                    .filter(|iv| iv.quality().is_billable() && period.contains(iv.from()))
                    .map(|iv| (iv.to().min(to) - iv.from()).whole_seconds().max(0))
                    .sum();
                let coverage = (Decimal::from(covered) * Decimal::ONE_HUNDRED / window_secs)
                    .round_dp_with_strategy(
                        2,
                        rust_decimal::RoundingStrategy::MidpointAwayFromZero,
                    );
                (
                    delivery,
                    findings.iter().map(|f| f.as_str().to_owned()).collect(),
                    Some(series.resolution().to_string()),
                    Some(coverage),
                )
            }
        };

        let required_action = match delivery {
            Delivery::Delivering => "keine",
            Delivery::Insufficient => "Zählerstandsgangmessung konfigurieren bzw. Lücken klären",
            Delivery::Absent => "Messwertlieferung beim MSB beauftragen",
        };

        items.push(SharingReadinessItem {
            malo_id: malo_id.clone(),
            delivery: delivery.as_str().to_owned(),
            resolution,
            coverage_pct,
            reading_count,
            reasons,
            required_action: required_action.to_owned(),
        });
    }

    let delivering = items.iter().filter(|i| i.delivery == "DELIVERING").count();
    let assessed = items.len();
    let ready_pct = if assessed == 0 {
        Decimal::ZERO
    } else {
        (Decimal::from(delivering) * Decimal::ONE_HUNDRED / Decimal::from(assessed))
            .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
    };

    Json(serde_json::json!({
        "assessed_at":          now.format(&Rfc3339).unwrap_or_default(),
        "month":                format!("{year:04}-{:02}", u8::from(month)),
        "window_from":          from.format(&Rfc3339).unwrap_or_default(),
        "window_to":            to.format(&Rfc3339).unwrap_or_default(),
        "min_coverage_pct":     threshold,
        "points_assessed":      assessed,
        "points_delivering":    delivering,
        "points_insufficient":  items.iter().filter(|i| i.delivery == "INSUFFICIENT").count(),
        "points_absent":        items.iter().filter(|i| i.delivery == "ABSENT").count(),
        "ready_pct":            ready_pct,
        "legal_basis":          "§42c Abs. 1 EnWG i. V. m. §2 Satz 1 Nr. 27 MsbG",
        "note":                 "Lieferung, nicht Fähigkeit — Stammdaten-Eignung via marktd GET /api/v1/melos/{id}/sharing-eligibility",
        "items":                items,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::multiply_first;
    use metering::allocation::community::{AllocationKey, allocate};
    use metering::{DayBoundary, MeloId, MeterInterval, QualityFlag, Resolution, Series};
    use rust_decimal::Decimal;
    use rust_decimal::dec;
    use time::macros::datetime;

    fn one(kwh: Decimal) -> Series {
        let iv = MeterInterval::quarter_hour(
            datetime!(2026-06-01 12:00 UTC),
            kwh,
            QualityFlag::Measured,
        )
        .expect("an interval");
        Series::new(Resolution::QUARTER_HOUR, DayBoundary::Strom, vec![iv]).expect("a series")
    }

    fn melo(n: u8) -> MeloId {
        format!("DE00012345678900000000000000000{n:02}")
            .parse()
            .expect("a MeLo")
    }

    /// **Invariant: a billed share multiplies before it divides.**
    ///
    /// Three equal participants of a 10 kWh pool are each owed
    /// `10 × 1 ÷ 3`, cut to six places: 3.333333 kWh. Forming the fraction
    /// first — `cut(1 ÷ 3) = 0.333333`, then `× 10` — credits 3.333330 kWh and
    /// drops 0.00001 kWh per participant into the residual, which no
    /// participant is then credited.
    #[test]
    fn a_three_way_split_multiplies_before_it_divides() {
        let plant = one(dec!(10));
        let tenants = [one(dec!(100)), one(dec!(100)), one(dec!(100))];
        let inputs: Vec<(MeloId, &Series)> = (1..=3).map(melo).zip(tenants.iter()).collect();
        for key in [AllocationKey::EqualShares, AllocationKey::Proportional] {
            let mut rows = allocate(&plant, &inputs, &key).expect("allocates");
            multiply_first(&mut rows, &key).expect("in range");
            let row = &rows[0];
            for s in &row.shares {
                assert_eq!(s.allocated, dec!(3.333333), "{key:?}: {s:?}");
            }
            assert_eq!(row.residual, dec!(0.000001), "{key:?}");
            assert_eq!(row.allocated() + row.residual, row.pool(), "{key:?}");
        }
    }

    /// The § 42b Abs. 5 Satz 4 cap still binds after the re-division: a
    /// participant is never credited more than it consumed in the interval.
    #[test]
    fn the_cap_still_binds_after_the_re_division() {
        let plant = one(dec!(10));
        let tenants = [one(dec!(1)), one(dec!(100)), one(dec!(100))];
        let inputs: Vec<(MeloId, &Series)> = (1..=3).map(melo).zip(tenants.iter()).collect();
        let key = AllocationKey::EqualShares;
        let mut rows = allocate(&plant, &inputs, &key).expect("allocates");
        multiply_first(&mut rows, &key).expect("in range");
        let row = &rows[0];
        assert_eq!(row.shares[0].allocated, dec!(1));
        assert_eq!(row.shares[1].allocated, dec!(3.333333));
        assert_eq!(row.allocated() + row.residual, row.pool());
    }
}

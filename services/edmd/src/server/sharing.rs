//! §42c EnWG energy sharing: VZW allocation and readiness report.

#[allow(unused_imports)]
use super::*;

// ── F-13: §42c Energy Sharing VZW allocation ──────────────────────────────────

/// `GET /api/v1/sharing/{community_id}/allocation?from=RFC3339&to=RFC3339`
///
/// The quarter-hour §42c EnWG allocation for one Energy-Sharing community:
/// what the shared plant produced in each interval, and how much of it each
/// participant was allocated.
///
/// **`community_id` is the plant's MeLo.** A GGV rule is written per tenant —
/// both variants name one `plant_melo_id` and one `tenant_melo_id` — so a
/// `virtual_meter_configs` row is a *participant*; the community is the set of
/// rules sharing a plant, which is what this resolves.
///
/// **The allocated share comes from the engine**, not a second computation:
///
/// ```text
/// allocated[t] = consumption[t] − net_grid_draw[t]
/// ```
///
/// so the `max(0, …)` cap of §42b Abs. 5 — a tenant is never credited more
/// community PV than they consumed in the interval — and the zero-consumption
/// guard apply here exactly as they do to the derived series.
///
/// §42c EnWG defines **no** new MaKo process (BNetzA Mitteilung Nr. 73 vom
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
    use metering::{AggregationRule, MeterInterval};
    use rust_decimal::Decimal;
    use sqlx::Row as _;
    use std::collections::HashMap;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-timeseries", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let (from, to) = match read_window(params.from.as_deref(), params.to.as_deref()) {
        Ok(w) => w,
        Err(refusal) => return refusal.into_response(),
    };

    // Every GGV rule naming this plant. The match is `jsonb_path_exists` with the
    // MeLo as a **bound variable**, not a `LIKE` over the serialised JSON: the
    // recursive wildcard covers both variants without hard-coding either tag,
    // while still comparing whole values, so one MeLo cannot match as a
    // substring of another and pull a stranger's community in.
    let pool = state.repo.pool();
    let rows = match sqlx::query(
        r"SELECT virtual_malo_id, rule_type, rule_json, display_name, legal_basis
          FROM virtual_meter_configs
          WHERE tenant = $2
            AND rule_type IN ('GGV_CONSTANT_ALLOCATION','GGV_PROPORTIONAL_ALLOCATION')
            AND jsonb_path_exists(
                    rule_json,
                    '$.**.plant_melo_id ? (@ == $p)',
                    jsonb_build_object('p', $1::text))
          ORDER BY virtual_malo_id",
    )
    .bind(&community_id)
    .bind(resource_tenant)
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, community_id, "edmd: sharing allocation query failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if rows.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "no Energy-Sharing participants are configured against this plant",
                "community_id": community_id,
                "hint": "`community_id` is the shared plant's MeLo — the `plant_melo_id` \
                         of the GGV rules. Create one rule per participant via \
                         POST /api/v1/virtual with a GgvConstantAllocation or \
                         GgvProportionalAllocation rule_json.",
            })),
        )
            .into_response();
    }

    // Read each series once, however many rules name it: the proportional
    // variant lists every participant's MeLo as its denominator, so a community
    // of n tenants would otherwise be n² reads.
    let mut series: HashMap<String, Vec<MeterInterval>> = HashMap::new();
    let mut wanted: Vec<(String, bool)> = Vec::new();
    let mut participants: Vec<(String, AggregationRule, String)> = Vec::new();
    for row in &rows {
        let rule_json: serde_json::Value = row.try_get("rule_json").unwrap_or_default();
        let rule: AggregationRule = match serde_json::from_value(rule_json) {
            Ok(r) => r,
            Err(e) => {
                // A stored rule that no longer deserialises is a config fault, and
                // silently dropping a participant under-allocates the settlement.
                tracing::error!(error = %e, community_id, "edmd: undeserialisable GGV rule");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("a stored GGV rule could not be read: {e}"),
                        "community_id": community_id,
                    })),
                )
                    .into_response();
            }
        };
        let virtual_malo_id: String = row.try_get("virtual_malo_id").unwrap_or_default();
        let display_name: String = row.try_get("display_name").unwrap_or_default();
        let generation = crate::server::generation_source_ids(&rule);
        for id in rule.source_malo_ids() {
            wanted.push((id.to_owned(), generation.contains(&id)));
        }
        participants.push((virtual_malo_id, rule, display_name));
    }
    wanted.sort();
    wanted.dedup();

    for (id, is_generation) in &wanted {
        let reads = match state
            .repo
            .query(&TimeSeriesQuery {
                malo_id: id.clone(),
                from,
                to,
                sparte: None,
                tenant: resource_tenant.to_owned(),
            })
            .await
        {
            Ok(r) => r,
            // A read that failed is not a participant who consumed nothing.
            // Under-allocating a §42c settlement silently is the one outcome
            // worth refusing outright.
            Err(e) => {
                tracing::error!(error = %e, melo = id, "edmd: sharing allocation read failed");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "error": format!("series {id} could not be read: {e}"),
                        "community_id": community_id,
                    })),
                )
                    .into_response();
            }
        };
        series.insert(
            id.clone(),
            crate::server::source_intervals(&reads, *is_generation),
        );
    }

    // The community's production: the plant's own generation series.
    let production = series.get(&community_id).cloned().unwrap_or_default();
    let production_kwh: Decimal = production.iter().map(|iv| iv.value).sum();

    // Per participant: the allocated energy, from the engine's own net grid draw.
    let mut allocations: Vec<serde_json::Value> = Vec::new();
    let mut allocated_total = Decimal::ZERO;
    for (virtual_malo_id, rule, display_name) in &participants {
        let tenant_melo = match rule {
            AggregationRule::GgvConstantAllocation { tenant_melo_id, .. }
            | AggregationRule::GgvProportionalAllocation { tenant_melo_id, .. } => {
                tenant_melo_id.clone()
            }
            // The query restricts `rule_type` to the two GGV variants, so this
            // is unreachable; skipping is the conservative answer if it is not.
            _ => continue,
        };
        // The allocation comes from the engine whole: `consumption ==
        // allocated + net_grid_draw` holds exactly in every interval, and the
        // §42b Abs. 5 `Pos()` cap is reported rather than inferred. Recovering
        // `allocated` by subtracting the net draw from a re-projected
        // consumption series made this endpoint restate arithmetic the engine
        // had already done, on a series it had to read twice.
        let allocation = match metering::compute_ggv_allocation(rule, &series) {
            Ok(intervals) => intervals,
            Err(e) => {
                tracing::error!(error = %e, virtual_malo_id, "edmd: GGV allocation failed");
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": e.to_string(),
                        "participant": virtual_malo_id,
                        "community_id": community_id,
                    })),
                )
                    .into_response();
            }
        };

        let mut intervals: Vec<serde_json::Value> = Vec::with_capacity(allocation.len());
        let mut participant_total = Decimal::ZERO;
        for iv in &allocation {
            participant_total += iv.allocated;
            intervals.push(serde_json::json!({
                "from": iv.from,
                "to": iv.to,
                "consumption_kwh": iv.consumption.to_string(),
                "generation_kwh": iv.generation.to_string(),
                // The nominal share before the cap, and the part of it the
                // tenant could not use — which fed the public grid.
                "share_kwh": iv.share.to_string(),
                "capped": iv.capped(),
                "surplus_to_grid_kwh": iv.surplus_to_grid().to_string(),
                "net_grid_draw_kwh": iv.net_grid_draw.to_string(),
                "allocated_kwh": iv.allocated.to_string(),
                "quality": crate::store::quality_to_str(iv.quality),
            }));
        }
        allocated_total += participant_total;
        allocations.push(serde_json::json!({
            "virtual_malo_id": virtual_malo_id,
            "display_name": display_name,
            "tenant_melo_id": tenant_melo,
            "rule_type": crate::server::rule_type_of(rule),
            "allocated_kwh": participant_total.to_string(),
            "interval_count": intervals.len(),
            "intervals": intervals,
        }));
    }

    let production_intervals: Vec<serde_json::Value> = production
        .iter()
        .map(|iv| {
            serde_json::json!({
                "from": iv.from,
                "to": iv.to,
                "total_kwh": iv.value.to_string(),
                // The real flag, not a hardcoded MEASURED: the projection admits
                // Estimated/Substituted/Corrected/Preliminary, and a §42c
                // settlement has to see that a slot of "production" is an
                // Ersatzwert.
                "quality": crate::store::quality_to_str(iv.quality),
            })
        })
        .collect();

    let legal_basis: Option<String> = rows[0].try_get("legal_basis").unwrap_or(None);
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "community_id":         community_id,
            "plant_melo_id":        community_id,
            "legal_basis":          legal_basis.as_deref().unwrap_or(
                "§42c EnWG · §42b Abs. 5 EnWG (Pos()-Deckel) · BNetzA Mitteilung Nr. 73"),
            "from":                 from,
            "to":                   to,
            "participant_count":    participants.len(),
            "total_production_kwh": production_kwh.to_string(),
            // What the plant produced but nobody was allocated: it fed the grid.
            // § 42b Abs. 5 caps each tenant at their own consumption, so this is
            // a real remainder and not a rounding error.
            "unallocated_kwh":      (production_kwh - allocated_total).max(Decimal::ZERO).to_string(),
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
pub(crate) struct SharingReadinessParams {
    /// RFC 3339 start of the observation window. Defaults to 30 days ago.
    from: Option<String>,
    /// RFC 3339 end of the observation window. Defaults to now.
    to: Option<String>,
    /// Comma-separated MaLo-IDs to assess. Defaults to every MaLo with readings
    /// in the window.
    malo_ids: Option<String>,
    /// Coverage threshold in percent. Defaults to
    /// [`metering::sharing::DEFAULT_COVERAGE_THRESHOLD_PCT`].
    min_coverage_pct: Option<f64>,
}

/// Per-point delivery verdict in the readiness report.
#[derive(Debug, serde::Serialize)]
pub(crate) struct SharingReadinessItem {
    malo_id: String,
    /// `DELIVERING` · `INSUFFICIENT` · `ABSENT`.
    delivery: String,
    /// Detected interval length in seconds, when determinable.
    interval_seconds: Option<i64>,
    /// Classification derived from the observed series.
    messtyp: Option<String>,
    /// Share of expected quarter-hour slots present, 0–100.
    coverage_pct: Option<f64>,
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
/// Resolution is derived per point from the median of `dtm_to - dtm_from` via
/// `metering::classification::detect_interval_length`; `meter_reads` stores no
/// resolution column. Coverage is measured against the number of quarter-hour
/// slots the window contains.
pub(crate) async fn get_sharing_readiness(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Query(params): Query<SharingReadinessParams>,
) -> impl IntoResponse {
    use metering::classification::{classify_messtyp, detect_interval_length};
    use metering::interval::MeterInterval;
    use metering::sharing::{
        DEFAULT_COVERAGE_THRESHOLD_PCT, Delivery, DeliveryEvidenceInput, assess_delivery,
    };
    use time::format_description::well_known::Rfc3339;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-timeseries", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let now = OffsetDateTime::now_utc();
    let to = params
        .to
        .as_deref()
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        .unwrap_or(now);
    let from = params
        .from
        .as_deref()
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        .unwrap_or_else(|| to - time::Duration::days(30));

    if from >= to {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": "`from` must precede `to`" })),
        )
            .into_response();
    }

    let threshold = params
        .min_coverage_pct
        .unwrap_or(DEFAULT_COVERAGE_THRESHOLD_PCT);

    // The coverage denominator: the window's own duration. Coverage is a
    // **duration ratio**, not an interval count — the same rule the delivery
    // sweep uses (`surveillance::assess_delivery`), and for the same two
    // reasons. Counting intervals against an assumed quarter-hour grid reported
    // every legitimately hourly series as 25 % covered; counting them across all
    // of a measuring point's registers ran a prosumer past 100 %, where the clamp
    // hid it and no multi-register point could ever fall below the threshold.
    #[allow(clippy::cast_precision_loss)]
    let window_secs = (to - from).whole_seconds().max(1) as f64;

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
                    return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
                }
            }
        }
    };

    let mut items = Vec::with_capacity(malo_ids.len());

    for malo_id in &malo_ids {
        // Per-MaLo read through the repository (version-resolved, tenant-scoped,
        // billable-only). A failed read must not abort the fleet report; surface
        // it as an explicit reason instead of silently dropping the point.
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
                    interval_seconds: None,
                    messtyp: None,
                    coverage_pct: None,
                    reading_count: 0,
                    reasons: vec![format!("Abfrage fehlgeschlagen: {e}")],
                    required_action: "edmd-Log prüfen".to_owned(),
                });
                continue;
            }
        };

        // `query` does not filter quality; keep only billable qualities
        // (`QualityFlag::is_billable`) — a faulty read is not a delivered
        // quarter-hour value.
        // `classify_messtyp` takes a typed `SeriesOrigin`. Only one
        // distinction matters to it — did this series come from a
        // Smart-Meter-Gateway — so edmd's own ingestion source is mapped onto
        // that question rather than handed over verbatim.
        let source_hint: Option<metering::SeriesOrigin> = reads.first().map(|r| {
            match r.source {
                // A gateway push, direct or over CLS.
                IngestionSource::DirectPush | IngestionSource::DirectGas => {
                    metering::SeriesOrigin::SmartMeterGateway
                }
                // MSCONS, imports, manual entry, substitutes: the interval
                // length is the only evidence, which is what `Other` means.
                _ => metering::SeriesOrigin::Other,
            }
        });
        // Split per register before anything is measured. `detect_interval_length`
        // takes the median interval *duration*, so a flattened multi-register
        // series answers with the median across registers: a point whose
        // Lastgang is quarter-hourly and whose secondary register reports
        // hourly can be classified on the secondary one. The same split
        // `domain::register_groups` exists for everywhere else.
        let billable: Vec<MeterRead> = reads
            .iter()
            .filter(|r| r.quality.is_billable())
            .cloned()
            .collect();
        let groups = crate::domain::register_groups(&billable);
        let reading_count = billable.len() as u64;

        // The best-covered register decides, not the sum and not the worst. The
        // question §42c asks is whether the point delivers quarter-hour values at
        // all; a point whose Lastgang is complete delivers them even if a
        // secondary register reports monthly.
        #[allow(clippy::cast_precision_loss)]
        let coverage_pct = groups
            .iter()
            .map(|g| {
                let covered: f64 = g
                    .intervals
                    .iter()
                    .map(|iv| (iv.to - iv.from).whole_seconds().max(0) as f64)
                    .sum();
                (covered / window_secs * 100.0).clamp(0.0, 100.0)
            })
            .fold(None::<f64>, |acc, c| Some(acc.map_or(c, |a: f64| a.max(c))));

        // Cadence and Messtyp are judged on the dominant register — the one
        // carrying the most intervals — for the same reason.
        let dominant: Vec<MeterInterval> = groups
            .iter()
            .max_by_key(|g| g.intervals.len())
            .map(|g| g.intervals.clone())
            .unwrap_or_default();
        let interval_class = detect_interval_length(&dominant);
        let messtyp = if dominant.is_empty() {
            None
        } else {
            Some(classify_messtyp(&dominant, source_hint))
        };

        let evidence = DeliveryEvidenceInput {
            resolution: interval_class,
            messtyp,
            coverage_pct,
            reading_count,
            last_reading_at: billable.iter().map(|r| r.dtm_to).max(),
        };
        let (delivery, reasons) = assess_delivery(&evidence, threshold);

        let required_action = match delivery {
            Delivery::Delivering => "keine",
            Delivery::Insufficient => "Zählerstandsgangmessung konfigurieren bzw. Lücken klären",
            Delivery::Absent => "Messwertlieferung beim MSB beauftragen",
        };

        items.push(SharingReadinessItem {
            malo_id: malo_id.clone(),
            delivery: match delivery {
                Delivery::Delivering => "DELIVERING",
                Delivery::Insufficient => "INSUFFICIENT",
                Delivery::Absent => "ABSENT",
            }
            .to_owned(),
            interval_seconds: interval_class.map(|r| i64::from(r.nominal_seconds())),
            messtyp: messtyp.map(|m| format!("{m:?}").to_uppercase()),
            coverage_pct,
            reading_count,
            // `assess_delivery` returns typed `Finding`s since metering 0.17,
            // where it returned free text. The API keeps emitting strings, so
            // they are rendered here — one place, one vocabulary, instead of
            // prose composed inside the crate.
            reasons: reasons.iter().map(|f| format!("{f:?}")).collect(),
            required_action: required_action.to_owned(),
        });
    }

    let delivering = items.iter().filter(|i| i.delivery == "DELIVERING").count();
    let assessed = items.len();
    let ready_pct = if assessed == 0 {
        0.0
    } else {
        (delivering as f64 / assessed as f64) * 100.0
    };

    Json(serde_json::json!({
        "assessed_at":          now.format(&Rfc3339).unwrap_or_default(),
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

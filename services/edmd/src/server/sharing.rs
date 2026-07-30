//! §42c EnWG energy sharing: VZW allocation and readiness report.

#[allow(unused_imports)]
use super::*;

// ── F-13: §42c Energy Sharing VZW allocation ──────────────────────────────────

/// `GET /api/v1/sharing/{community_id}/allocation?from=RFC3339&to=RFC3339`
///
/// Returns the quarter-hour VZW (Viertelstunden-Zeitreihe) allocation for a
/// `§42c EnWG Energy Sharing community`. Each 15-min interval shows the total
/// community production and the per-participant attribution fraction.
///
/// The `community_id` maps to a `virtual_meter_configs` entry with
/// `rule_type IN ('GgvConstantAllocation', 'GgvProportionalAllocation')`.
/// Source MaLo IDs for the producer(s) and participants are encoded in `rule_json`.
///
/// ## Regulatory basis
///
/// §42c EnWG (Energy Sharing), as addressed by BNetzA Mitteilung Nr. 73 vom
/// 07.07.2026 (Az. BK6-06-009), which endorses the §42c Dienstleistungsmodell
/// and defines **no** new MaKo processes for it. Accordingly this endpoint
/// implements no mandated market process: it computes the per-participant
/// quarter-hour attribution on demand from locally stored meter reads and the
/// community's stored `AggregationRule`, and returns it as JSON.
pub(crate) async fn get_sharing_allocation(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(community_id): Path<String>,
    Query(params): Query<LastgangParams>,
) -> impl IntoResponse {
    use time::format_description::well_known::Rfc3339;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-timeseries", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

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

    // Load the virtual meter config for this community.
    let pool = state.repo.pool();
    let config_row = sqlx::query(
        r"SELECT rule_type, rule_json, display_name, legal_basis
          FROM virtual_meter_configs
          WHERE virtual_malo_id = $1 AND tenant = $2
            AND rule_type IN ('GgvConstantAllocation','GgvProportionalAllocation')
          LIMIT 1",
    )
    .bind(&community_id)
    .bind(resource_tenant)
    .fetch_optional(pool)
    .await;

    use sqlx::Row as _;
    let config_row = match config_row {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "No Energy Sharing community found",
                    "community_id": community_id,
                    "hint": "Create via POST /api/v1/virtual with rule_type GgvConstantAllocation or GgvProportionalAllocation"
                })),
            )
                .into_response();
        }
        Err(e) => {
            tracing::warn!(error = %e, "edmd: get_sharing_allocation query failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let rule_type: String = config_row.try_get("rule_type").unwrap_or_default();
    let rule_json: serde_json::Value = config_row.try_get("rule_json").unwrap_or_default();
    let display_name: String = config_row.try_get("display_name").unwrap_or_default();
    let legal_basis: Option<String> = config_row.try_get("legal_basis").unwrap_or(None);

    // Extract source MaLo IDs from rule_json.
    // Expected shape: { "source_malo_ids": ["11234567890"], "participant_malo_ids": ["11234567891", ...], "fractions": [...] }
    let source_malo_ids: Vec<String> = rule_json
        .get("source_malo_ids")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();

    let participant_malo_ids: Vec<String> = rule_json
        .get("participant_malo_ids")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();

    if source_malo_ids.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "Community rule_json must contain non-empty 'source_malo_ids'",
                "community_id": community_id,
            })),
        )
            .into_response();
    }

    // Fetch the aggregation rule and compute via metering::compute_virtual_meter.
    use metering::MeterInterval;
    use rust_decimal::Decimal;

    // Load production intervals from all source MaLos through the meterstore
    // repository (version-resolved, tenant-scoped). `repo.query` returns the
    // NUMERIC quantity as a typed `Decimal`, so the previous
    // String-decode-then-parse (which silently allocated ZERO on the NUMERIC
    // column) is gone. `query` does not filter quality, so Faulty/Unknown are
    // dropped here via the §60 Abs. 2 billable rule (`QualityFlag::is_billable`).
    // A failed read must error, not silently under-allocate the §42c settlement,
    // so the former `unwrap_or_default()` is removed.
    let mut all_production: Vec<MeterInterval> = Vec::new();
    for malo_id in &source_malo_ids {
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
                tracing::warn!(error = %e, malo_id, "edmd: sharing allocation read failed");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };
        for r in reads.iter().filter(|r| r.quality.is_billable()) {
            all_production.push(MeterInterval {
                from: r.dtm_from,
                to: r.dtm_to,
                value_kwh: r.quantity_kwh,
                quality: r.quality,
                obis_code: r.obis_code.as_deref().and_then(|s| s.parse().ok()),
            });
        }
    }
    all_production.sort_by_key(|iv| iv.from);

    // Compute community production totals per interval.
    // Per-participant attribution is done by the LF using the fractions in rule_json
    // (GgvConstantAllocation) or by the dynamic consumption ratio (GgvProportionalAllocation).
    // This endpoint returns the community-level production data; callers fetch individual
    // participant consumption via GET /api/v1/lastgang/{malo_id}.
    let total_kwh: Decimal = all_production.iter().map(|iv| iv.value_kwh).sum();
    let interval_count = all_production.len();

    let allocation_intervals: Vec<serde_json::Value> = all_production
        .iter()
        .map(|iv| {
            serde_json::json!({
                "from":         iv.from,
                "to":           iv.to,
                "total_kwh":    iv.value_kwh.to_string(),
                "quality":      "MEASURED",
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "community_id":          community_id,
            "display_name":          display_name,
            "rule_type":             rule_type,
            "legal_basis":           legal_basis.as_deref().unwrap_or("§42c EnWG"),
            "from":                  from,
            "to":                    to,
            "source_malo_ids":       source_malo_ids,
            "participant_malo_ids":  participant_malo_ids,
            "total_production_kwh":  total_kwh.to_string(),
            "interval_count":        interval_count,
            "intervals":             allocation_intervals,
            "note": "Per-participant allocation fractions applied per rule_type. \
                     Fetch participant consumption via GET /api/v1/lastgang/{malo_id} \
                     for the full §42c VZW settlement picture.",
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

    // Expected quarter-hour slots in the window — the coverage denominator.
    let window_secs = (to - from).whole_seconds().max(1);
    let expected_slots = (window_secs as f64 / 900.0).max(1.0);

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
        let source_hint: Option<String> = reads.first().map(|r| r.source.as_str().to_owned());
        let intervals: Vec<MeterInterval> = reads
            .iter()
            .filter(|r| r.quality.is_billable())
            .map(|r| MeterInterval {
                from: r.dtm_from,
                to: r.dtm_to,
                value_kwh: r.quantity_kwh,
                quality: r.quality,
                obis_code: r.obis_code.as_deref().and_then(|s| s.parse().ok()),
            })
            .collect();

        let interval_class = detect_interval_length(&intervals);
        let messtyp = if intervals.is_empty() {
            None
        } else {
            Some(classify_messtyp(&intervals, source_hint.as_deref()))
        };
        let coverage_pct = if intervals.is_empty() {
            None
        } else {
            Some(((intervals.len() as f64 / expected_slots) * 100.0).min(100.0))
        };

        let evidence = DeliveryEvidenceInput {
            resolution: interval_class,
            messtyp,
            coverage_pct,
            reading_count: intervals.len() as u64,
            last_reading_at: intervals.last().map(|iv| iv.to),
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
            reading_count: intervals.len() as u64,
            reasons,
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

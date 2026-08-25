//! Cold-tier archive surface: status, single-MaLo and portfolio MMM aggregation,
//! raw time-series export, and read-only analytical SQL — all evaluated by
//! meterstore across the hot + cold tiers. External Iceberg clients use
//! meterstore's own catalog facade, not an edmd-hosted REST catalog.

#[allow(unused_imports)]
use super::*;
use crate::store::TENANT_COL;

/// A refusal, small enough to sit in an `Err` without carrying a whole response.
struct Refusal {
    status: StatusCode,
    error: String,
}

impl Refusal {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.error })),
        )
            .into_response()
    }
}

/// Parse a path MaLo-ID, answering `400` rather than failing inside the store.
///
/// `metering::MaloId` enforces the BDEW Bildungsvorschrift (eleven digits,
/// Vergabestelle 1–9, Anwendungshilfe check digit). A value that cannot be a
/// MaLo is a bad request, not a lookup that happened to find nothing.
fn parse_malo(malo_id: &str) -> Result<metering::MaloId, Refusal> {
    malo_id.parse().map_err(|e: metering::ParseError| Refusal {
        status: StatusCode::BAD_REQUEST,
        error: format!("{malo_id}: {e}"),
    })
}

/// Open a series query for `malo`, answering `500` if the store refuses.
fn open_series(
    store: &meterstore::MeterStore,
    malo: metering::MaloId,
) -> Result<meterstore::SeriesQuery<'_>, Refusal> {
    store.series(malo).map_err(|e| Refusal {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        error: e.to_string(),
    })
}

// ── Archive endpoint handlers ─────────────────────────────────────────────────

/// `GET /api/v1/archive/status`
///
/// Returns archive statistics and the 20 most recent batches.
pub(crate) async fn get_archive_status(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<Arc<PgPool>>,
    State(state): State<HandlerState>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-archive-status", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let _ = &pool;
    // Archival is owned by meterstore (hot Postgres + cold Iceberg). Report the
    // tier it manages rather than the former per-batch export bookkeeping — and
    // report it honestly: without `[archive] enabled = true` there is no cold
    // tier and no maintenance loop, so nothing is ever archived.
    let store = state.repo.store();
    Json(serde_json::json!({
        "cold_tier_enabled": state.cold_tier_enabled,
        "backend": "meterstore",
        "tables": {
            "resolved": store.resolved_table(),
            "raw": store.raw_table(),
        },
        "note": if state.cold_tier_enabled {
            "Cold-tier archival is managed by meterstore's tiering watermark; \
             per-batch archive statistics are not tracked."
        } else {
            "Hot tier only — `[archive] enabled` is false, so settled intervals \
             stay in PostgreSQL and the Iceberg warehouse is in-memory."
        },
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub(crate) struct ArchiveOlapParams {
    /// RFC 3339 start (inclusive).
    from: Option<String>,
    /// RFC 3339 end (inclusive).
    to: Option<String>,
}

/// `GET /api/v1/archive/olap/{malo_id}?from=RFC3339&to=RFC3339`
///
/// DataFusion OLAP query over archived `meter_reads` for one MaLo.
///
/// Returns the aggregated MMM result: total kWh, read count, and period bounds.
/// Requires Iceberg/S3 archival to be enabled and configured.
///
/// This is the primary endpoint for MMM aggregation over archived data.
/// For recent data (< 12 months) use `/api/v1/billing-period/{malo_id}`.
pub(crate) async fn get_archive_olap(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<ArchiveOlapParams>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-archive-olap", resource_tenant) {
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
    // MMM aggregation now runs against the version-resolved, tier-split series
    // meterstore hands back — the same numbers a settlement would reconcile.
    let query = match parse_malo(&malo_id).and_then(|m| open_series(state.repo.store(), m)) {
        Ok(q) => q,
        Err(refusal) => return refusal.into_response(),
    };

    let query = match query.column_eq(
        TENANT_COL,
        datafusion::scalar::ScalarValue::Utf8(Some(state.tenant.clone())),
    ) {
        Ok(q) => q,
        Err(e) => {
            tracing::error!(error = %e, "edmd: series read could not be scoped to the tenant");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    match query.range(from, to).collect().await {
        Ok(Some(series)) => {
            // The same canonical Bezug projection the REST and MCP aggregates
            // use: a resolved read spans every register, so summing it raw mixed
            // Einspeisung into the total, counted a dual-tariff meter's
            // consumption twice, and admitted Faulty intervals the settlement
            // side must never see (`domain::register`).
            let intervals = crate::domain::register::energy_intervals_from(
                series.intervals,
                crate::domain::EnergyDirection::Bezug,
            );
            let total: rust_decimal::Decimal = intervals.iter().map(|i| i.value).sum();
            Json(serde_json::json!({
                "malo_id": malo_id,
                "total_kwh": total.to_string(),
                "read_count": intervals.len(),
                "from": from.to_string(),
                "to": to.to_string(),
            }))
            .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "no data for this MaLo / period" })),
        )
            .into_response(),
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: archive OLAP query failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct PortfolioParams {
    from: Option<String>,
    to: Option<String>,
    #[serde(default = "default_portfolio_limit")]
    limit: usize,
}
pub(crate) fn default_portfolio_limit() -> usize {
    100
}

/// `GET /api/v1/archive/portfolio?from=RFC3339&to=RFC3339&limit=N`
///
/// Portfolio-level Bezug aggregation across both tiers, evaluated in one
/// version-resolved plan by meterstore, then projected onto the canonical
/// register set. Returns total kWh and read count per MaLo, ordered by
/// consumption descending, capped at `limit` measuring points.
///
/// # Why this is two steps rather than one `GROUP BY malo_id`
///
/// A bare `SUM("value") … GROUP BY "malo_id"` is the register defect at portfolio
/// scale: a prosumer's Einspeisung added to its Bezug, a dual-tariff meter's
/// `1.8.0` counted *again* as `1.8.1 + 1.8.2`, kvarh and kW registers in a kWh
/// figure, `FAULTY` intervals included — in a result this endpoint calls
/// portfolio-wide MMM, which is money.
///
/// The projection cannot be pushed into the scan: it needs to parse OBIS codes,
/// and meterstore registers calendar UDFs but no OBIS ones. So the scan groups
/// per `(malo_id, obis_code)` — a handful of rows per measuring point — and the
/// register decision is made in Rust by the *same* `energy_intervals_from` every
/// other aggregate uses, over one synthetic interval per register spanning that
/// register's own coverage. The total-vs-tariff overlap rule then does the right
/// thing: a tariff register whose coverage lies inside a total register's is
/// dropped, and one that reports where no total does is kept.
///
/// `limit` bounds **measuring points**. The scan itself is bounded separately,
/// and `truncated` says when that bound was reached — a capped list that looks
/// complete is how a partial portfolio gets mistaken for the whole one.
pub(crate) async fn get_archive_portfolio(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Query(params): Query<PortfolioParams>,
) -> impl IntoResponse {
    use metering::MeterInterval;
    use rust_decimal::Decimal;
    use std::collections::BTreeMap;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-archive-olap", resource_tenant) {
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
    // A hostile `limit` would otherwise reach the SQL text; clamp it to a sane
    // ceiling and render it as a plain integer so it cannot carry an injection.
    let limit = params.limit.clamp(1, 10_000);
    // Registers per measuring point, generously: Bezug + Einspeisung + HT + NT +
    // Blindarbeit + a maximum register + a fault counter is seven. The scan cap
    // is expressed here rather than as a magic number in the SQL so the response
    // can explain what `truncated` means.
    const REGISTERS_PER_POINT: usize = 8;
    let row_cap = limit.saturating_mul(REGISTERS_PER_POINT).max(1_000);

    // `from` is a SQL reserved word, hence quoted; the bounds and the tenant
    // travel as bound parameters so no value reaches the SQL text. The tenant
    // predicate is defence in depth — a deployment writes only its own tenant —
    // but it keeps the aggregate correct even if a store is ever shared.
    //
    // Non-billable qualities are excluded in the scan (§ 60 Abs. 2 MsbG) rather
    // than after materialising: it is the one part of the projection SQL *can*
    // express, and it is the part that removes the most rows.
    //
    // Ordered by `malo_id` so the cap is deterministic. Ordering by an
    // unprojected `SUM` would rank on precisely the inflated number this
    // endpoint exists to stop reporting.
    let store = state.repo.store();
    let sql = format!(
        r#"SELECT "malo_id",
                  "obis_code",
                  SUM("value")  AS total_kwh,
                  COUNT(*)      AS read_count,
                  MIN("from")   AS span_from,
                  MAX("to")     AS span_to
             FROM "{table}"
            WHERE "from" >= $1 AND "from" < $2 AND "tenant" = $3
              AND "quality" NOT IN ('FAULTY', 'UNKNOWN')
            GROUP BY "malo_id", "obis_code"
            ORDER BY "malo_id", "obis_code"
            LIMIT {row_cap}"#,
        table = store.resolved_table(),
    );

    let rows = match store
        .query_with_params(
            &sql,
            vec![
                ts_param(from),
                ts_param(to),
                datafusion::scalar::ScalarValue::Utf8(Some(state.tenant.clone())),
            ],
        )
        .await
    {
        Ok(result) => {
            let spans = result.spans_tiers();
            match result.to_json() {
                Ok(rows) => (rows, spans),
                Err(e) => {
                    tracing::warn!(error = %e, "edmd: portfolio JSON serialisation failed");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "edmd: portfolio aggregation query failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let (rows, spans_tiers) = rows;
    let truncated = rows.len() >= row_cap;

    // One synthetic interval per register, spanning that register's own
    // coverage, carrying its summed energy. `energy_intervals_from` then applies
    // the whole projection: non-kWh registers dropped, Einspeisung dropped, and
    // a tariff register dropped exactly where a total register covers it.
    let mut per_point: BTreeMap<String, (Vec<MeterInterval>, i64)> = BTreeMap::new();
    for row in &rows {
        let Some(malo_id) = row.get("malo_id").and_then(|v| v.as_str()) else {
            continue;
        };
        let obis = row
            .get("obis_code")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok());
        let Some(value) = row.get("total_kwh").and_then(decimal_from_json) else {
            continue;
        };
        let (Some(span_from), Some(span_to)) = (
            row.get("span_from").and_then(json_instant),
            row.get("span_to").and_then(json_instant),
        ) else {
            continue;
        };
        let count = row
            .get("read_count")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let entry = per_point.entry(malo_id.to_owned()).or_default();
        entry.0.push(MeterInterval {
            from: span_from,
            to: span_to,
            value,
            // The scan already excluded the non-billable qualities, so every
            // register that reaches here is billable. The synthetic interval
            // carries the flag that says so rather than one it did not measure.
            quality: QualityFlag::Measured,
            obis_code: obis,
        });
        entry.1 += count;
    }

    let mut portfolio: Vec<(String, Decimal, i64)> = per_point
        .into_iter()
        .map(|(malo_id, (registers, count))| {
            let projected = crate::domain::energy_intervals_from(
                registers,
                crate::domain::EnergyDirection::Bezug,
            );
            let total: Decimal = projected.iter().map(|iv| iv.value).sum();
            (malo_id, total, count)
        })
        .collect();
    // Ranked on the **projected** figure — the one the endpoint reports.
    portfolio.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let points_scanned = portfolio.len();
    portfolio.truncate(limit);

    Json(serde_json::json!({
        "from": from.to_string(),
        "to": to.to_string(),
        "malo_count": portfolio.len(),
        "points_scanned": points_scanned,
        // The register scan hit its cap, so the ranking is over what was
        // scanned rather than over the whole portfolio. Reported rather than
        // implied.
        "truncated": truncated,
        "spans_tiers": spans_tiers,
        "portfolio": portfolio
            .into_iter()
            .map(|(malo_id, total_kwh, read_count)| {
                serde_json::json!({
                    "malo_id": malo_id,
                    "total_kwh": total_kwh.to_string(),
                    "read_count": read_count,
                })
            })
            .collect::<Vec<_>>(),
        "projection": "Bezug, billable qualities only, one register set per point \
                       (domain::register)",
    }))
    .into_response()
}

/// A DataFusion `Decimal128` rendered into JSON, back as a `Decimal`.
///
/// `to_json` runs the batch through arrow's `ArrayWriter`, which renders a
/// `Decimal128` as a JSON **number**, not a string. Both forms are accepted here
/// because the writer's choice is not edmd's contract, and a number is parsed
/// through its decimal *text* rather than `as_f64`, so nothing is rounded twice.
///
/// **One precision bound is worth stating.** `serde_json` backs a number with an
/// `f64` unless `arbitrary_precision` is on, so the round trip is exact only
/// while the value fits f64's ~15–17 significant digits. `NUMERIC(18,5)` can
/// hold 18, so a figure above roughly 10¹⁰ kWh — ten TWh at a single measuring
/// point — could lose its last digits here. Real per-MaLo portfolio totals are
/// eight or nine significant digits, so this is a documented ceiling rather than
/// a live hazard; the exact path for bulk figures is the Arrow IPC one, which
/// carries `Decimal128(18,5)` unchanged.
fn decimal_from_json(v: &serde_json::Value) -> Option<rust_decimal::Decimal> {
    match v {
        serde_json::Value::String(s) => s.parse().ok(),
        serde_json::Value::Number(n) => n.to_string().parse().ok(),
        _ => None,
    }
}

/// A DataFusion timestamp rendered into JSON, back as an instant.
fn json_instant(v: &serde_json::Value) -> Option<OffsetDateTime> {
    let s = v.as_str()?;
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
        .ok()
        .or_else(|| {
            // DataFusion renders a timestamp without a zone; it is UTC.
            let fmt = time::macros::format_description!(
                "[year]-[month]-[day]T[hour]:[minute]:[second][optional [.[subsecond]]]"
            );
            time::PrimitiveDateTime::parse(s, &fmt)
                .ok()
                .map(time::PrimitiveDateTime::assume_utc)
        })
}

/// A UTC instant as the `TimestampMicrosecond` literal the storage schema binds
/// A UTC instant as the `TimestampMicrosecond` literal the storage schema binds
/// its interval bounds as (`col::FROM` / `col::TO`).
pub(crate) fn ts_param(t: OffsetDateTime) -> datafusion::scalar::ScalarValue {
    datafusion::scalar::ScalarValue::TimestampMicrosecond(
        Some((t.unix_timestamp_nanos() / 1_000) as i64),
        Some("UTC".into()),
    )
}

/// `"true"` / `"false"` for a boolean header value.
fn bool_str(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

/// Serialise meterstore's own `RecordBatch`es to an Arrow IPC stream.
///
/// The schema is passed explicitly so a zero-row result still yields a valid,
/// self-describing stream (schema message + no batches) rather than empty bytes.
/// meterstore's Arrow is the workspace Arrow (one unified 58.x), so its batches
/// write through edmd's `StreamWriter` without a copy.
fn batches_to_arrow_ipc(
    schema: &arrow::datatypes::SchemaRef,
    batches: &[arrow::record_batch::RecordBatch],
) -> anyhow::Result<Vec<u8>> {
    use arrow::ipc::writer::StreamWriter;
    let mut buf = Vec::new();
    let mut writer = StreamWriter::try_new(&mut buf, schema)?;
    for batch in batches {
        writer.write(batch)?;
    }
    writer.finish()?;
    Ok(buf)
}

/// `GET /api/v1/archive/timeseries/{malo_id}?from=RFC3339&to=RFC3339&limit=N`
///
/// Raw time-series export from the Iceberg cold tier.
/// Returns up to `limit` archived reads in chronological order.
pub(crate) async fn get_archive_timeseries(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<ArchiveOlapParams>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-archive-olap", resource_tenant) {
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
    let query = match parse_malo(&malo_id).and_then(|m| open_series(state.repo.store(), m)) {
        Ok(q) => q,
        Err(refusal) => return refusal.into_response(),
    };

    let query = match query.column_eq(
        TENANT_COL,
        datafusion::scalar::ScalarValue::Utf8(Some(state.tenant.clone())),
    ) {
        Ok(q) => q,
        Err(e) => {
            tracing::error!(error = %e, "edmd: series read could not be scoped to the tenant");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    match query.range(from, to).collect().await {
        Ok(Some(series)) if !series.intervals.is_empty() => {
            let rows: Vec<serde_json::Value> = series
                .intervals
                .iter()
                .map(|i| {
                    serde_json::json!({
                        "dtm_from": i.from.to_string(),
                        "dtm_to": i.to.to_string(),
                        "quantity_kwh": i.value.to_string(),
                        "quality": i.quality.as_str(),
                        "obis_code": i.obis_code.map(|o| o.to_string()),
                    })
                })
                .collect();
            Json(serde_json::json!({ "malo_id": malo_id, "rows": rows })).into_response()
        }
        Ok(_) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "no data for this MaLo / period" })),
        )
            .into_response(),
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: archive timeseries query failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ── Analytical SQL endpoint ─────────────────────────────────────────────────────
//
// Runs read-only SQL across both tiers in meterstore's DataFusion session over
// the version-resolved relation (`store.resolved_table()`). Results come back as
// JSON rows (default) or an Arrow IPC stream (`format: "arrow_ipc"`), and carry
// the tier provenance meterstore computed the answer against.
//
// Scope: an edmd deployment writes exactly one tenant (`cfg.tenant`) to every
// row, so its store holds that tenant alone and the result is tenant-scoped by
// construction. A caller that wants an explicit guard adds `WHERE "tenant" = …`;
// the endpoint does not inject one, because it cannot rewrite arbitrary SQL
// safely, and refuses the raw versioned relation so no query can double-count
// corrected intervals.
//
// The value column is `value` and the interval start is `from` (a reserved word,
// so quote it). Example:
//   POST /api/v1/query/sql
//   {"sql": "SELECT malo_id, SUM(\"value\") AS total_kwh
//            FROM meter_reads
//            WHERE \"from\" >= '2026-01-01' AND \"from\" < '2026-02-01'
//            GROUP BY malo_id ORDER BY total_kwh DESC LIMIT 10"}

#[derive(serde::Deserialize)]
pub(crate) struct SqlQueryRequest {
    sql: String,
    /// Maximum rows to return (default: 10_000).
    #[serde(default = "default_sql_limit")]
    limit: usize,
    /// Output format: "json" (default) or "arrow_ipc".
    #[serde(default)]
    format: SqlOutputFormat,
}

pub(crate) fn default_sql_limit() -> usize {
    10_000
}

#[derive(serde::Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SqlOutputFormat {
    #[default]
    Json,
    ArrowIpc,
}

pub(crate) async fn post_sql_query(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Json(req): Json<SqlQueryRequest>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    // Caller-supplied SQL runs against the archive tier, so it is gated by the
    // archive capability, not the generic hot-tier read action.
    if let Err(e) = enforcer.check(&claims.principal(), "read-archive-olap", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // Reject obviously dangerous SQL (allow only SELECT/WITH/SHOW).
    let sql_upper = req.sql.trim().to_uppercase();
    if !sql_upper.starts_with("SELECT")
        && !sql_upper.starts_with("WITH")
        && !sql_upper.starts_with("SHOW")
        && !sql_upper.starts_with("DESCRIBE")
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Only SELECT/WITH/SHOW/DESCRIBE queries are allowed"
            })),
        )
            .into_response();
    }

    // Two relations are out of bounds for caller-supplied SQL.
    //
    // The **raw, every-version** relation, because summing it double-counts
    // every corrected interval — callers use the resolved `meter_reads`.
    //
    // And the **ESA Typ-2** store, both of its relations. Typ-2 values are
    // non-authoritative (Codeliste 1.4 Kap. 4.6): they never reconcile against
    // `meter_reads` and never reach a billing path, and edmd's claim is that the
    // separation is *structural* — the `Typ2Repository` trait shares no read
    // method with the billing store. Every table lives in one DataFusion session,
    // though, so a free-form `SELECT * FROM esa_typ2_reads` walked straight
    // around that: the one query surface where the separation was a naming
    // convention rather than a type.
    let store = state.repo.store();
    let typ2 = state.typ2_repo.store();
    let forbidden = [
        (
            store.raw_table(),
            "the raw versioned relation double-counts corrections",
        ),
        (
            typ2.raw_table(),
            "ESA Typ-2 values are non-authoritative and unreachable from a billing query",
        ),
        (
            typ2.resolved_table(),
            "ESA Typ-2 values are non-authoritative and unreachable from a billing query",
        ),
    ];
    if let Some((relation, why)) = forbidden
        .iter()
        .find(|(name, _)| sql_upper.contains(&name.to_uppercase()))
    {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": format!("query references `{relation}`: {why}"),
                "use_instead": store.resolved_table(),
            })),
        )
            .into_response();
    }

    // Every typed read scopes to the tenant with `column_eq`; caller-supplied SQL
    // cannot, because the caller writes the `WHERE`. `scoped` injects the
    // predicate into the plan and enforces it *below the projection*, so no
    // statement can omit it, alias around it or `UNION` past it — the same
    // guarantee the typed paths have, on the one surface that cannot express it.
    let scoped = match store.scoped(TENANT_COL, state.tenant.as_str()).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "edmd: SQL session could not be scoped to the tenant");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // Run the statement across both tiers in meterstore's own DataFusion session
    // (the resolved relation is registered as `store.resolved_table()`), then take
    // the provenance the result carries. `spans_tiers` / `touched_hot_tier` tell a
    // reporting caller whether the figure is reproducible or read the mutable hot
    // tier — the one fact a bare row set cannot state about itself.
    let limit = req.limit.min(default_sql_limit());
    match scoped.query(&req.sql).await {
        Ok(result) => {
            let touched_hot = result.touched_hot_tier();
            let spans = result.spans_tiers();
            let total = result.num_rows();

            // Analytical clients ask for the columnar stream directly; the tier
            // provenance rides in headers since the body is now binary.
            if req.format == SqlOutputFormat::ArrowIpc {
                return match batches_to_arrow_ipc(&result.schema(), result.batches()) {
                    Ok(bytes) => (
                        StatusCode::OK,
                        [
                            ("content-type", "application/vnd.apache.arrow.stream"),
                            ("x-meterstore-spans-tiers", bool_str(spans)),
                            ("x-meterstore-touched-hot-tier", bool_str(touched_hot)),
                        ],
                        bytes,
                    )
                        .into_response(),
                    Err(e) => {
                        tracing::warn!(error = %e, "edmd: SQL result Arrow-IPC serialisation failed");
                        StatusCode::INTERNAL_SERVER_ERROR.into_response()
                    }
                };
            }

            match result.to_json() {
                Ok(mut rows) => {
                    let truncated = rows.len() > limit;
                    rows.truncate(limit);
                    Json(serde_json::json!({
                        "row_count": rows.len(),
                        "total_rows": total,
                        "truncated": truncated,
                        "spans_tiers": spans,
                        "touched_hot_tier": touched_hot,
                        "rows": rows,
                    }))
                    .into_response()
                }
                Err(e) => {
                    tracing::warn!(error = %e, "edmd: SQL result JSON serialisation failed");
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => (
            // A planning/type error is the caller's SQL, not an edmd fault.
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string(), "sql": req.sql })),
        )
            .into_response(),
    }
}

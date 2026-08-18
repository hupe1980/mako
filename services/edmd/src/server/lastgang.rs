//! Lastgang / Zeitreihe / Summenzeitreihe read endpoints (incl. Arrow IPC and ESA Typ-2).

#[allow(unused_imports)]
use super::*;

// ── Lastgang ──────────────────────────────────────────────────────────────────

/// `GET /api/v1/lastgang/{malo_id}?from=RFC3339&to=RFC3339`
///
/// Returns one `Lastgang` BO4E object per distinct OBIS-Kennzahl found in the
/// requested time window.  Reads without an OBIS code are grouped together
/// under a single `Lastgang` with `obis_kennzahl = null`.
///
/// The interval length (`zeit_intervall_laenge`) is inferred from the first
/// read pair:
/// - 15 min → `Mengeneinheit::ViertelStunde`
/// - 60 min → `Mengeneinheit::Stunde`
/// - other  → `Mengeneinheit::Minute` with the exact value
///
/// The `werte[].zeitraum` uses `startdatum`/`enddatum` (UTC date) plus
/// `startuhrzeit`/`enduhrzeit` in `HH:MM:SS+00:00` format.
///
/// Source: BO4E-Standard; MSCONS AHB Gas/Strom.
#[derive(Debug, Deserialize)]
pub(crate) struct LastgangParams {
    /// RFC 3339 start (inclusive). Defaults to Unix epoch.
    pub(crate) from: Option<String>,
    /// RFC 3339 end (inclusive). Defaults to now.
    pub(crate) to: Option<String>,
    /// Bitemporal point-in-time query (RFC 3339).
    ///
    /// When set, the query returns the meter reads **as they were stored at this timestamp**,
    /// not the current (potentially corrected) values. Enables § 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) point-in-time
    /// billing reconstruction: "what did we know at invoice date 2026-07-01T00:00:00Z?".
    ///
    /// Implementation: queries `meter_read_corrections` to find the state before any
    /// corrections applied after `as_of`. When `None`, returns current (latest) values.
    as_of: Option<String>,
}

pub(crate) async fn get_lastgang(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<LastgangParams>,
    reads_headers: axum::http::HeaderMap,
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

    let (from, to) = match read_window(params.from.as_deref(), params.to.as_deref()) {
        Ok(w) => w,
        Err(refusal) => return refusal.into_response(),
    };
    // ── Bitemporal query: ?as_of= (§ 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) point-in-time reconstruction) ──
    // When `as_of` is set, the read is served through meterstore's transaction-time
    // axis (`as_known_at`): version resolution runs under a `recorded_at` ceiling,
    // so the value returned is the one that was in force at that instant — a
    // correction delivered later, and an interval first stored later, are both
    // invisible. A malformed timestamp is rejected rather than silently returning
    // current values, which for a settlement auditor would be a wrong answer
    // dressed as the right one.
    let as_of_ts = match params.as_of.as_deref() {
        None => None,
        Some(s) => match OffsetDateTime::parse(s, &Rfc3339) {
            Ok(ts) => Some(ts),
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": format!("invalid as_of timestamp {s:?}; expected RFC 3339")
                    })),
                )
                    .into_response();
            }
        },
    };

    let q = TimeSeriesQuery {
        malo_id: malo_id.clone(),
        from,
        to,
        sparte: None,
        tenant: state.tenant.clone(),
    };

    let read_result = match as_of_ts {
        Some(as_of) => state.repo.query_as_of(&q, as_of).await,
        None => state.repo.query(&q).await,
    };
    let reads = match read_result {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: get_lastgang query failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if reads.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(
                serde_json::json!({ "error": "no meter reads for this MaLo in requested window" }),
            ),
        )
            .into_response();
    }

    // Group by OBIS code (None → empty-string sentinel key so BTreeMap works).
    let mut groups: BTreeMap<String, Vec<_>> = BTreeMap::new();
    for r in &reads {
        let key = r.obis_code.clone().unwrap_or_default();
        groups.entry(key).or_default().push(r);
    }

    let lastgaenge: Vec<Lastgang> = groups
        .into_iter()
        .map(|(obis_key, group)| {
            let sparte = edm_sparte_to_bo4e(group[0].sparte);
            let obis_kennzahl = if obis_key.is_empty() {
                None
            } else {
                rubo4e::identifiers::ObisCode::new(&obis_key).ok()
            };

            // Infer interval from first consecutive pair (fallback: 15 min).
            let interval_min = group
                .windows(2)
                .next()
                .map(|w| {
                    (w[1].dtm_from - w[0].dtm_from)
                        .whole_minutes()
                        .unsigned_abs() as u32
                })
                .filter(|&m| m > 0)
                .unwrap_or(15);

            let werte: Vec<Zeitreihenwert> =
                group.iter().map(|r| read_to_zeitreihenwert(r)).collect();

            Lastgang {
                id: None,
                marktlokation: None,
                messgroesse: None,
                messlokation: None,
                obis_kennzahl,
                sparte: Some(sparte),
                typ: None,
                version: None,
                werte: Some(werte),
                zeit_intervall_laenge: minutes_to_menge(interval_min),
                zusatz_attribute: None,
                _additional: Default::default(),
            }
        })
        .collect();

    // ── Arrow IPC response path ────────────────────────────────────────────────
    // If the caller sends `Accept: application/vnd.apache.arrow.stream`, return
    // the raw reads as an Arrow IPC stream instead of BO4E JSON. This gives
    // mabis-syncd and billingd a 10× throughput improvement for bulk reads
    // without requiring gRPC.
    if request_wants_arrow(&reads_headers) {
        return match reads_to_arrow_ipc(&reads) {
            Ok(bytes) => (
                [(
                    axum::http::header::CONTENT_TYPE,
                    "application/vnd.apache.arrow.stream",
                )],
                bytes,
            )
                .into_response(),
            Err(e) => {
                tracing::warn!(error = %e, malo_id, "edmd: arrow IPC serialization failed");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        };
    }

    Json(lastgaenge).into_response()
}

/// `GET /api/v1/feed-in/{malo_id}?from=RFC3339&to=RFC3339`
///
/// Purpose-built quarter-hour **Einspeisung** (feed-in) feed for the §51
/// Negativpreisregel derivation in `einsd`: plain JSON, one entry per ¼h, with a
/// billability flag (§60 Abs. 2 MsbG) and period coverage — so the settlement
/// side does not have to re-parse BO4E or re-derive the export-OBIS filter.
///
/// ```json
/// { "malo_id": "…", "resolution_min": 15, "coverage_pct": 98.5, "billable_pct": 100.0,
///   "intervals": [ { "start": "2026-07-01T12:00:00Z", "kwh": "3.2", "billable": true }, … ] }
/// ```
pub(crate) async fn get_feed_in(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
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

    let (from, to) = match read_window(params.from.as_deref(), params.to.as_deref()) {
        Ok(w) => w,
        Err(refusal) => return refusal.into_response(),
    };
    let q = TimeSeriesQuery {
        malo_id: malo_id.clone(),
        from,
        to,
        sparte: None,
        tenant: state.tenant.clone(),
    };
    let reads = match state.repo.query(&q).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: get_feed_in query failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // Only Einspeisung registers feed the § 51 EEG reduction, and the canonical
    // projection is what decides which those are — including the total-vs-HT/NT
    // rule, which a bare direction filter misses (`domain::register`).
    let selected =
        crate::domain::energy_intervals(&reads, crate::domain::EnergyDirection::Einspeisung);
    let intervals: Vec<serde_json::Value> = selected
        .iter()
        .map(|iv| {
            serde_json::json!({
                "start": iv.from.format(&Rfc3339).unwrap_or_default(),
                "kwh": iv.value.to_string(),
                // The projection admits only billable qualities, so every
                // interval here is one — but Estimated/Substituted are among
                // them, so the flag is reported rather than assumed.
                "quality": crate::store::quality_to_str(iv.quality),
            })
        })
        .collect();

    let count = intervals.len();
    // Coverage is a **duration ratio**, measured against the cadence the series
    // actually delivers. Assuming a quarter-hour grid reported a legitimately
    // hourly feed-in series as 25 % covered.
    let covered: i64 = selected
        .iter()
        .map(|iv| (iv.to - iv.from).whole_seconds().max(0))
        .sum();
    let window = (to - from).whole_seconds().max(1);
    #[allow(clippy::cast_precision_loss)]
    let coverage_pct = (covered as f64 / window as f64 * 100.0).clamp(0.0, 100.0);
    let resolution_min = metering::classification::detect_interval_length(&selected)
        .map(|r| r.nominal_seconds() / 60);

    Json(serde_json::json!({
        "malo_id": malo_id,
        "resolution_min": resolution_min,
        "coverage_pct": coverage_pct,
        "interval_count": count,
        "intervals": intervals,
    }))
    .into_response()
}

/// `true` when the request `Accept` header requests Arrow IPC stream format.
pub(crate) fn request_wants_arrow(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.contains("application/vnd.apache.arrow.stream"))
        .unwrap_or(false)
}

/// The precision the storage column and the Arrow schema share.
///
/// `meter_reads.quantity_kwh` is `NUMERIC(18,5)` because a settled energy figure
/// is a Buchungsbeleg (§ 147 AO / GoBD) and must be exact. The bulk transport has
/// to carry the same type: `mabis-syncd` and `billingd` read Lastgang and
/// Zeitreihe over this stream precisely *because* it is the high-volume path,
/// and it used to hand them `Float64`. Binary floating point cannot represent
/// 0.1 kWh, so every value crossed the wire rounded, and a month of quarter-hours
/// summed to a total that did not match the one the store would compute.
const QUANTITY_PRECISION: u8 = 18;
const QUANTITY_SCALE: i8 = 5;

/// Serialise a slice of `MeterRead` rows to an Arrow IPC stream.
///
/// Schema: `malo_id Utf8 · dtm_from TimestampMicrosecond(UTC) ·
/// dtm_to TimestampMicrosecond(UTC) · quantity_kwh Decimal128(18,5) ·
/// quality Utf8 · sparte Utf8 · obis_code Utf8(nullable) · pid Int32`.
///
/// Callers that receive `Content-Type: application/vnd.apache.arrow.stream`
/// can read the result with any Arrow library (DuckDB, Polars, PyArrow, etc.);
/// all of them map `Decimal128` onto an exact decimal type.
pub(crate) fn reads_to_arrow_ipc(reads: &[crate::domain::MeterRead]) -> anyhow::Result<Vec<u8>> {
    use arrow::array::{
        Decimal128Array, Int32Array, StringArray, StringBuilder, TimestampMicrosecondArray,
    };
    use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
    use arrow::ipc::writer::StreamWriter;
    use arrow::record_batch::RecordBatch;
    use std::sync::Arc;

    let schema = Arc::new(Schema::new(vec![
        Field::new("malo_id", DataType::Utf8, false),
        Field::new(
            "dtm_from",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            false,
        ),
        Field::new(
            "dtm_to",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            false,
        ),
        Field::new(
            "quantity_kwh",
            DataType::Decimal128(QUANTITY_PRECISION, QUANTITY_SCALE),
            false,
        ),
        Field::new("quality", DataType::Utf8, false),
        Field::new("sparte", DataType::Utf8, false),
        Field::new("obis_code", DataType::Utf8, true),
        Field::new("pid", DataType::Int32, false),
    ]));

    let n = reads.len();
    let malo_ids: StringArray = reads.iter().map(|r| Some(r.malo_id.as_str())).collect();
    let dtm_froms: TimestampMicrosecondArray = TimestampMicrosecondArray::from(
        reads
            .iter()
            .map(|r| (r.dtm_from.unix_timestamp_nanos() / 1_000) as i64)
            .collect::<Vec<i64>>(),
    )
    .with_timezone_opt(Some("UTC".to_string()));
    let dtm_tos: TimestampMicrosecondArray = TimestampMicrosecondArray::from(
        reads
            .iter()
            .map(|r| (r.dtm_to.unix_timestamp_nanos() / 1_000) as i64)
            .collect::<Vec<i64>>(),
    )
    .with_timezone_opt(Some("UTC".to_string()));
    // A `Decimal` rescaled to five places is an exact `i128` of scaled units;
    // `mantissa()` after `rescale` is that integer, which is what Arrow's
    // Decimal128 stores. A value too large for `NUMERIC(18,5)` could not have
    // been stored in the first place, so it is refused here rather than wrapped.
    let quantities: Decimal128Array = reads
        .iter()
        .map(|r| {
            let mut d = r.quantity_kwh;
            d.rescale(u32::from(QUANTITY_SCALE.unsigned_abs()));
            Some(d.mantissa())
        })
        .collect::<Decimal128Array>()
        .with_precision_and_scale(QUANTITY_PRECISION, QUANTITY_SCALE)
        .map_err(|e| anyhow::anyhow!("quantity_kwh does not fit NUMERIC(18,5): {e}"))?;
    // One spelling of a quality flag across the whole service — the same
    // `quality_to_str` the store writes and the JSON responses render, so the
    // columnar stream cannot drift into a second vocabulary.
    let qualities: StringArray = reads
        .iter()
        .map(|r| Some(crate::store::quality_to_str(r.quality)))
        .collect();
    let spartes: StringArray = reads.iter().map(|r| Some(r.sparte.as_str())).collect();
    let mut obis_builder = StringBuilder::with_capacity(n, n * 12);
    for r in reads {
        match &r.obis_code {
            Some(o) => obis_builder.append_value(o),
            None => obis_builder.append_null(),
        }
    }
    let obis_codes = obis_builder.finish();
    let pids: Int32Array = reads.iter().map(|r| Some(r.pid as i32)).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(malo_ids),
            Arc::new(dtm_froms),
            Arc::new(dtm_tos),
            Arc::new(quantities),
            Arc::new(qualities),
            Arc::new(spartes),
            Arc::new(obis_codes),
            Arc::new(pids),
        ],
    )
    .map_err(|e| anyhow::anyhow!("RecordBatch: {e}"))?;

    let mut buf = Vec::new();
    let mut writer = StreamWriter::try_new(&mut buf, &schema)
        .map_err(|e| anyhow::anyhow!("StreamWriter: {e}"))?;
    writer
        .write(&batch)
        .map_err(|e| anyhow::anyhow!("write batch: {e}"))?;
    writer
        .finish()
        .map_err(|e| anyhow::anyhow!("finish: {e}"))?;
    Ok(buf)
}

// ── Zeitreihe ─────────────────────────────────────────────────────────────────

/// `GET /api/v1/zeitreihe/{malo_id}?from=RFC3339&to=RFC3339`
///
/// Returns one `Zeitreihe` BO4E object per distinct OBIS-Kennzahl found in
/// the requested time window.  Unlike [`get_lastgang`], which carries interval
/// metadata (`zeit_intervall_laenge`, OBIS code, Sparte), `Zeitreihe` exposes
/// the generic time-series contract used by API-Webdienste Strom consumers.
///
/// - `messart` is set to `Mittelwert` (interval-average, typical for SLP/RLM).
/// - `einheit` is set to `kWh`.
/// - `medium` reflects the commodity (Strom / Gas).
///
/// Source: BO4E-Standard Zeitreihe; API-Webdienste Strom §5.3.
/// `GET /api/v1/esa/typ2/{malo_id}` — read ESA "Werte nach Typ 2" for a MaLo.
///
/// Reads the separate `esa_typ2_reads` store exclusively. These values are
/// non-authoritative (Codeliste 1.4 Kap. 4.6 · WiM Strom Teil 2 §4) and have no
/// bearing on billing — this endpoint exists so the ESA can retrieve what it was
/// delivered, kept structurally apart from every billing/aggregation endpoint.
pub(crate) async fn get_esa_typ2(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<LastgangParams>,
) -> impl IntoResponse {
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
    let q = TimeSeriesQuery {
        malo_id: malo_id.clone(),
        from,
        to,
        sparte: None,
        tenant: state.tenant.clone(),
    };

    match state.typ2_repo.query_typ2(&q).await {
        Ok(reads) => Json(serde_json::json!({
            "malo_id": malo_id,
            "non_authoritative": true,
            "hinweis": "Werte nach Typ 2 — ohne Bezug zur Netznutzungs-, Bilanzkreis- \
                        oder Mehr-/Mindermengenabrechnung (Codeliste 1.4 Kap. 4.6).",
            "count": reads.len(),
            "werte": reads,
        }))
        .into_response(),
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: get_esa_typ2 query failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub(crate) async fn get_zeitreihe(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<LastgangParams>,
    zr_headers: axum::http::HeaderMap,
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

    let (from, to) = match read_window(params.from.as_deref(), params.to.as_deref()) {
        Ok(w) => w,
        Err(refusal) => return refusal.into_response(),
    };
    // Same transaction-time semantics as `get_lastgang`: an `?as_of=` reads
    // through meterstore's `recorded_at` ceiling, a malformed one is rejected.
    let as_of_ts = match params.as_of.as_deref() {
        None => None,
        Some(s) => match OffsetDateTime::parse(s, &Rfc3339) {
            Ok(ts) => Some(ts),
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": format!("invalid as_of timestamp {s:?}; expected RFC 3339")
                    })),
                )
                    .into_response();
            }
        },
    };

    let q = TimeSeriesQuery {
        malo_id: malo_id.clone(),
        from,
        to,
        sparte: None,
        tenant: state.tenant.clone(),
    };

    let read_result = match as_of_ts {
        Some(as_of) => state.repo.query_as_of(&q, as_of).await,
        None => state.repo.query(&q).await,
    };
    let reads = match read_result {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: get_zeitreihe query failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if reads.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(
                serde_json::json!({ "error": "no meter reads for this MaLo in requested window" }),
            ),
        )
            .into_response();
    }

    // Group by OBIS code.
    let mut groups: BTreeMap<String, Vec<_>> = BTreeMap::new();
    for r in &reads {
        let key = r.obis_code.clone().unwrap_or_default();
        groups.entry(key).or_default().push(r);
    }

    let zeitreihen: Vec<Zeitreihe> = groups
        .into_iter()
        .map(|(obis_key, group)| {
            let medium = edm_sparte_to_medium(group[0].sparte);
            let bezeichnung = if obis_key.is_empty() {
                format!("Zeitreihe MaLo {malo_id}")
            } else {
                format!("Zeitreihe MaLo {malo_id} OBIS {obis_key}")
            };
            let werte: Vec<Zeitreihenwert> =
                group.iter().map(|r| read_to_zeitreihenwert(r)).collect();
            Zeitreihe {
                bezeichnung: Some(bezeichnung),
                einheit: Some(edm_sparte_to_einheit(group[0].sparte)),
                medium: Some(medium),
                messart: Some(Messart::Mittelwert),
                werte: Some(werte),
                ..Default::default()
            }
        })
        .collect();

    // Arrow IPC response path — same reads, binary columnar format.
    if request_wants_arrow(&zr_headers) {
        return match reads_to_arrow_ipc(&reads) {
            Ok(bytes) => (
                [(
                    axum::http::header::CONTENT_TYPE,
                    "application/vnd.apache.arrow.stream",
                )],
                bytes,
            )
                .into_response(),
            Err(e) => {
                tracing::warn!(error = %e, malo_id, "edmd: zeitreihe arrow IPC serialization failed");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        };
    }

    Json(zeitreihen).into_response()
}

// ── Resampled Lastgang ───────────────────────────────────────────────────

/// Query parameters for resampled Lastgang.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct ResampledParams {
    from: Option<String>,
    to: Option<String>,
    /// Target resolution: `HOUR`, `DAY`, `MONTH`, `YEAR`. Default: `HOUR`.
    resolution: Option<String>,
}

/// `GET /api/v1/lastgang/{malo_id}/resampled`
///
/// Returns the metered time series down-sampled to a coarser time resolution.
/// Useful for dashboards, billing previews, and Mehr-/Mindermengensaldo summaries.
///
/// | Resolution | Use case |
/// |---|---|
/// | `HOUR` | Hourly dashboard chart (default) |
/// | `DAY` | Daily totals for SLP billing |
/// | `MONTH` | Monthly totals for MMM / GPKE Teil 1 Kap. 8.4 |
/// | `YEAR` | Annual settlement |
///
/// Each bucket carries:
/// - `total_kwh` — summed energy
/// - `peak_kw` — maximum 15-min demand kW (RLM Strom)
/// - `coverage_pct` — completeness indicator
/// - `has_missing_data` — `true` when source intervals are missing
pub(crate) async fn get_lastgang_resampled(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<ResampledParams>,
) -> impl IntoResponse {
    use metering::{ResampleConfig, resample};

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
    let config = match params
        .resolution
        .as_deref()
        .unwrap_or("HOUR")
        .to_uppercase()
        .as_str()
    {
        "HOUR" => ResampleConfig::to_hourly(),
        "DAY" => ResampleConfig::to_daily(),
        "MONTH" => ResampleConfig::to_monthly(),
        "YEAR" => ResampleConfig::to_yearly(),
        other => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("unknown resolution {other:?} — use HOUR, DAY, MONTH, or YEAR")
                })),
            )
                .into_response();
        }
    };

    let q = TimeSeriesQuery {
        malo_id: malo_id.clone(),
        from,
        to,
        sparte: None,
        tenant: state.tenant.clone(),
    };

    let reads = match state.repo.query(&q).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: get_lastgang_resampled query failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if reads.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "no meter reads for this MaLo in requested window"
            })),
        )
            .into_response();
    }

    // Project onto one canonical energy series before resampling. A resolved read
    // spans every register the measuring point reported, and a bucket total built
    // from all of them adds a prosumer's Einspeisung to its Bezug and counts a
    // dual-tariff meter's consumption twice (`domain::register`). Non-billable
    // qualities are dropped by the same projection (§ 60 Abs. 2).
    let intervals = crate::domain::energy_intervals(&reads, crate::domain::EnergyDirection::Bezug);

    let buckets = resample(&intervals, &config);

    let response: Vec<serde_json::Value> = buckets
        .iter()
        .map(|b| {
            serde_json::json!({
                "from": b.from,
                "to": b.to,
                "total_kwh": b.total,
                "peak_kw": b.peak_kw,
                "interval_count": b.interval_count,
                "expected_count": b.expected_count,
                "coverage_pct": b.coverage_pct(),
                "has_missing_data": b.has_missing_data(),
                "quality": format!("{:?}", b.quality),
            })
        })
        .collect();

    Json(serde_json::json!({
        "malo_id": malo_id,
        "resolution": params.resolution.as_deref().unwrap_or("HOUR"),
        "from": from,
        "to": to,
        "bucket_count": response.len(),
        "buckets": response,
    }))
    .into_response()
}

// ── Summenzeitreihe ─────────────────────────────────────────────────────

/// `GET /api/v1/summenzeitreihe/{malo_id}?from=&to=`
///
/// Returns monthly aggregated energy data (Summenzeitreihe) for a MaLo.
///
/// This is the canonical data format for:
/// - MABIS balance group accounting (PID 13003)
/// - Mehr-/Mindermengensaldo (GPKE Teil 1 Kap. 8.4)
/// - Annual Jahresabrechnung summaries
///
/// Each month bucket includes: `total_kwh`, `peak_kw`, `coverage_pct`, `quality`.
pub(crate) async fn get_summenzeitreihe(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<SimpleTimeParams>,
) -> impl IntoResponse {
    use metering::{ResampleConfig, resample};

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
    let (from, to) = match read_window(params.from.as_deref(), params.to.as_deref()) {
        Ok(w) => w,
        Err(refusal) => return refusal.into_response(),
    };
    let q = TimeSeriesQuery {
        malo_id: malo_id.clone(),
        from,
        to,
        sparte: None,
        tenant: state.tenant.clone(),
    };
    let reads = match state.repo.query(&q).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: summenzeitreihe query failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // The Summenzeitreihe feeds MaBiS and the Mehr-/Mindermengensaldo, so it is
    // the **Bezug**, projected onto one canonical register set: billable
    // qualities only (§ 60 Abs. 2), no Einspeisung folded in, and no total
    // register added to its own HT/NT split (`domain::register`).
    let intervals = crate::domain::energy_intervals(&reads, crate::domain::EnergyDirection::Bezug);

    let buckets = resample(&intervals, &ResampleConfig::to_monthly());
    let total_kwh: rust_decimal::Decimal = buckets.iter().map(|b| b.total).sum();

    let months: Vec<serde_json::Value> = buckets
        .iter()
        .map(|b| {
            serde_json::json!({
                "from": b.from,
                "to": b.to,
                "total_kwh": b.total,
                "peak_kw": b.peak_kw,
                "coverage_pct": b.coverage_pct(),
                "has_missing_data": b.has_missing_data(),
                "quality": format!("{:?}", b.quality),
            })
        })
        .collect();

    Json(serde_json::json!({
        "malo_id": malo_id,
        "from": from,
        "to": to,
        "total_kwh": total_kwh,
        "month_count": months.len(),
        "months": months,
        "legal_basis": "MABIS PID 13003 / GPKE (BK6-24-174) Teil 1 Kap. 8.4 Mehr-/Mindermengensaldo",
    }))
    .into_response()
}

#[cfg(test)]
mod arrow_transport_tests {
    use super::*;
    use crate::domain::{IngestionSource, MeterRead, QualityFlag, Sparte};
    use time::macros::datetime;

    fn read(kwh: &str) -> MeterRead {
        MeterRead {
            malo_id: "51238696012".to_owned(),
            melo_id: None,
            dtm_from: datetime!(2026-07-01 10:00 UTC),
            dtm_to: datetime!(2026-07-01 10:15 UTC),
            quantity_kwh: kwh.parse().expect("decimal"),
            quality: QualityFlag::Measured,
            pid: 13025,
            sparte: Sparte::Strom,
            obis_code: Some("1-0:1.8.0".to_owned()),
            tenant: "9900357000004".to_owned(),
            source: IngestionSource::Mscons,
            push_session: None,
            quality_warnings: None,
            sender_mp_id: None,
            allocation_version: "INITIAL".to_owned(),
            valid_from_tx: None,
            mscons_version: None,
        }
    }

    /// The bulk path carries the stored value exactly, not a float of it.
    ///
    /// `0.1` has no binary floating-point representation, so an `f64` column
    /// handed `mabis-syncd` and `billingd` a different number than the store
    /// holds — on the transport chosen *because* it is the high-volume one, and
    /// for a figure § 147 AO requires to be exact.
    #[test]
    fn the_arrow_stream_carries_the_exact_stored_decimal() {
        use arrow::array::Array as _;

        let reads = [read("0.10000"), read("2.34567"), read("123456.78901")];
        let bytes = reads_to_arrow_ipc(&reads).expect("serialises");

        let mut reader =
            arrow::ipc::reader::StreamReader::try_new(std::io::Cursor::new(bytes), None)
                .expect("valid IPC stream");
        let batch = reader.next().expect("one batch").expect("readable");

        let column = batch
            .column_by_name("quantity_kwh")
            .expect("quantity_kwh column");
        assert_eq!(
            column.data_type(),
            &arrow::datatypes::DataType::Decimal128(QUANTITY_PRECISION, QUANTITY_SCALE),
            "the wire type must match NUMERIC(18,5)"
        );
        let values = column
            .as_any()
            .downcast_ref::<arrow::array::Decimal128Array>()
            .expect("Decimal128");
        for (i, expected) in ["0.10000", "2.34567", "123456.78901"].iter().enumerate() {
            assert_eq!(
                values.value_as_string(i),
                *expected,
                "row {i} must round-trip exactly"
            );
        }
    }

    /// The columnar path spells quality flags the way the store does.
    #[test]
    fn arrow_quality_flags_use_the_stored_vocabulary() {
        for flag in QualityFlag::ALL {
            let mut r = read("1.0");
            r.quality = flag;
            let bytes = reads_to_arrow_ipc(&[r]).expect("serialises");
            let mut reader =
                arrow::ipc::reader::StreamReader::try_new(std::io::Cursor::new(bytes), None)
                    .expect("valid IPC stream");
            let batch = reader.next().expect("one batch").expect("readable");
            let quality = batch
                .column_by_name("quality")
                .expect("quality column")
                .as_any()
                .downcast_ref::<arrow::array::StringArray>()
                .expect("Utf8");
            assert_eq!(quality.value(0), crate::store::quality_to_str(flag));
        }
    }
}

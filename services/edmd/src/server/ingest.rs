//! Direct-push ingest (iMSys/SMGW/Gas), V01–V10 validation, corrections and bulk ingestion.

#[allow(unused_imports)]
use super::*;

// \u2500\u2500 iMSys / SMGW 15-min direct push \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500

/// One 15-min (or other fixed-length) metered interval in a direct-push batch.
#[derive(Debug, serde::Deserialize)]
pub struct DirectInterval {
    /// Interval start (RFC 3339 UTC).  Must be an exact quarter-hour for iMSys.
    #[serde(with = "time::serde::rfc3339")]
    pub from: OffsetDateTime,
    /// Interval end (RFC 3339 UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub to: OffsetDateTime,
    /// Energy quantity, expressed in [`Self::unit`].
    pub value: Decimal,
    /// Physical unit the meter registered, parsed by
    /// [`metering::interval::MeasurementUnit::parse_scaled`]: `kWh`/`MWh`/`GJ`/
    /// `MJ`/`Wh` for energy, `m3`/`m\u00b3`/`l` for volume. It must be either the
    /// unit the Sparte is measured in or the one it is billed in; anything else
    /// is rejected rather than stored under a guessed interpretation.
    #[serde(default = "default_unit_kwh")]
    pub unit: String,
    /// Reading quality per BDEW Messwertstatus.
    #[serde(default)]
    pub quality: Option<String>,
}

pub(crate) fn default_unit_kwh() -> String {
    "kWh".to_owned()
}

/// Request body for `POST /api/v1/meter-reads/rlm/{malo_id}`
/// and `POST /api/v1/meter-reads/gas/{malo_id}`.
///
/// Designed for SMGW direct push, iMSys CLS gateway, and ERP data import.
/// Each call is idempotent when the same `session_id` is re-submitted.
///
/// ## Format
///
/// ```json
/// {
///   "session_id": "SMGW-SN1234-2026-07-12T00:00:00Z",
///   "source": "SMGW",
///   "obis_code": "1-0:1.8.0",
///   "melo_id": "DE00001234567890723456789012345",
///   "intervals": [
///     { "from": "2026-07-12T00:00:00Z", "to": "2026-07-12T00:15:00Z", "value": "2.345" },
///     { "from": "2026-07-12T00:15:00Z", "to": "2026-07-12T00:30:00Z", "value": "2.412" }
///   ]
/// }
/// ```
///
/// ## Gas variant
///
/// For Gas, set `unit = "m3"` and supply `brennwert_kwh_per_m3` + `zustandszahl`
/// for the Hs-based conversion.  The handler stores converted kWh_Hs values.
#[derive(Debug, serde::Deserialize)]
pub struct DirectPushRequest {
    /// Caller-supplied idempotency key (e.g. SMGW SN + timestamp).
    /// Re-submitting the same key returns 200 with the original result.
    pub session_id: Option<String>,
    /// Human-readable source identifier (e.g. `"SMGW"`, `"CLS_GATEWAY"`, `"ERP"`).
    #[serde(default = "default_source")]
    pub source: String,
    /// OBIS-Kennzahl (e.g. `"1-0:1.8.0"` for Wirkarbeit Tarif 1 + 2).
    pub obis_code: Option<String>,
    /// 33-character MeLo-ID (optional but recommended for device tracing).
    pub melo_id: Option<String>,
    /// MP-ID of the sender (MSB or SMGW system). Stored as `sender_mp_id` per § 60 Abs. 6 MsbG
    /// per-interval MSB attribution — required after a WiM MSB switch (PID 55039).
    pub sender_mp_id: Option<String>,
    /// Metered intervals (15-min for iMSys; 60-min or 1440-min for SLP).
    pub intervals: Vec<DirectInterval>,
    // ── Gas-specific fields ───────────────────────────────────────────────────
    /// Brennwert (superior calorific value) in kWh/m³ — required when `unit = "m3"`.
    pub brennwert_kwh_per_m3: Option<Decimal>,
    /// Zustandszahl (volume correction factor) — default 1.0 when absent.
    pub zustandszahl: Option<Decimal>,
}

pub(crate) fn default_source() -> String {
    "DIRECT_PUSH".to_owned()
}

/// `POST /api/v1/meter-reads/rlm/{malo_id}`
///
/// iMSys / SMGW direct push for **Strom RLM** and **iMSys** customers.
///
/// ## Why direct push?
///
/// - MSCONS round-trip via `makod` adds 15\u201360 min latency.
/// - \u00a741a EnWG dynamic tariffs need sub-hourly resolution for real-time billing.
/// - High-frequency RLM meters (up to 96 intervals/day) saturate the EDIFACT pipeline.
///
/// ## Idempotency
///
/// Submit the same `session_id` twice to get the stored result back without re-processing.
///
/// ## Quality scoring
///
/// Gap detection, consecutive-zero analysis, and 3-sigma outlier detection run at
/// ingest time.  If `has_warnings = true`, `edmd` emits `de.messwert.reading.quality.warning`
/// to the ERP webhook so `agentd` can investigate.
pub async fn post_direct_reads_rlm(
    State(state): State<HandlerState>,
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Path(malo_id): Path<String>,
    Json(req): Json<DirectPushRequest>,
) -> impl IntoResponse {
    // Cedar RBAC: only MSB, LF, or admin roles may push direct reads.
    if let Err(e) = enforcer.check(
        &claims.principal(),
        "write-meter-reads",
        state.tenant.as_str(),
    ) {
        tracing::warn!(malo_id, error = %e, "edmd: direct push RBAC denied");
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    if req.intervals.is_empty() {
        return (StatusCode::BAD_REQUEST, "intervals must not be empty").into_response();
    }

    post_direct_reads_inner(&state, &malo_id, req, "STROM", "DIRECT_PUSH").await
}

/// `POST /api/v1/meter-reads/gas/{malo_id}`
///
/// iMSys / SMGW direct push for **Gas RLM** customers.
/// Accepts m\u00b3 readings and converts to kWh_Hs using Brennwert \u00d7 Zustandszahl.
pub async fn post_direct_reads_gas(
    State(state): State<HandlerState>,
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Path(malo_id): Path<String>,
    Json(req): Json<DirectPushRequest>,
) -> impl IntoResponse {
    if let Err(_e) = enforcer.check(
        &claims.principal(),
        "write-meter-reads",
        state.tenant.as_str(),
    ) {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    if req.intervals.is_empty() {
        return (StatusCode::BAD_REQUEST, "intervals must not be empty").into_response();
    }

    post_direct_reads_inner(&state, &malo_id, req, "GAS", "DIRECT_GAS").await
}

/// Internal implementation shared by Strom and Gas direct-push handlers.
#[allow(clippy::too_many_lines)]
/// Parse a caller-supplied quality flag from its wire spelling.
///
/// Returns `None` for anything outside the set, so an unrecognised flag is
/// refused at the boundary rather than stored as `UNKNOWN` — a caller that
/// asserts a quality we cannot interpret has made a claim about billability
/// that must not be silently downgraded.
pub(crate) fn quality_flag_from_wire(s: &str) -> Option<QualityFlag> {
    match s.to_uppercase().as_str() {
        "MEASURED" => Some(QualityFlag::Measured),
        "ESTIMATED" => Some(QualityFlag::Estimated),
        "SUBSTITUTED" => Some(QualityFlag::Substituted),
        "CALCULATED" => Some(QualityFlag::Calculated),
        "CORRECTED" => Some(QualityFlag::Corrected),
        "PRELIMINARY" => Some(QualityFlag::Preliminary),
        "FAULTY" => Some(QualityFlag::Faulty),
        "UNKNOWN" => Some(QualityFlag::Unknown),
        _ => None,
    }
}

pub(crate) async fn post_direct_reads_inner(
    state: &HandlerState,
    malo_id: &str,
    req: DirectPushRequest,
    sparte_str: &str,
    source_default: &str,
) -> axum::response::Response {
    use rust_decimal::Decimal;

    // Bound the batch like the bulk path (`MAX_BATCH` below): an unbounded
    // direct-push batch is write amplification the request-rate limiter cannot
    // see — one request can carry millions of intervals and saturate the write
    // path for every other tenant.
    const MAX_BATCH: usize = 50_000;
    if req.intervals.len() > MAX_BATCH {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("batch too large: {} > {MAX_BATCH}", req.intervals.len()),
        )
            .into_response();
    }

    let pool = state.repo.pool();
    let source = if req.source.is_empty() {
        source_default.to_owned()
    } else {
        req.source.clone()
    };

    // \u2500\u2500 Idempotency check \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    let session_id = req.session_id.clone().unwrap_or_else(|| {
        // Auto-generate from malo_id + first interval timestamp
        req.intervals
            .first()
            .map(|iv| format!("{malo_id}-{}", iv.from))
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    });

    // Check if this session was already committed.
    //
    // Scoped by tenant: two tenants may legitimately use the same `session_id`
    // for the same MaLo-ID, and without it one would read the other's summary
    // and skip its own ingest.
    let existing: Option<serde_json::Value> = match sqlx::query_scalar(
        r"SELECT quality_summary FROM direct_push_sessions
          WHERE session_id = $1 AND malo_id = $2 AND tenant = $3
            AND status = 'committed'",
    )
    .bind(&session_id)
    .bind(malo_id)
    .bind(&state.tenant)
    .fetch_optional(state.repo.pool())
    .await
    {
        Ok(row) => row.flatten(),
        // A failed lookup is not evidence that the session is new. Re-ingesting
        // on a transient database error would be safe for the readings, which
        // upsert, but it would also re-emit the CloudEvents that trigger a
        // billing recompute downstream.
        Err(e) => {
            tracing::error!(malo_id, error = %e, "edmd: direct push idempotency check failed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "could not verify whether this session was already committed",
                })),
            )
                .into_response();
        }
    };

    if let Some(summary) = existing {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "session_id": session_id,
                "malo_id": malo_id,
                "status": "already_committed",
                "quality": summary,
            })),
        )
            .into_response();
    }

    // \u2500\u2500 Interval validation + kWh conversion \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    // Gas m³ → kWh_Hs via metering::gas_m3_to_kwh_hs (§25 Nr. 4 MessEV / DVGW G 685)
    // Units are parsed by the same `metering` machinery as the IoT path, so the
    // ingest families share one unit contract. A string compare against `"m3"`
    // missed the superscript `"m³"` that `MeasurementUnit` accepts, and the
    // electricity endpoint never checked the unit against the Sparte at all.
    let msparte = match sparte_str {
        "GAS" => metering::interval::Sparte::Gas,
        _ => metering::interval::Sparte::Strom,
    };
    let z = req.zustandszahl.unwrap_or(Decimal::ONE);

    let mut accepted: Vec<&DirectInterval> = Vec::new();
    let mut rejected_count = 0usize;
    let mut validation_errors: Vec<String> = Vec::new();

    for iv in &req.intervals {
        if iv.from >= iv.to {
            validation_errors.push(format!(
                "interval from={} to={}: from must be before to",
                iv.from, iv.to
            ));
            rejected_count += 1;
            continue;
        }
        let duration_secs = (iv.to - iv.from).whole_seconds();
        if duration_secs <= 0 || duration_secs > 86400 {
            validation_errors.push(format!(
                "interval from={}: duration {}s is out of range [1, 86400]",
                iv.from, duration_secs
            ));
            rejected_count += 1;
            continue;
        }
        if iv.value < Decimal::ZERO {
            validation_errors.push(format!(
                "interval from={}: negative value {}",
                iv.from, iv.value
            ));
            rejected_count += 1;
            continue;
        }
        let Some(scale) = metering::interval::MeasurementUnit::parse_scaled(&iv.unit) else {
            validation_errors.push(format!(
                "interval from={}: unknown unit `{}`; expected kWh/MWh/GJ/MJ/Wh (energy) \
                 or m³/l (volume)",
                iv.from, iv.unit
            ));
            rejected_count += 1;
            continue;
        };
        if scale.unit != msparte.measured_unit() && scale.unit != msparte.billing_unit() {
            validation_errors.push(format!(
                "interval from={}: unit {} is not valid for sparte {} — expected {} (as measured) \
                 or {} (as billed)",
                iv.from,
                scale.unit.as_str(),
                msparte.as_str(),
                msparte.measured_unit().as_str(),
                msparte.billing_unit().as_str()
            ));
            rejected_count += 1;
            continue;
        }
        if msparte.requires_conversion()
            && scale.unit == msparte.measured_unit()
            && req.brennwert_kwh_per_m3.is_none()
        {
            validation_errors.push(format!(
                "interval from={}: brennwert_kwh_per_m3 is required when submitting gas in m³ \
                 (§25 Nr. 4 MessEV); submit unit=kWh to supply pre-converted values",
                iv.from
            ));
            rejected_count += 1;
            continue;
        }
        if let Some(q) = iv.quality.as_deref()
            && quality_flag_from_wire(q).is_none()
        {
            validation_errors.push(format!(
                "interval from={}: unknown quality `{q}`; expected one of MEASURED, \
                 ESTIMATED, SUBSTITUTED, CALCULATED, CORRECTED, PRELIMINARY, FAULTY, UNKNOWN",
                iv.from
            ));
            rejected_count += 1;
            continue;
        }
        accepted.push(iv);
    }

    if accepted.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "all intervals failed validation",
                "validation_errors": validation_errors,
            })),
        )
            .into_response();
    }

    // \u2500\u2500 Quality scoring \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    let period_start = accepted.iter().map(|iv| iv.from).min().unwrap();
    let period_end = accepted.iter().map(|iv| iv.to).max().unwrap();

    let mut quality = compute_quality(&accepted, period_start, period_end);
    quality.intervals_rejected = rejected_count;

    record_quality_assessment(
        pool,
        &state.tenant,
        malo_id,
        period_start,
        period_end,
        &source,
        &quality,
    )
    .await;

    let quality_json = serde_json::json!({
        "intervals_accepted": quality.intervals_accepted,
        "intervals_rejected": quality.intervals_rejected,
        "gaps_detected": quality.gaps_detected,
        "zero_run_length": quality.zero_run_length,
        "outlier_intervals": quality.outlier_intervals,
        "spike_intervals": quality.spike_intervals,
        "intervals_consistent": quality.intervals_consistent,
        "has_warnings": quality.has_warnings,
        "coverage_pct": quality.coverage_pct,
        "grade": quality.grade,
        "algorithm": "hampel_k3_t3",
    });

    // \u2500\u2500 Persist intervals \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    let obis_code = req.obis_code.as_deref();
    let melo_id = req.melo_id.as_deref();

    let sparte_enum = match sparte_str {
        "GAS" => EdmSparte::Gas,
        _ => EdmSparte::Strom,
    };
    let ingestion_source = IngestionSource::from_db_str(&source);

    let mut batch: Vec<MeterRead> = Vec::with_capacity(accepted.len());
    for iv in &accepted {
        // Every accepted interval parsed in the loop above, so the unit is known
        // and valid for this Sparte.
        let scale = metering::interval::MeasurementUnit::parse_scaled(&iv.unit)
            .expect("unit validated in the accept loop");
        let rescaled = scale.apply(iv.value);
        // m³ → kWh_Hs for Gas (§25 Nr. 4 MessEV / DVGW G 685). The Brennwert is
        // required rather than defaulted: it varies by supply area and month, so
        // a national average would systematically mis-bill an L-Gas network.
        let kwh = if msparte.requires_conversion() && scale.unit == msparte.measured_unit() {
            let hs = req
                .brennwert_kwh_per_m3
                .expect("brennwert presence validated in the accept loop");
            metering::gas_m3_to_kwh_hs(rescaled, hs, z)
        } else {
            rescaled
        };

        batch.push(MeterRead {
            malo_id: malo_id.to_owned(),
            melo_id: melo_id.map(str::to_owned),
            dtm_from: iv.from,
            dtm_to: iv.to,
            quantity_kwh: kwh,
            // Unrecognised flags are rejected in the accept loop, so the
            // fallback only covers an omitted one. A direct push carries a
            // register reading, so it defaults to MEASURED — matching the IoT
            // path, and leaving substitution to the § 60 Abs. 2 MsbG flow that
            // records who substituted and why.
            quality: iv
                .quality
                .as_deref()
                .and_then(quality_flag_from_wire)
                .unwrap_or(QualityFlag::Measured),
            pid: 0, // no MSCONS process behind a direct push
            sparte: sparte_enum,
            obis_code: obis_code.map(str::to_owned),
            tenant: state.tenant.clone(),
            source: ingestion_source,
            push_session: Some(session_id.clone()),
            // Session-level Hampel scoring. `ValidatedReads::validate` adds the
            // per-interval V01–V10 findings under a `validation` key.
            quality_warnings: quality.has_warnings.then(|| quality_json.clone()),
            sender_mp_id: req.sender_mp_id.clone(),
            allocation_version: "INITIAL".to_owned(),
            valid_from_tx: Some(OffsetDateTime::now_utc()),
        });
    }

    let (validated, validation) = crate::domain::validation::ValidatedReads::validate(
        batch,
        "DIRECT_PUSH_VALIDATION",
        malo_id,
    );

    if let Err(e) = state.repo.store_reads(validated).await {
        tracing::error!(malo_id, error = %e, "edmd: direct push batch insert failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    // \u2500\u2500 Record session \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    let _ = sqlx::query(
        r"INSERT INTO direct_push_sessions
              (session_id, malo_id, source, obis_code, interval_count,
               period_from, period_to, status, quality_summary, tenant)
          VALUES ($1, $2, $3, $4, $5, $6, $7, 'committed', $8, $9)
          ON CONFLICT (session_id) DO UPDATE
              SET status          = 'committed',
                  quality_summary = EXCLUDED.quality_summary",
    )
    .bind(&session_id)
    .bind(malo_id)
    .bind(&source)
    .bind(obis_code)
    .bind(accepted.len() as i32)
    .bind(period_start)
    .bind(period_end)
    .bind(&quality_json)
    .bind(state.tenant.as_str())
    .execute(state.repo.pool())
    .await;

    // \u2500\u2500 Recompute billing period aggregates \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    // After a direct push, the cached meter_billing_periods aggregate for the
    // affected period is refreshed by `store_reads` (cache invalidation) plus the
    // read-through `billing_period()` path, so billingd picks up the new data.
    let period_from_date = period_start.date();
    let period_to_date = period_end.date();

    // No manual recompute here: `store_reads` above already invalidated the
    // cached `meter_billing_periods` aggregate for the affected window, and
    // `billing_period()` is read-through — it recomputes from the version-resolved
    // series on the next read and re-caches. The former raw `SELECT ... FROM
    // meter_reads` recompute was both broken (that relation is DataFusion-only,
    // not a Postgres table) and redundant against the read-through model.

    // \u2500\u2500 CloudEvent notifications \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    let correlation_id = uuid::Uuid::new_v4().to_string();

    // Both quality signals in one place, so the status code below and the event
    // raised here cannot disagree about whether this batch was clean.
    let alert = crate::server::quality_alert::QualityAlert {
        malo_id,
        door: "rlm-direct-push",
        correlation_id: &correlation_id,
        causation_id: &session_id,
        sparte: Some(sparte_str),
        period_from: accepted.first().map(|r| r.from),
        period_to: accepted.last().map(|r| r.to),
        validation: &validation,
        hampel: Some(quality_json.clone()),
    };
    crate::server::quality_alert::raise_quality_warning(
        state.erp_webhook_url.as_deref(),
        state.webhook_secret_bytes(),
        &state.tenant,
        &alert,
    )
    .await;

    if let Some(ref webhook_url) = state.erp_webhook_url {
        let client = mako_service::http::default_client();

        // Always emit de.messwert.reading.direct.stored so billingd knows to recompute.
        let stored_ce = mako_service::CloudEvent::new(
            mako_service::source("edmd", &state.tenant),
            mako_events::messwert::READING_DIRECT_STORED,
            malo_id,
            serde_json::json!({
                "malo_id": malo_id,
                "session_id": session_id,
                "sparte": sparte_str,
                "obis_code": obis_code,
                "period_from": period_from_date.to_string(),
                "period_to": period_to_date.to_string(),
                "intervals_stored": accepted.len(),
                "source": source,
            }),
        )
        .extension("tenantid", state.tenant.clone())
        .extension("correlationid", correlation_id.clone())
        .extension("causationid", session_id.clone());
        if let Err(e) = mako_service::post_ce_with_retry(
            &client,
            webhook_url,
            &stored_ce,
            state.webhook_secret_bytes(),
        )
        .await
        {
            tracing::error!(error = %e, "edmd: CloudEvent delivery failed — event lost");
        }
    }

    let status = if alert.is_warning() {
        StatusCode::ACCEPTED // 202 — stored but with quality warnings
    } else {
        StatusCode::CREATED // 201 — clean store
    };

    (
        status,
        Json(serde_json::json!({
            "session_id": session_id,
            "malo_id": malo_id,
            "sparte": sparte_str,
            "intervals_accepted": accepted.len(),
            "intervals_rejected": rejected_count,
            "validation_errors": validation_errors,
            "period_from": period_from_date.to_string(),
            "period_to": period_to_date.to_string(),
            "quality": quality_json,
            "validation": {
                "issue_count":         validation.issue_count,
                "billing_block_count": validation.billing_block_count,
                "rules":               validation.rules,
            },
            // The cached billing-period aggregate for this window was invalidated
            // on store; it is recomputed lazily on the next read (read-through),
            // not eagerly here.
            "billing_period_cache_invalidated": true,
            "note": if alert.is_warning() {
                "de.messwert.reading.quality.warning emitted — investigate before billing run"
            } else {
                "de.messwert.reading.direct.stored emitted — billing period cache invalidated (read-through recompute)"
            },
        })),
    )
        .into_response()
}

// ─── M7 unit tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod ingest_contract_tests {
    use super::*;
    use crate::domain::model::Sparte;
    use time::macros::datetime;

    // ── DST transitions ───────────────────────────────────────────────────────
    //
    // Germany runs on Europe/Berlin, so two days a year are not 24 hours long:
    // the last Sunday in March has 23 (CET→CEST) and the last Sunday in October
    // has 25 (CEST→CET). Quarter-hour metering therefore carries 92 and 100
    // intervals on those days rather than 96.
    //
    // The failure this guards is a *silent* one. A head-end that emits local
    // wall-clock timestamps collapses the repeated 02:00 hour on the fall-back
    // day, so an hour of energy simply disappears — the series still parses,
    // still validates against every other rule, and bills low. V07 exists to
    // catch exactly that, and these tests prove edmd's own wrapper surfaces it
    // rather than only the upstream `metering` rule doing so in isolation.

    /// Build `count` consecutive quarter-hour reads starting at `start` (UTC).
    fn quarter_hours(start: OffsetDateTime, count: usize) -> Vec<MeterRead> {
        (0..count)
            .map(|i| {
                let from = start + time::Duration::minutes(15 * i as i64);
                MeterRead {
                    malo_id: "51238696780".to_owned(),
                    melo_id: None,
                    dtm_from: from,
                    dtm_to: from + time::Duration::minutes(15),
                    quantity_kwh: rust_decimal::Decimal::new(250, 2),
                    quality: QualityFlag::Measured,
                    pid: 13025,
                    sparte: Sparte::Strom,
                    obis_code: Some("1-1:1.29.0".to_owned()),
                    tenant: "9900357000004".to_owned(),
                    source: crate::domain::model::IngestionSource::Mscons,
                    push_session: None,
                    quality_warnings: None,
                    sender_mp_id: None,
                    allocation_version: "INITIAL".to_owned(),
                    valid_from_tx: None,
                }
            })
            .collect()
    }

    /// Drive V07 through the public seam — the same call the ingest handlers
    /// make, so a change to the validation entry point breaks these tests
    /// rather than silently bypassing them.
    fn raised_v07(batch: Vec<MeterRead>) -> bool {
        let (_, report) =
            crate::domain::validation::ValidatedReads::validate(batch, "TEST", "51238696780");
        report.rules.iter().any(|r: &String| r.contains("V07"))
    }

    /// 2026-10-25 is 25 hours long: 00:00 CEST is 2026-10-24T22:00Z and the day
    /// ends at 2026-10-25T23:00Z. A correct series carries 100 quarter-hours.
    #[test]
    fn a_complete_fall_back_day_is_100_quarter_hours_and_is_accepted() {
        let start = datetime!(2026-10-24 22:00:00 UTC);
        let batch = quarter_hours(start, 100);
        assert_eq!(
            batch.last().expect("non-empty").dtm_to,
            datetime!(2026-10-25 23:00:00 UTC),
            "100 quarter-hours from 00:00 CEST must land on 00:00 CET the next day"
        );
        assert!(
            !raised_v07(batch),
            "a full 25-hour day is correct and must not raise V07"
        );
    }

    /// A day that omits the *repeated hour itself* — an hour of energy is gone,
    /// and the surviving intervals are ambiguous between the two passes.
    ///
    /// The fixture drops the four quarter-hours covering 01:00–02:00 UTC, which
    /// is the second pass of local 02:00–03:00. That is what a collapsed
    /// fall-back day looks like on the wire.
    ///
    /// It used to be "any 96-interval day", which passed under `metering` 0.16
    /// because V07 counted a day's intervals. 0.17 looks at the two-hour window
    /// around the transition instead, and its own docs say why: an interval
    /// count "cannot tell a collapsed hour from an ordinary gap", so *any* two
    /// missing quarter-hours anywhere on the day produced a confident report
    /// that the repeated hour had been collapsed — "which was simply untrue and
    /// sent the reader looking in the wrong place". The old fixture was one of
    /// those false positives: 96 contiguous quarter-hours from 22:00 UTC do
    /// cover the repeated hour and simply stop an hour early.
    #[test]
    fn a_collapsed_fall_back_day_raises_v07_through_edmds_own_wrapper() {
        let start = datetime!(2026-10-24 22:00:00 UTC);
        let repeated_from = datetime!(2026-10-25 01:00:00 UTC);
        let repeated_to = datetime!(2026-10-25 02:00:00 UTC);
        let batch: Vec<MeterRead> = quarter_hours(start, 100)
            .into_iter()
            .filter(|r| r.dtm_from < repeated_from || r.dtm_from >= repeated_to)
            .collect();
        assert_eq!(
            batch.len(),
            96,
            "the repeated hour is the part that is gone"
        );
        assert!(
            raised_v07(batch),
            "a fall-back day missing the repeated hour bills an hour short, and \
             no other rule can tell — V07 must fire"
        );
    }

    /// A day that is merely *short* is not reported as a collapsed hour.
    ///
    /// The distinction 0.17 introduced, pinned from mako's side: a truncated
    /// read is a truncated read. Reporting it as "the repeated hour was
    /// collapsed" names a cause that did not happen.
    #[test]
    fn a_fall_back_day_that_is_merely_truncated_is_not_reported_as_collapsed() {
        let start = datetime!(2026-10-24 22:00:00 UTC);
        // 96 contiguous quarter-hours: the repeated hour is present, the day
        // just stops an hour before the local day ends.
        assert!(
            !raised_v07(quarter_hours(start, 96)),
            "a complete-but-short series is not an ambiguous one"
        );
    }

    /// 2026-03-29 is 23 hours long: 00:00 CET is 2026-03-28T23:00Z and the day
    /// ends at 2026-03-29T22:00Z. A correct series carries 92 quarter-hours.
    #[test]
    fn a_complete_spring_forward_day_is_92_quarter_hours_and_is_accepted() {
        let start = datetime!(2026-03-28 23:00:00 UTC);
        let batch = quarter_hours(start, 92);
        assert_eq!(
            batch.last().expect("non-empty").dtm_to,
            datetime!(2026-03-29 22:00:00 UTC),
            "92 quarter-hours from 00:00 CET must land on 00:00 CEST the next day"
        );
        assert!(
            !raised_v07(batch),
            "a 23-hour day is correct and must not raise V07"
        );
    }

    #[test]
    fn every_wire_quality_flag_is_one_the_column_check_accepts() {
        // The set here and the CHECK in 0001_schema.sql must agree: a flag this
        // function accepts but the column rejects fails the insert at runtime.
        const SCHEMA_CHECK_VALUES: [&str; 8] = [
            "MEASURED",
            "ESTIMATED",
            "SUBSTITUTED",
            "CALCULATED",
            "CORRECTED",
            "PRELIMINARY",
            "FAULTY",
            "UNKNOWN",
        ];
        for value in SCHEMA_CHECK_VALUES {
            let parsed = quality_flag_from_wire(value)
                .unwrap_or_else(|| panic!("`{value}` is in the column CHECK but is not accepted"));
            assert_eq!(
                parsed.as_str(),
                value,
                "`{value}` must round-trip to the same spelling the column stores"
            );
        }
    }

    #[test]
    fn an_unknown_quality_flag_is_refused_rather_than_coerced() {
        // Binding an unrecognised flag raw would violate the column CHECK; the
        // bulk path used to swallow that error and still count the row stored.
        assert!(quality_flag_from_wire("SUBSTITUTION_VALUE").is_none());
        assert!(quality_flag_from_wire("").is_none());
        assert!(quality_flag_from_wire("banana").is_none());
    }

    #[test]
    fn quality_flags_are_accepted_case_insensitively() {
        assert_eq!(
            quality_flag_from_wire("measured"),
            Some(QualityFlag::Measured)
        );
    }

    #[test]
    fn the_superscript_cubic_metre_is_recognised_as_a_volume_unit() {
        // A string compare against "m3" missed this spelling, so gas submitted
        // as m³ was stored unconverted — roughly a tenfold under-count.
        use metering::interval::{MeasurementUnit, Sparte};
        for spelling in ["m3", "m³", "M3"] {
            let scale = MeasurementUnit::parse_scaled(spelling)
                .unwrap_or_else(|| panic!("`{spelling}` must parse as a volume unit"));
            assert_eq!(scale.unit, MeasurementUnit::CubicMetre);
            assert_eq!(scale.unit, Sparte::Gas.measured_unit());
        }
    }

    #[test]
    fn cubic_metres_are_not_a_valid_unit_for_electricity() {
        // The electricity endpoint had no unit check, so a value labelled "m3"
        // was multiplied by the gas Brennwert and stored as STROM.
        use metering::interval::{MeasurementUnit, Sparte};
        let m3 = MeasurementUnit::CubicMetre;
        assert_ne!(m3, Sparte::Strom.measured_unit());
        assert_ne!(m3, Sparte::Strom.billing_unit());
    }
}

// ── § 60 Abs. 6 MsbG Bitemporal Corrections ─────────────────────────────────────────

/// `POST /api/v1/corrections/{malo_id}`
///
/// Submit one or more retroactive corrections to stored meter intervals.
///
/// ## § 60 Abs. 6 MsbG compliance
///
/// Every correction creates an immutable `meter_read_corrections` row that
/// preserves the original value, corrected value, reason, and operator identity.
/// This enables BNetzA auditors to reconstruct the billing basis at any point
/// in time over the mandatory 3-year retention period.
///
/// ## Request body
///
/// ```json
/// {
///   "corrections": [
///     {
///       "malo_id": "51238696780",
///       "dtm_from": "2026-06-01T00:00:00Z",
///       "dtm_to": "2026-06-01T00:15:00Z",
///       "original_kwh": "2.500",
///       "original_quality": "MEASURED",
///       "corrected_kwh": "2.420",
///       "corrected_quality": "CORRECTED",
///       "reason": "Ablese-Korrekturbericht MSB 2026-07-01: Zählerfehlstand Q2/2026",
///       "source": "OPERATOR",
///       "corrected_by": "dispatcher@netzbetreiber.de"
///     }
///   ]
/// }
/// ```
///
/// ## Response
///
/// ```json
/// {
///   "corrected_count": 1,
///   "correction_ids": ["<uuid of the correction record>"]
/// }
/// ```
pub async fn post_corrections(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Json(req): Json<crate::domain::CorrectionRequest>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "write-timeseries", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    if req.corrections.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "corrections array must not be empty",
        )
            .into_response();
    }

    // Validate: all corrections must reference the path MaLo
    for (i, rec) in req.corrections.iter().enumerate() {
        if rec.malo_id != malo_id {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                format!(
                    "correction[{}].malo_id {:?} does not match path malo_id {:?}",
                    i, rec.malo_id, malo_id
                ),
            )
                .into_response();
        }
        if rec.reason.trim().is_empty() {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                format!(
                    "correction[{i}].reason must not be empty (§ 60 Abs. 6 MsbG audit requirement)"
                ),
            )
                .into_response();
        }
        // The same V06/V08-class boundary checks the ingest paths run: a
        // corrected interval must still be a valid interval, and a correction
        // cannot claim energy for time that has not happened yet.
        if rec.dtm_from >= rec.dtm_to {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                format!(
                    "correction[{}]: dtm_from {} must be before dtm_to {}",
                    i, rec.dtm_from, rec.dtm_to
                ),
            )
                .into_response();
        }
        if rec.dtm_from > OffsetDateTime::now_utc() + time::Duration::minutes(15) {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                format!(
                    "correction[{}]: dtm_from {} lies in the future (V08)",
                    i, rec.dtm_from
                ),
            )
                .into_response();
        }
    }

    match state.repo.store_corrections(&req.corrections).await {
        Ok(correction_ids) => {
            let count = correction_ids.len();
            tracing::info!(
                malo_id,
                corrected_count = count,
                "edmd: {} interval(s) corrected (§ 60 Abs. 6 MsbG)",
                count
            );
            (
                axum::http::StatusCode::OK,
                Json(crate::domain::CorrectionResponse {
                    corrected_count: count,
                    correction_ids,
                }),
            )
                .into_response()
        }
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Bulk ingestion ────────────────────────────────────────────────────────────

/// Request body for `POST /api/v1/meter-reads/{malo_id}/bulk`.
///
/// Accepts a batch of interval readings for one MaLo in a single HTTP request.
/// This is the performance path for large MSCONS deliveries and MSB bulk uploads.
///
/// ## Idempotency
///
/// Each interval is upserted — re-submitting the same `(malo_id, dtm_from, dtm_to)`
/// updates the value and quality. Supply `session_id` to deduplicate entire batches.
///
/// ## Validation
///
/// The batch is validated with [`metering::validate_intervals`] before it is
/// stored, and the resulting issues are written to `quality_warnings` on the
/// intervals they name, in the same statement as the readings themselves.
#[derive(Debug, serde::Deserialize)]
pub struct BulkReadRequest {
    /// Idempotency key — re-submitting the same `session_id` is a no-op if already committed.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Energy commodity (STROM or GAS).
    pub sparte: String,
    /// OBIS-Kennzahl (optional — defaults to `1-0:1.8.0*255` for Strom Bezug).
    #[serde(default)]
    pub obis_code: Option<String>,
    /// Source identifier (default: `API_IMPORT`).
    #[serde(default)]
    pub source: Option<String>,
    /// The interval readings.
    pub reads: Vec<BulkReadEntry>,
}

/// One interval in a bulk read batch.
#[derive(Debug, serde::Deserialize)]
pub struct BulkReadEntry {
    /// Interval start (RFC 3339 UTC).
    pub dtm_from: String,
    /// Interval end (RFC 3339 UTC).
    pub dtm_to: String,
    /// Energy quantity (kWh or kWh_Hs for Gas).
    pub quantity_kwh: String,
    /// Quality flag (MEASURED / ESTIMATED / SUBSTITUTED / …). Defaults to MEASURED.
    #[serde(default)]
    pub quality: Option<String>,
    /// Messlokations-ID (optional).
    #[serde(default)]
    pub melo_id: Option<String>,
}

/// `POST /api/v1/meter-reads/{malo_id}/bulk`
///
/// Batch ingestion endpoint. Accepts up to 50 000 intervals per request.
///
/// The whole batch is validated (V01–V10) and then written in one statement, so
/// `stored_count` reflects rows that actually committed and a failure leaves
/// nothing behind for the caller to reconcile.
///
/// Returns a summary of stored intervals and any validation issues.
pub async fn post_bulk_reads(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Json(req): Json<BulkReadRequest>,
) -> impl IntoResponse {
    use metering::QualityFlag;
    use time::format_description::well_known::Rfc3339;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "write-timeseries", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    if req.reads.is_empty() {
        return (StatusCode::BAD_REQUEST, "reads array must not be empty").into_response();
    }
    const MAX_BATCH: usize = 50_000;
    if req.reads.len() > MAX_BATCH {
        return (
            StatusCode::BAD_REQUEST,
            format!("batch too large: {} > {MAX_BATCH}", req.reads.len()),
        )
            .into_response();
    }

    // Deduplicate by session_id
    if let Some(ref sid) = req.session_id {
        let existing: Option<i64> = sqlx::query_scalar(
            // Only a committed session is a duplicate. Matching any status made
            // a `failed` session permanently unretryable.
            "SELECT interval_count FROM direct_push_sessions
             WHERE session_id = $1 AND tenant = $2 AND status = 'committed'",
        )
        .bind(sid)
        .bind(state.tenant.as_str())
        .fetch_optional(state.repo.pool())
        .await
        .unwrap_or(None);
        if let Some(count) = existing {
            return (
                StatusCode::OK,
                Json(serde_json::json!({
                    "session_id": sid,
                    "stored_count": count,
                    "deduplicated": true
                })),
            )
                .into_response();
        }
    }

    // Sparte determines the storage unit, so an unrecognised value is rejected
    // rather than defaulted.
    let sparte = match req.sparte.to_uppercase().as_str() {
        "STROM" => "STROM",
        "GAS" => "GAS",
        "WAERME" | "WÄRME" => "WAERME",
        "WASSER" => "WASSER",
        other => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": format!(
                        "unknown sparte `{other}`; expected STROM, GAS, WAERME or WASSER"
                    )
                })),
            )
                .into_response();
        }
    };
    let source = req.source.as_deref().unwrap_or("API_IMPORT");
    let session_id = req
        .session_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let sparte_enum = match sparte {
        "GAS" => EdmSparte::Gas,
        "WAERME" => EdmSparte::Waerme,
        "WASSER" => EdmSparte::Wasser,
        _ => EdmSparte::Strom,
    };
    let ingestion_source = IngestionSource::from_db_str(source);

    let mut batch: Vec<MeterRead> = Vec::with_capacity(req.reads.len());

    for entry in &req.reads {
        let dtm_from = match OffsetDateTime::parse(&entry.dtm_from, &Rfc3339) {
            Ok(t) => t,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    format!("invalid dtm_from {:?}: {e}", entry.dtm_from),
                )
                    .into_response();
            }
        };
        let dtm_to = match OffsetDateTime::parse(&entry.dtm_to, &Rfc3339) {
            Ok(t) => t,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    format!("invalid dtm_to {:?}: {e}", entry.dtm_to),
                )
                    .into_response();
            }
        };
        let qty: rust_decimal::Decimal = match entry.quantity_kwh.parse() {
            Ok(d) => d,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    format!("invalid quantity {:?}: {e}", entry.quantity_kwh),
                )
                    .into_response();
            }
        };
        // An unrecognised flag is refused rather than coerced: binding it raw
        // would fail the column CHECK, and treating it as UNKNOWN would silently
        // strip the row from every billing aggregate.
        let quality = match entry.quality.as_deref() {
            None => QualityFlag::Measured,
            Some(q) => match quality_flag_from_wire(q) {
                Some(f) => f,
                None => {
                    return (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        Json(serde_json::json!({
                            "error": format!(
                                "interval {}: unknown quality `{q}`; expected one of MEASURED, \
                                 ESTIMATED, SUBSTITUTED, CALCULATED, CORRECTED, PRELIMINARY, \
                                 FAULTY, UNKNOWN",
                                entry.dtm_from
                            )
                        })),
                    )
                        .into_response();
                }
            },
        };

        batch.push(MeterRead {
            malo_id: malo_id.clone(),
            melo_id: entry.melo_id.clone(),
            dtm_from,
            dtm_to,
            quantity_kwh: qty,
            quality,
            pid: 0, // no MSCONS process behind an API import
            sparte: sparte_enum,
            obis_code: req.obis_code.clone(),
            tenant: state.tenant.clone(),
            source: ingestion_source,
            push_session: Some(session_id.clone()),
            quality_warnings: None,
            sender_mp_id: None,
            allocation_version: "INITIAL".to_owned(),
            valid_from_tx: Some(OffsetDateTime::now_utc()),
        });
    }

    // Validation runs before the write so its findings can be stored with the
    // rows they describe, in the same statement.
    batch.sort_by_key(|r| r.dtm_from);
    let (validated, validation) = crate::domain::validation::ValidatedReads::validate(
        batch,
        "BULK_IMPORT_VALIDATION",
        &malo_id,
    );

    let period_from = validated.as_slice().first().map(|r| r.dtm_from);
    let period_to = validated.as_slice().last().map(|r| r.dtm_to);

    // One batched statement, so the count reported is the count committed.
    let stored = validated.len();
    if let Err(e) = state.repo.store_reads(validated).await {
        tracing::error!(malo_id = %malo_id, error = %e, "edmd: bulk import batch insert failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": e.to_string(),
                "stored_count": 0,
                "session_id": session_id,
            })),
        )
            .into_response();
    }

    let issues_summary = serde_json::json!({
        "is_clean": validation.is_clean(),
        "billing_block_count": validation.billing_block_count,
        "issue_count": validation.issue_count,
        "rules_triggered": validation.rules,
    });

    // Persist session record
    let _ = sqlx::query(
        r"INSERT INTO direct_push_sessions
              (session_id, malo_id, source, obis_code, interval_count,
               period_from, period_to, status, quality_summary, tenant)
          VALUES ($1,$2,$3,$4,$5,$6,$7,'committed',$8,$9)
          ON CONFLICT (session_id) DO NOTHING",
    )
    .bind(&session_id)
    .bind(&malo_id)
    .bind(source)
    .bind(&req.obis_code)
    .bind(stored as i32)
    .bind(period_from)
    .bind(period_to)
    .bind(&issues_summary)
    .bind(state.tenant.as_str())
    .execute(state.repo.pool())
    .await;

    // A bulk import used to answer 201 and stay silent whatever the V01–V10 pass
    // found, so an operator uploading a month of corrected readings got the same
    // "created" for a clean file and for one carrying billing-blocking intervals.
    let alert = crate::server::quality_alert::QualityAlert {
        malo_id: &malo_id,
        door: "bulk-import",
        correlation_id: &session_id,
        causation_id: &session_id,
        sparte: Some(sparte),
        period_from,
        period_to,
        validation: &validation,
        hampel: None,
    };
    crate::server::quality_alert::raise_quality_warning(
        state.erp_webhook_url.as_deref(),
        state.webhook_secret_bytes(),
        &state.tenant,
        &alert,
    )
    .await;

    (
        if alert.is_warning() {
            StatusCode::ACCEPTED
        } else {
            StatusCode::CREATED
        },
        Json(serde_json::json!({
            "session_id": session_id,
            "malo_id": malo_id,
            "stored_count": stored,
            "validation": issues_summary,
        })),
    )
        .into_response()
}

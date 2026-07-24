//! IoT meter ingest (LoRaWAN / M-Bus / REST heat meters).

#[allow(unused_imports)]
use super::*;

// ── IoT meter ingest (LoRaWAN / M-Bus / REST heat meters) ────────────────────

/// One decoded interval in an IoT push.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct IotInterval {
    /// RFC 3339 interval start (inclusive).
    from: String,
    /// RFC 3339 interval end (exclusive).
    to: String,
    /// Consumption in `unit` over the interval.
    value: rust_decimal::Decimal,
}

/// An IoT meter-reading push.
///
/// The envelope is **transport-agnostic and already decoded**. See
/// [`post_iot_reads`] for why `edmd` does not decode wM-Bus frames itself.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct IotPushRequest {
    /// `WAERME` · `WASSER` · `STROM` · `GAS`.
    sparte: String,
    /// `KWH` or `M3`. Must be consistent with `sparte`.
    unit: String,
    /// Stable per-batch idempotency key. For LoRaWAN use `devEUI:fCnt`; for
    /// OMS/M-Bus the telegram access number.
    session_id: String,
    /// Transport the reading arrived over, for provenance:
    /// `LORAWAN` · `MBUS` · `WMBUS` · `REST`.
    transport: String,
    /// Optional device identity (LoRaWAN devEUI, M-Bus secondary address).
    device_id: Option<String>,
    /// Optional OBIS code. Medium group: 4 = Heizkostenverteiler, 5/6 = thermal,
    /// 7 = gas, 8 = cold water, 9 = hot water.
    obis_code: Option<String>,
    /// Optional Messlokation.
    melo_id: Option<String>,
    /// Raw, undecoded payload as received (base64 or hex, verbatim).
    ///
    /// Retained as the system of record: network-server codecs are mutable and
    /// carry no version on the uplink, so a stored value can only be re-derived
    /// from the original frame.
    raw_payload: Option<String>,
    /// Brennwert Hs in kWh/m³. **Required** when `sparte = GAS` and `unit = M3`.
    ///
    /// Published monthly per supply area by the NB. There is no safe
    /// default: the calorific value determines the billed quantity.
    brennwert_kwh_per_m3: Option<rust_decimal::Decimal>,
    /// Zustandszahl (dimensionless), default 1.0 when not separately metered.
    zustandszahl: Option<rust_decimal::Decimal>,
    /// Calibration validity (`Eichfrist`) end date, `YYYY-MM-DD`, if known.
    ///
    /// Per §34 Abs. 2 MessEV a Eichfrist of at least a year ends only *"mit dem
    /// Ende des Jahres, in dem die Frist rechnerisch endet"*, so callers send
    /// `YYYY-12-31`. Leave unset for Heizkostenverteiler, which have no Eichfrist.
    eichung_bis: Option<String>,
    intervals: Vec<IotInterval>,
}

/// `POST /api/v1/meter-reads/iot/{malo_id}`
///
/// Ingest metering data that does not pass through MSCONS: LoRaWAN uplinks,
/// M-Bus/wM-Bus concentrators, and REST-capable heat meters.
///
/// Heat and water submetering points have no Smart-Meter-Gateway and are
/// governed by **HeizkostenV**: §5 Abs. 3 requires remote readability by
/// 31 December 2026, §6a a monthly consumption message, and §12 Abs. 1 grants a
/// 3 % Kürzungsrecht on two independent grounds — a missing fernablesbare device
/// (Satz 2) and information supplied "nicht oder nicht vollständig" (Satz 3).
///
/// ## Payload
///
/// Values arrive already decoded. wM-Bus/OMS payload specifications are vendor-
/// gated and the device keys sit at the network server, so decoding belongs
/// there. `raw_payload` is retained verbatim so a value can be re-derived if a
/// codec changes.
///
/// ## Calibration
///
/// An expired Eichfrist is recorded as a warning, not a rejection.
///
/// §37 Abs. 1 Satz 1 Nr. 1 MessEG bars *use of the Messgerät* once the Eichfrist
/// has run; §33 Abs. 1 MessEG then bars the resulting values, since a device used
/// contrary to §37 was not "bestimmungsgemäß verwendet". BGH VIII ZR 112/10 holds
/// that in civil billing such a reading loses only its *Vermutung der
/// Richtigkeit*. Public-law Gebührenabrechnung is stricter (BayVGH 20 B 21.2421),
/// which is a billing-side decision.
///
/// §37 Abs. 2 also ends a Eichfrist early on defect or tampering, so an expiry
/// date alone is not the whole eichrechtliche validity test.
pub(crate) async fn post_iot_reads(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Json(req): Json<IotPushRequest>,
) -> impl IntoResponse {
    use metering::interval::{MeasurementUnit, Sparte as MSparte};
    use time::format_description::well_known::Rfc3339;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "write-meter-reads", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    if req.intervals.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "intervals must not be empty" })),
        )
            .into_response();
    }
    if req.session_id.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "session_id is required — use devEUI:fCnt (LoRaWAN) or the \
                          telegram access number (OMS/M-Bus)"
            })),
        )
            .into_response();
    }

    let sparte = match req.sparte.to_uppercase().as_str() {
        "STROM" => MSparte::Strom,
        "GAS" => MSparte::Gas,
        "WAERME" | "WÄRME" => MSparte::Waerme,
        "WASSER" => MSparte::Wasser,
        other => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": format!("unknown sparte `{other}`; expected STROM, GAS, WAERME or WASSER")
                })),
            )
                .into_response();
        }
    };

    // EN 1434-1 cl. 6.3.1 permits heat registers in Joules or Watt-hours and any
    // decimal multiple; water submeters commonly report litres. The scale is an
    // exact rational, so GJ→kWh (2500/9) stays exact.
    let Some(scale) = MeasurementUnit::parse_scaled(&req.unit) else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!(
                    "unknown unit `{}`; expected kWh/MWh/GJ/MJ/Wh (energy) or m³/l (volume)",
                    req.unit
                )
            })),
        )
            .into_response();
    };
    let unit = scale.unit;

    // A reading may arrive in the unit the meter registers, or already converted
    // to the settlement unit. Anything else is a decode error.
    if unit != sparte.measured_unit() && unit != sparte.billing_unit() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!(
                    "unit {} is not valid for sparte {} — expected {} (as measured) or {} (as billed)",
                    unit.as_str(),
                    sparte.as_str(),
                    sparte.measured_unit().as_str(),
                    sparte.billing_unit().as_str()
                )
            })),
        )
            .into_response();
    }

    // Gas is metered in m³ and billed in kWh, so a raw gas uplink needs the
    // Brennwert before it can be stored in an energy column. The calorific value
    // varies by supply area and month, so it is required rather than defaulted.
    let conversion = if sparte.requires_conversion() && unit == sparte.measured_unit() {
        let Some(hs) = req.brennwert_kwh_per_m3 else {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "brennwert_kwh_per_m3 is required when submitting gas in m³ \
                              (§25 Nr. 4 MessEV); submit unit=KWH to supply pre-converted values"
                })),
            )
                .into_response();
        };
        Some((hs, req.zustandszahl.unwrap_or(rust_decimal::Decimal::ONE)))
    } else {
        None
    };

    let pool = state.repo.pool();

    // Idempotency: a committed session replays as 200, never as duplicate rows.
    let already: Option<String> = sqlx::query_scalar(
        r"SELECT status FROM direct_push_sessions
          WHERE session_id = $1 AND malo_id = $2 AND tenant = $3 AND status = 'committed'",
    )
    .bind(&req.session_id)
    .bind(&malo_id)
    .bind(resource_tenant)
    .fetch_optional(state.repo.pool())
    .await
    .ok()
    .flatten();

    if already.is_some() {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "malo_id": malo_id,
                "session_id": req.session_id,
                "status": "already_committed",
            })),
        )
            .into_response();
    }

    // Calibration check. Expired → warn, never reject (see fn docs).
    let mut warnings: Vec<String> = Vec::new();
    let eichung_expired = req.eichung_bis.as_deref().and_then(|d| {
        time::Date::parse(
            d,
            &time::macros::format_description!("[year]-[month]-[day]"),
        )
        .ok()
    });
    if let Some(bis) = eichung_expired
        && bis < time::OffsetDateTime::now_utc().date()
    {
        warnings.push(format!(
            "Eichfrist am {bis} abgelaufen (§37 Abs. 1 Satz 1 Nr. 1 MessEG) — \
             Messwerte behalten ihre Verwendbarkeit, verlieren aber die Vermutung \
             der Richtigkeit (BGH VIII ZR 112/10); Befundprüfung nach §39 MessEG \
             empfohlen"
        ));
    }

    // OBIS normalisation happens once, inside `store_reads` — it feeds the
    // primary key, so a second implementation here could drift from it.

    // Hampel-filter quality scoring with media-aware thresholds: heat and water
    // profiles contain long legitimate zero runs.
    let mut scored: Vec<(f64, i64)> = req
        .intervals
        .iter()
        .filter_map(|iv| {
            let from = time::OffsetDateTime::parse(&iv.from, &Rfc3339).ok()?;
            let rescaled = scale.apply(iv.value);
            let converted = conversion.map_or(rescaled, |(hs, z)| {
                metering::gas_m3_to_kwh_hs(rescaled, hs, z)
            });
            let v = converted.to_string().parse::<f64>().ok()?;
            Some((v, (from.unix_timestamp_nanos() as i64)))
        })
        .collect();
    scored.sort_by_key(|(_, ts)| *ts);

    let quality = if scored.len() >= 3 {
        let values: Vec<f64> = scored.iter().map(|(v, _)| *v).collect();
        let stamps: Vec<i64> = scored.iter().map(|(_, t)| *t).collect();
        let period_end = req
            .intervals
            .iter()
            .filter_map(|iv| time::OffsetDateTime::parse(&iv.to, &Rfc3339).ok())
            .map(|t| t.unix_timestamp_nanos() as i64)
            .max()
            .unwrap_or_else(|| stamps[stamps.len() - 1]);
        Some(metering::score_intervals_f64(
            &values,
            &stamps,
            stamps[0],
            period_end,
            metering::QualityConfig::for_sparte(sparte),
        ))
    } else {
        None
    };

    // `score_intervals_f64` reports outliers as `"t+<unix_nanos>"`, not RFC 3339
    // — it takes raw `i64` nanosecond stamps and has no calendar to format
    // against. Parsing them as RFC 3339 silently yields an empty set, which
    // makes the PRELIMINARY flag below unreachable.
    let outlier_stamps: std::collections::HashSet<i64> = quality
        .as_ref()
        .map(|q| {
            q.outlier_intervals
                .iter()
                .chain(q.spike_intervals.iter())
                .filter_map(|ts| ts.strip_prefix("t+").and_then(|n| n.parse::<i64>().ok()))
                .collect()
        })
        .unwrap_or_default();

    let stored_unit = sparte.billing_unit();

    let mut stored = 0usize;
    let mut rejected: Vec<String> = Vec::new();
    let mut batch: Vec<MeterRead> = Vec::with_capacity(req.intervals.len());

    for iv in &req.intervals {
        let (Ok(from), Ok(to)) = (
            time::OffsetDateTime::parse(&iv.from, &Rfc3339),
            time::OffsetDateTime::parse(&iv.to, &Rfc3339),
        ) else {
            rejected.push(format!("unparseable interval {}..{}", iv.from, iv.to));
            continue;
        };
        if from >= to {
            rejected.push(format!("from >= to at {from}"));
            continue;
        }
        // BDEW requires quantities to be positive or zero; direction is carried
        // in the OBIS code.
        if iv.value < rust_decimal::Decimal::ZERO {
            rejected.push(format!(
                "negative value {} at {from} — direction belongs in the OBIS code",
                iv.value
            ));
            continue;
        }

        // An outlier is flagged, not discarded: § 60 Abs. 2 MsbG substitution is a
        // downstream decision.
        // `PRELIMINARY` (MSCONS Z84, vorläufiger Wert): measured but not yet
        // confirmed. `FAULTY` would assert a defect the filter cannot establish.
        let quality_flag = if outlier_stamps.contains(&(from.unix_timestamp_nanos() as i64)) {
            "PRELIMINARY"
        } else {
            "MEASURED"
        };

        // `meter_reads.quantity_kwh` holds the settlement quantity, so gas is
        // converted before it lands.
        let rescaled = scale.apply(iv.value);
        let quantity = conversion.map_or(rescaled, |(hs, z)| {
            metering::gas_m3_to_kwh_hs(rescaled, hs, z)
        });

        // Rows are accumulated and written in one batched `unnest` statement
        // below, so the whole push lands or none of it does.
        batch.push(MeterRead {
            malo_id: malo_id.clone(),
            melo_id: req.melo_id.clone(),
            dtm_from: from,
            dtm_to: to,
            quantity_kwh: quantity,
            quality: if quality_flag == "PRELIMINARY" {
                QualityFlag::Preliminary
            } else {
                QualityFlag::Measured
            },
            pid: 0,
            sparte: edm_sparte_from_metering(sparte),
            obis_code: req.obis_code.clone(),
            tenant: resource_tenant.to_owned(),
            source: IngestionSource::IotPush,
            push_session: Some(req.session_id.clone()),
            quality_warnings: None,
            sender_mp_id: req.device_id.clone(),
            allocation_version: "INITIAL".to_owned(),
            valid_from_tx: Some(OffsetDateTime::now_utc()),
        });
        stored += 1;
    }

    let validation = validate_and_annotate(&mut batch, "IOT_PUSH_VALIDATION", &malo_id);

    // The IoT path scores with `score_intervals_f64` rather than
    // `compute_quality`, so the report is adapted before it is recorded — the
    // history must not depend on which door the reading came in by.
    if let (Some(q), Some(first), Some(last)) = (
        quality.as_ref(),
        batch.first().map(|r| r.dtm_from),
        batch.last().map(|r| r.dtm_to),
    ) {
        let report = QualityReport {
            intervals_accepted: q.intervals_analysed,
            intervals_rejected: rejected.len(),
            gaps_detected: q.gaps_detected,
            zero_run_length: q.max_zero_run,
            outlier_intervals: q.outlier_intervals.clone(),
            spike_intervals: q.spike_intervals.clone(),
            intervals_consistent: q.intervals_consistent,
            has_warnings: q.has_warnings,
            coverage_pct: q.coverage_pct,
            grade: q.grade.as_str(),
        };
        record_quality_assessment(
            pool,
            resource_tenant,
            &malo_id,
            first,
            last,
            "IOT_PUSH",
            &report,
        )
        .await;
    }

    if !batch.is_empty()
        && let Err(e) = state.repo.store_reads(&batch).await
    {
        tracing::error!(malo_id = %malo_id, error = %e, "edmd: IoT batch insert failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // Commit the session only when something landed, so a wholly-failed batch
    // stays retryable.
    if stored > 0 {
        let _ = sqlx::query(
            r"INSERT INTO direct_push_sessions
                (session_id, malo_id, interval_count, status, tenant)
              VALUES ($1,$2,$3,'committed',$4)
              ON CONFLICT (session_id) DO UPDATE SET status = 'committed'",
        )
        .bind(&req.session_id)
        .bind(&malo_id)
        .bind(i32::try_from(stored).unwrap_or(i32::MAX))
        .bind(resource_tenant)
        .execute(state.repo.pool())
        .await;
    }

    let status = if stored == 0 {
        StatusCode::UNPROCESSABLE_ENTITY
    } else if warnings.is_empty() && rejected.is_empty() && validation.is_clean() {
        StatusCode::CREATED
    } else {
        StatusCode::ACCEPTED
    };

    (
        status,
        Json(serde_json::json!({
            "malo_id":     malo_id,
            "session_id":  req.session_id,
            "transport":   req.transport,
            "device_id":   req.device_id,
            "sparte":      sparte.as_str(),
            "unit_submitted": unit.as_str(),
            "unit_stored":     stored_unit.as_str(),
            "converted":       conversion.is_some(),
            "stored":      stored,
            "rejected":    rejected,
            "warnings":    warnings,
            "raw_retained": req.raw_payload.is_some(),
            "quality": quality.as_ref().map(|q| serde_json::json!({
                "grade":         q.grade.as_str(),
                "coverage_pct":  q.coverage_pct,
                "gaps_detected": q.gaps_detected,
                "outliers":      q.outlier_intervals.len() + q.spike_intervals.len(),
                "blocks_billing": q.grade.blocks_billing(),
            })),
            "validation": {
                "issue_count":         validation.issue_count,
                "billing_block_count": validation.billing_block_count,
                "rules":               validation.rules,
            },
            "legal_basis": "HeizkostenV §5 Abs. 3 / §6a; MessEG §37",
        })),
    )
        .into_response()
}

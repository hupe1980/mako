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
    ///
    /// Recorded on the session row (`direct_push_sessions.transport`).
    transport: String,
    /// Optional device identity (LoRaWAN devEUI, M-Bus secondary address).
    ///
    /// Recorded on the session row beside the transport. It is deliberately
    /// **not** a `sender_mp_id`: that column is the BDEW Codenummer of the
    /// operator who assigned the MSCONS version, and it keys the meterstore
    /// version scope. Filing a devEUI there put every device in a scope of its
    /// own, so a replaced meter's readings could never supersede the ones they
    /// corrected — and every read-back reported the devEUI as the reporting
    /// market partner.
    device_id: Option<String>,
    /// MP-ID of the market partner delivering these values (the MSB operating
    /// the submetering, when there is one).
    ///
    /// Keys the meterstore version scope. Absent, the scope falls back to the
    /// tenant, which is the right answer for a device nobody reports on behalf of.
    #[serde(default)]
    sender_mp_id: Option<String>,
    /// Optional OBIS code. Medium group: 4 = Heizkostenverteiler, 5/6 = thermal,
    /// 7 = gas, 8 = cold water, 9 = hot water.
    obis_code: Option<String>,
    /// Optional Messlokation.
    melo_id: Option<String>,
    /// Raw, undecoded payload as received (base64 or hex, verbatim).
    ///
    /// Recorded on the session row (`direct_push_sessions.raw_payload`) so a
    /// value can be re-derived if the network server\'s codec changes: codecs
    /// are mutable and carry no version on the uplink. `raw_retained: true` in
    /// the response is a claim about evidence, so the payload must actually
    /// reach the row.
    raw_payload: Option<String>,
    /// Brennwert Hs in kWh/m³. **Required** when `sparte = GAS` and `unit = M3`.
    ///
    /// Published monthly per supply area by the NB. There is no safe
    /// default: the calorific value determines the billed quantity.
    brennwert_kwh_per_m3: Option<rust_decimal::Decimal>,
    /// Zustandszahl (dimensionless), default 1.0 when not separately metered.
    zustandszahl: Option<rust_decimal::Decimal>,
    /// Physical capacity ceiling of the metered plant, in kW.
    ///
    /// Supply it and **V12** (`ImplausiblePower`) checks each interval's average
    /// power against it. Omit it and V12 stays off — edmd holds no master data
    /// of its own and will not invent a ceiling.
    #[serde(default)]
    max_plant_power_kw: Option<rust_decimal::Decimal>,
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
    use metering::interval::MeasurementUnit;
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

    let sparte = match crate::domain::parse_sparte(&req.sparte) {
        Some(s) => s,
        None => {
            let other = req.sparte.as_str();
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
    // Typed intervals: `metering` 0.17's `score_intervals` takes the same
    // `MeterInterval` the rest of the pipeline already speaks, so the values
    // keep their `Decimal` precision instead of being flattened to `f64` and
    // the timestamps stay instants instead of bare nanosecond counts.
    let mut samples: Vec<metering::MeterInterval> = req
        .intervals
        .iter()
        .filter_map(|iv| {
            let from = time::OffsetDateTime::parse(&iv.from, &Rfc3339).ok()?;
            let to = time::OffsetDateTime::parse(&iv.to, &Rfc3339).ok()?;
            let rescaled = scale.apply(iv.value);
            let converted = conversion.map_or(rescaled, |(hs, z)| {
                metering::gas_m3_to_kwh_hs(rescaled, hs, z)
            });
            Some(metering::MeterInterval {
                from,
                to,
                value: converted,
                quality: metering::QualityFlag::Measured,
                // The register the batch names, so the scorer's own
                // energy-register filter (`domain::register`) sees what the
                // stored rows carry. Left `None`, a Heizkostenverteiler batch —
                // dimensionless Verbrauchseinheiten, not energy — was scored as
                // though it were a kWh series.
                obis_code: req.obis_code.as_deref().and_then(|s| s.parse().ok()),
            })
        })
        .collect();
    samples.sort_by_key(|iv| iv.from);

    // The same scorer every other door runs, so the grade a reading gets does
    // not depend on which door it came in by. A pushed batch declares its own
    // span, so that is the period coverage is measured against.
    let quality = match (samples.len() >= 3, samples.first(), samples.last()) {
        (true, Some(first), Some(last)) => {
            Some(compute_quality(&samples, sparte, first.from, last.to))
        }
        _ => None,
    };

    // Findings carry the instant they are about, so the anomalous slots are read
    // straight off them.
    let outlier_stamps: std::collections::HashSet<OffsetDateTime> = quality
        .as_ref()
        .map(|q| {
            q.outlier_intervals
                .iter()
                .chain(q.spike_intervals.iter())
                .filter_map(|t| {
                    OffsetDateTime::parse(t, &time::format_description::well_known::Rfc3339)
                        .ok()
                        .or_else(|| {
                            samples
                                .iter()
                                .map(|iv| iv.from)
                                .find(|f| f.to_string() == *t)
                        })
                })
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
        let is_outlier = outlier_stamps.contains(&from);

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
            quality: if is_outlier {
                QualityFlag::Preliminary
            } else {
                QualityFlag::Measured
            },
            pid: 0,
            sparte,
            obis_code: req.obis_code.clone(),
            tenant: resource_tenant.to_owned(),
            source: IngestionSource::IotPush,
            push_session: Some(req.session_id.clone()),
            quality_warnings: None,
            sender_mp_id: req.sender_mp_id.clone(),
            allocation_version: "INITIAL".to_owned(),
            valid_from_tx: Some(OffsetDateTime::now_utc()),
            mscons_version: None,
        });
        stored += 1;
    }

    let (validated, validation) = crate::domain::ValidatedReads::validate(
        batch,
        crate::domain::IngestContext::new("IOT_PUSH_VALIDATION", &malo_id)
            .with_capacity_kw(req.max_plant_power_kw),
    );

    // Captured before the batch moves into the store, so the alert below can
    // name the window it covers. Min/max rather than the ends of the slice: a
    // batch is not required to arrive sorted.
    let (period_from, period_to) = crate::domain::batch_period(validated.as_slice());

    if !validated.is_empty()
        && let Err(e) = state.repo.store_reads(validated).await
    {
        tracing::error!(malo_id = %malo_id, error = %e, "edmd: IoT batch insert failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // Recorded once the readings are stored, for the same reason every other
    // door does: a verdict written first stands for a delivery that failed.
    if let (Some(q), Some(first), Some(last)) = (quality.as_ref(), period_from, period_to) {
        let mut report = QualityReport { ..q.clone() };
        report.intervals_rejected = rejected.len();
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

    // Commit the session only when something landed, so a wholly-failed batch
    // stays retryable.
    if stored > 0 {
        let _ = sqlx::query(
            r"INSERT INTO direct_push_sessions
                (session_id, malo_id, source, obis_code, interval_count,
                 period_from, period_to, status, raw_payload, transport, device_id, tenant)
              VALUES ($1,$2,'IOT_PUSH',$3,$4,$5,$6,'committed',$7,$8,$9,$10)
              ON CONFLICT (tenant, session_id) DO UPDATE
                  SET status      = 'committed',
                      raw_payload = COALESCE(EXCLUDED.raw_payload,
                                             direct_push_sessions.raw_payload),
                      transport   = COALESCE(EXCLUDED.transport,
                                             direct_push_sessions.transport),
                      device_id   = COALESCE(EXCLUDED.device_id,
                                             direct_push_sessions.device_id)",
        )
        .bind(&req.session_id)
        .bind(&malo_id)
        .bind(&req.obis_code)
        .bind(i32::try_from(stored).unwrap_or(i32::MAX))
        .bind(period_from)
        .bind(period_to)
        .bind(&req.raw_payload)
        .bind(req.transport.trim())
        .bind(&req.device_id)
        .bind(resource_tenant)
        .execute(state.repo.pool())
        .await;
    }

    // Same warning every other ingest door raises. Without it a FAULTY reading
    // pushed by a LoRaWAN device is annotated and then silent — agentd's
    // meter-data-agent and replacement-value-agent are event-driven.
    let hampel = quality.as_ref().map(|q| {
        serde_json::json!({
            "grade": q.grade,
            "coverage_pct": q.coverage_pct,
            "gaps_detected": q.gaps_detected,
            "outliers": q.outlier_intervals.len() + q.spike_intervals.len(),
            "has_warnings": q.has_warnings,
            "blocks_billing": q.grade == "F",
            "algorithm": q.algorithm,
        })
    });
    let alert = crate::server::quality_alert::QualityAlert {
        malo_id: &malo_id,
        door: "iot-push",
        correlation_id: &req.session_id,
        causation_id: &req.session_id,
        sparte: Some(sparte.as_str()),
        period_from,
        period_to,
        validation: &validation,
        hampel,
    };
    crate::server::quality_alert::raise_quality_warning(
        state.erp_webhook_url.as_deref(),
        state.webhook_secret_bytes(),
        &state.tenant,
        &alert,
    )
    .await;

    let status = if stored == 0 {
        StatusCode::UNPROCESSABLE_ENTITY
    } else if warnings.is_empty() && rejected.is_empty() && !alert.is_warning() {
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
                "grade":              q.grade,
                "coverage_pct":       q.coverage_pct,
                "gaps_detected":      q.gaps_detected,
                "outliers":           q.outlier_intervals.len() + q.spike_intervals.len(),
                "expected_intervals": q.expected_intervals,
                "interval_secs":      q.interval_secs,
                "blocks_billing":     q.grade == "F",
                "algorithm":          q.algorithm,
            })),
            "validation": {
                "issue_count":         validation.issue_count,
                "billing_block_count": validation.billing_block_count,
                "rules":               validation.rules,
                "skipped_rules":      validation.skipped_rules,
            },
            "legal_basis": "HeizkostenV §5 Abs. 3 / §6a; MessEG §37",
        })),
    )
        .into_response()
}

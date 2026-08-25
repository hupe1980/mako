//! Zählerstandsgang ingest — the MSB-side differencing of BK6-24-174.
//!
//! # What this door is for
//!
//! An intelligentes Messsystem does not measure energy per quarter-hour. It
//! reads a **register**, and § 2 Satz 1 Nr. 27 MsbG says so verbatim: a
//! Zählerstandsgang is *"die Messung einer Reihe **viertelstündig ermittelter
//! Zählerstände** von elektrischer Arbeit und **stündlich ermittelter
//! Zählerstände** von Gasmengen"*. Two media, two resolutions.
//!
//! **BK6-24-174** — *"Anpassung der Marktkommunikation zur Realisierung der nach
//! dem Messstellenbetriebsgesetz geforderten Übermittlung von
//! Zählerstandsgängen (Datenübermittlung ZSG)"*, Beschluss 24.10.2024, wirksam
//! **06.06.2025** — puts the differencing at the Messstellenbetreiber:
//!
//! ```text
//! SMGW ──Zählerstandsgang──► MSB ──Lastgang──► NB, Lieferant
//!                             └── this module
//! ```
//!
//! edmd is the MSB side. Every other ingest door takes a Lastgang somebody else
//! differenced; this one takes what the gateway produced, where the register
//! width, the connection capacity and the audit trail all live.
//!
//! # Both halves are stored
//!
//! The readings **and** the intervals they produced. § 146 Abs. 4 AO requires
//! the original to stay recoverable after a change, and a stored difference
//! cannot reproduce the register values it came from — nor can a customer check
//! it against the number on their meter. The readings are also what answers
//! § 40 Abs. 2 Nr. 6 EnWG, the opening and closing Zählerstand on an invoice
//! (`meter_billing_periods.zaehlerstand_anfang` / `_ende`).
//!
//! # Nothing is invented
//!
//! Where a difference cannot be taken honestly — a backwards step no register
//! width explains, a jump beyond the connection's capacity — `metering::reading`
//! emits **no interval** and records why. The hole then shows up as a V01 gap in
//! validation and is filled, with its own audit row, by the § 60 Abs. 2 MsbG
//! substitute path. Guessing here would bury the problem inside a value that
//! looks measured, and the two audit logs together say "this quarter-hour is an
//! Ersatzwert *because* the register went backwards", which neither says alone.
//!
//! # Rollover belongs here
//!
//! A wrap is a property of a **register** — a six-digit Zählwerk going from
//! 999 999 to 0 — so it is detectable only where readings live, which is here.
//! It cannot be a validation rule over intervals: an interval value is not
//! cumulative and has nothing to roll over.

#[allow(unused_imports)]
use super::*;

use metering::reading::{LastgangConfig, MeterReading as Zaehlerstand, to_lastgang};

/// One register reading in a Zählerstandsgang push.
#[derive(Debug, serde::Deserialize)]
pub struct ZsgReading {
    /// The instant the register held this value (RFC 3339 UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// The register value, in the unit the register counts — kWh for
    /// electricity and heat, m³ for gas and water. **Not** pre-converted: the
    /// conversion applies to the difference (§ 25 Nr. 4 MessEV).
    pub value: Decimal,
    /// Reading quality per BDEW Messwertstatus. Defaults to `MEASURED`.
    #[serde(default)]
    pub quality: Option<String>,
}

/// Request body for `POST /api/v1/zaehlerstandsgang/{malo_id}`.
///
/// ```json
/// {
///   "session_id": "SMGW-SN1234-2026-07-12T00:00:00Z",
///   "sparte": "STROM",
///   "obis_code": "1-0:1.8.0",
///   "sender_mp_id": "9900000000001",
///   "register_digits": 6,
///   "max_plant_power_kw": "30.0",
///   "readings": [
///     { "at": "2026-07-12T00:00:00Z", "value": "14230.500" },
///     { "at": "2026-07-12T00:15:00Z", "value": "14231.250" }
///   ]
/// }
/// ```
#[derive(Debug, serde::Deserialize)]
pub struct ZsgPushRequest {
    /// Caller-supplied idempotency key (e.g. SMGW serial + timestamp).
    pub session_id: Option<String>,
    /// Ingestion source, from the `IngestionSource` vocabulary. Defaults to
    /// `DIRECT_PUSH` — a Zählerstandsgang comes from a gateway.
    #[serde(default)]
    pub source: Option<String>,
    /// `STROM` (default) · `GAS` · `WAERME` · `WASSER`.
    #[serde(default)]
    pub sparte: Option<String>,
    /// OBIS register the Zählerstände were read from, e.g. `1-0:1.8.0`.
    ///
    /// Value group `D = 8` is a Zählerstand; the derived intervals are labelled
    /// with the **Lastgang** code for the same direction instead (`1-0:1.29.0`),
    /// because a Lastgang is a different channel from the register it came from.
    pub obis_code: Option<String>,
    /// 33-character MeLo-ID, if known.
    pub melo_id: Option<String>,
    /// MP-ID of the reporting MSB. Keys the meterstore version scope.
    pub sender_mp_id: Option<String>,
    /// Decimal places **before** the point on the register, so it wraps at
    /// `10^digits`.
    ///
    /// German electricity meters are typically six-digit (999 999 kWh), gas
    /// meters five or six. Omit it and a backwards step is refused as an anomaly
    /// rather than reconstructed — which is the safe direction, because
    /// **guessing the width wrong turns a meter exchange into a million kWh of
    /// consumption**.
    pub register_digits: Option<u32>,
    /// Connection capacity in kW, the plausibility ceiling on one difference.
    ///
    /// A backwards step has two explanations — a register wrap and a meter
    /// exchange — and this is what tells them apart: a wrap implying more energy
    /// than the connection can pass is refused. It is the same ceiling V12
    /// applies to the resulting Lastgang, but here it can prevent a bad value
    /// rather than merely flag one.
    pub max_plant_power_kw: Option<Decimal>,
    /// Brennwert Hs in kWh/m³ — required when the Sparte is gas, because the
    /// **difference** is converted before it is stored (§ 25 Nr. 4 MessEV).
    pub brennwert_kwh_per_m3: Option<Decimal>,
    /// Zustandszahl; defaults to 1.0.
    pub zustandszahl: Option<Decimal>,
    /// MSCONS correction version this Zählerstandsgang is delivered under.
    #[serde(default)]
    pub mscons_version: Option<u128>,
    /// The register readings. Order does not matter — they are sorted before
    /// differencing, so a series merged out of order converts correctly instead
    /// of producing a run of negative differences.
    pub readings: Vec<ZsgReading>,
}

/// `POST /api/v1/zaehlerstandsgang/{malo_id}`
///
/// Ingest a Zählerstandsgang, difference it into a Lastgang, and store both.
///
/// Returns `201` for a clean conversion, `202` when the conversion had anything
/// to report (a reconstructed wrap, a refused difference, or a V-rule finding on
/// the resulting Lastgang), and `200` for a replayed `session_id`.
///
/// **Cedar action**: `write-meter-reads`
pub async fn post_zaehlerstandsgang(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Json(req): Json<ZsgPushRequest>,
) -> impl IntoResponse {
    use crate::domain::{MeterReading, ZSG_OUTCOME_ROLLOVER, ZsgConversionEntry, anomaly_outcome};
    use time::format_description::well_known::Rfc3339;

    let tenant = state.tenant.clone();
    if let Err(e) = enforcer.check(&claims.principal(), "write-meter-reads", tenant.as_str()) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    if req.readings.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "readings must not be empty" })),
        )
            .into_response();
    }
    // The same ceiling the interval doors carry, for the same reason: one
    // request must not be able to saturate the write path for every other
    // tenant. A Zählerstandsgang is one reading per slot, so the bound is on
    // readings rather than intervals.
    const MAX_READINGS: usize = 50_000;
    if req.readings.len() > MAX_READINGS {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({
                "error": format!("batch too large: {} > {MAX_READINGS}", req.readings.len()),
            })),
        )
            .into_response();
    }

    let sparte = match req.sparte.as_deref() {
        None => crate::domain::Sparte::Strom,
        Some(raw) => match crate::domain::parse_sparte(raw) {
            Some(s) => s,
            None => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": format!("unknown sparte `{raw}`"),
                        "expected": crate::domain::Sparte::CODES,
                    })),
                )
                    .into_response();
            }
        },
    };

    let ingestion_source = match req
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => IngestionSource::DirectPush,
        Some(raw) => match IngestionSource::parse_db_str(raw) {
            Some(s) => s,
            None => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": format!("unknown source `{raw}`"),
                        "expected": IngestionSource::ALL
                            .iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    })),
                )
                    .into_response();
            }
        },
    };

    // Gas is metered in m³ and settled in kWh, so the **difference** needs the
    // Brennwert before it can be stored. Required rather than defaulted: the
    // calorific value varies by supply area and month, and a national average
    // would systematically mis-bill an L-Gas network.
    if sparte.requires_conversion() && req.brennwert_kwh_per_m3.is_none() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "brennwert_kwh_per_m3 is required for a gas Zählerstandsgang \
                          (§ 25 Nr. 4 MessEV) — the register counts m³ and the difference \
                          is stored in kWh_Hs",
            })),
        )
            .into_response();
    }

    let session_id = req.session_id.clone().unwrap_or_else(|| {
        req.readings.first().map_or_else(
            || uuid::Uuid::new_v4().to_string(),
            |r| format!("{malo_id}-{}", r.at),
        )
    });

    // Idempotency, on the same table and with the same tenant scoping as every
    // other push door.
    match sqlx::query_scalar::<_, String>(
        r"SELECT status FROM direct_push_sessions
          WHERE session_id = $1 AND malo_id = $2 AND tenant = $3 AND status = 'committed'",
    )
    .bind(&session_id)
    .bind(&malo_id)
    .bind(&tenant)
    .fetch_optional(state.repo.pool())
    .await
    {
        Ok(Some(_)) => {
            return (
                StatusCode::OK,
                Json(serde_json::json!({
                    "malo_id": malo_id,
                    "session_id": session_id,
                    "status": "already_committed",
                })),
            )
                .into_response();
        }
        Ok(None) => {}
        // A failed lookup is not evidence that the session is new — re-ingesting
        // would re-emit the CloudEvents a billing recompute hangs off.
        Err(e) => {
            tracing::error!(malo_id, error = %e, "edmd: ZSG idempotency check failed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "could not verify whether this session was already committed",
                })),
            )
                .into_response();
        }
    }

    // ── Parse the readings ───────────────────────────────────────────────────
    let mut zsg: Vec<Zaehlerstand> = Vec::with_capacity(req.readings.len());
    let mut rejected: Vec<String> = Vec::new();
    let register: Option<metering::obis::ObisCode> =
        req.obis_code.as_deref().and_then(|s| s.parse().ok());
    for r in &req.readings {
        // A register counts upward from zero; a negative Zählerstand is a decode
        // fault, not a measurement.
        if r.value < Decimal::ZERO {
            rejected.push(format!("negative Zählerstand {} at {}", r.value, r.at));
            continue;
        }
        let quality = match r.quality.as_deref() {
            None => QualityFlag::Measured,
            Some(raw) => match quality_flag_from_wire(raw) {
                Some(q) => q,
                None => {
                    rejected.push(format!("unknown quality `{raw}` at {}", r.at));
                    continue;
                }
            },
        };
        zsg.push(Zaehlerstand {
            at: r.at,
            value: r.value,
            quality,
            obis_code: register,
        });
    }
    if zsg.len() < 2 {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "a Zählerstandsgang needs at least two usable readings — \
                          one reading is a Zählerstand, and the interval energy is \
                          the difference between two of them",
                "usable": zsg.len(),
                "rejected": rejected,
            })),
        )
            .into_response();
    }

    // ── Difference ───────────────────────────────────────────────────────────
    //
    // The result is labelled with the **Lastgang** code for the register's
    // direction, not the Zählerstand code it came from: `1-0:1.8.0` is a
    // register reading (D = 8) and `1-0:1.29.0` is the Lastgang (D = 29).
    // Carrying the reading's own code through is convenient and mislabels the
    // result — and edmd's register projection keys on exactly these codes.
    let mut config = LastgangConfig::default();
    if let Some(code) = lastgang_code(register) {
        config = config.labelled(code);
    }
    if let Some(digits) = req.register_digits {
        config = config.with_register_digits(digits);
    }
    // The plausibility ceiling, expressed for the *reading interval* rather than
    // per hour, from the observed cadence of this very Zählerstandsgang.
    //
    // **In the register's own unit.** `with_capacity_kw` derives
    // `max_delta = kW × hours`, which is kWh — and the differences being capped
    // are whatever the register counts. For electricity and heat those agree;
    // for a gas register counting m³ they do not, and comparing an m³ difference
    // against a kWh ceiling makes the cap wrong by the Brennwert factor, roughly
    // tenfold too loose. The conversion is the same one the difference itself
    // gets, run backwards: `m³ = kWh / (Hs × Z)`.
    let cadence_secs = observed_cadence_secs(&zsg, sparte);
    let z = req.zustandszahl.unwrap_or(Decimal::ONE);
    if let Some(kw) = req.max_plant_power_kw.filter(|v| *v > Decimal::ZERO) {
        let hours = Decimal::from(cadence_secs) / Decimal::from(3600u32);
        let max_delta_kwh = kw * hours;
        let max_delta = if sparte.requires_conversion() {
            // Refuse the cap rather than apply it in the wrong dimension: a
            // ceiling ten times too loose catches nothing and reads as though it
            // does. The gas door already requires the Brennwert, so this is only
            // reachable for a zero or negative one.
            let factor = req.brennwert_kwh_per_m3.unwrap_or(Decimal::ZERO) * z;
            (factor > Decimal::ZERO).then(|| max_delta_kwh / factor)
        } else {
            Some(max_delta_kwh)
        };
        if let Some(max_delta) = max_delta {
            config = config.with_max_delta(max_delta);
        } else {
            tracing::warn!(
                malo_id,
                sparte = sparte.as_str(),
                "edmd: ZSG capacity ceiling not applied — it is stated in kW and the \
                 register counts m³, and no usable Brennwert was supplied to convert it"
            );
        }
    }
    let lastgang = to_lastgang(&zsg, &config);

    // ── Persist the readings — the primary record ────────────────────────────
    let obis_for_readings = req.obis_code.clone();
    let readings: Vec<MeterReading> = zsg
        .iter()
        .map(|r| MeterReading {
            malo_id: malo_id.clone(),
            read_at: r.at,
            zaehlerstand: r.value,
            quality: r.quality,
            sparte,
            obis_code: obis_for_readings.clone(),
            melo_id: req.melo_id.clone(),
            tenant: tenant.clone(),
            source: ingestion_source,
            sender_mp_id: req.sender_mp_id.clone(),
            push_session: Some(session_id.clone()),
        })
        .collect();
    if let Err(e) = state.repo.store_readings(&readings).await {
        tracing::error!(malo_id, error = %e, "edmd: Zählerstandsgang store failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // ── The conversion audit (§ 146 Abs. 4 AO) ───────────────────────────────
    let obis_norm = crate::domain::normalise_obis_code(req.obis_code.as_deref());
    let mut audit: Vec<ZsgConversionEntry> = Vec::new();
    for r in &lastgang.rollovers {
        audit.push(ZsgConversionEntry {
            tenant: tenant.clone(),
            malo_id: malo_id.clone(),
            obis_code_norm: obis_norm.clone(),
            // The wrap explains the span between the two readings it sits
            // between, and `Rollover` carries both instants.
            span_from: r.from,
            span_to: r.to,
            outcome: ZSG_OUTCOME_ROLLOVER,
            previous_value: r.previous,
            current_value: r.current,
            delta: Some(r.delta),
            register_capacity: Some(r.register_capacity),
            session_id: Some(session_id.clone()),
        });
    }
    for a in &lastgang.anomalies {
        audit.push(ZsgConversionEntry {
            tenant: tenant.clone(),
            malo_id: malo_id.clone(),
            obis_code_norm: obis_norm.clone(),
            span_from: a.from,
            span_to: a.to,
            outcome: anomaly_outcome(a.kind),
            previous_value: a.previous,
            current_value: a.current,
            delta: None,
            register_capacity: None,
            session_id: Some(session_id.clone()),
        });
    }
    if !audit.is_empty()
        && let Err(e) = state.repo.log_zsg_conversion(&audit).await
    {
        // The readings are stored and the intervals are about to be. A missing
        // audit row is a gap in the § 146 Abs. 4 AO trail rather than lost data,
        // so it is surfaced and the request continues.
        tracing::warn!(malo_id, error = %e, "edmd: ZSG conversion audit could not be written");
    }

    // ── Store the derived Lastgang through the ordinary validated path ───────
    let batch: Vec<MeterRead> = lastgang
        .intervals
        .iter()
        .map(|iv| MeterRead {
            malo_id: malo_id.clone(),
            melo_id: req.melo_id.clone(),
            dtm_from: iv.from,
            dtm_to: iv.to,
            // Gas registers m³ and settles in kWh_Hs. The conversion is applied
            // to the **difference**, which is the quantity § 25 Nr. 4 MessEV is
            // about — the register value itself stays as the meter displays it.
            quantity_kwh: match req.brennwert_kwh_per_m3 {
                Some(hs) if sparte.requires_conversion() => {
                    metering::gas_m3_to_kwh_hs(iv.value, hs, z)
                }
                _ => iv.value,
            },
            quality: iv.quality,
            pid: 0,
            sparte,
            obis_code: iv.obis_code.map(|c| c.to_string()),
            tenant: tenant.clone(),
            source: ingestion_source,
            push_session: Some(session_id.clone()),
            quality_warnings: None,
            sender_mp_id: req.sender_mp_id.clone(),
            allocation_version: "INITIAL".to_owned(),
            valid_from_tx: Some(OffsetDateTime::now_utc()),
            mscons_version: req.mscons_version,
        })
        .collect();

    // V12 checks the **stored** quantity, which is kWh for every Sparte that
    // settles in energy — the gas conversion has already been applied above — so
    // the ceiling goes in as stated, unconverted. Water settles in m³ and has no
    // kW ceiling to speak of, so it is not offered one.
    let capacity_for_v12 = req
        .max_plant_power_kw
        .filter(|_| sparte.billing_unit() == metering::MeasurementUnit::KiloWattHour);
    let (validated, validation) = crate::domain::ValidatedReads::validate(
        batch,
        crate::domain::IngestContext::new("ZSG_VALIDATION", &malo_id)
            .with_capacity_kw(capacity_for_v12),
    );
    let (period_from, period_to) = crate::domain::batch_period(validated.as_slice());
    let hampel = crate::server::score_batch(validated.as_slice());
    let stored = validated.len();
    if let Err(e) = state.repo.store_reads(validated).await {
        tracing::error!(malo_id, error = %e, "edmd: derived Lastgang store failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    if let Some(q) = &hampel {
        q.record(state.repo.pool(), &tenant, &malo_id).await;
    }

    let _ = sqlx::query(
        r"INSERT INTO direct_push_sessions
              (session_id, malo_id, source, obis_code, interval_count,
               period_from, period_to, status, tenant)
          VALUES ($1,$2,$3,$4,$5,$6,$7,'committed',$8)
          ON CONFLICT (tenant, session_id) DO UPDATE
              SET status         = 'committed',
                  interval_count = EXCLUDED.interval_count",
    )
    .bind(&session_id)
    .bind(&malo_id)
    .bind(ingestion_source.as_str())
    .bind(&req.obis_code)
    .bind(i32::try_from(stored).unwrap_or(i32::MAX))
    .bind(period_from)
    .bind(period_to)
    .bind(&tenant)
    .execute(state.repo.pool())
    .await;

    let alert = crate::server::quality_alert::QualityAlert {
        malo_id: &malo_id,
        door: "zaehlerstandsgang",
        correlation_id: &session_id,
        causation_id: &session_id,
        sparte: Some(sparte.as_str()),
        period_from,
        period_to,
        validation: &validation,
        hampel: hampel
            .as_ref()
            .map(|q| crate::server::hampel_summary(&q.report)),
    };
    crate::server::quality_alert::raise_quality_warning(
        state.erp_webhook_url.as_deref(),
        state.webhook_secret_bytes(),
        &tenant,
        &alert,
    )
    .await;

    let anomalies: Vec<serde_json::Value> = lastgang
        .anomalies
        .iter()
        .map(|a| {
            serde_json::json!({
                "from": a.from.format(&Rfc3339).unwrap_or_default(),
                "to": a.to.format(&Rfc3339).unwrap_or_default(),
                "kind": anomaly_outcome(a.kind),
                "reason": a.kind.description(),
                "previous": a.previous.to_string(),
                "current": a.current.to_string(),
            })
        })
        .collect();
    let rollovers: Vec<serde_json::Value> = lastgang
        .rollovers
        .iter()
        .map(|r| {
            serde_json::json!({
                "from": r.from.format(&Rfc3339).unwrap_or_default(),
                "to": r.to.format(&Rfc3339).unwrap_or_default(),
                "previous": r.previous.to_string(),
                "current": r.current.to_string(),
                "register_capacity": r.register_capacity.to_string(),
                "delta": r.delta.to_string(),
            })
        })
        .collect();

    let clean = lastgang.is_clean() && rejected.is_empty() && !alert.is_warning();
    (
        if clean {
            StatusCode::CREATED
        } else {
            StatusCode::ACCEPTED
        },
        Json(serde_json::json!({
            "malo_id": malo_id,
            "session_id": session_id,
            "readings_stored": readings.len(),
            "intervals_derived": stored,
            "cadence_secs": cadence_secs,
            // n usable readings give n−1 intervals, minus one per refused span.
            // Reported rather than left to be inferred: the difference between
            // those two numbers is exactly what the § 60 Abs. 2 substitute path
            // will be asked to fill.
            "rollovers": rollovers,
            "anomalies": anomalies,
            "rejected_readings": rejected,
            "validation": {
                "issue_count": validation.issue_count,
                "billing_block_count": validation.billing_block_count,
                "rules": validation.rules,
                "skipped_rules": validation.skipped_rules,
            },
            "quality": hampel.as_ref().map(|q| crate::server::hampel_summary(&q.report)),
            "legal_basis": "BK6-24-174 (Datenübermittlung ZSG) · § 2 Satz 1 Nr. 27 MsbG",
        })),
    )
        .into_response()
}

/// `GET /api/v1/zaehlerstandsgang/{malo_id}?from=&to=`
///
/// The stored register readings — the primary record behind the derived
/// Lastgang, and the source of the § 40 Abs. 2 Nr. 6 EnWG opening and closing
/// Zählerstand on an invoice.
pub(crate) async fn get_zaehlerstandsgang(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<SimpleTimeParams>,
) -> impl IntoResponse {
    use time::format_description::well_known::Rfc3339;

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
    match state
        .repo
        .readings(&malo_id, from, to, state.tenant.as_str())
        .await
    {
        Ok(readings) => {
            let items: Vec<serde_json::Value> = readings
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "at": r.read_at.format(&Rfc3339).unwrap_or_default(),
                        "zaehlerstand": r.zaehlerstand.to_string(),
                        // The unit the register counts, not the settlement unit:
                        // a gas Zählerstand is m³ and stays m³.
                        "unit": r.sparte.measured_unit().as_str(),
                        "quality": crate::store::quality_to_str(r.quality),
                        "obis_code": r.obis_code,
                    })
                })
                .collect();
            Json(serde_json::json!({
                "malo_id": malo_id,
                "count": items.len(),
                "readings": items,
                "legal_basis": "BK6-24-174 (Datenübermittlung ZSG) · § 2 Satz 1 Nr. 27 MsbG",
            }))
            .into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: Zählerstandsgang read failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// The Lastgang OBIS code for the direction a Zählerstand register reports.
///
/// A Lastgang is a different channel from the register it was differenced from:
/// `1-0:1.8.0` is a Zählerstand (Messart `D = 8`) and `1-0:1.29.0` the Lastgang
/// (`D = 29`). Carrying the reading's own code onto the derived intervals is
/// convenient and mislabels them — and edmd's register projection keys on
/// exactly these codes, so a Lastgang wearing a Zählerstand's label is a
/// Zählerstand to every aggregate downstream.
///
/// `None` for anything that is not an electricity import/export register: the
/// mapping is only defined where the Messart axis is, and inventing one for a
/// gas Messgröße would relabel the series with a code that means something else.
fn lastgang_code(code: Option<metering::obis::ObisCode>) -> Option<metering::obis::ObisCode> {
    use metering::obis::ObisCode;
    let c = code?;
    if !c.is_electricity() {
        return None;
    }
    if c.is_import() {
        Some(ObisCode::STROM_BEZUG_LASTGANG)
    } else if c.is_export() {
        Some(ObisCode::STROM_EINSPEISUNG_LASTGANG)
    } else {
        None
    }
}

/// The cadence of a Zählerstandsgang, in seconds.
///
/// Taken from the readings themselves — the median spacing between consecutive
/// instants — because that is what the plausibility ceiling has to be expressed
/// for. § 2 Satz 1 Nr. 27 MsbG fixes the two normal answers (quarter-hourly for
/// electricity, hourly for gas) and those are the fallback, but a device that
/// reports on some other grid must not have its differences judged against a
/// window it never used.
///
/// Unlike an interval series, a Zählerstandsgang carries no durations to take a
/// median of — a reading is a point — so this measures the gaps between points.
fn observed_cadence_secs(readings: &[Zaehlerstand], sparte: crate::domain::Sparte) -> u32 {
    let fallback = || {
        metering::QualityConfig::for_sparte(sparte)
            .validation
            .expected_interval_secs
            .unwrap_or(900)
    };
    let mut ats: Vec<OffsetDateTime> = readings.iter().map(|r| r.at).collect();
    ats.sort_unstable();
    let mut gaps: Vec<i64> = ats
        .windows(2)
        .map(|w| (w[1] - w[0]).whole_seconds())
        .filter(|s| *s > 0)
        .collect();
    if gaps.is_empty() {
        return fallback();
    }
    gaps.sort_unstable();
    u32::try_from(gaps[gaps.len() / 2]).unwrap_or_else(|_| fallback())
}

#[cfg(test)]
mod tests {
    use super::*;
    use metering::obis::ObisCode;

    #[test]
    fn a_derived_lastgang_is_labelled_as_a_lastgang() {
        let zaehlerstand: ObisCode = "1-0:1.8.0".parse().expect("obis");
        assert_eq!(
            lastgang_code(Some(zaehlerstand)),
            Some(ObisCode::STROM_BEZUG_LASTGANG),
            "the difference of a Bezugs-Zählerstand is the Bezugs-Lastgang"
        );
        let einspeisung: ObisCode = "1-0:2.8.0".parse().expect("obis");
        assert_eq!(
            lastgang_code(Some(einspeisung)),
            Some(ObisCode::STROM_EINSPEISUNG_LASTGANG)
        );
    }

    #[test]
    fn a_gas_register_keeps_its_own_code() {
        // Value group C is a Messgröße on a gas code, not a direction, so there
        // is no Lastgang code to map onto and inventing one would relabel the
        // series with something that means a different quantity.
        let gas: ObisCode = "7-1:99.33.17".parse().expect("obis");
        assert_eq!(lastgang_code(Some(gas)), None);
        assert_eq!(lastgang_code(None), None);
    }

    /// The plausibility ceiling is stated in kW and the differences are in the
    /// register's own unit. For gas those disagree by the Brennwert.
    #[test]
    fn a_gas_capacity_ceiling_is_converted_into_the_registers_unit() {
        use rust_decimal::Decimal;

        // 30 kW over a quarter-hour is 7.5 kWh.
        let kw = Decimal::from(30u32);
        let hours = Decimal::from(900u32) / Decimal::from(3600u32);
        let max_delta_kwh = kw * hours;
        assert_eq!(max_delta_kwh, Decimal::from_str_exact("7.5").expect("dec"));

        // A gas register counts m³, and at Hs = 10 kWh/m³ with Z = 1 that same
        // ceiling is 0.75 m³. Applied unconverted it would be 7.5 m³ — ten times
        // too loose, so it would catch nothing while appearing to.
        let hs = Decimal::from(10u32);
        let z = Decimal::ONE;
        let max_delta_m3 = max_delta_kwh / (hs * z);
        assert_eq!(max_delta_m3, Decimal::from_str_exact("0.75").expect("dec"));
        assert_ne!(max_delta_m3, max_delta_kwh);

        // And the conversion is the exact inverse of the one the difference gets.
        assert_eq!(
            metering::gas_m3_to_kwh_hs(max_delta_m3, hs, z),
            max_delta_kwh,
            "converting the ceiling into m³ and the value into kWh must agree"
        );
    }

    #[test]
    fn the_cadence_is_the_readings_own_spacing() {
        let base = OffsetDateTime::UNIX_EPOCH;
        let hourly: Vec<Zaehlerstand> = (0..5)
            .map(|i| Zaehlerstand {
                at: base + time::Duration::hours(i),
                value: Decimal::from(i),
                quality: QualityFlag::Measured,
                obis_code: None,
            })
            .collect();
        assert_eq!(
            observed_cadence_secs(&hourly, crate::domain::Sparte::Strom),
            3600,
            "an hourly ZSG is not judged against a quarter-hour ceiling"
        );

        // Too short to observe: the Sparte's own default, not a hard-coded 900.
        let one = &hourly[..1];
        assert_eq!(
            observed_cadence_secs(one, crate::domain::Sparte::Gas),
            3600,
            "§ 2 Satz 1 Nr. 27 MsbG makes gas hourly"
        );
    }
}

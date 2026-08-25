//! Annual consumption projection (§ 40a Abs. 2 EnWG Verbrauchsschätzung) and
//! §22 EnWG Netzverlust.

#[allow(unused_imports)]
use super::*;

/// What an annual projection is for, and under which provision.
///
/// § 40a Abs. 2 EnWG authorises billing on an estimate — based on the previous
/// period or on comparable customers — where no reading was transmitted;
/// § 13 Abs. 1 StromGVV sizes an Abschlag the same way. Both verified against the
/// consolidated texts. Not § 60 Abs. 2 MsbG, which is Ersatzwertbildung in the
/// Smart-Meter-Gateway — a different obligation.
pub(crate) const FORECAST_LEGAL_BASIS: &str =
    "§ 40a Abs. 2 EnWG (Verbrauchsschätzung) · § 13 Abs. 1 StromGVV (Abschlagshöhe)";

// ── Annual forecast ─────────────────────────────────────────────────────

/// `GET /api/v1/forecast/{malo_id}?from=&to=`
///
/// Projects a year's consumption from the meter reads in the given window,
/// with a prior-year seasonal correction when the same window one year earlier
/// has data.
///
/// ## What this is, legally
///
/// It is a **Verbrauchsschätzung** in the sense of **§ 40a Abs. 2 EnWG**: where
/// no reading has been transmitted, the supplier may bill on an estimate made
/// *"unter angemessener Berücksichtigung der tatsächlichen Verhältnisse"*, based
/// on the previous period's consumption or on comparable customers — which is
/// exactly a prior-year-corrected projection over the point's own history. The
/// same figure sizes an Abschlag under **§ 13 Abs. 1 StromGVV**, which measures
/// it *"anteilig entsprechend dem Verbrauch im vorangegangenen
/// Abrechnungszeitraum"*.
///
/// It is **not** § 60 Abs. 2 MsbG: that provision places *Plausibilisierung und
/// Ersatzwertbildung* in the Smart-Meter-Gateway (BSI assessment, BNetzA
/// Festlegung under § 75), which is the anchor for `server::substitute` and says
/// nothing about forecasting.
///
/// Uses:
/// - Setting Abschlag (advance payment) amounts
/// - Anticipating the Mehr-/Mindermengensaldo at year-end
/// - Feeding a Jahresverbrauchsprognose into SLP-Bilanzierung
pub(crate) async fn get_annual_forecast(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<SimpleTimeParams>,
) -> impl IntoResponse {
    use metering::project_annual_consumption;

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
            tracing::warn!(error = %e, malo_id, "edmd: forecast query failed");
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

    // A Jahresprognose is about the consumption, so the reads go through the
    // canonical Bezug projection: projecting a series that still carried the
    // measuring point's Einspeisung, its Blindarbeit and its HT/NT split beside
    // the total forecast a year of energy the customer will never draw.
    let intervals = crate::domain::energy_intervals(&reads, crate::domain::EnergyDirection::Bezug);

    // Same window one year earlier: with prior-year data the projection
    // applies the seasonal correction factor (a winter-only observation no
    // longer over-projects the year). Passing `None` here made the seasonal
    // branch unreachable — every API forecast was the naive daily × 365.
    let prior_q = TimeSeriesQuery {
        malo_id: malo_id.clone(),
        from: from - time::Duration::days(365),
        to: to - time::Duration::days(365),
        sparte: None,
        tenant: state.tenant.clone(),
    };
    let prior_intervals: Vec<metering::MeterInterval> = match state.repo.query(&prior_q).await {
        // The seasonal factor compares two windows, so both must be the same
        // projection — comparing a raw prior year against a projected current one
        // would scale the forecast by the register mix rather than the season.
        Ok(prior) => crate::domain::energy_intervals(&prior, crate::domain::EnergyDirection::Bezug),
        Err(e) => {
            tracing::debug!(error = %e, malo_id, "edmd: no prior-year window for seasonal correction");
            Vec::new()
        }
    };
    let prior = (!prior_intervals.is_empty()).then_some(prior_intervals.as_slice());

    // `malo_id` is the caller's, not the forecast's: `metering` 0.17 dropped it
    // from `AnnualForecast`, correctly — a projection is arithmetic over a
    // series and does not know whose series it is. The endpoint already has it.
    match project_annual_consumption(&intervals, prior) {
        Some(forecast) => Json(serde_json::json!({
            "malo_id": malo_id,
            "observation_from": forecast.observation_from,
            "observation_to": forecast.observation_to,
            "observed_kwh": forecast.observed,
            "observed_days": forecast.observed_days,
            "target_year_days": forecast.target_year_days,
            "projected_annual_kwh": forecast.projected_annual,
            "seasonal_correction_applied": forecast.seasonal_correction_applied,
            "seasonal_factor": forecast.seasonal_factor,
            // New in 0.17: a 95 % prediction interval over the observed daily
            // sums. `None` when fewer than two whole days were observed — a
            // projection with no spread stated is one a caller cannot judge.
            "confidence_lower_kwh": forecast.confidence_lower,
            "confidence_upper_kwh": forecast.confidence_upper,
            "prediction_interval_note": metering::AnnualForecast::prediction_interval_note(),
            "legal_basis": FORECAST_LEGAL_BASIS,
        }))
        .into_response(),
        None => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "insufficient data for annual forecast (minimum 7 days required)"
            })),
        )
            .into_response(),
    }
}

// ── Netzverlust (§22 EnWG) ────────────────────────────────────────────────────

/// `GET /api/v1/netzverlust?from=&to=`
///
/// Indicative grid-loss balance over the tenant's metered portfolio:
/// infeed (OBIS `x:2.*` — generation feed-in and imports) minus offtake
/// (OBIS `x:1.*`). §22 Abs. 1 EnWG obliges the Netzbetreiber to procure
/// Verlustenergie; this figure is the metering-side indicator for that
/// quantity. Accuracy is bounded by metering coverage — unmetered infeed
/// or offtake shows up as phantom loss/gain, which is why the response
/// labels itself indicative rather than settlement-grade.
pub(crate) async fn get_netzverlust(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
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
    // Netzverlust is a tenant-wide aggregate across every Strom MaLo, so it is a
    // cross-MaLo query over meterstore's version-resolved relation rather than a
    // per-MaLo `repo.query`. The value column is `value` and the interval bounds
    // are `from`/`to` (reserved words, hence quoted); the tenant and time bounds
    // travel as bound parameters so no value reaches the SQL text.
    //
    // The OBIS value groups are split out rather than pattern-matched. Direction
    // is group **C** (1 = Bezug, 2 = Einspeisung), the Messart is **D** and the
    // tariff stage is **E**, and `LIKE '%:1.%'` tested none of them: it matched
    // any code with `:1.` anywhere, so it swept in the `1-0:1.6.0` maximum
    // register — a **kW** peak — as though it were energy.
    //
    // The aggregate is per MaLo and per stage, not one grand total, because a
    // meter reporting `1.8.0` beside `1.8.1`/`1.8.2` reports the same
    // consumption twice: the total register *is* the tariff registers' sum. The
    // per-MaLo rows are folded below, preferring the total where there is one.
    let store = state.repo.store();
    let sql = format!(
        r#"WITH groups AS (
              SELECT "malo_id",
                     "value",
                     SPLIT_PART(SPLIT_PART("obis_code", ':', 2), '.', 1) AS c,
                     SPLIT_PART(SPLIT_PART("obis_code", ':', 2), '.', 2) AS d,
                     SPLIT_PART(SPLIT_PART("obis_code", ':', 2), '.', 3) AS e
                FROM "{table}"
               WHERE "tenant" = $1
                 AND "from" >= $2 AND "to" <= $3
                 AND "sparte" = 'STROM'
                 AND "quality" NOT IN ('FAULTY', 'UNKNOWN')
           )
           SELECT "malo_id",
                  COALESCE(SUM(CASE WHEN c = '1' AND e =  '0' THEN "value" END), 0) AS bezug_total,
                  COALESCE(SUM(CASE WHEN c = '1' AND e <> '0' THEN "value" END), 0) AS bezug_tariff,
                  COUNT(CASE WHEN c = '1' AND e =  '0' THEN 1 END)                  AS bezug_total_n,
                  COALESCE(SUM(CASE WHEN c = '2' AND e =  '0' THEN "value" END), 0) AS einsp_total,
                  COALESCE(SUM(CASE WHEN c = '2' AND e <> '0' THEN "value" END), 0) AS einsp_tariff,
                  COUNT(CASE WHEN c = '2' AND e =  '0' THEN 1 END)                  AS einsp_total_n
             FROM groups
            WHERE c IN ('1', '2')
              AND d <> '6'
              AND e <> '63'
            GROUP BY "malo_id""#,
        table = store.resolved_table(),
    );
    let result = store
        .query_with_params(
            &sql,
            vec![
                datafusion::scalar::ScalarValue::Utf8(Some(state.tenant.clone())),
                ts_param(from),
                ts_param(to),
            ],
        )
        .await;

    match result.and_then(|r| r.to_json()) {
        Ok(rows) => {
            // One row per MaLo. Decimal sums render as JSON number literals; parse
            // them back through their text to keep full precision for this
            // indicative figure.
            let dec = |row: &serde_json::Value, key: &str| -> rust_decimal::Decimal {
                row.get(key)
                    .map(std::string::ToString::to_string)
                    .and_then(|s| s.trim_matches('"').parse().ok())
                    .unwrap_or_default()
            };
            let count = |row: &serde_json::Value, key: &str| -> i64 {
                row.get(key)
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0)
            };
            // Per MaLo and per direction: the total register when the meter
            // reports one, otherwise the tariff registers summed. Presence is
            // decided by the row **count**, not by a non-zero sum — a meter that
            // legitimately drew nothing in the window still reported a total, and
            // testing the sum would fall through to double-counting its split.
            let mut einspeisung = rust_decimal::Decimal::ZERO;
            let mut entnahme = rust_decimal::Decimal::ZERO;
            for row in &rows {
                entnahme += if count(row, "bezug_total_n") > 0 {
                    dec(row, "bezug_total")
                } else {
                    dec(row, "bezug_tariff")
                };
                einspeisung += if count(row, "einsp_total_n") > 0 {
                    dec(row, "einsp_total")
                } else {
                    dec(row, "einsp_tariff")
                };
            }
            let losses = metering::network_losses(einspeisung, entnahme);
            Json(serde_json::json!({
                "from": from.format(&Rfc3339).unwrap_or_default(),
                "to": to.format(&Rfc3339).unwrap_or_default(),
                "einspeisung_kwh": losses.einspeisung_kwh,
                "entnahme_kwh": losses.entnahme_kwh,
                "verlust_kwh": losses.verlust_kwh,
                "verlust_prozent": losses.verlust_prozent,
                "legal_basis": "§22 Abs. 1 EnWG (Verlustenergie)",
                "hinweis": "Indikative Kennzahl — Genauigkeit hängt von der \
                            Messabdeckung ab; keine abrechnungsfähige Größe.",
            }))
            .into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "edmd: netzverlust query failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

//! Annual forecast (§ 60 Abs. 2 MsbG Jahresprognose) and §22 EnWG Netzverlust.

#[allow(unused_imports)]
use super::*;

// ── Annual forecast ─────────────────────────────────────────────────────

/// `GET /api/v1/forecast/{malo_id}?from=&to=`
///
/// Computes an annual energy consumption forecast from the available meter reads
/// in the given window. Returns the projected annual kWh per § 60 Abs. 2 MsbG.
///
/// This is useful for:
/// - Setting Abschlag (advance payment) amounts
/// - Anticipating Mehr-/Mindermengensaldo at year-end
/// - Informing Jahresprognose in MSCONS
pub(crate) async fn get_annual_forecast(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<SimpleTimeParams>,
) -> impl IntoResponse {
    use metering::project_annual_consumption;
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

    let intervals: Vec<metering::MeterInterval> = reads
        .iter()
        .map(|r| metering::MeterInterval {
            from: r.dtm_from,
            to: r.dtm_to,
            value_kwh: r.quantity_kwh,
            quality: r.quality,
            obis_code: r.obis_code.as_deref().and_then(|s| s.parse().ok()),
        })
        .collect();

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
        Ok(prior) => prior
            .iter()
            .map(|r| metering::MeterInterval {
                from: r.dtm_from,
                to: r.dtm_to,
                value_kwh: r.quantity_kwh,
                quality: r.quality,
                obis_code: r.obis_code.as_deref().and_then(|s| s.parse().ok()),
            })
            .collect(),
        Err(e) => {
            tracing::debug!(error = %e, malo_id, "edmd: no prior-year window for seasonal correction");
            Vec::new()
        }
    };
    let prior = (!prior_intervals.is_empty()).then_some(prior_intervals.as_slice());

    match project_annual_consumption(&malo_id, &intervals, prior) {
        Some(forecast) => Json(serde_json::json!({
            "malo_id": forecast.malo_id,
            "observation_from": forecast.observation_from,
            "observation_to": forecast.observation_to,
            "observed_kwh": forecast.observed_kwh,
            "observed_days": forecast.observed_days,
            "projected_annual_kwh": forecast.projected_annual_kwh,
            "seasonal_correction_applied": forecast.seasonal_correction_applied,
            "seasonal_factor": forecast.seasonal_factor,
            "method": format!("{:?}", forecast.method),
            "legal_basis": "§ 60 Abs. 2 MsbG Jahresprognose",
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

    // Netzverlust is a tenant-wide aggregate across every Strom MaLo, so it is a
    // cross-MaLo query over meterstore's version-resolved relation rather than a
    // per-MaLo `repo.query`. The value column is `value` and the interval bounds
    // are `from`/`to` (reserved words, hence quoted); the tenant and time bounds
    // travel as bound parameters so no value reaches the SQL text.
    //
    // OBIS measurement group C decides the direction: C=1 forward active energy
    // (offtake from grid), C=2 reverse active energy (infeed). meterstore stores
    // the full `obis_code` (`medium-channel:C.D.E`), so match on `:1.`/`:2.`.
    let store = state.repo.store();
    let sql = format!(
        r#"SELECT
              COALESCE(SUM(CASE WHEN "obis_code" LIKE '%:2.%' THEN "value" ELSE 0 END), 0)
                  AS einspeisung_kwh,
              COALESCE(SUM(CASE WHEN "obis_code" LIKE '%:1.%' THEN "value" ELSE 0 END), 0)
                  AS entnahme_kwh
           FROM "{table}"
          WHERE "tenant" = $1
            AND "from" >= $2 AND "to" <= $3
            AND "sparte" = 'STROM'
            AND "quality" NOT IN ('FAULTY', 'UNKNOWN')"#,
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
            // The aggregate is exactly one row. Decimal sums render as JSON number
            // literals; parse them back through their text to keep full precision
            // for this indicative figure.
            let dec = |key: &str| -> rust_decimal::Decimal {
                rows.first()
                    .and_then(|row| row.get(key))
                    .map(std::string::ToString::to_string)
                    .and_then(|s| s.trim_matches('"').parse().ok())
                    .unwrap_or_default()
            };
            let einspeisung = dec("einspeisung_kwh");
            let entnahme = dec("entnahme_kwh");
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

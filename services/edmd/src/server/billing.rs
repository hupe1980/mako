//! Deliveries, imbalance and billing-period read endpoints.

#[allow(unused_imports)]
use super::*;

/// Mehr-/Mindermengen are a GPKE / GaBi Gas process, not a StromNZV one.
///
/// § 13 Abs. 3 StromNZV and § 25 GasNZV were repealed with effect from the end
/// of 31 December 2025; the rules live in the Festlegungen themselves.
const IMBALANCE_LEGAL_BASIS: &str =
    "GPKE (BK6-24-174) Teil 1 Kap. 8.4 (Strom) · GaBi Gas 2.1 (BK7-24-01-008) Ziff. 3a (Gas)";

/// The commodity a settlement query balances on, from `?sparte=`.
///
/// Defaults to Strom, because that is the calendar-day case and the historic
/// behaviour of these endpoints. `sparte=gas` selects the 06:00–06:00 Gastag —
/// the boundary GaBi Gas settles on, and the one an aggregate must use or it
/// carries six hours of the neighbouring day.
fn sparte_of(raw: Option<&str>) -> crate::domain::Sparte {
    raw.and_then(crate::domain::parse_sparte)
        .unwrap_or(crate::domain::Sparte::Strom)
}

#[derive(Debug, Deserialize)]
pub(crate) struct DeliveryQueryParams {
    from: Option<String>,
    to: Option<String>,
}

pub(crate) async fn get_deliveries(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<DeliveryQueryParams>,
) -> impl IntoResponse {
    // Cedar check — resource tenant is the service-level tenant injected at startup.
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

    match state.repo.query(&q).await {
        Ok(reads) => {
            let energiemengen: Vec<Energiemenge> = reads.iter().map(read_to_energiemenge).collect();
            Json(energiemengen).into_response()
        }
        Err(err) => {
            tracing::warn!(%err, malo_id, "edmd: get_deliveries failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `GET /api/v1/imbalance/{malo_id}/{year}/{month}` query parameters.
#[derive(Debug, Deserialize)]
pub(crate) struct ImbalanceParams {
    /// `strom` (default) · `gas` — the saldo aggregates over the commodity's
    /// balancing day, and for Gas that is the 06:00 Gastag.
    sparte: Option<String>,
    /// The **bilanzierte** (profile-allocated) quantity for the period, in kWh.
    ///
    /// Required. edmd measures; it does not balance. The bilanzierte Menge is
    /// what the Bilanzkreis was charged from the load profile — a commercial
    /// figure held by the supplier or read from the MaBiS/allocation side — and
    /// no amount of metering data yields it. Without it there is no comparison
    /// to make, so the endpoint refuses rather than returning the measured total
    /// twice and calling the difference zero.
    bilanziert_kwh: Option<Decimal>,
}

pub(crate) async fn get_imbalance(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path((malo_id, year, month)): Path<(String, i32, u8)>,
    Query(params): Query<ImbalanceParams>,
) -> impl IntoResponse {
    use time::{Date, Month};

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-imbalance", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let month_enum = match Month::try_from(month) {
        Ok(m) => m,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid month").into_response(),
    };

    let from = match Date::from_calendar_date(year, month_enum, 1) {
        Ok(d) => d,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid date").into_response(),
    };
    // The last day of the month, from `Month::length`, which already knows about
    // leap years. Rolling forward to the first of the next month and stepping
    // back needed a December special case and a `Month::try_from(month + 1)` that
    // only avoided panicking because of it.
    let to = match Date::from_calendar_date(year, month_enum, month_enum.length(year)) {
        Ok(d) => d,
        Err(_) => return (StatusCode::BAD_REQUEST, "date calculation failed").into_response(),
    };

    let Some(bilanziert_kwh) = params.bilanziert_kwh else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "bilanziert_kwh is required",
                "hinweis": "Der Mehr-/Mindermengensaldo vergleicht die gemessene mit der \
                            bilanzierten Menge. edmd hält nur die gemessene Hälfte; die \
                            bilanzierte Menge stammt aus der Bilanzkreisabrechnung und ist \
                            als Parameter zu übergeben.",
                "legal_basis": IMBALANCE_LEGAL_BASIS,
            })),
        )
            .into_response();
    };

    match state
        .repo
        .imbalance(
            &malo_id,
            from,
            to,
            &state.tenant,
            sparte_of(params.sparte.as_deref()),
            bilanziert_kwh,
        )
        .await
    {
        Ok(report) => {
            let mut body = serde_json::to_value(&report).unwrap_or_default();
            if let Some(obj) = body.as_object_mut() {
                obj.insert(
                    "legal_basis".to_owned(),
                    serde_json::json!(IMBALANCE_LEGAL_BASIS),
                );
                // Which side owes whom, spelled out: the naming is from the
                // network operator's side and inverts the intuitive reading.
                obj.insert(
                    "richtung".to_owned(),
                    serde_json::json!(if report.mehrmenge_kwh > Decimal::ZERO {
                        "MEHRMENGE — Netzbetreiber vergütet dem Lieferanten"
                    } else if report.mindermenge_kwh > Decimal::ZERO {
                        "MINDERMENGE — Netzbetreiber stellt dem Lieferanten in Rechnung"
                    } else {
                        "AUSGEGLICHEN"
                    }),
                );
            }
            Json(body).into_response()
        }
        Err(crate::domain::error::EdmError::NoData { .. }) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "no billable reading for this MaLo in the period",
                "hinweis": "Ohne gemessene Menge ist der Saldo unbekannt, nicht null.",
            })),
        )
            .into_response(),
        Err(crate::domain::error::EdmError::InvalidMaloId { malo_id, reason }) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("{malo_id}: {reason}") })),
        )
            .into_response(),
        Err(err) => {
            tracing::warn!(%err, malo_id, "edmd: get_imbalance failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `GET /api/v1/billing-period/{malo_id}?from=YYYY-MM-DD&to=YYYY-MM-DD`
///
/// Returns the aggregated billing-period summary for a MaLo.
///
/// Consumed by `invoicd` for RLM plausibility and by `netzbilanzd` for
/// NNE invoice generation (N4).  Includes:
/// - `arbeitsmenge_kwh` — total energy quantity
/// - `spitzenleistung_kw` — peak demand (RLM Strom only)
/// - `brennwert_kwh_per_m3` / `zustandszahl` — Gas conversion factors
///
/// Source: GPKE (BK6-24-174) Teil 1; GeLi Gas 3.0 (BK7-24-01-009).
#[derive(Debug, Deserialize)]
pub(crate) struct BillingPeriodParams {
    /// ISO 8601 date `YYYY-MM-DD` — start of billing period (inclusive).
    from: Option<String>,
    /// ISO 8601 date `YYYY-MM-DD` — end of billing period (inclusive).
    to: Option<String>,
    /// `strom` (default) · `gas` · `wasser` · `waerme` — decides whether the
    /// period runs on calendar days or on the Gastag.
    sparte: Option<String>,
}

pub(crate) async fn get_billing_period(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<BillingPeriodParams>,
) -> impl IntoResponse {
    use time::macros::format_description;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-billing-period", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let fmt = format_description!("[year]-[month]-[day]");

    let period_from = match params.from.as_deref() {
        Some(s) => match time::Date::parse(s, &fmt) {
            Ok(d) => d,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": "invalid 'from' date — use YYYY-MM-DD" })),
                )
                    .into_response();
            }
        },
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "'from' query parameter is required" })),
            )
                .into_response();
        }
    };

    let period_to = match params.to.as_deref() {
        Some(s) => match time::Date::parse(s, &fmt) {
            Ok(d) => d,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": "invalid 'to' date — use YYYY-MM-DD" })),
                )
                    .into_response();
            }
        },
        None => period_from, // Default: single-day period
    };

    if period_to < period_from {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "'to' must be >= 'from'" })),
        )
            .into_response();
    }

    let q = BillingPeriodQuery {
        malo_id: malo_id.clone(),
        period_from,
        period_to,
        tenant: state.tenant.clone(),
        sparte: sparte_of(params.sparte.as_deref()),
    };

    match state.repo.billing_period(&q).await {
        Ok(Some(period)) => Json(serde_json::to_value(period).unwrap_or_default()).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "no meter reads found for this MaLo / period" })),
        )
            .into_response(),
        Err(err) => {
            tracing::warn!(%err, %malo_id, "edmd: get_billing_period failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ── Billing periods collection (mabis-syncd MaLo discovery) ──────────────────

/// `GET /api/v1/billing-periods?from=YYYY-MM-DD&to=YYYY-MM-DD&tenant=...`
///
/// Every MaLo that **has meter data** in the requested window, with its
/// commodity and the observed extent of that data.
///
/// Used by `mabis-syncd` to discover which MaLo IDs to submit Summenzeitreihen
/// for. That is a question about readings, so it is answered from the readings:
/// a cross-MaLo `SELECT DISTINCT` over meterstore's version-resolved relation,
/// evaluated across both tiers in one plan.
///
/// It used to scan `meter_billing_periods` instead — the **cache**, which
/// `billing_period()` fills lazily on read-through. A MaLo whose aggregate had
/// never been requested had no row there, so it was invisible to discovery and
/// its Summenzeitreihe was never submitted: a MaBiS gap that grew quietly and
/// that no error surfaced, because an empty list is a valid answer.
///
/// This is the collection form; `GET /api/v1/billing-period/{malo_id}` returns
/// the aggregate for a single MaLo.
#[derive(serde::Deserialize)]
pub(crate) struct BillingPeriodsParams {
    /// Period start date inclusive (YYYY-MM-DD). Defaults to the start of the
    /// previous month — the window MaBiS actually asks about.
    from: Option<String>,
    /// Period end date inclusive (YYYY-MM-DD). Defaults to today.
    to: Option<String>,
    /// Optional tenant filter. It must equal the authorised tenant; it exists so
    /// a caller can assert which tenant it believes it is talking to.
    tenant: Option<String>,
    /// Max MaLos to return (default 1000, hard cap 5000). The collection is a
    /// discovery surface, so it is always bounded.
    limit: Option<i64>,
}

pub(crate) async fn list_billing_periods(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Query(params): Query<BillingPeriodsParams>,
) -> impl IntoResponse {
    use time::macros::format_description;
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-billing-period", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // Cedar authorised the caller against `resource_tenant`, so the query must
    // run against that same tenant. Binding a caller-supplied `?tenant=` would
    // let a principal cleared for its own tenant read any other tenant's
    // portfolio, since the parameter is never re-authorised.
    if let Some(requested) = params.tenant.as_deref()
        && requested != resource_tenant
    {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "tenant parameter does not match the authorised tenant",
            })),
        )
            .into_response();
    }

    let fmt = format_description!("[year]-[month]-[day]");
    let today = mako_fristen::heute();
    let parse_date = |raw: Option<&str>, fallback: time::Date| match raw {
        Some(s) => time::Date::parse(s, fmt)
            .map_err(|_| s.to_owned())
            .map(Some),
        None => Ok(Some(fallback)),
    };
    // A malformed date is a 400, not a silent `Date::MIN`/`Date::MAX` that
    // quietly widens the scan to all of history.
    let (from_date, to_date) = match (
        parse_date(params.from.as_deref(), previous_month_start(today)),
        parse_date(params.to.as_deref(), today),
    ) {
        (Ok(Some(f)), Ok(Some(t))) if f <= t => (f, t),
        (Err(bad), _) | (_, Err(bad)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("invalid date {bad:?}; expected YYYY-MM-DD")
                })),
            )
                .into_response();
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "`to` must be on or after `from`" })),
            )
                .into_response();
        }
    };

    let limit = params.limit.unwrap_or(1000).clamp(1, 5000);
    let from_ts = metering::calendar::day_start_utc(from_date);
    let to_ts = metering::calendar::day_end_utc(to_date);

    // Cross-MaLo discovery over the version-resolved relation. `from` is a SQL
    // reserved word, hence quoted; the bounds and the tenant travel as bound
    // parameters so no value reaches the SQL text, and `limit` is clamped above
    // before it is rendered.
    let store = state.repo.store();
    let sql = format!(
        r#"SELECT "malo_id",
                  MIN("from")  AS first_interval,
                  MAX("to")    AS last_interval,
                  MIN("sparte") AS sparte,
                  COUNT(*)     AS interval_count
             FROM "{table}"
            WHERE "tenant" = $1 AND "from" >= $2 AND "from" < $3
            GROUP BY "malo_id"
            ORDER BY "malo_id"
            LIMIT {limit}"#,
        table = store.resolved_table(),
    );

    match store
        .query_with_params(
            &sql,
            vec![
                datafusion::scalar::ScalarValue::Utf8(Some(resource_tenant.to_owned())),
                ts_param(from_ts),
                ts_param(to_ts),
            ],
        )
        .await
        .and_then(|r| r.to_json())
    {
        Ok(rows) => {
            let truncated = i64::try_from(rows.len()).unwrap_or(i64::MAX) >= limit;
            Json(serde_json::json!({
                "billing_periods": rows,
                "count":           rows.len(),
                "from":            from_date.to_string(),
                "to":              to_date.to_string(),
                // A silently capped discovery list reads as "that is all of
                // them", which is exactly the MaBiS gap this endpoint exists to
                // prevent.
                "truncated":       truncated,
                "limit":           limit,
            }))
            .into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "edmd: list_billing_periods failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// The first day of the month before the one containing `day`.
fn previous_month_start(day: time::Date) -> time::Date {
    let first = day.replace_day(1).unwrap_or(day);
    first.previous_day().map_or(first, |last_of_prev| {
        last_of_prev.replace_day(1).unwrap_or(last_of_prev)
    })
}

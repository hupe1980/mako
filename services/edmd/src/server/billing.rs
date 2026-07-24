//! Deliveries, imbalance and billing-period read endpoints.

#[allow(unused_imports)]
use super::*;

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
    use time::format_description::well_known::Rfc3339;

    // Cedar check — resource tenant is the service-level tenant injected at startup.
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

pub(crate) async fn get_imbalance(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path((malo_id, year, month)): Path<(String, i32, u8)>,
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
    let to = match from.replace_month(month_enum).and_then(|d| {
        // Last day of month.
        let next_month = if month == 12 {
            Date::from_calendar_date(year + 1, Month::January, 1)
        } else {
            Date::from_calendar_date(year, Month::try_from(month + 1).unwrap(), 1)
        };
        next_month.map(|nm| nm.previous_day().unwrap_or(d))
    }) {
        Ok(d) => d,
        Err(_) => return (StatusCode::BAD_REQUEST, "date calculation failed").into_response(),
    };

    match state
        .repo
        .imbalance(&malo_id, from, to, &state.tenant)
        .await
    {
        Ok(report) => Json(serde_json::to_value(report).unwrap_or_default()).into_response(),
        Err(mako_edm::error::EdmError::NoData { .. }) => {
            (StatusCode::NOT_FOUND, "no data for this MaLo / period").into_response()
        }
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
/// Source: GPKE BK6-22-024 §3; GeLi Gas 3.0 (BK7-24-01-009) §3.
#[derive(Debug, Deserialize)]
pub(crate) struct BillingPeriodParams {
    /// ISO 8601 date `YYYY-MM-DD` — start of billing period (inclusive).
    from: Option<String>,
    /// ISO 8601 date `YYYY-MM-DD` — end of billing period (inclusive).
    to: Option<String>,
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
/// Returns a list of `(malo_id, messtyp, period_from, period_to)` for all
/// MaLos that have billing period aggregates in the requested date window.
///
/// Used by `mabis-syncd` to discover which MaLo IDs have meter data in a given
/// month so it can submit Summenzeitreihen to BIKO (BK6-22-024 Anlage 3).
///
/// This is the collection form; `GET /api/v1/billing-period/{malo_id}` returns a
/// single MaLo.
#[derive(serde::Deserialize)]
pub(crate) struct BillingPeriodsParams {
    /// Period start date inclusive (YYYY-MM-DD).
    from: Option<String>,
    /// Period end date inclusive (YYYY-MM-DD).
    to: Option<String>,
    /// Optional tenant filter — overrides the instance tenant when set.
    tenant: Option<String>,
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
    let tenant = resource_tenant.to_owned();

    let fmt = format_description!("[year]-[month]-[day]");
    let from_date = params
        .from
        .as_deref()
        .and_then(|s| time::Date::parse(s, fmt).ok())
        .unwrap_or(time::Date::MIN);
    let to_date = params
        .to
        .as_deref()
        .and_then(|s| time::Date::parse(s, fmt).ok())
        .unwrap_or(time::Date::MAX);

    let pool = state.repo.pool();
    let rows = sqlx::query(
        r"SELECT malo_id, messtyp, sparte, period_from, period_to
          FROM meter_billing_periods
          WHERE period_from >= $1
            AND period_to   <= $2
            AND tenant       = $3
          ORDER BY malo_id, period_from",
    )
    .bind(from_date)
    .bind(to_date)
    .bind(&tenant)
    .fetch_all(pool)
    .await;

    match rows {
        Ok(rows) => {
            let items: Vec<serde_json::Value> = rows
                .iter()
                .map(|r| {
                    use sqlx::Row as _;
                    let period_from: time::Date =
                        r.try_get("period_from").unwrap_or(time::Date::MIN);
                    let period_to: time::Date = r.try_get("period_to").unwrap_or(time::Date::MIN);
                    serde_json::json!({
                        "malo_id":     r.try_get::<String, _>("malo_id").unwrap_or_default(),
                        "messtyp":     r.try_get::<String, _>("messtyp").unwrap_or_default(),
                        "sparte":      r.try_get::<String, _>("sparte").unwrap_or_default(),
                        "period_from": period_from.to_string(),
                        "period_to":   period_to.to_string(),
                    })
                })
                .collect();
            Json(serde_json::json!({ "billing_periods": items, "count": items.len() }))
                .into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "edmd: list_billing_periods failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

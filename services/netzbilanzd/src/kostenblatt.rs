//! Redispatch 2.0 cost sheets and §13a compensation.
//!
//! Two different documents that share one input — the energy an activation
//! actually moved:
//!
//! - the **Kostenblatt** (BK6-20-061 §4.2), which the VNB submits to the ÜNB by
//!   the 15th of the following month; and
//! - the **angemessene Vergütung** under §13a Abs. 2 EnWG, which the NB owes the
//!   operator of the resource it curtailed.
//!
//! Both resolve their quantity from the same `edmd` Lastgang window, so the
//! resolution lives here once rather than being reached through a fabricated
//! request of the other kind.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
};
use mako_service::{ApiError, ApiResult};
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::PgPool;
use time::format_description::well_known::Rfc3339;

use crate::config::NetzbilanzConfig;
use crate::pg::{self, UpsertKostenblattRequest};

type Cfg = Extension<Arc<NetzbilanzConfig>>;

// ── Energy resolution ─────────────────────────────────────────────────────────

/// An activation window, as every endpoint here needs it.
#[derive(Debug, Clone, Copy)]
pub struct Window {
    /// Inclusive start.
    pub start: time::OffsetDateTime,
    /// Exclusive end.
    pub end: time::OffsetDateTime,
}

impl Window {
    /// Parse and order an RFC 3339 window.
    ///
    /// # Errors
    ///
    /// `400` when either bound is unparseable or the window is not positive.
    pub fn parse(start: &str, end: &str) -> ApiResult<Self> {
        let start = time::OffsetDateTime::parse(start, &Rfc3339).map_err(|_| {
            ApiError::bad_request(format!(
                "activation_start_utc {start:?} is not RFC 3339 (e.g. 2026-01-15T10:00:00Z)"
            ))
        })?;
        let end = time::OffsetDateTime::parse(end, &Rfc3339).map_err(|_| {
            ApiError::bad_request(format!(
                "activation_end_utc {end:?} is not RFC 3339 (e.g. 2026-01-15T10:15:00Z)"
            ))
        })?;
        if end <= start {
            return Err(ApiError::bad_request(
                "activation_end_utc must be strictly after activation_start_utc",
            ));
        }
        Ok(Self { start, end })
    }
}

/// Sum the `edmd` Lastgang over an activation window.
///
/// Returns `None` when `edmd` has no data for the window — 404, a non-2xx, an
/// unparseable body, or a sum of zero. The caller decides what to do about it;
/// for a Kostenblatt that means the billing-period fallback, and for a §13a
/// Vergütung it means refusing rather than guessing.
pub async fn lastgang_sum(
    client: &reqwest::Client,
    cfg: &NetzbilanzConfig,
    malo_id: &str,
    window: Window,
) -> Option<Decimal> {
    let edmd = cfg.edmd_url.as_deref()?.trim_end_matches('/');
    let (from, to) = (
        window.start.format(&Rfc3339).ok()?,
        window.end.format(&Rfc3339).ok()?,
    );
    let mut req = client.get(format!(
        "{edmd}/api/v1/lastgang/{malo_id}?from={from}&to={to}"
    ));
    if let Some(key) = cfg.edmd_api_key.as_deref() {
        req = req.bearer_auth(key);
    }

    let lastgaenge: Vec<rubo4e::current::Lastgang> = match req.send().await {
        Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND => return None,
        Ok(r) if !r.status().is_success() => {
            tracing::debug!(
                malo_id,
                status = r.status().as_u16(),
                "netzbilanzd: edmd Lastgang non-2xx"
            );
            return None;
        }
        Ok(r) => match r.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, malo_id, "netzbilanzd: edmd Lastgang body is not a BO4E Lastgang list");
                return None;
            }
        },
        Err(e) => {
            tracing::warn!(%e, malo_id, "netzbilanzd: edmd Lastgang fetch failed");
            return None;
        }
    };
    sum_lastgang_kwh(&lastgaenge, window)
}

/// Sum interval values across the OBIS groups of a Lastgang response.
///
/// Values are filtered to `[start, end)`. A value whose interval start cannot be
/// resolved is kept: `edmd` already filtered server-side, and dropping it would
/// under-report the activation.
#[must_use]
pub fn sum_lastgang_kwh(
    lastgaenge: &[rubo4e::current::Lastgang],
    window: Window,
) -> Option<Decimal> {
    let total: Decimal = lastgaenge
        .iter()
        .flat_map(|lg| lg.werte.iter().flatten())
        .filter(|zw| {
            zw.zeitraum
                .as_ref()
                .and_then(zeitraum_start_utc)
                .is_none_or(|t| t >= window.start && t < window.end)
        })
        .filter_map(|zw| zw.wert)
        .sum();
    (total > Decimal::ZERO).then_some(total)
}

/// Resolve a BO4E `Zeitraum` interval start to a UTC instant.
///
/// `startuhrzeit` carries its UTC offset (`10:00:00+00:00`, `11:00:00+01:00`),
/// and the offset is honoured rather than assumed away. Truncating to `hh:mm:ss`
/// and calling it UTC put a German-local Lastgang an hour off: on a 15-minute
/// Redispatch activation that selects the wrong quarter-hours entirely, so the
/// Einsatzkosten submitted to the ÜNB price energy from a window the resource
/// was never curtailed in.
///
/// A bare `hh:mm:ss` with no offset is read as UTC — that is what BO4E's own
/// examples show, and the only reading available.
fn zeitraum_start_utc(z: &rubo4e::current::Zeitraum) -> Option<time::OffsetDateTime> {
    let date = z.startdatum?;
    let raw = z.startuhrzeit.as_deref()?;
    let hms = raw.get(..8)?;
    let clock = time::Time::parse(
        hms,
        time::macros::format_description!("[hour]:[minute]:[second]"),
    )
    .ok()?;
    let offset = parse_utc_offset(&raw[8..]);
    Some(
        date.with_time(clock)
            .assume_offset(offset)
            .to_offset(time::UtcOffset::UTC),
    )
}

/// The UTC offset trailing a BO4E time — `Z`, `+01:00`, `-0330`, or absent.
fn parse_utc_offset(suffix: &str) -> time::UtcOffset {
    let suffix = suffix.trim();
    if suffix.is_empty() || suffix.eq_ignore_ascii_case("z") {
        return time::UtcOffset::UTC;
    }
    let sign = match suffix.as_bytes().first() {
        Some(b'+') => 1_i8,
        Some(b'-') => -1_i8,
        _ => return time::UtcOffset::UTC,
    };
    let digits: String = suffix[1..].chars().filter(char::is_ascii_digit).collect();
    let (Some(h), Some(m)) = (
        digits.get(..2).and_then(|h| h.parse::<i8>().ok()),
        digits
            .get(2..4)
            .and_then(|m| m.parse::<i8>().ok())
            .or(Some(0)),
    ) else {
        return time::UtcOffset::UTC;
    };
    time::UtcOffset::from_hms(sign * h, sign * m, 0).unwrap_or(time::UtcOffset::UTC)
}

/// Parse a JSON value as a `Decimal`, whether it arrived as a string or a number.
fn decimal_from_json(v: &serde_json::Value) -> Option<Decimal> {
    match v {
        serde_json::Value::String(s) => s.parse().ok(),
        serde_json::Value::Number(n) => n.to_string().parse().ok(),
        _ => None,
    }
}

// ── Kostenblatt REST ──────────────────────────────────────────────────────────

/// `PUT /api/v1/redispatch/kostenblatt/{activation_id}`
///
/// # Errors
///
/// `422` when `kosten_json` is not a `rubo4e::current::Kosten`.
pub async fn put_kostenblatt(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Cfg,
    Path(activation_id): Path<String>,
    Json(req): Json<UpsertKostenblattRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    if let Some(kosten) = &req.kosten_json {
        serde_json::from_value::<rubo4e::current::Kosten>(kosten.clone())
            .map_err(|e| ApiError::unprocessable(format!("invalid Kosten payload: {e}")))?;
    }
    let id = pg::upsert_kostenblatt(&pool, &cfg.tenant, &activation_id, &req)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(serde_json::json!({
        "id": id,
        "activation_id": activation_id,
        "tr_id": req.tr_id,
        "einsatzkosten_eur": (req.dispatch_kwh * req.arbeitspreis_eur_per_kwh).to_string(),
    })))
}

/// `GET /api/v1/redispatch/kostenblatt/{activation_id}`
///
/// Returns every TechnischeRessource dispatched under the activation. One
/// activation routinely curtails several, and returning an arbitrary one of them
/// hides the rest of the month's costs.
///
/// # Errors
///
/// `404` when the activation has no records for this tenant.
pub async fn get_kostenblatt(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Cfg,
    Path(activation_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let rows = pg::fetch_kostenblatt(&pool, &cfg.tenant, &activation_id)
        .await
        .map_err(ApiError::Internal)?;
    if rows.is_empty() {
        return Err(ApiError::NotFound);
    }
    let total: Decimal = rows.iter().filter_map(|r| r.einsatzkosten_eur).sum();
    Ok(Json(serde_json::json!({
        "activation_id": activation_id,
        "count": rows.len(),
        "total_einsatzkosten_eur": total.to_string(),
        "records": rows,
    })))
}

/// Query string for the monthly Kostenblatt listing.
#[derive(Debug, Deserialize)]
pub struct KostenblattListQuery {
    /// Calendar year.
    pub year: i16,
    /// Calendar month.
    pub month: i16,
    /// Submission status.
    pub status: Option<String>,
}

/// `GET /api/v1/redispatch/kostenblatt?year=&month=&status=`
///
/// # Errors
///
/// `400` for a month outside 1–12.
pub async fn list_kostenblatt(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Cfg,
    Query(q): Query<KostenblattListQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    check_month(q.month)?;
    let rows = pg::list_kostenblatt(&pool, &cfg.tenant, q.year, q.month, q.status.as_deref())
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(
        serde_json::json!({ "count": rows.len(), "records": rows }),
    ))
}

/// `GET /api/v1/redispatch/kostenblatt/gaps/{year}/{month}`
///
/// Activations registered but never quantified — the list to work through
/// before the 15th.
///
/// # Errors
///
/// `400` for a month outside 1–12.
pub async fn get_kostenblatt_gaps(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Cfg,
    Path((year, month)): Path<(i16, i16)>,
) -> ApiResult<Json<serde_json::Value>> {
    check_month(month)?;
    let rows = pg::list_kostenblatt_gaps(&pool, &cfg.tenant, year, month)
        .await
        .map_err(ApiError::Internal)?;
    let (deadline_year, deadline_month) = next_month(year, month);
    Ok(Json(serde_json::json!({
        "year": year,
        "month": month,
        "gaps": rows.len(),
        "deadline": format!("{deadline_year}-{deadline_month:02}-15"),
        "action": "POST /api/v1/redispatch/kostenblatt/{activation_id}/compute for each record",
        "records": rows,
    })))
}

/// `POST /api/v1/redispatch/kostenblatt/submit/{year}/{month}`
///
/// Marks the month's pending records submitted, in a single statement, and
/// returns what actually moved.
///
/// # Errors
///
/// `400` for a month outside 1–12.
pub async fn post_submit_kostenblatt(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Cfg,
    Path((year, month)): Path<(i16, i16)>,
) -> ApiResult<Json<serde_json::Value>> {
    check_month(month)?;
    let dispatch_ref = format!("KB-{year}-{month:02}");
    let submitted = pg::submit_pending_kostenblatt(&pool, &cfg.tenant, year, month, &dispatch_ref)
        .await
        .map_err(ApiError::Internal)?;
    let total: Decimal = submitted.iter().filter_map(|r| r.einsatzkosten_eur).sum();
    Ok(Json(serde_json::json!({
        "period": format!("{year}-{month:02}"),
        "submitted": submitted.len(),
        "total_einsatzkosten_eur": total.to_string(),
        "dispatch_ref": dispatch_ref,
        "records": submitted,
    })))
}

fn check_month(month: i16) -> ApiResult<()> {
    if (1..=12).contains(&month) {
        Ok(())
    } else {
        Err(ApiError::bad_request("month must be 1–12"))
    }
}

const fn next_month(year: i16, month: i16) -> (i16, i16) {
    if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    }
}

// ── Kostenblatt auto-compute ──────────────────────────────────────────────────

/// Request body for the Kostenblatt compute endpoint.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputeRequest {
    /// TechnischeRessource dispatched.
    pub tr_id: String,
    /// Grid connection point.
    pub malo_id: String,
    /// Calendar year of the activation.
    pub period_year: i16,
    /// Calendar month of the activation.
    pub period_month: i16,
    /// ÜNB receiving the Kostenblatt.
    pub uenb_mp_id: String,
    /// VNB sending it.
    pub vnb_mp_id: String,
    /// Activation window start, RFC 3339.
    pub activation_start_utc: String,
    /// Activation window end, RFC 3339.
    pub activation_end_utc: String,
    /// Contract rate from the Redispatch agreement, in EUR/kWh.
    pub arbeitspreis_eur_per_kwh: Decimal,
    /// Skip `edmd` and use a verified operator figure instead.
    #[serde(default)]
    pub dispatch_kwh_override: Option<Decimal>,
}

/// `POST /api/v1/redispatch/kostenblatt/{activation_id}/compute`
///
/// Resolves the dispatched energy, computes `Einsatzkosten = kWh × rate`, builds
/// the typed BO4E `Kosten` payload for CIM export, and stores the record.
///
/// Energy is resolved in the order BK6-20-061 §4.2 implies: an operator override
/// first, then the Lastgang summed over the exact activation window, and only
/// then the monthly billing-period aggregate. A 15-minute activation settled
/// against a monthly aggregate is wrong by three orders of magnitude, so the
/// fallback is logged loudly and recorded in `dispatch_source`.
///
/// # Errors
///
/// - `400` on an unparseable or inverted window.
/// - `404` when no energy data exists and no override was supplied.
/// - `422` when the resolved quantity is not positive, or `edmd` is not
///   configured and no override was supplied.
pub async fn post_compute(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Cfg,
    Extension(http): Extension<Arc<reqwest::Client>>,
    Path(activation_id): Path<String>,
    Json(req): Json<ComputeRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    check_month(req.period_month)?;
    let window = Window::parse(&req.activation_start_utc, &req.activation_end_utc)?;

    let (dispatch_kwh, source) = match req.dispatch_kwh_override {
        Some(kwh) => (kwh, "manual_override"),
        None => {
            if cfg.edmd_url.is_none() {
                return Err(ApiError::Unprocessable(
                    "edmd_url is not configured — supply dispatch_kwh_override".to_owned(),
                ));
            }
            match lastgang_sum(&http, &cfg, &req.malo_id, window).await {
                Some(kwh) => (kwh, "lastgang_sum"),
                None => match billing_period_fallback(&http, &cfg, &req.malo_id, window).await {
                    Some(kwh) => {
                        tracing::warn!(
                            malo_id = %req.malo_id,
                            window = %req.activation_start_utc,
                            "netzbilanzd: no Lastgang for the activation window — falling back to \
                             the monthly billing-period aggregate, which is not window-specific"
                        );
                        (kwh, "billing_period")
                    }
                    None => {
                        return Err(ApiError::NotFound);
                    }
                },
            }
        }
    };

    if dispatch_kwh <= Decimal::ZERO {
        return Err(ApiError::unprocessable(format!(
            "resolved dispatch_kwh is {dispatch_kwh} for MaLo {} — check the edmd Lastgang \
             or supply dispatch_kwh_override",
            req.malo_id
        )));
    }

    let einsatzkosten = dispatch_kwh * req.arbeitspreis_eur_per_kwh;
    let kosten_json = serde_json::json!({
        "_typ": "KOSTEN",
        "kostenbloecke": [{
            "_typ": "KOSTENBLOCK",
            "kostenblockbezeichnung": "Redispatch 2.0 Einsatzkosten",
            "kostenpositionen": [{
                "_typ": "KOSTENPOSITION",
                "positionstitel": "Arbeitspreis Redispatch",
                "artikeldetail": req.tr_id,
                "menge": { "_typ": "MENGE", "wert": dispatch_kwh.to_string(), "einheit": "KWH" },
                "einzelpreis": { "_typ": "PREIS", "wert": req.arbeitspreis_eur_per_kwh.to_string(), "einheit": "EUR" },
                "betragKostenposition": { "_typ": "BETRAG", "wert": einsatzkosten.to_string(), "waehrung": "EUR" },
                "von": req.activation_start_utc,
                "bis": req.activation_end_utc,
            }]
        }]
    });
    // Built from a literal, so a `rubo4e` field rename must fail here rather
    // than silently ship a Kosten object the ÜNB's parser drops.
    serde_json::from_value::<rubo4e::current::Kosten>(kosten_json.clone()).map_err(|e| {
        ApiError::Internal(anyhow::Error::new(e).context("generated Kosten payload is not BO4E"))
    })?;

    let upsert = UpsertKostenblattRequest {
        tr_id: req.tr_id.clone(),
        malo_id: Some(req.malo_id.clone()),
        period_year: req.period_year,
        period_month: req.period_month,
        uenb_mp_id: req.uenb_mp_id.clone(),
        vnb_mp_id: req.vnb_mp_id.clone(),
        dispatch_kwh,
        arbeitspreis_eur_per_kwh: req.arbeitspreis_eur_per_kwh,
        kosten_json: Some(kosten_json),
        activation_start_utc: Some(window.start),
        activation_end_utc: Some(window.end),
        dispatch_source: Some(source.to_owned()),
    };

    let mut tx = pool.begin().await.map_err(ApiError::from)?;
    let record_id = pg::upsert_kostenblatt(&mut *tx, &cfg.tenant, &activation_id, &upsert)
        .await
        .map_err(ApiError::Internal)?;

    if cfg.erp_webhook_url.is_some() {
        let ce = mako_service::CloudEvent::new(
            mako_service::source("netzbilanzd", &cfg.tenant),
            mako_events::netzbilanz::KOSTENBLATT_COMPUTED,
            String::new(),
            serde_json::json!({
                "record_id": record_id,
                "activation_id": activation_id,
                "tr_id": req.tr_id,
                "malo_id": req.malo_id,
                "period_year": req.period_year,
                "period_month": req.period_month,
                "dispatch_kwh": dispatch_kwh.to_string(),
                "einsatzkosten_eur": einsatzkosten.to_string(),
                "dispatch_source": source,
            }),
        )
        .without_subject();
        mako_service::outbox::enqueue(&mut tx, &ce)
            .await
            .map_err(ApiError::from)?;
    }
    tx.commit().await.map_err(ApiError::from)?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": record_id,
            "activation_id": activation_id,
            "tr_id": req.tr_id,
            "malo_id": req.malo_id,
            "dispatch_kwh": dispatch_kwh.to_string(),
            "arbeitspreis_eur_per_kwh": req.arbeitspreis_eur_per_kwh.to_string(),
            "einsatzkosten_eur": einsatzkosten.to_string(),
            "dispatch_source": source,
        })),
    ))
}

/// The monthly billing-period aggregate, as a last resort.
async fn billing_period_fallback(
    client: &reqwest::Client,
    cfg: &NetzbilanzConfig,
    malo_id: &str,
    window: Window,
) -> Option<Decimal> {
    let edmd = cfg.edmd_url.as_deref()?.trim_end_matches('/');
    let day = window.start.date();
    let mut req = client.get(format!(
        "{edmd}/api/v1/billing-period/{malo_id}?from={day}&to={day}"
    ));
    if let Some(key) = cfg.edmd_api_key.as_deref() {
        req = req.bearer_auth(key);
    }
    let body: serde_json::Value = req.send().await.ok()?.json().await.ok()?;
    body.get("arbeitsmenge_kwh")
        .and_then(decimal_from_json)
        .filter(|&v| v > Decimal::ZERO)
}

// ── §13a Abs. 2 Vergütung ─────────────────────────────────────────────────────

/// Which redispatch case an activation was.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RedispatchFall {
    /// The EIV steers the resource to a transmitted schedule.
    Aufforderungsfall,
    /// The NB steers the resource over the Steuerkanal.
    Duldungsfall,
}

/// Request body for the §13a Vergütung endpoint.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerguetungRequest {
    /// Grid connection point of the curtailed resource.
    pub malo_id: String,
    /// Activation window start, RFC 3339.
    pub activation_start_utc: String,
    /// Activation window end, RFC 3339.
    pub activation_end_utc: String,
    /// Z01 EEG / Z02 KWKG / Z03 sonstige.
    pub verguetungsart: grid_billing::RedispatchVerguetungsart,
    /// Which case this was — the two use different counterfactuals.
    pub abwicklung: RedispatchFall,
    /// The anzulegender Wert in ct/kWh (§13a Abs. 2 S. 3 Nr. 5 EnWG).
    #[serde(default)]
    pub anzulegender_wert_ct_per_kwh: Option<Decimal>,
    /// Proven lost revenue in EUR (Nr. 3) — required for Z03.
    #[serde(default)]
    pub entgangene_einnahmen_eur_override: Option<Decimal>,
    /// Zusätzliche Aufwendungen in EUR (Nr. 1/2/4).
    #[serde(default)]
    pub zusaetzliche_aufwendungen_eur: Decimal,
    /// Ersparte Aufwendungen in EUR (Satz 4).
    #[serde(default)]
    pub ersparte_aufwendungen_eur: Decimal,
    /// The Ausfallarbeit, when it does not come from the measured Lastgang.
    #[serde(default)]
    pub ausfallarbeit_kwh_override: Option<Decimal>,
}

/// `POST /api/v1/redispatch/verguetung/{activation_id}/compute`
///
/// Computes the angemessene Vergütung for one activation. Nothing is persisted:
/// the figure and its per-component trace go to the operator's payment run.
///
/// # Errors
///
/// - `400` on an unparseable window.
/// - `422` when the case is an Aufforderungsfall with no transmitted schedule,
///   or when the Vergütungsart's revenue basis is missing.
/// - `404` when the Duldungsfall has no Lastgang to settle against.
pub async fn post_verguetung(
    Extension(cfg): Cfg,
    Extension(http): Extension<Arc<reqwest::Client>>,
    Path(activation_id): Path<String>,
    Json(req): Json<VerguetungRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let window = Window::parse(&req.activation_start_utc, &req.activation_end_utc)?;

    // The two cases settle against different counterfactuals. In a
    // Duldungsfall the NB steered the resource, so what it would have produced
    // is derived from the measured Lastgang. In an Aufforderungsfall the EIV
    // steered to a transmitted schedule, and that schedule *is* the
    // counterfactual — settling against the Lastgang there measures what
    // happened rather than what was instructed, which is a money error in
    // whichever direction the plant deviated.
    let basis = match req.abwicklung {
        RedispatchFall::Duldungsfall => grid_billing::AusfallarbeitBasis::GemessenerLastgang,
        RedispatchFall::Aufforderungsfall => {
            grid_billing::AusfallarbeitBasis::UebermittelterFahrplan
        }
    };
    if matches!(req.abwicklung, RedispatchFall::Aufforderungsfall)
        && req.ausfallarbeit_kwh_override.is_none()
    {
        return Err(ApiError::unprocessable(
            "Aufforderungsfall: supply ausfallarbeit_kwh_override from the transmitted \
             schedule — the measured Lastgang is the Duldungsfall basis and would settle \
             §13a Abs. 2 against the wrong counterfactual",
        ));
    }

    let (ausfallarbeit_kwh, source) = match req.ausfallarbeit_kwh_override {
        Some(kwh) => (kwh, "manual_override"),
        None => (
            lastgang_sum(&http, &cfg, &req.malo_id, window)
                .await
                .ok_or(ApiError::NotFound)?,
            "lastgang_sum",
        ),
    };

    let entgangene = match (
        req.entgangene_einnahmen_eur_override,
        req.anzulegender_wert_ct_per_kwh,
        req.verguetungsart,
    ) {
        (Some(eur), _, _) => eur,
        (None, Some(aw), _) => grid_billing::eeg_entgangene_einnahmen(ausfallarbeit_kwh, aw),
        (None, None, grid_billing::RedispatchVerguetungsart::Sonstige) => {
            return Err(ApiError::unprocessable(
                "Z03 (sonstige) requires entgangene_einnahmen_eur_override — lost market \
                 revenue must be proven, not derived (§13a Abs. 2 S. 3 Nr. 3 EnWG)",
            ));
        }
        (None, None, _) => {
            return Err(ApiError::unprocessable(
                "EEG/KWKG resources require anzulegender_wert_ct_per_kwh, or an explicit \
                 entgangene_einnahmen_eur_override",
            ));
        }
    };

    let verguetung =
        grid_billing::redispatch_verguetung(&grid_billing::RedispatchVerguetungInput {
            ausfallarbeit_kwh,
            basis,
            verguetungsart: req.verguetungsart,
            entgangene_einnahmen_eur: entgangene,
            zusaetzliche_aufwendungen_eur: req.zusaetzliche_aufwendungen_eur,
            ersparte_aufwendungen_eur: req.ersparte_aufwendungen_eur,
        })
        .map_err(|e| ApiError::unprocessable(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "activation_id": activation_id,
        "malo_id": req.malo_id,
        "ausfallarbeit_source": source,
        "verguetung": verguetung,
        "legal_basis": "§13a Abs. 2 EnWG",
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;

    fn utc(s: &str) -> time::OffsetDateTime {
        time::OffsetDateTime::parse(s, &Rfc3339).expect("valid RFC 3339")
    }

    fn edmd_shaped_response() -> Vec<rubo4e::current::Lastgang> {
        serde_json::from_value(serde_json::json!([{
            "_typ": "LASTGANG",
            "obisKennzahl": "1-0:1.29.0",
            "sparte": "STROM",
            "zeitIntervallLaenge": { "wert": "15", "einheit": "VIERTEL_STUNDE" },
            "werte": [
                { "wert": "1.25", "zeitraum": {
                    "startdatum": "2026-01-15", "startuhrzeit": "10:00:00+00:00",
                    "enddatum": "2026-01-15", "enduhrzeit": "10:15:00+00:00" } },
                { "wert": "1.30", "zeitraum": {
                    "startdatum": "2026-01-15", "startuhrzeit": "10:15:00+00:00",
                    "enddatum": "2026-01-15", "enduhrzeit": "10:30:00+00:00" } },
                { "wert": "0.80", "zeitraum": {
                    "startdatum": "2026-01-15", "startuhrzeit": "10:30:00+00:00",
                    "enddatum": "2026-01-15", "enduhrzeit": "10:45:00+00:00" } }
            ]
        }]))
        .expect("edmd-shaped JSON parses as Vec<Lastgang>")
    }

    /// The window bounds the sum, half-open, so a 15-minute activation settles
    /// on its own quarter-hours rather than the neighbouring ones.
    #[test]
    fn the_window_bounds_the_lastgang_sum() {
        let lastgaenge = edmd_shaped_response();
        let all = Window {
            start: utc("2026-01-15T09:00:00Z"),
            end: utc("2026-01-15T12:00:00Z"),
        };
        assert_eq!(sum_lastgang_kwh(&lastgaenge, all), Some(dec!(3.35)));

        let first_two = Window {
            start: utc("2026-01-15T10:00:00Z"),
            end: utc("2026-01-15T10:30:00Z"),
        };
        assert_eq!(sum_lastgang_kwh(&lastgaenge, first_two), Some(dec!(2.55)));

        let single = Window {
            start: utc("2026-01-15T10:30:00Z"),
            end: utc("2026-01-15T10:45:00Z"),
        };
        assert_eq!(sum_lastgang_kwh(&lastgaenge, single), Some(dec!(0.80)));
    }

    /// A local-time Lastgang is converted, not read as UTC.
    ///
    /// `edmd` may serialise a German-local Zeitraum (`+01:00`). Truncating the
    /// offset away shifted every interval by an hour, so a 15-minute activation
    /// summed the wrong quarter-hours — and the Kostenblatt priced energy the
    /// resource produced while it was not curtailed.
    #[test]
    fn a_lastgang_offset_is_honoured_not_truncated() {
        let local: Vec<rubo4e::current::Lastgang> = serde_json::from_value(serde_json::json!([{
            "_typ": "LASTGANG",
            "obisKennzahl": "1-0:1.29.0",
            "sparte": "STROM",
            "zeitIntervallLaenge": { "wert": "15", "einheit": "VIERTEL_STUNDE" },
            "werte": [
                { "wert": "1.25", "zeitraum": {
                    "startdatum": "2026-01-15", "startuhrzeit": "11:00:00+01:00" } },
                { "wert": "1.30", "zeitraum": {
                    "startdatum": "2026-01-15", "startuhrzeit": "11:15:00+01:00" } }
            ]
        }]))
        .expect("edmd-shaped JSON parses");

        // 11:00+01:00 is 10:00 UTC — inside the window.
        let window = Window {
            start: utc("2026-01-15T10:00:00Z"),
            end: utc("2026-01-15T10:30:00Z"),
        };
        assert_eq!(sum_lastgang_kwh(&local, window), Some(dec!(2.55)));

        // Read as bare UTC the same values would land at 11:00, outside it.
        let later = Window {
            start: utc("2026-01-15T11:00:00Z"),
            end: utc("2026-01-15T11:30:00Z"),
        };
        assert_eq!(sum_lastgang_kwh(&local, later), None);
    }

    /// Every offset form BO4E emits resolves; anything else falls back to UTC.
    #[test]
    fn a_utc_offset_parses_in_every_form_bo4e_emits() {
        use time::UtcOffset;
        assert_eq!(parse_utc_offset(""), UtcOffset::UTC);
        assert_eq!(parse_utc_offset("Z"), UtcOffset::UTC);
        assert_eq!(parse_utc_offset("+00:00"), UtcOffset::UTC);
        assert_eq!(
            parse_utc_offset("+02:00"),
            UtcOffset::from_hms(2, 0, 0).expect("valid")
        );
        assert_eq!(
            parse_utc_offset("-0330"),
            UtcOffset::from_hms(-3, -30, 0).expect("valid")
        );
    }

    /// An empty response is `None`, which is the caller's fallback signal.
    #[test]
    fn an_empty_lastgang_is_none() {
        let window = Window {
            start: utc("2026-01-15T10:00:00Z"),
            end: utc("2026-01-15T10:15:00Z"),
        };
        assert_eq!(sum_lastgang_kwh(&[], window), None);
    }

    /// A window must be RFC 3339 and strictly positive.
    #[test]
    fn a_window_is_parsed_and_ordered() {
        assert!(Window::parse("2026-01-15T10:00:00Z", "2026-01-15T10:15:00Z").is_ok());
        assert!(Window::parse("15.01.2026 10:00", "2026-01-15T10:15:00Z").is_err());
        assert!(
            Window::parse("2026-01-15T10:15:00Z", "2026-01-15T10:00:00Z").is_err(),
            "an inverted window would sum nothing and report zero cost"
        );
        assert!(
            Window::parse("2026-01-15T10:00:00Z", "2026-01-15T10:00:00Z").is_err(),
            "a zero-length activation moved no energy"
        );
    }

    /// The 15th-of-month deadline rolls into the next year at December.
    #[test]
    fn the_deadline_rolls_over_the_year() {
        assert_eq!(next_month(2026, 11), (2026, 12));
        assert_eq!(next_month(2026, 12), (2027, 1));
    }

    /// Quantities arrive from `edmd` as strings or numbers; both parse exactly.
    #[test]
    fn a_quantity_parses_from_either_json_shape() {
        assert_eq!(
            decimal_from_json(&serde_json::json!("1.25")),
            Some(dec!(1.25))
        );
        assert_eq!(decimal_from_json(&serde_json::json!(2.5)), Some(dec!(2.5)));
        assert_eq!(decimal_from_json(&serde_json::json!({ "wert": "1" })), None);
    }
}

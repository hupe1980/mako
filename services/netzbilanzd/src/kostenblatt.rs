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

/// The dispatched (fed-in) energy over an activation window, from `edmd`.
///
/// Reads `GET /api/v1/energy?direction=EINSPEISUNG` — the **canonical projected
/// series**, one entry per interval in one direction. Both callers settle lost
/// *generation*: the Kostenblatt prices the curtailed energy, and §13a Abs. 2
/// Ausfallarbeit is by definition what the resource would have produced.
///
/// # Why not `GET /api/v1/lastgang`
///
/// That endpoint is the BO4E **export**: one `Lastgang` per register, both
/// directions, every quality, non-kWh registers included. Folding it back into
/// one figure *is* the register projection, and doing it here would repeat —
/// differently — what `edmd` already does once for everyone:
///
/// - it sums the grid **draw** into a figure that means feed-in;
/// - it counts a total register (`1-0:1.8.0`) on top of the tariff registers
///   (`…1.8.1`, `…1.8.2`) that already cover the same energy;
/// - it keeps qualities § 60 Abs. 2 MsbG excludes from settlement.
///
/// `coverage_pct` comes with the projection, so an incomplete window is a fact
/// the caller can act on rather than a smaller number it cannot distinguish
/// from a small dispatch.
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
    let edmd = cfg.edmd(client.clone())?;
    let path = format!("/api/v1/energy/{malo_id}");
    let request = edmd.get(&path).query(&[
        ("direction", "EINSPEISUNG".to_owned()),
        ("from", window.start.format(&Rfc3339).ok()?),
        ("to", window.end.format(&Rfc3339).ok()?),
    ]);
    let body: serde_json::Value = match edmd.json(request).await {
        Ok(Some(v)) => v,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(%e, malo_id, "netzbilanzd: edmd energy series unavailable");
            return None;
        }
    };
    sum_energy_intervals(&body, malo_id, window)
}

/// Sum the `intervals` of an `edmd` energy-series response over `window`.
///
/// The projection is already one entry per interval in one direction, so this
/// only clips to the activation and adds up. An incomplete window is reported
/// rather than silently summed short: `coverage_pct` is the share of the
/// requested period the series actually spans, and a Kostenblatt built from a
/// half-covered window under-compensates the resource with nothing in the figure
/// to say so.
fn sum_energy_intervals(
    body: &serde_json::Value,
    malo_id: &str,
    window: Window,
) -> Option<Decimal> {
    if let Some(pct) = body.get("coverage_pct").and_then(serde_json::Value::as_f64)
        && pct < 99.9
    {
        tracing::warn!(
            malo_id,
            coverage_pct = pct,
            from = %window.start,
            to = %window.end,
            "netzbilanzd: edmd covers only part of the activation window"
        );
    }

    let mut total = Decimal::ZERO;
    for iv in body.get("intervals")?.as_array()? {
        let start = iv
            .get("start")
            .and_then(serde_json::Value::as_str)
            .and_then(|s| time::OffsetDateTime::parse(s, &Rfc3339).ok())?;
        if start < window.start || start >= window.end {
            continue;
        }
        if let Some(kwh) = iv.get("kwh").and_then(decimal_from_json) {
            total = total.checked_add(kwh)?;
        }
    }
    (total > Decimal::ZERO).then_some(total)
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

    // Built typed, then serialised. A struct literal fails to compile on a
    // field rename; a `json!` literal round-tripped through
    // `from_value::<Kosten>()` would not, because rubo4e captures unknown keys
    // in `_additional` rather than rejecting them — the renamed field would
    // decode cleanly, read back as `None`, and ship to the ÜNB missing.
    //
    // `_typ` is stamped by rubo4e on all four nested COMs, and
    // `Preis.bezugswert` states the reference quantity, without which the
    // Arbeitspreis is EUR per *nothing* rather than EUR/kWh.
    use rubo4e::current::{
        Betrag, Kosten, Kostenblock, Kostenposition, Menge, Mengeneinheit, Preis, Waehrungscode,
        Waehrungseinheit,
    };
    let kosten = Kosten {
        kostenbloecke: Some(vec![Kostenblock {
            kostenblockbezeichnung: Some("Redispatch 2.0 Einsatzkosten".to_owned()),
            kostenpositionen: Some(vec![Kostenposition {
                positionstitel: Some("Arbeitspreis Redispatch".to_owned()),
                artikeldetail: Some(req.tr_id.clone()),
                menge: Some(Menge {
                    wert: Some(dispatch_kwh),
                    einheit: Some(Mengeneinheit::Kwh),
                    ..Default::default()
                }),
                einzelpreis: Some(Preis {
                    wert: Some(req.arbeitspreis_eur_per_kwh),
                    einheit: Some(Waehrungseinheit::Eur),
                    bezugswert: Some(Mengeneinheit::Kwh),
                    ..Default::default()
                }),
                betrag_kostenposition: Some(Betrag {
                    wert: Some(einsatzkosten),
                    waehrung: Some(Waehrungscode::Eur),
                    ..Default::default()
                }),
                von: Some(window.start),
                bis: Some(window.end),
                ..Default::default()
            }]),
            ..Default::default()
        }]),
        ..Default::default()
    };
    // The outbound gate: this document is settled against by the ÜNB.
    mako_markt::bo4e::ensure_conformant(&kosten)
        .map_err(|e| ApiError::Unprocessable(format!("the Kostenblatt is not valid BO4E: {e}")))?;
    let kosten_json = mako_markt::bo4e::to_canonical_json(&kosten)
        .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;

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
    let edmd = cfg.edmd(client.clone())?;
    let day = window.start.date();
    let path = format!("/api/v1/billing-period/{malo_id}");
    let request = edmd
        .get(&path)
        .query(&[("from", day.to_string()), ("to", day.to_string())]);
    let body: serde_json::Value = edmd.json(request).await.ok().flatten()?;
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

    /// An `edmd` `GET /api/v1/energy` response: the projected series, one
    /// entry per interval, already in one direction.
    fn edmd_energy_response(coverage_pct: f64) -> serde_json::Value {
        serde_json::json!({
            "malo_id": "51238696012",
            "direction": "EINSPEISUNG",
            "resolution_min": 15,
            "coverage_pct": coverage_pct,
            "billable_pct": 100.0,
            "interval_count": 3,
            "intervals": [
                { "start": "2026-01-15T10:00:00Z", "end": "2026-01-15T10:15:00Z",
                  "kwh": "1.25", "quality": "MEASURED" },
                { "start": "2026-01-15T10:15:00Z", "end": "2026-01-15T10:30:00Z",
                  "kwh": "1.30", "quality": "MEASURED" },
                { "start": "2026-01-15T10:30:00Z", "end": "2026-01-15T10:45:00Z",
                  "kwh": "0.80", "quality": "MEASURED" }
            ]
        })
    }

    /// The window bounds the sum, half-open, so a 15-minute activation settles
    /// on its own quarter-hours rather than the neighbouring ones.
    #[test]
    fn the_window_bounds_the_energy_sum() {
        let body = edmd_energy_response(100.0);
        let sum = |w| super::sum_energy_intervals(&body, "51238696012", w);

        assert_eq!(
            sum(Window {
                start: utc("2026-01-15T09:00:00Z"),
                end: utc("2026-01-15T12:00:00Z"),
            }),
            Some(dec!(3.35))
        );
        assert_eq!(
            sum(Window {
                start: utc("2026-01-15T10:00:00Z"),
                end: utc("2026-01-15T10:30:00Z"),
            }),
            Some(dec!(2.55))
        );
        assert_eq!(
            sum(Window {
                start: utc("2026-01-15T10:30:00Z"),
                end: utc("2026-01-15T10:45:00Z"),
            }),
            Some(dec!(0.80))
        );
        // A window the series does not reach yields nothing, not zero.
        assert_eq!(
            sum(Window {
                start: utc("2026-01-15T11:00:00Z"),
                end: utc("2026-01-15T11:30:00Z"),
            }),
            None
        );
    }

    /// A decimal spelled as a JSON number sums the same as one spelled as a
    /// string — `edmd` serialises through `rust_decimal`, but a hand-built or
    /// proxied body may not.
    #[test]
    fn both_decimal_spellings_sum() {
        let body = serde_json::json!({
            "coverage_pct": 100.0,
            "intervals": [
                { "start": "2026-01-15T10:00:00Z", "kwh": 1.25 },
                { "start": "2026-01-15T10:15:00Z", "kwh": "1.30" }
            ]
        });
        let window = Window {
            start: utc("2026-01-15T10:00:00Z"),
            end: utc("2026-01-15T10:30:00Z"),
        };
        assert_eq!(
            super::sum_energy_intervals(&body, "51238696012", window),
            Some(dec!(2.55))
        );
    }

    /// A partly covered window still yields its figure — the resource was
    /// curtailed for the part that is there — but the shortfall is reported.
    #[test]
    fn a_partly_covered_window_still_sums() {
        let body = edmd_energy_response(50.0);
        let window = Window {
            start: utc("2026-01-15T10:00:00Z"),
            end: utc("2026-01-15T10:30:00Z"),
        };
        assert_eq!(
            super::sum_energy_intervals(&body, "51238696012", window),
            Some(dec!(2.55))
        );
    }

    /// An empty response is `None`, which is the caller's fallback signal.
    #[test]
    fn an_empty_series_is_none() {
        let body = serde_json::json!({ "coverage_pct": 0.0, "intervals": [] });
        let window = Window {
            start: utc("2026-01-15T10:00:00Z"),
            end: utc("2026-01-15T10:15:00Z"),
        };
        assert_eq!(
            super::sum_energy_intervals(&body, "51238696012", window),
            None
        );
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

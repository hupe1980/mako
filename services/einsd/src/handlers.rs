//! HTTP handlers for `einsd`.

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::PgPool;

use crate::{
    config::EinsdConfig,
    pg::{
        AnlageUpsertRequest, AnlagenQuery, SettleOverrides, build_settle_input,
        decommission_anlage, fetch_anlage, fetch_epex_price, fetch_jahresmarktwert_single,
        list_anlagen, list_expiring, list_settlement_receipts, list_unsettled,
        lookup_verguetungssatz, run_settlement, upsert_anlage, upsert_epex_price,
        upsert_jahresmarktwert, zusammenlegen,
    },
};

// ── edmd auto-fetch helper ────────────────────────────────────────────────────

/// Fetch `arbeitsmenge_kwh` from `edmd` for a given MaLo and billing month.
///
/// Calls `GET {edmd_url}/api/v1/billing-period/{malo_id}?from=YYYY-MM-01&to=YYYY-MM-LD`
/// and extracts `arbeitsmenge_kwh` from the response JSON.
/// Returns `None` when `edmd_url` is not configured or the MaLo has no data.
pub async fn fetch_einspeisemenge_from_edmd(
    cfg: &EinsdConfig,
    malo_id: &str,
    year: i16,
    month: i16,
) -> Option<Decimal> {
    let edmd_url = cfg.edmd_url.as_deref()?;
    let last_day = days_in_month(year, month);
    let from = format!("{year:04}-{month:02}-01");
    let to = format!("{year:04}-{month:02}-{last_day:02}");
    let url = format!("{edmd_url}/api/v1/billing-period/{malo_id}?from={from}&to={to}");

    let client = reqwest::Client::new();
    let mut req = client.get(&url);
    if let Some(key) = cfg.edmd_api_key.as_deref() {
        req = req.bearer_auth(key);
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    body.get("arbeitsmenge_kwh")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<Decimal>().ok())
        .or_else(|| {
            body.get("arbeitsmenge_kwh")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
        })
}

fn days_in_month(year: i16, month: i16) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let y = year as i32;
            if y % 400 == 0 || (y % 4 == 0 && y % 100 != 0) {
                29
            } else {
                28
            }
        }
        _ => 28,
    }
}

// ── CloudEvent emission ───────────────────────────────────────────────────────

/// Emit a settlement CloudEvent to `erp_webhook_url`.
///
/// Returns the CloudEvent UUID on success; `None` on failure or when webhook
/// is not configured.  Failures are logged as warnings — they do not roll back
/// the settlement calculation, which is already persisted.
///
/// CE types emitted:
/// - `de.eeg.verguetung.berechnet` — VERGUETUNG, MIETERSTROM, POST_EEG_SPOT,
///   EIGENVERBRAUCH, KWKG_ZUSCHLAG, FLEXIBILITAET
/// - `de.eeg.marktpraemie.berechnet` — DIREKTVERMARKTUNG, AUSSCHREIBUNG
pub async fn emit_settlement_ce(
    cfg: &EinsdConfig,
    ce_type: &str,
    tr_id: &str,
    malo_id: &str,
    result: &crate::pg::SettleResult,
    year: i16,
    month: i16,
) -> Option<uuid::Uuid> {
    let webhook_url = cfg.erp_webhook_url.as_deref()?;
    let ce_id = uuid::Uuid::new_v4();
    let now = time::OffsetDateTime::now_utc();

    let payload = serde_json::json!({
        "specversion": "1.0",
        "type": ce_type,
        "source": format!("urn:einsd:tenant:{}", cfg.tenant),
        "id": ce_id.to_string(),
        "time": now.to_string(),
        "subject": tr_id,
        "datacontenttype": "application/json",
        "data": {
            "tr_id": tr_id,
            "malo_id": malo_id,
            "billing_year": year,
            "billing_month": month,
            "settlement_model": result.settlement_model,
            "einspeisemenge_kwh": result.einspeisemenge_kwh,
            "settlement_eur": result.settlement_eur,
            "status": result.status,
        }
    });

    let client = reqwest::Client::new();
    let body = serde_json::to_string(&payload).unwrap_or_default();
    let mut req = client
        .post(webhook_url)
        .header("Content-Type", "application/cloudevents+json")
        .body(body.clone());

    // HMAC-SHA256 signing when secret is configured.
    if let Some(secret) = cfg.erp_hmac_secret.as_deref() {
        let sig = mako_service::webhook::hmac_hex(secret.as_bytes(), body.as_bytes());
        req = req.header("X-Mako-Signature", format!("sha256={sig}"));
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => Some(ce_id),
        Ok(resp) => {
            tracing::warn!(
                ce_type, tr_id, status = %resp.status(),
                "einsd: ERP webhook delivery failed"
            );
            None
        }
        Err(e) => {
            tracing::warn!(ce_type, tr_id, error = %e, "einsd: ERP webhook error");
            None
        }
    }
}

/// Emit `de.eeg.anlage.foerderung_auslaufend` for a plant about to expire.
pub async fn emit_foerderung_alert_ce(
    cfg: &EinsdConfig,
    tr_id: &str,
    malo_id: &str,
    foerderendedatum: time::Date,
    days_remaining: i64,
) {
    let Some(webhook_url) = cfg.erp_webhook_url.as_deref() else {
        return;
    };

    let ce_id = uuid::Uuid::new_v4();
    let payload = serde_json::json!({
        "specversion": "1.0",
        "type": "de.eeg.anlage.foerderung_auslaufend",
        "source": format!("urn:einsd:tenant:{}", cfg.tenant),
        "id": ce_id.to_string(),
        "time": time::OffsetDateTime::now_utc().to_string(),
        "subject": tr_id,
        "datacontenttype": "application/json",
        "data": {
            "tr_id": tr_id,
            "malo_id": malo_id,
            "foerderendedatum": foerderendedatum.to_string(),
            "days_remaining": days_remaining,
        }
    });

    let client = reqwest::Client::new();
    let _ = client
        .post(webhook_url)
        .header("Content-Type", "application/cloudevents+json")
        .json(&payload)
        .send()
        .await;
}

// ── EEG Anlage CRUD ───────────────────────────────────────────────────────────

/// `POST /api/v1/anlagen`  — Register or replace a plant.
pub async fn post_anlage(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Json(req): Json<AnlageUpsertRequest>,
) -> impl IntoResponse {
    match upsert_anlage(&pool, &cfg.tenant, req).await {
        Ok(()) => StatusCode::CREATED.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// `PUT /api/v1/anlagen/{tr_id}`  — Update an existing plant.
pub async fn put_anlage(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
    Json(mut req): Json<AnlageUpsertRequest>,
) -> impl IntoResponse {
    req.tr_id = tr_id;
    match upsert_anlage(&pool, &cfg.tenant, req).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/anlagen/{tr_id}`
pub async fn get_anlage(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
) -> impl IntoResponse {
    match fetch_anlage(&pool, &cfg.tenant, &tr_id).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/anlagen`  — List plants with optional filters.
pub async fn get_anlagen(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Query(q): Query<AnlagenQuery>,
) -> impl IntoResponse {
    match list_anlagen(&pool, &cfg.tenant, &q).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `DELETE /api/v1/anlagen/{tr_id}`  — Decommission (set status = 'abgemeldet').
pub async fn delete_anlage(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
) -> impl IntoResponse {
    match decommission_anlage(&pool, &cfg.tenant, &tr_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── 180-day expiry alert ──────────────────────────────────────────────────────

/// `GET /api/v1/anlagen/foerderung-auslaufend`
///
/// Returns plants whose `foerderendedatum` is within 180 days of today.
/// Used by the background alert worker and ERP dashboards.
pub async fn get_foerderung_auslaufend(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Query(q): Query<HorizonQuery>,
) -> impl IntoResponse {
    let days = q.days.unwrap_or(180);
    match list_expiring(&pool, &cfg.tenant, days).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct HorizonQuery {
    /// Look-ahead window in days (default: 180).
    pub days: Option<i32>,
}

// ── Settlement ────────────────────────────────────────────────────────────────

/// Request body for `POST /api/v1/anlagen/{tr_id}/settle/{year}/{month}`.
#[derive(Debug, Deserialize)]
pub struct SettleTriggerRequest {
    /// Einspeisemenge kWh for the billing month.
    /// When absent, `einsd` will return `status = "no_data"`.
    pub einspeisemenge_kwh: Option<Decimal>,
    /// Override EPEX monthly average ct/kWh (only for DIREKTVERMARKTUNG /
    /// POST_EEG_SPOT).  When absent, the value stored in `epex_monthly_prices`
    /// is used automatically.
    pub epex_avg_ct_kwh: Option<Decimal>,
    /// Override §20 Abs. 3 EEG 2023 Managementprämie ct/kWh.
    /// Defaults to 0.4 ct/kWh (0.2 ct/kWh for plants >100 MW).
    /// Only applies to DIREKTVERMARKTUNG and AUSSCHREIBUNG settlement models.
    pub managementpraemie_ct_override: Option<Decimal>,
    /// §19 EEG 2023 — kWh curtailed by NB this billing month.
    ///
    /// The NB must compensate the operator at the AW rate for these kWh
    /// (§19 Abs. 2 EEG 2023: §51 Negativpreisregel does NOT apply to EInsMan kWh).
    /// Pass the total curtailed kWh from MSCONS IFTSTA messages in this period.
    #[serde(default)]
    pub einspeisemanagement_kwh: Option<Decimal>,
    /// §51a EEG 2023 — quarter-hours during which the EPEX price was negative
    /// AND the plant's §51 threshold was met.
    ///
    /// Used to compute the Verlängerungsanspruch (Förderzeitraum extension):
    /// Solar PV: `ceil(qh / 2)` · Others: `qh` (1:1 factor).
    /// Pass the total QH count from hourly EPEX data for the billing month.
    #[serde(default)]
    pub negative_price_quarter_hours: Option<u64>,
}

/// `POST /api/v1/anlagen/{tr_id}/settle/{year}/{month}`
///
/// Trigger monthly EEG settlement for one plant.  Idempotent — re-running
/// overwrites the previous result for the same (tr_id, year, month).
pub async fn post_settle(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path((tr_id, year, month)): Path<(String, i16, i16)>,
    Json(req): Json<SettleTriggerRequest>,
) -> impl IntoResponse {
    // Load plant to get settlement parameters.
    let anlage = match fetch_anlage(&pool, &cfg.tenant, &tr_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Auto-fetch Einspeisemenge from edmd when not supplied in request.
    // Calls GET {edmd_url}/api/v1/billing-period/{malo_id}?from=...&to=...
    // Uses arbeitsmenge_kwh from the MeterBillingPeriod response.
    let einspeisemenge_kwh = match req.einspeisemenge_kwh {
        Some(kwh) => Some(kwh),
        None => fetch_einspeisemenge_from_edmd(&cfg, &anlage.malo_id, year, month).await,
    };

    // Resolve EPEX price from DB when not supplied in request.
    let epex_avg_ct_kwh = match req.epex_avg_ct_kwh {
        Some(p) => Some(p),
        None => match fetch_epex_price(&pool, year, month).await {
            Ok(p) => p,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
    };

    let input = build_settle_input(
        &cfg.tenant,
        &anlage,
        year,
        month,
        SettleOverrides {
            einspeisemenge_kwh,
            epex_avg_ct_kwh,
            managementpraemie_ct_override: req.managementpraemie_ct_override,
            einspeisemanagement_kwh: req.einspeisemanagement_kwh,
            negative_price_quarter_hours: req.negative_price_quarter_hours,
            correction_of: None,
            jahresmarktwert_ct_kwh: None, // auto-fetched by run_settlement
        },
    );

    match run_settlement(&pool, input).await {
        Ok(result) => {
            // ── Emit CloudEvent to ERP webhook ───────────────────────────────
            // de.eeg.verguetung.berechnet   — VERGUETUNG, MIETERSTROM, POST_EEG_SPOT, EIGENVERBRAUCH, KWKG_ZUSCHLAG, FLEXIBILITAET
            // de.eeg.marktpraemie.berechnet — DIREKTVERMARKTUNG, AUSSCHREIBUNG
            if result.status == "calculated" {
                let ce_type = match anlage.settlement_model.as_str() {
                    "DIREKTVERMARKTUNG" | "AUSSCHREIBUNG" => "de.eeg.marktpraemie.berechnet",
                    _ => "de.eeg.verguetung.berechnet",
                };
                let ce_id = emit_settlement_ce(
                    &cfg,
                    ce_type,
                    &tr_id,
                    &anlage.malo_id,
                    &result,
                    year,
                    month,
                )
                .await;
                // Update ce_id in DB (best-effort — failure doesn't affect settlement result).
                if let Some(ce_id) = ce_id {
                    let _ = sqlx::query(
                        "UPDATE settlement_receipts SET ce_id = $1 \
                         WHERE tr_id = $2 AND tenant = $3 AND billing_year = $4 AND billing_month = $5",
                    )
                    .bind(ce_id)
                    .bind(&tr_id)
                    .bind(&cfg.tenant)
                    .bind(year)
                    .bind(month)
                    .execute(&pool)
                    .await;
                }
            }
            (StatusCode::OK, Json(result)).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/anlagen/{tr_id}/settlements`
pub async fn get_settlements(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
    Query(q): Query<SettlementsQuery>,
) -> impl IntoResponse {
    match list_settlement_receipts(&pool, &cfg.tenant, &tr_id, q.limit.unwrap_or(24).min(200)).await
    {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct SettlementsQuery {
    pub limit: Option<i64>,
}

// ── EPEX monthly prices ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct EpexPriceBody {
    pub avg_ct_kwh: Decimal,
    pub source: Option<String>,
}

// ── §20 Abs. 2 Jahresmarktwert prices ──────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct JahresmarktwertBody {
    pub avg_ct_kwh: Decimal,
    pub source: Option<String>,
}

/// `PUT /api/v1/jahresmarktwert/{year}/{month}/{erzeugungsart}`
///
/// Import or update a technology-specific monthly Jahresmarktwert price
/// (§20 Abs. 2 + Anlage 1 EEG 2023), published by ÜNB at netztransparenz.de.
///
/// `erzeugungsart` must match an `erzeugungsart` column value (e.g. `WIND_ONSHORE`,
/// `SOLAR_AUFDACH`, `BIOMASSE`) or `DEFAULT` for the generic fallback row.
///
/// For MarketPremium (Direktvermarktung / Ausschreibung) settlements, the
/// technology-specific Jahresmarktwert takes precedence over the generic EPEX
/// monthly average from `epex_monthly_prices`.
pub async fn put_jahresmarktwert(
    Extension(pool): Extension<PgPool>,
    Path((year, month, erzeugungsart)): Path<(i16, i16, String)>,
    Json(body): Json<JahresmarktwertBody>,
) -> impl IntoResponse {
    if !(1..=12).contains(&month) {
        return (StatusCode::BAD_REQUEST, "month must be 1–12").into_response();
    }
    let source = body.source.as_deref().unwrap_or("manual");
    match upsert_jahresmarktwert(&pool, year, month, &erzeugungsart, body.avg_ct_kwh, source).await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/jahresmarktwert/{year}/{month}/{erzeugungsart}`
pub async fn get_jahresmarktwert(
    Extension(pool): Extension<PgPool>,
    Path((year, month, erzeugungsart)): Path<(i16, i16, String)>,
) -> impl IntoResponse {
    match fetch_jahresmarktwert_single(&pool, year, month, &erzeugungsart).await {
        Ok(Some(p)) => Json(serde_json::json!({
            "billing_year": year,
            "billing_month": month,
            "erzeugungsart": erzeugungsart,
            "avg_ct_kwh": p,
        }))
        .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `PUT /api/v1/epex-monthly/{year}/{month}`
pub async fn put_epex_price(
    Extension(pool): Extension<PgPool>,
    Path((year, month)): Path<(i16, i16)>,
    Json(body): Json<EpexPriceBody>,
) -> impl IntoResponse {
    let source = body.source.as_deref().unwrap_or("manual");
    match upsert_epex_price(&pool, year, month, body.avg_ct_kwh, source).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/epex-monthly/{year}/{month}`
pub async fn get_epex_price(
    Extension(pool): Extension<PgPool>,
    Path((year, month)): Path<(i16, i16)>,
) -> impl IntoResponse {
    match fetch_epex_price(&pool, year, month).await {
        Ok(Some(p)) => Json(serde_json::json!({
            "billing_year": year,
            "billing_month": month,
            "avg_ct_kwh": p,
        }))
        .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Repowering (§22 EEG 2023) ────────────────────────────────────────────────

/// Request body for `POST /api/v1/anlagen/{tr_id}/repowering`.
#[derive(Debug, Deserialize)]
pub struct RepoweringRequest {
    /// ISO 8601 date when the new components were commissioned.
    /// The Förderendedatum is reset to `repowering_datum + 20 years`.
    pub repowering_datum: String,
    /// New installed capacity in kWp (may differ from original).
    pub leistung_kwp_neu: Option<Decimal>,
    /// New Vergütungssatz at the repowering date (ct/kWh).
    /// When absent, auto-lookup via `eeg_verguetungssaetze` table.
    pub verguetungssatz_ct_neu: Option<Decimal>,
}

/// `POST /api/v1/anlagen/{tr_id}/repowering`
///
/// Trigger a repowering event for an existing plant.  Per §22 EEG 2023:
/// - The 20-year Förderungsdauer resets from `repowering_datum`.
/// - The Vergütungssatz is updated to the rate applicable at `repowering_datum`.
/// - The original commissioning date is preserved in `ursprungs_inbetriebnahme`.
/// - The plant status transitions from `aktiv` to `aktiv` (remains active).
///
/// Idempotent: re-posting with the same date is safe.
pub async fn post_repowering(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
    Json(req): Json<RepoweringRequest>,
) -> impl IntoResponse {
    use time::format_description::well_known::Iso8601;

    let repowering_datum = match time::Date::parse(&req.repowering_datum, &Iso8601::DEFAULT) {
        Ok(d) => d,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                "invalid repowering_datum, expected ISO 8601",
            )
                .into_response();
        }
    };

    // Load existing plant.
    let anlage = match fetch_anlage(&pool, &cfg.tenant, &tr_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // §25 Abs. 1 Satz 2 EEG 2023: statutory plants extend to Dec 31 of the 20th year.
    // foerderendedatum_repowering() uses the correct formula (Dec 31 of year+20),
    // NOT the Ausschreibung rule (exact +20y anniversary).
    let foerderendedatum_neu = match eeg_billing::foerderendedatum_repowering(repowering_datum) {
        Ok(d) => d,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Auto-lookup new Vergütungssatz when not supplied.
    let verguetungssatz_ct_neu = if let Some(ct) = req.verguetungssatz_ct_neu {
        ct
    } else {
        match lookup_verguetungssatz(
            &pool,
            &anlage.erzeugungsart,
            req.leistung_kwp_neu.unwrap_or(anlage.leistung_kwp),
            &req.repowering_datum,
        ).await {
            Ok(Some(ct)) => ct,
            Ok(None) => return (
                StatusCode::UNPROCESSABLE_ENTITY,
                "No Vergütungssatz found for this plant type and repowering date — supply verguetungssatz_ct_neu explicitly",
            ).into_response(),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    };

    let res = sqlx::query(
        r"UPDATE eeg_anlagen SET
              ist_repowering           = true,
              ursprungs_inbetriebnahme = COALESCE(ursprungs_inbetriebnahme, inbetriebnahme),
              repowering_datum         = $3,
              inbetriebnahme           = $3,
              foerderendedatum         = $4,
              verguetungssatz_ct       = $5,
              leistung_kwp             = COALESCE($6, leistung_kwp),
              updated_at               = now()
          WHERE tr_id = $1 AND tenant = $2",
    )
    .bind(&tr_id)
    .bind(&cfg.tenant)
    .bind(repowering_datum)
    .bind(foerderendedatum_neu)
    .bind(verguetungssatz_ct_neu)
    .bind(req.leistung_kwp_neu)
    .execute(&pool)
    .await;

    match res {
        Ok(r) if r.rows_affected() > 0 => Json(serde_json::json!({
            "tr_id": tr_id,
            "repowering_datum": repowering_datum.to_string(),
            "foerderendedatum_neu": foerderendedatum_neu.to_string(),
            "verguetungssatz_ct_neu": verguetungssatz_ct_neu,
        }))
        .into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Vergütungssatz lookup ─────────────────────────────────────────────────────

/// Request body for `POST /api/v1/verguetungssatz-lookup`.
#[derive(Debug, Deserialize)]
pub struct VerguetungssatzLookupRequest {
    pub erzeugungsart: String,
    pub leistung_kwp: Decimal,
    /// ISO 8601 Inbetriebnahmedatum.
    pub inbetriebnahme: String,
}

// ── MaStR registration confirmation ──────────────────────────────────────────

/// Request body for `POST /api/v1/anlagen/{tr_id}/mastr-registrierung`.
#[derive(Debug, Deserialize)]
pub struct MastrRegistrierungRequest {
    /// MaStR Registrierungsnummer (format: `SEE000000000000`, `EEE000000000000`, etc.).
    ///
    /// Issued by BNetzA at marktstammdatenregister.de.
    pub mastr_nummer: String,
    /// Date of MaStR registration (ISO 8601). Defaults to today if omitted.
    pub mastr_datum: Option<String>,
}

/// `POST /api/v1/anlagen/{tr_id}/mastr-registrierung`
///
/// Confirm MaStR registration for a plant. Transitions:
/// - `mastr_registriert` → `true`
/// - `status` `angemeldet` → `aktiv` (if it was angemeldet)
///
/// ## Legal basis
///
/// §52 Abs. 1 Nr. 11 EEG 2023: plant operators must register in MaStR.
/// - EEG 2023 plants: until confirmed, €10/kW/month Pflichtzahlung accrues.
/// - EEG ≤2021 plants: until confirmed, Vergütung = 0 (old §52/§47 via §100).
///
/// ## CloudEvent emitted
///
/// `de.eeg.anlage.mastr_registriert` — signals ERP to release pending Vergütung.
pub async fn post_mastr_registrierung(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
    Json(req): Json<MastrRegistrierungRequest>,
) -> impl IntoResponse {
    use time::format_description::well_known::Iso8601;

    let mastr_datum = if let Some(ref ds) = req.mastr_datum {
        match time::Date::parse(ds, &Iso8601::DEFAULT) {
            Ok(d) => d,
            Err(_) => {
                return (StatusCode::UNPROCESSABLE_ENTITY, "invalid mastr_datum").into_response();
            }
        }
    } else {
        time::OffsetDateTime::now_utc().date()
    };

    let rows = sqlx::query(
        "UPDATE eeg_anlagen SET \
            mastr_registriert    = true, \
            mastr_nummer         = $3, \
            mastr_datum          = $4, \
            mastr_violation_start = NULL, \
            status               = CASE WHEN status = 'angemeldet' THEN 'aktiv' ELSE status END, \
            updated_at           = now() \
         WHERE tr_id = $1 AND tenant = $2 \
           AND status IN ('angemeldet', 'aktiv')",
    )
    .bind(&tr_id)
    .bind(&cfg.tenant)
    .bind(&req.mastr_nummer)
    .bind(mastr_datum)
    .execute(&pool)
    .await;

    match rows {
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Ok(r) if r.rows_affected() == 0 => StatusCode::NOT_FOUND.into_response(),
        Ok(_) => {
            // Emit CloudEvent to ERP webhook
            let ce_body = serde_json::json!({
                "tr_id": tr_id,
                "mastr_nummer": req.mastr_nummer,
                "mastr_datum": mastr_datum.to_string(),
            });
            if let Some(webhook_url) = cfg.erp_webhook_url.as_deref() {
                let ce_id = uuid::Uuid::new_v4();
                let now = time::OffsetDateTime::now_utc();
                let _ = reqwest::Client::new()
                    .post(webhook_url)
                    .header("ce-id", ce_id.to_string())
                    .header("ce-type", "de.eeg.anlage.mastr_registriert")
                    .header("ce-source", format!("/einsd/anlagen/{tr_id}"))
                    .header("ce-specversion", "1.0")
                    .header("ce-time", now.to_string())
                    .header("content-type", "application/json")
                    .json(&ce_body)
                    .send()
                    .await;
            }
            StatusCode::NO_CONTENT.into_response()
        }
    }
}

/// `POST /api/v1/verguetungssatz-lookup`
///
/// Returns the applicable EEG feed-in tariff rate for a plant.
/// Used during Anlage registration to auto-populate `verguetungssatz_ct`
/// without requiring the operator to manually look up BNetzA tables.
pub async fn post_verguetungssatz_lookup(
    Extension(pool): Extension<PgPool>,
    Json(req): Json<VerguetungssatzLookupRequest>,
) -> impl IntoResponse {
    match lookup_verguetungssatz(
        &pool,
        &req.erzeugungsart,
        req.leistung_kwp,
        &req.inbetriebnahme,
    )
    .await
    {
        Ok(Some(ct)) => Json(serde_json::json!({
            "erzeugungsart": req.erzeugungsart,
            "leistung_kwp": req.leistung_kwp,
            "inbetriebnahme": req.inbetriebnahme,
            "verguetungssatz_ct": ct,
        }))
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            "No matching EEG tariff rate found. Use PUT /api/v1/verguetungssaetze to import additional rates.",
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Batch settlement (POST /api/v1/settle/{year}/{month}) ────────────────────

/// Request body for `POST /api/v1/settle/{year}/{month}`.
#[derive(Debug, serde::Deserialize)]
pub struct BatchSettleRequest {
    /// EPEX monthly average ct/kWh.  When absent, uses stored `epex_monthly_prices`.
    pub epex_avg_ct_kwh: Option<Decimal>,
    /// Dry-run mode — calculates but does not persist or emit CloudEvents.
    #[serde(default)]
    pub dry_run: bool,
    /// Maximum plants to settle in one request (default 500, max 2000).
    pub limit: Option<i64>,
}

/// `POST /api/v1/settle/{year}/{month}`
///
/// **Batch EEG/KWKG settlement — settle all unsettled active plants for a month.**
///
/// Idempotent: plants already settled for this period are skipped.
/// Auto-fetches Einspeisemenge from `edmd` for each plant's MaLo when
/// `edmd_url` is configured.
///
/// Returns a summary with per-plant results and aggregate totals.
pub async fn post_batch_settle(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path((year, month)): Path<(i16, i16)>,
    Json(req): Json<BatchSettleRequest>,
) -> impl IntoResponse {
    // Resolve EPEX price once for the whole batch.
    let epex_avg_ct_kwh = match req.epex_avg_ct_kwh {
        Some(p) => Some(p),
        None => match fetch_epex_price(&pool, year, month).await {
            Ok(p) => p,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
    };

    let limit = req.limit.unwrap_or(500).min(2000);
    let plants = match list_unsettled(&pool, &cfg.tenant, year, month).await {
        Ok(p) => p,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let plants: Vec<_> = plants.into_iter().take(limit as usize).collect();
    let total_plants = plants.len();
    let mut settled = 0u32;
    let mut skipped_no_data = 0u32;
    let mut skipped_price_missing = 0u32;
    let mut errors = 0u32;
    let mut total_settlement_eur = rust_decimal::Decimal::ZERO;
    let mut results: Vec<serde_json::Value> = Vec::with_capacity(plants.len().min(100));

    if req.dry_run {
        // Dry-run: count without persisting (no DB writes needed).
        for anlage in &plants {
            let has_data = fetch_einspeisemenge_from_edmd(&cfg, &anlage.malo_id, year, month)
                .await
                .is_some();
            if has_data {
                settled += 1;
            } else {
                skipped_no_data += 1;
            }
        }
    } else {
        // ── Parallel batch settlement with bounded concurrency ────────────────
        // Use JoinSet + Semaphore (20 concurrent) to parallelize DB + edmd I/O.
        // Each task has its own pool handle (PgPool is Arc-backed, clone is cheap).
        use std::sync::Arc;
        use tokio::sync::Semaphore;
        use tokio::task::JoinSet;

        const MAX_CONCURRENT: usize = 20;
        let sem = Arc::new(Semaphore::new(MAX_CONCURRENT));
        let mut join_set: JoinSet<(String, String, anyhow::Result<crate::pg::SettleResult>)> =
            JoinSet::new();

        for anlage in plants {
            let cfg = Arc::clone(&cfg);
            let pool = pool.clone();
            let sem = Arc::clone(&sem);
            let malo_id = anlage.malo_id.clone();
            let tr_id = anlage.tr_id.clone();
            let settlement_model = anlage.settlement_model.clone();
            join_set.spawn(async move {
                let _permit = sem.acquire().await.expect("semaphore closed");
                let einspeisemenge_kwh =
                    fetch_einspeisemenge_from_edmd(&cfg, &malo_id, year, month).await;
                let input = build_settle_input(
                    &cfg.tenant,
                    &anlage,
                    year,
                    month,
                    SettleOverrides {
                        einspeisemenge_kwh,
                        epex_avg_ct_kwh,
                        managementpraemie_ct_override: None,
                        einspeisemanagement_kwh: None,
                        negative_price_quarter_hours: None,
                        correction_of: None,
                        jahresmarktwert_ct_kwh: None,
                    },
                );
                let res = run_settlement(&pool, input).await;
                // Best-effort CE emission for calculated results
                if let Ok(ref result) = res
                    && result.status == "calculated"
                {
                    let ce_type = match settlement_model.as_str() {
                        "DIREKTVERMARKTUNG" | "AUSSCHREIBUNG" | "MARKET_PREMIUM" => {
                            "de.eeg.marktpraemie.berechnet"
                        }
                        _ => "de.eeg.verguetung.berechnet",
                    };
                    emit_settlement_ce(&cfg, ce_type, &tr_id, &malo_id, result, year, month).await;
                }
                (tr_id, settlement_model, res)
            });
        }

        while let Some(join_result) = join_set.join_next().await {
            match join_result {
                Ok((_tr_id, _model, Ok(result))) => match result.status.as_str() {
                    "calculated" | "foerderung_beendet" => {
                        settled += 1;
                        if let Some(eur) = result.settlement_eur {
                            total_settlement_eur += eur;
                        }
                        if results.len() < 100 {
                            results.push(serde_json::json!({
                                "tr_id": result.tr_id,
                                "status": result.status,
                                "settlement_eur": result.settlement_eur,
                                "einspeisemenge_kwh": result.einspeisemenge_kwh,
                            }));
                        }
                    }
                    "no_data" => skipped_no_data += 1,
                    "price_missing" => skipped_price_missing += 1,
                    _ => errors += 1,
                },
                Ok((tr_id, _, Err(e))) => {
                    tracing::warn!(tr_id, error = %e, "batch_settle: settlement error");
                    errors += 1;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "batch_settle: task join error");
                    errors += 1;
                }
            }
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "billing_year": year,
            "billing_month": month,
            "dry_run": req.dry_run,
            "total_plants": total_plants,
            "settled": settled,
            "skipped_no_data": skipped_no_data,
            "skipped_price_missing": skipped_price_missing,
            "errors": errors,
            "total_settlement_eur": total_settlement_eur.to_string(),
            "results_sample": results,
            "hint": if total_plants == limit as usize {
                format!("More plants may be unsettled. Re-run to settle the next batch of {}.", limit)
            } else {
                "All unsettled plants processed.".to_owned()
            }
        })),
    )
        .into_response()
}

// ── §24 EEG 2023 — Zusammenlegung ────────────────────────────────────────────

/// Request body for `POST /api/v1/anlagen/{tr_id}/zusammenlegen`.
#[derive(Debug, serde::Deserialize)]
pub struct ZusammenlegungRequest {
    /// TR-ID of the parent (surviving) plant.
    pub parent_tr_id: String,
    /// Combined installed capacity in kWp after merger.
    /// When absent, the parent's capacity is unchanged.
    pub combined_leistung_kwp: Option<Decimal>,
}

/// `POST /api/v1/anlagen/{tr_id}/zusammenlegen`
///
/// **§24 EEG 2023 — Zusammenlegung (plant merger).**
///
/// Merges `{tr_id}` (child) into `parent_tr_id`.  Per §24 EEG 2023:
/// - The child plant is deregistered (`status = abgemeldet`).
/// - `parent_tr_id` is set on the child for audit trail.
/// - The parent plant assumes the combined capacity (`combined_leistung_kwp`).
/// - The parent's `foerderendedatum` is **NOT** reset (only Repowering resets it).
/// - Future settlements continue only on the parent plant.
///
/// This is distinct from Repowering (§22 EEG): Zusammenlegung is an
/// administrative merger, not a hardware replacement. No new commissioning date.
pub async fn post_zusammenlegen(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(child_tr_id): Path<String>,
    Json(req): Json<ZusammenlegungRequest>,
) -> impl IntoResponse {
    if child_tr_id == req.parent_tr_id {
        return (
            StatusCode::BAD_REQUEST,
            "child and parent tr_id must differ",
        )
            .into_response();
    }

    match zusammenlegen(&pool, &cfg.tenant, &child_tr_id, &req.parent_tr_id, req.combined_leistung_kwp).await {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "child_tr_id": child_tr_id,
                "parent_tr_id": req.parent_tr_id,
                "child_status": "abgemeldet",
                "combined_leistung_kwp": req.combined_leistung_kwp,
                "note": "§24 EEG 2023 Zusammenlegung complete. Future settlements run on parent plant only.",
            })),
        )
            .into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, format!("plant {child_tr_id} not found or not aktiv")).into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

// ── §21b EEG 2023 — Veräußerungsform Wechsel ─────────────────────────────────

/// Request body for `POST /api/v1/anlagen/{tr_id}/switch-veraeusserungsform`.
#[derive(Debug, serde::Deserialize)]
pub struct VeraeusserungsformWechselRequest {
    /// The new settlement model to switch to.
    /// Must be either `"FEED_IN_TARIFF"` / `"VERGUETUNG"` (switch to Einspeisevergütung)
    /// or `"MARKET_PREMIUM"` / `"DIREKTVERMARKTUNG"` (switch to Direktvermarktung).
    pub new_model: String,
    /// Effective date for the switch (must be the 1st of a calendar month).
    pub effective_date: String,
    /// For Direktvermarktung switches: the Direktvermarkter's MP-ID.
    pub direktvermarkter_mp_id: Option<String>,
    /// For Direktvermarktung switches: the agreed Anzulegender Wert ct/kWh.
    pub direktverm_aw_ct: Option<rust_decimal::Decimal>,
}

/// `POST /api/v1/anlagen/{tr_id}/switch-veraeusserungsform`
///
/// **§21b EEG 2023 — Veräußerungsform Wechsel.**
///
/// Switches the plant between Einspeisevergütung (§21) and Direktvermarktung (§20).
///
/// Rules enforced by `eeg_billing::direktverm::validate_switch_to_vergütung`:
/// - Plants > 100 kW (mandatory Direktvermarktung) cannot switch back to Einspeisevergütung.
/// - Plants can only switch once per calendar month (§21b / §21c EEG 2023).
/// - The effective date must be the 1st of a calendar month.
///
/// On success: updates `settlement_model`, `direktverm_mp_id`, `direktverm_aw_ct`,
/// and `last_veraeusserungsform_switch` on the plant record.
pub async fn post_switch_veraeusserungsform(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
    Json(req): Json<VeraeusserungsformWechselRequest>,
) -> impl IntoResponse {
    use eeg_billing::EegGesetz;
    use eeg_billing::direktverm::{SwitchBlockedReason, validate_switch_to_vergütung};
    use time::format_description::well_known::Iso8601;

    let anlage = match fetch_anlage(&pool, &cfg.tenant, &tr_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let effective_date = match time::Date::parse(&req.effective_date, &Iso8601::DEFAULT) {
        Ok(d) => d,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                "invalid effective_date — use ISO 8601 format (YYYY-MM-DD)",
            )
                .into_response();
        }
    };

    if effective_date.day() != 1 {
        return (
            StatusCode::BAD_REQUEST,
            "effective_date must be the 1st of a calendar month (§21c EEG 2023)",
        )
            .into_response();
    }

    let eeg_gesetz = EegGesetz::from_db_year(anlage.eeg_gesetz).unwrap_or(EegGesetz::Eeg2023);

    // Only validate the switch-to-Vergütung direction (mandatory plants cannot switch back).
    // Switching to Direktvermarktung is always allowed.
    let is_switching_to_verguetung =
        matches!(req.new_model.as_str(), "FEED_IN_TARIFF" | "VERGUETUNG");

    if is_switching_to_verguetung
        && let Err(reason) = validate_switch_to_vergütung(
            anlage.leistung_kwp,
            eeg_gesetz,
            effective_date,
            anlage.last_veraeusserungsform_switch,
        )
    {
        let msg = match reason {
            SwitchBlockedReason::PflichtgemasseDirektvermarktung => {
                "plant is subject to mandatory Direktvermarktung (§20 EEG 2023 — >100 kW) and cannot switch back to Einspeisevergütung"
            }
            SwitchBlockedReason::AlreadySwitchedThisMonth { last_switch } => &format!(
                "already switched this calendar month (last switch: {last_switch}) — §21b EEG 2023 allows only one switch per month"
            ),
        };
        return (StatusCode::UNPROCESSABLE_ENTITY, msg.to_owned()).into_response();
    }

    let new_model = match req.new_model.as_str() {
        "FEED_IN_TARIFF" | "VERGUETUNG" => "FEED_IN_TARIFF",
        "MARKET_PREMIUM" | "DIREKTVERMARKTUNG" => "MARKET_PREMIUM",
        other => {
            return (
                StatusCode::BAD_REQUEST,
                format!("unsupported model: {other}"),
            )
                .into_response();
        }
    };

    match sqlx::query(
        r"UPDATE eeg_anlagen
          SET settlement_model               = $3,
              direktverm_mp_id               = $4,
              direktverm_aw_ct               = $5,
              last_veraeusserungsform_switch  = $6,
              updated_at                     = now()
          WHERE tr_id = $1 AND tenant = $2",
    )
    .bind(&tr_id)
    .bind(&cfg.tenant)
    .bind(new_model)
    .bind(&req.direktvermarkter_mp_id)
    .bind(req.direktverm_aw_ct)
    .bind(effective_date)
    .execute(&pool)
    .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            // ── §21c EEG 2023: emit notification CloudEvent to NB ─────────────
            // §21c: operator must notify the NB of the switch by end of the calendar month.
            // We emit de.eeg.veraeusserungsform.gewechselt to the ERP webhook which is
            // expected to forward it to the GPKE process handler (makod PID 55022/55023).
            let ce_id =
                emit_veraeusserungsform_ce(&cfg, &tr_id, new_model, &req.effective_date).await;
            // Record the notification timestamp (best-effort — failure does not block the switch).
            if ce_id.is_some() {
                let _ = sqlx::query(
                    "UPDATE eeg_anlagen
                     SET veraeusserungsform_notification_sent_at = now()
                     WHERE tr_id = $1 AND tenant = $2",
                )
                .bind(&tr_id)
                .bind(&cfg.tenant)
                .execute(&pool)
                .await;
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "tr_id": tr_id,
                    "new_model": new_model,
                    "effective_date": req.effective_date,
                    "notification_sent": ce_id.is_some(),
                    "note": format!(
                        "§21b EEG 2023 Veräußerungsform Wechsel to {} recorded. \
                         §21c notification {}.",
                        new_model,
                        if ce_id.is_some() { "dispatched" } else { "pending — configure erp_webhook_url" }
                    )
                })),
            )
                .into_response()
        }
        Ok(_) => (StatusCode::NOT_FOUND, format!("plant {tr_id} not found")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Emit `de.eeg.veraeusserungsform.gewechselt` CloudEvent for §21c EEG 2023.
///
/// The ERP webhook is expected to forward this to the GPKE process handler
/// (makod, PID 55022 Wechsel Marktrollen / PID 55023 Wechselbestätigung).
async fn emit_veraeusserungsform_ce(
    cfg: &EinsdConfig,
    tr_id: &str,
    new_model: &str,
    effective_date: &str,
) -> Option<uuid::Uuid> {
    let webhook_url = cfg.erp_webhook_url.as_deref()?;
    let ce_id = uuid::Uuid::new_v4();
    let now = time::OffsetDateTime::now_utc();
    let payload = serde_json::json!({
        "specversion": "1.0",
        "type": "de.eeg.veraeusserungsform.gewechselt",
        "source": format!("urn:einsd:tenant:{}", cfg.tenant),
        "id": ce_id.to_string(),
        "time": now.to_string(),
        "subject": tr_id,
        "datacontenttype": "application/json",
        "data": {
            "tr_id": tr_id,
            "new_model": new_model,
            "effective_date": effective_date,
            "legal_basis": "§21c EEG 2023",
            "deadline": "End of calendar month of effective_date"
        }
    });
    let client = reqwest::Client::new();
    let body = serde_json::to_string(&payload).unwrap_or_default();
    let mut req = client
        .post(webhook_url)
        .header("Content-Type", "application/cloudevents+json")
        .body(body.clone());
    if let Some(secret) = cfg.erp_hmac_secret.as_deref() {
        let sig = mako_service::webhook::hmac_hex(secret.as_bytes(), body.as_bytes());
        req = req.header("X-Mako-Signature", format!("sha256={sig}"));
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => Some(ce_id),
        Ok(resp) => {
            tracing::warn!(tr_id, status = %resp.status(), "§21c CE delivery failed");
            None
        }
        Err(e) => {
            tracing::warn!(tr_id, error = %e, "§21c CE error");
            None
        }
    }
}

// ── §22 MessZV — Correction Settlement ───────────────────────────────────────

/// Request body for `POST /api/v1/anlagen/{tr_id}/settlements/{year}/{month}/correction`.
#[derive(Debug, serde::Deserialize)]
pub struct CorrectionSettleRequest {
    /// Corrected Einspeisemenge kWh.
    pub einspeisemenge_kwh: Option<rust_decimal::Decimal>,
    /// Corrected EPEX average ct/kWh (for Direktvermarktung / Post-EEG).
    pub epex_avg_ct_kwh: Option<rust_decimal::Decimal>,
    /// Reason for the correction.
    pub reason: eeg_billing::scheme::CorrectionReason,
    /// Free-text explanation for audit trail.
    pub reason_detail: Option<String>,
}

/// `POST /api/v1/anlagen/{tr_id}/settlements/{year}/{month}/correction`
///
/// **§22 MessZV — Correction Settlement.**
///
/// Creates a correction receipt that supersedes the original settlement for the
/// given billing period. The original receipt is preserved for audit trail.
///
/// Use cases:
/// - Corrected meter reading arrives (§22 MessZV).
/// - Tariff error discovered.
/// - MaStR registration retroactively confirmed (retroactive §52 sanction removal).
/// - Capacity correction.
///
/// The correction stores `SettlementType::Correction { original_id, reason }` for
/// traceability per §22 MessZV.
pub async fn post_correction_settle(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<EinsdConfig>>,
    Path((tr_id, year, month)): Path<(String, i16, i16)>,
    Json(req): Json<CorrectionSettleRequest>,
) -> impl IntoResponse {
    let anlage = match fetch_anlage(&pool, &cfg.tenant, &tr_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Fetch the original receipt to get its ID for the traceability link.
    let original_id: Option<String> = sqlx::query_scalar(
        r"SELECT id::text FROM settlement_receipts
          WHERE tr_id = $1 AND tenant = $2 AND billing_year = $3 AND billing_month = $4
          ORDER BY settled_at DESC LIMIT 1",
    )
    .bind(&tr_id)
    .bind(&cfg.tenant)
    .bind(year)
    .bind(month)
    .fetch_optional(&pool)
    .await
    .ok()
    .flatten();

    let original_id_str = original_id
        .clone()
        .unwrap_or_else(|| format!("{tr_id}/{year}/{month}"));

    let epex_avg_ct_kwh = match req.epex_avg_ct_kwh {
        Some(p) => Some(p),
        None => match fetch_epex_price(&pool, year, month).await {
            Ok(p) => p,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
    };

    let einspeisemenge_kwh = req.einspeisemenge_kwh;

    let input = build_settle_input(
        &cfg.tenant,
        &anlage,
        year,
        month,
        SettleOverrides {
            einspeisemenge_kwh,
            epex_avg_ct_kwh,
            managementpraemie_ct_override: None,
            einspeisemanagement_kwh: None,
            negative_price_quarter_hours: None,
            // §22 MessZV: correction receipt linked to original
            correction_of: original_id
                .as_deref()
                .and_then(|s| uuid::Uuid::parse_str(s).ok()),
            jahresmarktwert_ct_kwh: None,
        },
    );

    match run_settlement(&pool, input).await {
        Ok(result) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "id": result.id,
                "original_id": original_id_str,
                "correction_reason": format!("{:?}", req.reason),
                "reason_detail": req.reason_detail,
                "billing_year": year,
                "billing_month": month,
                "settlement_eur": result.settlement_eur,
                "status": result.status,
                "note": "§22 MessZV correction receipt created. Original receipt preserved for audit trail.",
            })),
        ).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

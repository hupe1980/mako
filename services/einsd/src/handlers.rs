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
        AnlageUpsertRequest, AnlagenQuery, SettleInput, decommission_anlage, fetch_anlage,
        fetch_epex_price, list_anlagen, list_expiring, list_settlement_receipts, list_unsettled,
        lookup_verguetungssatz, run_settlement, upsert_anlage, upsert_epex_price, zusammenlegen,
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

    let input = SettleInput {
        tr_id: tr_id.clone(),
        tenant: cfg.tenant.clone(),
        billing_year: year,
        billing_month: month,
        einspeisemenge_kwh,
        epex_avg_ct_kwh,
        settlement_model: anlage.settlement_model.clone(),
        verguetungssatz_ct: anlage.verguetungssatz_ct,
        direktverm_aw_ct: anlage.direktverm_aw_ct,
        mieter_zuschlag_ct: anlage.mieter_zuschlag_ct,
        flex_praemie_ct_kwh: anlage.flex_praemie_ct_kwh,
        // §20 Abs. 3 EEG 2023 Managementprämie: 0.4 ct/kWh statutory default for
        // Direktvermarktung plants. Callers may override via SettleTriggerRequest.
        // Reduced to 0.2 ct/kWh for plants >100 MW (§20 Abs. 3 Nr. 1).
        managementpraemie_ct: if matches!(
            anlage.settlement_model.as_str(),
            "DIREKTVERMARKTUNG" | "AUSSCHREIBUNG"
        ) {
            Some(req.managementpraemie_ct_override.unwrap_or_else(|| {
                use rust_decimal_macros::dec;
                if anlage.leistung_kwp > dec!(100_000) {
                    dec!(0.2)
                } else {
                    dec!(0.4)
                }
            }))
        } else {
            None
        },
        // KWKG: pass accumulated kWh and max limit for hour-limit enforcement.
        kwk_strom_kwh_gesamt: if anlage.settlement_model == "KWKG_ZUSCHLAG" {
            anlage.kwk_strom_kwh_gesamt
        } else {
            None
        },
        // KWKG §8 KWKG 2023: max total kWh = rated_kW × full_load_hours.
        // E.g. 2.5 MW × 30 000 h = 75 000 000 kWh cap.
        kwk_max_kwh: anlage
            .kwk_foerderdauer_h
            .map(|h| rust_decimal::Decimal::from(h) * anlage.leistung_kwp),
        // sanktion is computed in run_settlement from mastr_registriert + EegGesetz.
        sanktion: None, // derived from mastr_registriert in run_settlement
        mastr_registriert: anlage.mastr_registriert,
        kwh_during_negative_epex: None,
        inbetriebnahme: Some(anlage.inbetriebnahme),
        // Installed capacity — used for §27 threshold (≥100 kWp) and auto Managementprämie.
        leistung_kwp: Some(anlage.leistung_kwp),
        // Förderdauer end date — triggers FoerderungBeendet when billing_date > foerderendedatum.
        foerderendedatum: Some(anlage.foerderendedatum),
        // First day of the billing month for auto FoerderungBeendet detection.
        billing_date: time::Date::from_calendar_date(
            year as i32,
            time::Month::try_from(month as u8).unwrap_or(time::Month::January),
            1,
        )
        .ok(),
        eeg_gesetz: anlage.eeg_gesetz,
        erzeugungsart: anlage.erzeugungsart.clone(),
    };

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

    // §22 EEG 2023: foerderendedatum = repowering_datum + 20 years.
    let foerderendedatum_neu = match repowering_datum.replace_year(repowering_datum.year() + 20) {
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
            mastr_registriert = true, \
            mastr_nummer      = $3, \
            mastr_datum       = $4, \
            status            = CASE WHEN status = 'angemeldet' THEN 'aktiv' ELSE status END, \
            updated_at        = now() \
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

    for anlage in &plants {
        // Auto-fetch Einspeisemenge from edmd.
        let einspeisemenge_kwh =
            fetch_einspeisemenge_from_edmd(&cfg, &anlage.malo_id, year, month).await;

        let mgmt_ct = if matches!(
            anlage.settlement_model.as_str(),
            "DIREKTVERMARKTUNG" | "AUSSCHREIBUNG"
        ) {
            use rust_decimal_macros::dec;
            Some(if anlage.leistung_kwp > dec!(100_000) {
                dec!(0.2)
            } else {
                dec!(0.4)
            })
        } else {
            None
        };

        let input = SettleInput {
            tr_id: anlage.tr_id.clone(),
            tenant: cfg.tenant.clone(),
            billing_year: year,
            billing_month: month,
            einspeisemenge_kwh,
            epex_avg_ct_kwh,
            settlement_model: anlage.settlement_model.clone(),
            verguetungssatz_ct: anlage.verguetungssatz_ct,
            direktverm_aw_ct: anlage.direktverm_aw_ct,
            mieter_zuschlag_ct: anlage.mieter_zuschlag_ct,
            flex_praemie_ct_kwh: anlage.flex_praemie_ct_kwh,
            managementpraemie_ct: mgmt_ct,
            kwk_strom_kwh_gesamt: if anlage.settlement_model == "KWKG_ZUSCHLAG" {
                anlage.kwk_strom_kwh_gesamt
            } else {
                None
            },
            // §8 KWKG 2023: max total kWh = rated_kW × full_load_hours
            kwk_max_kwh: anlage
                .kwk_foerderdauer_h
                .map(|h| rust_decimal::Decimal::from(h) * anlage.leistung_kwp),
            sanktion: None, // derived from mastr_registriert in run_settlement
            mastr_registriert: anlage.mastr_registriert,
            kwh_during_negative_epex: None,
            inbetriebnahme: Some(anlage.inbetriebnahme),
            leistung_kwp: Some(anlage.leistung_kwp),
            foerderendedatum: Some(anlage.foerderendedatum),
            billing_date: time::Date::from_calendar_date(
                year as i32,
                time::Month::try_from(month as u8).unwrap_or(time::Month::January),
                1,
            )
            .ok(),
            eeg_gesetz: anlage.eeg_gesetz,
            erzeugungsart: anlage.erzeugungsart.clone(),
        };

        if req.dry_run {
            // Dry-run: just count, don't persist.
            match einspeisemenge_kwh {
                None => skipped_no_data += 1,
                Some(_) => settled += 1,
            }
            continue;
        }

        match run_settlement(&pool, input).await {
            Ok(result) => {
                match result.status.as_str() {
                    "calculated" | "foerderung_beendet" => {
                        settled += 1;
                        if let Some(eur) = result.settlement_eur {
                            total_settlement_eur += eur;
                        }
                        // Emit CloudEvent (best-effort).
                        if result.status == "calculated" {
                            let ce_type = match anlage.settlement_model.as_str() {
                                "DIREKTVERMARKTUNG" | "AUSSCHREIBUNG" => {
                                    "de.eeg.marktpraemie.berechnet"
                                }
                                _ => "de.eeg.verguetung.berechnet",
                            };
                            emit_settlement_ce(
                                &cfg,
                                ce_type,
                                &anlage.tr_id,
                                &anlage.malo_id,
                                &result,
                                year,
                                month,
                            )
                            .await;
                        }
                        if results.len() < 100 {
                            results.push(serde_json::json!({
                                "tr_id": anlage.tr_id,
                                "status": result.status,
                                "settlement_eur": result.settlement_eur,
                                "einspeisemenge_kwh": result.einspeisemenge_kwh,
                            }));
                        }
                    }
                    "no_data" => skipped_no_data += 1,
                    "price_missing" => skipped_price_missing += 1,
                    _ => errors += 1,
                }
            }
            Err(e) => {
                tracing::warn!(tr_id = %anlage.tr_id, error = %e, "batch_settle: settlement error");
                errors += 1;
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

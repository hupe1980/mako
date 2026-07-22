//! HTTP handlers for `netzbilanzd`.

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::makod_client::MakodClient;
use mako_markt::marktd_client::MarktdClient;
use serde::Deserialize;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

use crate::billing::{BillingPosition, BillingRunRequest, run_billing_internal};
use crate::config::NetzbilanzConfig;
use crate::pg::{
    AuditQuery, UpsertFremdkostenRequest, UpsertKostenblattRequest, approve_and_dispatch,
    billing_history_for_malo, billing_summary, dispatch_batch, fetch_draft, fetch_fremdkosten,
    fetch_kostenblatt, list_audit, list_drafts_pg, list_kostenblatt, list_kostenblatt_gaps,
    mark_draft_disputed, mark_draft_paid, mark_kostenblatt_submitted, reject_draft_pg,
    upsert_fremdkosten, upsert_kostenblatt,
};

// ── CloudEvent helper ─────────────────────────────────────────────────────────

/// Fire-and-forget CloudEvent POST to the configured ERP webhook URL.
/// Errors are logged but never propagated — the billing pipeline must succeed
/// regardless of ERP webhook availability.
fn emit_cloud_event(
    client: Arc<reqwest::Client>,
    webhook_url: String,
    ce_type: &'static str,
    payload: serde_json::Value,
) {
    let ce_type = ce_type.to_owned();
    tokio::spawn(async move {
        let body = serde_json::json!({
            "specversion": "1.0",
            "type":        ce_type,
            "source":      "netzbilanzd",
            "id":          uuid::Uuid::new_v4().to_string(),
            "time":        time::OffsetDateTime::now_utc()
                               .format(&time::format_description::well_known::Rfc3339)
                               .unwrap_or_default(),
            "data":        payload,
        });
        let _ = client
            .post(&webhook_url)
            .header("Content-Type", "application/cloudevents+json")
            .json(&body)
            .send()
            .await;
    });
}

// ── POST /api/v1/billing/run ──────────────────────────────────────────────────

/// `POST /api/v1/billing/run`
///
/// Generates invoice drafts for the given MaLos in the specified billing period.
/// Each draft is stored with `status = 'draft'` and validated against
/// `invoic-checker` checks 1–3 before storage.
pub async fn run_billing(
    Extension(pool): Extension<PgPool>,
    Extension(marktd): Extension<Arc<MarktdClient>>,
    Extension(cfg): Extension<Arc<NetzbilanzConfig>>,
    Extension(http_client): Extension<Arc<reqwest::Client>>,
    Json(req): Json<BillingRunRequest>,
) -> impl IntoResponse {
    match run_billing_internal(&pool, &marktd, &cfg.tenant, cfg.vnb_mp_id.as_deref(), req).await {
        Ok(ids) => {
            // Emit CloudEvent for each drafted invoice (fire-and-forget).
            if let Some(ref url) = cfg.erp_webhook_url {
                for id in &ids {
                    emit_cloud_event(
                        Arc::clone(&http_client),
                        url.clone(),
                        "de.netzbilanz.invoic.drafted",
                        serde_json::json!({ "draft_id": id, "tenant": cfg.tenant }),
                    );
                }
            }
            (
                StatusCode::CREATED,
                Json(serde_json::json!({ "draft_ids": ids })),
            )
                .into_response()
        }
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

// ── GET /api/v1/billing/drafts ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DraftsQuery {
    pub status: Option<String>,
    pub malo_id: Option<String>,
    pub nb_mp_id: Option<String>,
    pub limit: Option<i64>,
}

/// `GET /api/v1/billing/drafts`
pub async fn list_drafts(
    Extension(pool): Extension<PgPool>,
    Query(q): Query<DraftsQuery>,
) -> impl IntoResponse {
    match list_drafts_pg(
        &pool,
        q.status.as_deref(),
        q.malo_id.as_deref(),
        q.nb_mp_id.as_deref(),
        q.limit.unwrap_or(100).min(1000),
    )
    .await
    {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── GET /api/v1/billing/drafts/{id} ──────────────────────────────────────────

/// `GET /api/v1/billing/drafts/{id}`
pub async fn get_draft(
    Extension(pool): Extension<PgPool>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match fetch_draft(&pool, id).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── PUT /api/v1/billing/drafts/{id}/dispatch ─────────────────────────────────

/// `PUT /api/v1/billing/drafts/{id}/dispatch`
///
/// Validates the draft via `invoic-checker`, then dispatches it via `makod`
/// if the check outcome is not `Dispute`.  Updates status to `dispatched`.
pub async fn dispatch_draft(
    Extension(pool): Extension<PgPool>,
    Extension(makod): Extension<Arc<MakodClient>>,
    Extension(cfg): Extension<Arc<NetzbilanzConfig>>,
    Extension(http_client): Extension<Arc<reqwest::Client>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match approve_and_dispatch(&pool, &makod, id).await {
        Ok(ref_id) => {
            if let Some(ref url) = cfg.erp_webhook_url {
                emit_cloud_event(
                    Arc::clone(&http_client),
                    url.clone(),
                    "de.netzbilanz.invoic.dispatched",
                    serde_json::json!({ "draft_id": id, "dispatch_ref": ref_id, "tenant": cfg.tenant }),
                );
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({ "dispatch_ref": ref_id })),
            )
                .into_response()
        }
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

// ── PUT /api/v1/billing/drafts/{id}/reject ───────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct RejectRequest {
    pub reason: String,
}

/// `PUT /api/v1/billing/drafts/{id}/reject`
pub async fn reject_draft(
    Extension(pool): Extension<PgPool>,
    Path(id): Path<Uuid>,
    Json(req): Json<RejectRequest>,
) -> impl IntoResponse {
    match reject_draft_pg(&pool, id, &req.reason).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── N6: MMM auto-run (Mehr-/Mindermenge claim automation) ────────────────────

/// Request body for `POST /api/v1/billing/mmm-run/{malo_id}`.
#[derive(Debug, Deserialize)]
pub struct MmmAutoRunRequest {
    pub nb_mp_id: String,
    pub lf_mp_id: String,
    /// Billing year (calendar year).
    pub period_year: i32,
    /// Billing month (1–12).
    pub period_month: u8,
    /// Invoice issue date (`YYYY-MM-DD`). Defaults to today.
    pub invoice_date: Option<String>,
    /// Payment due date (`YYYY-MM-DD`). Defaults to 30 days after invoice_date.
    pub due_date: Option<String>,
    /// Rechnungsnummer prefix (auto-generated when absent).
    pub rechnungsnummer_prefix: Option<String>,
    /// Override: supply Mehrmengen price ct/kWh instead of auto-fetching from marktd.
    pub mehr_preis_ct_per_kwh: Option<rust_decimal::Decimal>,
    /// Override: supply Mindermengen price ct/kWh instead of auto-fetching from marktd.
    pub minder_preis_ct_per_kwh: Option<rust_decimal::Decimal>,
    /// SLP Lastprofil designation (e.g. "H0"). Auto-derived from marktd when absent.
    pub lastprofil: Option<String>,
}

/// `POST /api/v1/billing/mmm-run/{malo_id}`
///
/// **Automatic Mehr-/Mindermenge (MMM) billing — N6 (hard cut).**
///
/// Operators need only supply `nb_mp_id`, `lf_mp_id`, `period_year`,
/// `period_month`.  Everything else is auto-fetched:
///
/// - `profil_kwh` ← `edmd GET /api/v1/imbalance/{malo_id}/{year}/{month}`
///   (`nb_quantity_kwh` = NB-settled SLP profile consumption)
/// - `mehr_preis` / `minder_preis` ← `marktd GET /api/v1/mmm-preise/strom/{year}/{month}`
///   (when not overridden in request)
///
/// Then runs `run_billing_internal` → INVOIC 31002 draft + self-validation.
pub async fn post_mmm_auto_run(
    Extension(pool): Extension<PgPool>,
    Extension(marktd): Extension<Arc<MarktdClient>>,
    Extension(cfg): Extension<Arc<crate::config::NetzbilanzConfig>>,
    Extension(http_client): Extension<Arc<reqwest::Client>>,
    Path(malo_id): Path<String>,
    Json(req): Json<MmmAutoRunRequest>,
) -> impl IntoResponse {
    use rust_decimal::Decimal;
    use time::{Date, Month};

    // ── Fetch imbalance from edmd ─────────────────────────────────────────────
    let edmd_url = match cfg.edmd_url.as_deref() {
        Some(u) => u.trim_end_matches('/').to_owned(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "edmd_url not configured in netzbilanzd.toml",
            )
                .into_response();
        }
    };

    let url = format!(
        "{edmd_url}/api/v1/imbalance/{}/{}/{}",
        malo_id, req.period_year, req.period_month
    );
    let mut http_req = http_client.get(&url);
    if let Some(key) = cfg.edmd_api_key.as_deref() {
        http_req = http_req.bearer_auth(key);
    }

    let imbalance_body: serde_json::Value = match http_req.send().await {
        Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND => {
            return (
                StatusCode::NOT_FOUND,
                format!(
                    "no edmd imbalance for MaLo {malo_id} {}/{}",
                    req.period_year, req.period_month
                ),
            )
                .into_response();
        }
        Ok(r) if !r.status().is_success() => {
            return (StatusCode::BAD_GATEWAY, format!("edmd: {}", r.status())).into_response();
        }
        Ok(r) => r.json().await.unwrap_or_default(),
        Err(e) => return (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    };

    let profil_kwh: Decimal = imbalance_body
        .get("nb_quantity_kwh")
        .and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse().ok())
                .or_else(|| v.as_f64().and_then(|f| Decimal::try_from(f).ok()))
        })
        .unwrap_or(Decimal::ZERO);

    if profil_kwh == Decimal::ZERO {
        return (StatusCode::UNPROCESSABLE_ENTITY, "nb_quantity_kwh is zero").into_response();
    }

    // ── Build dates ───────────────────────────────────────────────────────────
    let today = time::OffsetDateTime::now_utc().date().to_string();
    let invoice_date = req.invoice_date.clone().unwrap_or_else(|| today.clone());
    let due_date = req.due_date.clone().unwrap_or_else(|| {
        (time::OffsetDateTime::now_utc() + time::Duration::days(30))
            .date()
            .to_string()
    });
    let period_from = format!("{:04}-{:02}-01", req.period_year, req.period_month);
    let period_to = {
        let m = Month::try_from(req.period_month).unwrap_or(Month::December);
        let last = if req.period_month == 12 {
            Date::from_calendar_date(req.period_year + 1, Month::January, 1)
                .map(|d| d.previous_day().unwrap_or(d))
        } else {
            Date::from_calendar_date(
                req.period_year,
                Month::try_from(req.period_month + 1).unwrap(),
                1,
            )
            .map(|d| d.previous_day().unwrap_or(d))
        };
        last.unwrap_or_else(|_| Date::from_calendar_date(req.period_year, m, 28).unwrap())
            .to_string()
    };
    let prefix = req.rechnungsnummer_prefix.clone().unwrap_or_else(|| {
        format!(
            "MMM-{malo_id}-{:04}-{:02}",
            req.period_year, req.period_month
        )
    });

    let billing_req = BillingRunRequest {
        nb_mp_id: req.nb_mp_id.clone(),
        lf_mp_id: req.lf_mp_id.clone(),
        invoice_date,
        due_date,
        rechnungsnummer_prefix: prefix,
        positions: vec![BillingPosition {
            malo_id: malo_id.clone(),
            period_from,
            period_to,
            billing_type: "mmm".to_owned(),
            arbeitsmenge_kwh: None,
            arbeitspreis_ct_per_kwh: None,
            arbeitsmenge_ht_kwh: None,
            arbeitspreis_ht_ct_per_kwh: None,
            arbeitsmenge_nt_kwh: None,
            arbeitspreis_nt_ct_per_kwh: None,
            spitzenleistung_kw: None,
            leistungspreis_eur_per_kw: None,
            ka_satz_ct_per_kwh: None,
            profil_kwh: Some(profil_kwh),
            mehr_preis_ct_per_kwh: req.mehr_preis_ct_per_kwh,
            minder_preis_ct_per_kwh: req.minder_preis_ct_per_kwh,
            lastprofil: req.lastprofil.clone(),
            msb_mp_id: None,
            grundgebuehr_eur_per_month: None,
            billing_months: None,
            messdienstleistung_eur: None,
        }],
    };

    match run_billing_internal(
        &pool,
        &marktd,
        &cfg.tenant,
        cfg.vnb_mp_id.as_deref(),
        billing_req,
    )
    .await
    {
        Ok(draft_ids) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "draft_ids": draft_ids,
                "malo_id": malo_id,
                "period_year": req.period_year,
                "period_month": req.period_month,
                "profil_kwh_from_edmd": profil_kwh,
                "source": "edmd_auto",
            })),
        )
            .into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

// ── N4: Kostenblatt REST API (Redispatch 2.0, BK6-20-061) ────────────────────

#[derive(Debug, Deserialize)]
pub struct KostenblattListQuery {
    pub year: i16,
    pub month: i16,
    pub status: Option<String>,
}

/// `PUT /api/v1/redispatch/kostenblatt/{activation_id}`
///
/// Create or update a Kostenblatt entry for a Redispatch 2.0 activation.
///
/// `einsatzkosten_eur = dispatch_kwh × arbeitspreis_eur_per_kwh` is stored as
/// a generated column.  `kosten_json` optionally carries the full typed
/// `rubo4e::current::Kosten` payload for CIM XML export.
pub async fn put_kostenblatt(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<NetzbilanzConfig>>,
    Path(activation_id): Path<String>,
    Json(req): Json<UpsertKostenblattRequest>,
) -> impl IntoResponse {
    let einsatzkosten = req.dispatch_kwh * req.arbeitspreis_eur_per_kwh;
    match upsert_kostenblatt(&pool, &cfg.tenant, &activation_id, &req).await {
        Ok(id) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "id": id,
                "activation_id": activation_id,
                "einsatzkosten_eur": einsatzkosten.to_string(),
            })),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/redispatch/kostenblatt/{activation_id}`
pub async fn get_kostenblatt(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<NetzbilanzConfig>>,
    Path(activation_id): Path<String>,
) -> impl IntoResponse {
    match fetch_kostenblatt(&pool, &activation_id, &cfg.tenant).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/redispatch/kostenblatt?year=&month=&status=`
///
/// List Kostenblatt records for a billing period.
/// `?status=pending` → records due for 15th-of-month submission.
pub async fn list_kostenblatt_handler(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<NetzbilanzConfig>>,
    Query(q): Query<KostenblattListQuery>,
) -> impl IntoResponse {
    match list_kostenblatt(&pool, &cfg.tenant, q.year, q.month, q.status.as_deref()).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `POST /api/v1/redispatch/kostenblatt/submit/{year}/{month}`
///
/// Mark all `pending` Kostenblatt records for the month as `submitted` and
/// return the aggregated summary for ERP / manual ÜNB submission.
///
/// **BK6-20-061:** Kostenblatt due 15th of following month.
/// The operator reviews the summary and dispatches the CIM XML to the ÜNB
/// via `makod` or direct AS4.
pub async fn post_submit_kostenblatt(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<NetzbilanzConfig>>,
    Path((year, month)): Path<(i16, i16)>,
) -> impl IntoResponse {
    use rust_decimal::Decimal;

    let pending = match list_kostenblatt(&pool, &cfg.tenant, year, month, Some("pending")).await {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    if pending.is_empty() {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "submitted": 0,
                "message": format!("no pending Kostenblatt records for {year}-{month:02}"),
            })),
        )
            .into_response();
    }

    let mut submitted = 0u32;
    let mut total_einsatzkosten = Decimal::ZERO;
    let mut summaries: Vec<serde_json::Value> = Vec::new();

    for record in &pending {
        let einsatzkosten = record
            .einsatzkosten_eur
            .unwrap_or_else(|| record.dispatch_kwh * record.arbeitspreis_eur_per_kwh);
        total_einsatzkosten += einsatzkosten;

        // Mark as submitted (dispatch ref = auto-generated ref for ÜNB)
        let dispatch_ref = format!(
            "KB-{year}-{month:02}-{}",
            &record.activation_id[..record.activation_id.len().min(8)]
        );
        let _ = mark_kostenblatt_submitted(&pool, record.id, &dispatch_ref).await;
        submitted += 1;

        summaries.push(serde_json::json!({
            "activation_id": record.activation_id,
            "tr_id": record.tr_id,
            "dispatch_kwh": record.dispatch_kwh,
            "einsatzkosten_eur": einsatzkosten,
            "dispatch_ref": dispatch_ref,
        }));
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "submitted": submitted,
            "total_einsatzkosten_eur": total_einsatzkosten.to_string(),
            "period": format!("{year}-{month:02}"),
            "positions": summaries,
        })),
    )
        .into_response()
}

// ── N5: Kostenblatt edmd auto-compute (Redispatch 2.0, BK6-20-061) ────────────

/// Request body for
/// `POST /api/v1/redispatch/kostenblatt/{activation_id}/compute`.
///
/// The endpoint auto-fetches the dispatched energy quantity from `edmd` using
/// the activation window, computes `Einsatzkosten = dispatch_kwh × arbeitspreis_eur_per_kwh`,
/// generates a typed BO4E `Kosten`/`KostenBlock`/`KostenPosition` JSON payload,
/// and upserts the Kostenblatt record.
///
/// ## Energy source priority (BK6-20-061 §4.2)
///
/// 1. `dispatch_kwh_override` — manual operator value (bypasses edmd entirely)
/// 2. **`edmd Lastgang sum`** — sum of 15-min intervals in the exact activation window
///    (most precise; mandatory for short activations; source = `"lastgang_sum"`)
/// 3. `edmd billing-period` — monthly aggregate fallback when Lastgang absent
///    (source = `"billing_period"`; triggers a warning log)
///
/// For a 15-minute Redispatch activation, using the monthly billing-period
/// aggregate would give the wrong value by several orders of magnitude.
/// The Lastgang sum is the correct BK6-20-061 §4.2 approach.
#[derive(Debug, serde::Deserialize)]
pub struct KostenblattComputeRequest {
    /// `TechnischeRessource`-ID of the dispatched resource.
    pub tr_id: String,
    /// 11-digit MaLo-ID of the resource's grid connection point.
    pub malo_id: String,
    /// Calendar year of the activation month (for Kostenblatt period).
    pub period_year: i16,
    /// Calendar month of the activation (1–12).
    pub period_month: i16,
    /// ÜNB MP-ID receiving the Kostenblatt.
    pub uenb_mp_id: String,
    /// VNB MP-ID (sender).
    pub vnb_mp_id: String,
    /// UTC activation start — RFC 3339 e.g. `"2026-01-15T10:00:00Z"`.
    pub activation_start_utc: String,
    /// UTC activation end — RFC 3339 e.g. `"2026-01-15T10:15:00Z"`.
    pub activation_end_utc: String,
    /// Contract rate EUR/kWh from the Redispatch 2.0 bilateral agreement.
    pub arbeitspreis_eur_per_kwh: rust_decimal::Decimal,
    /// Manual override for `dispatch_kwh`.  When set, `edmd` is **not** queried.
    /// Use when the operator has a verified meter reading outside `edmd`.
    pub dispatch_kwh_override: Option<rust_decimal::Decimal>,
}

/// `POST /api/v1/redispatch/kostenblatt/{activation_id}/compute`
///
/// **N5 — Redispatch Kostenblatt energy-quantity link (BK6-20-061 §4.2).**
///
/// Steps:
/// 1. Parse + validate the activation window (RFC 3339 UTC timestamps).
/// 2. Fetch dispatched energy from `edmd Lastgang` (15-min sum — primary) or
///    `edmd billing-period` (monthly aggregate — fallback).
///    Override with `dispatch_kwh_override` to skip edmd entirely.
/// 3. Compute `Einsatzkosten = dispatch_kwh × arbeitspreis_eur_per_kwh`.
/// 4. Build typed BO4E `Kosten`/`KostenBlock`/`KostenPosition` JSON for CIM export.
/// 5. Upsert the `kostenblatt_records` row (idempotent on `activation_id` + `tr_id`).
/// 6. Emit `de.netzbilanz.kostenblatt.computed` CloudEvent.
///
/// Stores `activation_start_utc`, `activation_end_utc`, and `dispatch_source`
/// for audit trail and re-computation capability.
///
/// Returns `503` when `edmd_url` is not configured and no override is supplied.
#[allow(clippy::too_many_lines)]
pub async fn post_kostenblatt_compute(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<crate::config::NetzbilanzConfig>>,
    Extension(http_client): Extension<Arc<reqwest::Client>>,
    Path(activation_id): Path<String>,
    Json(req): Json<KostenblattComputeRequest>,
) -> impl IntoResponse {
    use rust_decimal::Decimal;
    use time::format_description::well_known::Rfc3339;

    // Parse activation window to typed OffsetDateTime for DB storage and validation.
    let activation_start = match time::OffsetDateTime::parse(&req.activation_start_utc, &Rfc3339) {
        Ok(t) => t,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                format!(
                    "invalid activation_start_utc '{}' — expected RFC 3339 e.g. '2026-01-15T10:00:00Z'",
                    req.activation_start_utc
                ),
            )
                .into_response();
        }
    };
    let activation_end = match time::OffsetDateTime::parse(&req.activation_end_utc, &Rfc3339) {
        Ok(t) => t,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                format!(
                    "invalid activation_end_utc '{}' — expected RFC 3339 e.g. '2026-01-15T10:15:00Z'",
                    req.activation_end_utc
                ),
            )
                .into_response();
        }
    };
    if activation_end <= activation_start {
        return (
            StatusCode::BAD_REQUEST,
            "activation_end_utc must be strictly after activation_start_utc",
        )
            .into_response();
    }

    // ── Step 1: Resolve dispatch_kwh (Lastgang → billing-period → override) ──
    //
    // Priority order per BK6-20-061 §4.2:
    //   1. `dispatch_kwh_override` — operator-supplied (e.g. from manual meter read)
    //   2. `edmd Lastgang sum`     — sum of 15-min intervals in activation window
    //                                (most precise; covers short activations correctly)
    //   3. `edmd billing-period`   — monthly aggregate (fallback when Lastgang absent)
    //
    // For a 15-minute Redispatch activation, using the billing-period monthly
    // aggregate would massively over-count (e.g. 2,500 kWh/month ≠ 2.5 kWh/15min).
    // The Lastgang sum over the exact window is the correct approach.
    let (dispatch_kwh, dispatch_source) = if let Some(override_kwh) = req.dispatch_kwh_override {
        (override_kwh, "manual_override")
    } else {
        let edmd_url = match cfg.edmd_url.as_deref() {
            Some(u) => u.trim_end_matches('/').to_owned(),
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "edmd_url not configured in netzbilanzd.toml — supply dispatch_kwh_override \
                     or configure [edmd] url in netzbilanzd.toml",
                )
                    .into_response();
            }
        };

        // ── Primary: Lastgang 15-min interval sum ────────────────────────────
        let lastgang_kwh =
            fetch_dispatch_kwh_from_lastgang(&http_client, &edmd_url, &req.malo_id, &cfg, &req)
                .await;

        if let Some(kwh) = lastgang_kwh {
            (kwh, "lastgang_sum")
        } else {
            // ── Fallback: billing-period aggregate ───────────────────────────
            // Only useful when Lastgang data is absent (e.g. SLP metering or
            // data not yet ingested).  Uses activation_start date as period key.
            let period_date = activation_start.date().to_string(); // YYYY-MM-DD
            let bp_url = format!(
                "{edmd_url}/api/v1/billing-period/{}?from={}&to={}",
                req.malo_id, period_date, period_date
            );
            let mut bp_req = http_client.get(&bp_url);
            if let Some(key) = cfg.edmd_api_key.as_deref() {
                bp_req = bp_req.bearer_auth(key);
            }
            let bp_kwh = match bp_req.send().await {
                Ok(r) if r.status().is_success() => {
                    let body: serde_json::Value = r.json().await.unwrap_or_default();
                    body.get("arbeitsmenge_kwh")
                        .and_then(decimal_from_json_value)
                        .filter(|&v| v > Decimal::ZERO)
                }
                _ => None,
            };
            match bp_kwh {
                Some(kwh) => {
                    tracing::warn!(
                        malo_id = %req.malo_id,
                        activation_start = %req.activation_start_utc,
                        "N5 Kostenblatt: Lastgang empty — using billing-period aggregate as fallback. \
                         Monthly total, not window-specific. Verify meter data in edmd."
                    );
                    (kwh, "billing_period")
                }
                None => {
                    return (
                        StatusCode::NOT_FOUND,
                        format!(
                            "no Lastgang or billing-period data for MaLo {} in activation \
                             window {} / {}. Ingest Redispatch MSCONS (PIDs 13020–13023, 13026) or \
                             supply dispatch_kwh_override.",
                            req.malo_id, req.activation_start_utc, req.activation_end_utc
                        ),
                    )
                        .into_response();
                }
            }
        }
    };

    if dispatch_kwh <= Decimal::ZERO {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "dispatch_kwh is zero for MaLo {} — no energy data recorded for the activation \
                 window. Check edmd Lastgang or supply dispatch_kwh_override.",
                req.malo_id
            ),
        )
            .into_response();
    }

    let einsatzkosten_eur = dispatch_kwh * req.arbeitspreis_eur_per_kwh;

    // ── Step 2: Build typed BO4E Kosten/KostenBlock/KostenPosition JSON ───────
    //
    // Maps to `rubo4e::current::Kosten` + nested types.
    // Stored as JSONB for CIM XML export; format follows BO4E v202607.
    let kosten_json = serde_json::json!({
        "_typ": "KOSTEN",
        "summe": [{
            "_typ": "KOSTENBLOCK",
            "kostenblockbezeichnung": "Redispatch 2.0 Einsatzkosten",
            "kostenpositionen": [{
                "_typ": "KOSTENPOSITION",
                "positionsbezeichnung": "Arbeitspreis Redispatch",
                "artikelId": req.tr_id,
                "menge": {
                    "_typ": "MENGE",
                    "wert": dispatch_kwh.to_string(),
                    "einheit": "KWH"
                },
                "einzelpreis": {
                    "_typ": "PREIS",
                    "wert": req.arbeitspreis_eur_per_kwh.to_string(),
                    "einheit": "EUR"
                },
                "betragKostenstelle": {
                    "_typ": "BETRAG",
                    "wert": einsatzkosten_eur.to_string(),
                    "waehrung": "EUR"
                },
                "zeitraum": {
                    "_typ": "ZEITRAUM",
                    "startdatum": req.activation_start_utc,
                    "enddatum": req.activation_end_utc
                }
            }]
        }],
        "aktivierungszeitraum": {
            "_typ": "ZEITRAUM",
            "startdatum": req.activation_start_utc,
            "enddatum": req.activation_end_utc
        },
        "dispatchSource": dispatch_source
    });

    // ── Step 3: Upsert the Kostenblatt record (idempotent on activation_id + tr_id) ──
    let upsert_req = UpsertKostenblattRequest {
        tr_id: req.tr_id.clone(),
        malo_id: Some(req.malo_id.clone()),
        period_year: req.period_year,
        period_month: req.period_month,
        uenb_mp_id: req.uenb_mp_id.clone(),
        vnb_mp_id: req.vnb_mp_id.clone(),
        dispatch_kwh,
        arbeitspreis_eur_per_kwh: req.arbeitspreis_eur_per_kwh,
        kosten_json: Some(kosten_json),
        activation_start_utc: Some(activation_start),
        activation_end_utc: Some(activation_end),
        dispatch_source: Some(dispatch_source.to_owned()),
    };

    let record_id = match upsert_kostenblatt(&pool, &cfg.tenant, &activation_id, &upsert_req).await
    {
        Ok(id) => id,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };

    // ── Step 4: Emit CloudEvent ───────────────────────────────────────────────
    if let Some(ref webhook_url) = cfg.erp_webhook_url {
        emit_cloud_event(
            Arc::clone(&http_client),
            webhook_url.clone(),
            "de.netzbilanz.kostenblatt.computed",
            serde_json::json!({
                "record_id":              record_id,
                "activation_id":          activation_id,
                "tr_id":                  req.tr_id,
                "malo_id":                req.malo_id,
                "period_year":            req.period_year,
                "period_month":           req.period_month,
                "dispatch_kwh":           dispatch_kwh.to_string(),
                "arbeitspreis_eur_per_kwh": req.arbeitspreis_eur_per_kwh.to_string(),
                "einsatzkosten_eur":      einsatzkosten_eur.to_string(),
                "dispatch_source":        dispatch_source,
                "activation_start_utc":   req.activation_start_utc,
                "activation_end_utc":     req.activation_end_utc,
            }),
        );
    }

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id":                       record_id,
            "activation_id":            activation_id,
            "tr_id":                    req.tr_id,
            "malo_id":                  req.malo_id,
            "dispatch_kwh":             dispatch_kwh.to_string(),
            "arbeitspreis_eur_per_kwh": req.arbeitspreis_eur_per_kwh.to_string(),
            "einsatzkosten_eur":        einsatzkosten_eur.to_string(),
            "dispatch_source":          dispatch_source,
            "activation_start_utc":     req.activation_start_utc,
            "activation_end_utc":       req.activation_end_utc,
        })),
    )
        .into_response()
}

/// Fetch the total dispatched energy (kWh) for an activation window from edmd Lastgang.
///
/// Calls `GET /api/v1/lastgang/{malo_id}?from={start}&to={end}` and sums all
/// 15-min interval `wert` values.
///
/// Handles both:
/// - Array format: `[{ "wert": "1.25", "timestamp_utc": "..." }, ...]`
/// - BO4E Lastgang format: `{ "werteliste": [{ "wert": "1.25", ... }] }`
///
/// Returns `None` when the Lastgang endpoint returns 404, an empty array, or
/// a sum of zero — the caller should fall back to billing-period in that case.
async fn fetch_dispatch_kwh_from_lastgang(
    client: &reqwest::Client,
    edmd_url: &str,
    malo_id: &str,
    cfg: &NetzbilanzConfig,
    req: &KostenblattComputeRequest,
) -> Option<rust_decimal::Decimal> {
    use rust_decimal::Decimal;
    let url = format!(
        "{edmd_url}/api/v1/lastgang/{malo_id}?from={}&to={}",
        req.activation_start_utc, req.activation_end_utc
    );
    let mut http_req = client.get(&url);
    if let Some(key) = cfg.edmd_api_key.as_deref() {
        http_req = http_req.bearer_auth(key);
    }
    let body: serde_json::Value = match http_req.send().await {
        Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND => return None,
        Ok(r) if !r.status().is_success() => {
            tracing::debug!(
                malo_id,
                status = r.status().as_u16(),
                "N5 Kostenblatt: lastgang non-2xx — falling back to billing-period"
            );
            return None;
        }
        Ok(r) => r.json().await.unwrap_or_default(),
        Err(e) => {
            tracing::warn!(%e, malo_id, "N5 Kostenblatt: lastgang fetch error");
            return None;
        }
    };

    // Extract interval values from either response format.
    let intervals: Vec<&serde_json::Value> = if let Some(arr) = body.as_array() {
        // Direct array: [{ "wert": "...", "timestamp_utc": "..." }, ...]
        arr.iter().collect()
    } else if let Some(arr) = body.get("werteliste").and_then(|v| v.as_array()) {
        // BO4E Lastgang: { "werteliste": [{ "wert": "...", "zeitstempel": "..." }] }
        arr.iter().collect()
    } else if let Some(arr) = body.get("zeitreihenwerteliste").and_then(|v| v.as_array()) {
        arr.iter().collect()
    } else {
        return None;
    };

    let total: Decimal = intervals
        .iter()
        .filter_map(|v| {
            v.get("wert")
                .or_else(|| v.get("kwh"))
                .and_then(decimal_from_json_value)
        })
        .sum();

    if total > Decimal::ZERO {
        Some(total)
    } else {
        None
    }
}

/// Parse a JSON value as `rust_decimal::Decimal`.
fn decimal_from_json_value(v: &serde_json::Value) -> Option<rust_decimal::Decimal> {
    match v {
        serde_json::Value::String(s) => s.parse().ok(),
        serde_json::Value::Number(n) => n.to_string().parse().ok(),
        _ => None,
    }
}

// ── N5a: Kostenblatt gap detection ────────────────────────────────────────────

/// `GET /api/v1/redispatch/kostenblatt/gaps/{year}/{month}`
///
/// Lists Kostenblatt records for the month where `dispatch_kwh = 0` and
/// `dispatch_source IS NULL` — activations that were registered but whose
/// energy quantity was never computed.
///
/// Operators should call
/// `POST /api/v1/redispatch/kostenblatt/{activation_id}/compute`
/// for each gap before the 15th-of-month submission deadline (BK6-20-061).
pub async fn get_kostenblatt_gaps(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<NetzbilanzConfig>>,
    Path((year, month)): Path<(i16, i16)>,
) -> impl IntoResponse {
    match list_kostenblatt_gaps(&pool, &cfg.tenant, year, month).await {
        Ok(rows) => Json(serde_json::json!({
            "year":     year,
            "month":    month,
            "gaps":     rows.len(),
            "hint": format!(
                "For each gap, call POST /api/v1/redispatch/kostenblatt/{{activation_id}}/compute \
                 before the 15th of {}-{:02} (BK6-20-061)",
                year + if month == 12 { 1 } else { 0 },
                if month == 12 { 1 } else { month + 1 }
            ),
            "records":  rows,
        }))
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── §42a GGV NNE NB side (N8: Gemeinschaftliche Gebäudeversorgung) ────────────

/// Request body for `POST /api/v1/billing/ggv-nne/{ggv_malo_id}`.
///
/// The NB bills each GGV tenant MaLo for its proportional NNE share.
/// §42a EEG 2023 requires the NB to treat each tenant as an individual
/// Marktlokation for NNE purposes.
#[derive(Debug, serde::Deserialize)]
pub struct GgvNneRequest {
    /// NB MP-ID (invoice sender).
    pub nb_mp_id: String,
    /// LF MP-ID (invoice recipient — the GGV LF or each tenant's LF).
    pub lf_mp_id: String,
    /// Billing period start (`YYYY-MM-DD`).
    pub period_from: String,
    /// Billing period end (`YYYY-MM-DD`).
    pub period_to: String,
    /// Invoice issue date (defaults to today).
    pub invoice_date: Option<String>,
    /// Payment due date (defaults to today + 30 days).
    pub due_date: Option<String>,
    /// NNE Arbeitspreis ct/kWh for this GGV (from `PreisblattNetznutzung`).
    pub arbeitspreis_ct_per_kwh: rust_decimal::Decimal,
    /// NNE Grundpreis EUR/year for this GGV (from `PreisblattNetznutzung`).
    pub grundpreis_eur_per_year: Option<rust_decimal::Decimal>,
    /// KA rate ct/kWh (KAV §2, optional).
    pub ka_satz_ct_per_kwh: Option<rust_decimal::Decimal>,
    /// Per-tenant consumption in kWh.
    ///
    /// If not provided, NNE is split equally among tenants from the
    /// `Lokationszuordnung` graph.  When provided, each key is an 11-digit
    /// MaLo-ID and the value is the metered consumption kWh.
    pub tenant_consumption: Option<std::collections::HashMap<String, rust_decimal::Decimal>>,
    /// Total GGV output kWh (PV generation + grid purchase) for equal-split fallback.
    pub total_kwh: Option<rust_decimal::Decimal>,
}

/// `POST /api/v1/billing/ggv-nne/{ggv_malo_id}`
///
/// **N8 — §42a GGV Netzentgelt NB-side billing.**
///
/// §42a EEG 2023 (mandatory from 01.01.2024): The NB must bill each GGV
/// tenant Marktlokation for its individual NNE share.  Attribution is
/// proportional to measured consumption (`tenant_consumption`) or equal
/// split when consumption data is absent.
///
/// Pipeline:
/// 1. Fetch GGV topology from `marktd` `GET /api/v1/malo/{ggv_malo_id}/lokationen`
///    — returns `Lokationszuordnung` graph edges where `beziehungstyp = "GGV_MIETER"`.
/// 2. Determine tenant MaLo-IDs from graph edges (typ `MALO`).
/// 3. For each tenant: compute proportional `arbeitsmenge_kwh` and generate INVOIC 31001
///    draft via `run_billing_internal`.
/// 4. Return all draft IDs plus attribution summary.
#[allow(clippy::too_many_lines)]
pub async fn post_ggv_nne(
    Extension(pool): Extension<PgPool>,
    Extension(marktd): Extension<Arc<MarktdClient>>,
    Extension(cfg): Extension<Arc<crate::config::NetzbilanzConfig>>,
    Extension(http_client): Extension<Arc<reqwest::Client>>,
    Path(ggv_malo_id): Path<String>,
    Json(req): Json<GgvNneRequest>,
) -> impl IntoResponse {
    use rust_decimal::Decimal;

    // ── Step 1: Fetch GGV Lokationszuordnung from marktd ─────────────────────
    let tenant_malos: Vec<String> = if let Some(ref consumption) = req.tenant_consumption {
        // Use explicitly provided tenant list (caller already knows the topology).
        consumption.keys().cloned().collect()
    } else {
        // Auto-discover tenant MaLos via marktd Lokationszuordnung graph.
        let marktd_base = cfg.marktd_url.trim_end_matches('/').to_owned();
        let url = format!("{marktd_base}/api/v1/malo/{ggv_malo_id}/lokationen");
        let mut http_req = http_client.get(&url);
        http_req = http_req.bearer_auth(&cfg.marktd_api_key);

        let edges: serde_json::Value = match http_req.send().await {
            Ok(r) if r.status().is_success() => r.json().await.unwrap_or_default(),
            Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND => {
                return (
                    StatusCode::NOT_FOUND,
                    format!("GGV MaLo {ggv_malo_id} not found in marktd"),
                )
                    .into_response();
            }
            Ok(r) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    format!("marktd returned HTTP {}", r.status()),
                )
                    .into_response();
            }
            Err(e) => return (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
        };

        // Extract tenant MaLo-IDs from graph edges.
        // Edges where beziehungstyp = "GGV_MIETER" or lokationstyp_ziel = "MALO".
        edges
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|edge| {
                let typ = edge
                    .get("beziehungstyp")
                    .or_else(|| edge.get("lokationstyp_ziel"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let id = edge
                    .get("ziel_id")
                    .or_else(|| edge.get("lokation_id"))
                    .and_then(|v| v.as_str());
                // Include MALO edges that are tenants, exclude the PV MaLo itself.
                if (typ == "GGV_MIETER" || typ.contains("MALO"))
                    && id.is_some_and(|s| s != ggv_malo_id)
                {
                    id.map(ToOwned::to_owned)
                } else {
                    None
                }
            })
            .collect()
    };

    if tenant_malos.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "no GGV tenant MaLos found for {ggv_malo_id} — provision Lokationszuordnung in marktd or supply tenant_consumption"
            ),
        )
            .into_response();
    }

    let n_tenants = tenant_malos.len();
    let today = time::OffsetDateTime::now_utc().date().to_string();
    let invoice_date = req.invoice_date.clone().unwrap_or_else(|| today.clone());
    let due_date = req.due_date.clone().unwrap_or_else(|| {
        (time::OffsetDateTime::now_utc() + time::Duration::days(30))
            .date()
            .to_string()
    });

    // ── Step 2: Generate N × NNE drafts ──────────────────────────────────────
    let mut all_draft_ids: Vec<String> = Vec::new();
    let mut attribution: Vec<serde_json::Value> = Vec::new();

    for (i, tenant_malo) in tenant_malos.iter().enumerate() {
        // Proportional or equal-split consumption.
        let kwh_tenant = if let Some(ref consumption_map) = req.tenant_consumption {
            *consumption_map.get(tenant_malo).unwrap_or(&Decimal::ZERO)
        } else if let Some(total_kwh) = req.total_kwh {
            // Equal split.
            (total_kwh / Decimal::from(n_tenants)).round_dp(3)
        } else {
            // No consumption data — cannot generate invoice.
            tracing::warn!(
                ggv_malo_id = %ggv_malo_id,
                tenant_malo = %tenant_malo,
                "netzbilanzd GGV NNE: no consumption data — skipping tenant"
            );
            continue;
        };

        if kwh_tenant <= Decimal::ZERO {
            continue;
        }

        let rechnungsnummer_prefix = format!(
            "GGV-NNE-{}-{}-{}",
            ggv_malo_id, tenant_malo, &req.period_from
        );

        let billing_req = BillingRunRequest {
            nb_mp_id: req.nb_mp_id.clone(),
            lf_mp_id: req.lf_mp_id.clone(),
            invoice_date: invoice_date.clone(),
            due_date: due_date.clone(),
            rechnungsnummer_prefix,
            positions: vec![BillingPosition {
                malo_id: tenant_malo.clone(),
                period_from: req.period_from.clone(),
                period_to: req.period_to.clone(),
                billing_type: "nne_strom".to_owned(),
                arbeitsmenge_kwh: Some(kwh_tenant),
                arbeitspreis_ct_per_kwh: Some(req.arbeitspreis_ct_per_kwh),
                arbeitsmenge_ht_kwh: None,
                arbeitspreis_ht_ct_per_kwh: None,
                arbeitsmenge_nt_kwh: None,
                arbeitspreis_nt_ct_per_kwh: None,
                spitzenleistung_kw: None,
                leistungspreis_eur_per_kw: None,
                ka_satz_ct_per_kwh: req.ka_satz_ct_per_kwh,
                profil_kwh: None,
                mehr_preis_ct_per_kwh: None,
                minder_preis_ct_per_kwh: None,
                lastprofil: None,
                msb_mp_id: None,
                grundgebuehr_eur_per_month: req
                    .grundpreis_eur_per_year
                    .map(|gp| gp / Decimal::from(12)),
                billing_months: None,
                messdienstleistung_eur: None,
            }],
        };

        match run_billing_internal(
            &pool,
            &marktd,
            &cfg.tenant,
            cfg.vnb_mp_id.as_deref(),
            billing_req,
        )
        .await
        {
            Ok(ids) => {
                let share_pct = Decimal::from(100) / Decimal::from(n_tenants);
                attribution.push(serde_json::json!({
                    "tenant_malo": tenant_malo,
                    "kwh": kwh_tenant.to_string(),
                    "share_pct": if req.tenant_consumption.is_some() {
                        (kwh_tenant / req.total_kwh.unwrap_or(Decimal::ONE) * Decimal::from(100)).round_dp(2).to_string()
                    } else {
                        share_pct.round_dp(2).to_string()
                    },
                    "draft_ids": ids,
                }));
                for id in ids {
                    all_draft_ids.push(id.to_string());
                }
            }
            Err(e) => {
                tracing::warn!(
                    tenant_malo = %tenant_malo,
                    i,
                    error = %e,
                    "netzbilanzd GGV NNE: billing failed for tenant MaLo"
                );
            }
        }
    }

    if all_draft_ids.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "all GGV tenant billing runs failed — check arbeitspreis and period",
        )
            .into_response();
    }

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "ggv_malo_id": ggv_malo_id,
            "tenant_count": n_tenants,
            "drafted_count": all_draft_ids.len(),
            "draft_ids": all_draft_ids,
            "period_from": req.period_from,
            "period_to": req.period_to,
            "attribution": attribution,
            "source": "§42a_ggv_nne_nb",
        })),
    )
        .into_response()
}

// ── Korrekturrechnung / Stornorechnung (§ 147 AO / GoBD audit trail) ───────────────

/// Request body for `POST /api/v1/billing/drafts/{id}/correction`.
#[derive(Debug, serde::Deserialize)]
pub struct DraftCorrectionRequest {
    /// Human-readable correction reason (mandatory, stored as `korrekturGrund`).
    pub reason: String,
    /// Amended Rechnung JSONB.  When `None`, a pure Stornorechnung (negative clone)
    /// is generated.  When supplied, a Korrekturrechnung with the amended positions
    /// is generated instead.
    pub amended_rechnung: Option<serde_json::Value>,
}

/// `POST /api/v1/billing/drafts/{id}/correction`
///
/// **Korrekturrechnung / Stornorechnung (§ 147 AO / GoBD).**
///
/// Creates a correction or reversal for an existing INVOIC draft.
///
/// - No `amended_rechnung` → **Stornorechnung**: clones original with negated
///   `gross_eur_units` and `rechnungsart = "STORNORECHNUNG"`.
/// - With `amended_rechnung` → **Korrekturrechnung**: stores the amended Rechnung
///   with `rechnungsart = "KORREKTURRECHNUNG"`.
///
/// Both cases embed `zusatzAttribute.originalRechnungsnummer` and
/// `zusatzAttribute.korrekturGrund` for BNetzA §20 audit compliance.
///
/// The original record is **never modified** — corrections produce new rows.
pub async fn post_draft_correction(
    Extension(pool): Extension<PgPool>,
    Path(id): Path<Uuid>,
    Json(req): Json<DraftCorrectionRequest>,
) -> impl IntoResponse {
    let is_storno = req.amended_rechnung.is_none();
    match crate::pg::insert_correction_draft(&pool, id, &req.reason, req.amended_rechnung).await {
        Ok(new_id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "correction_draft_id": new_id,
                "original_draft_id": id,
                "rechnungsart": if is_storno { "STORNORECHNUNG" } else { "KORREKTURRECHNUNG" },
                "reason": req.reason,
                "status": "draft",
                "hint": "Review the correction draft, then PUT /api/v1/billing/drafts/{correction_draft_id}/dispatch to send INVOIC to makod.",
            })),
        )
            .into_response(),
        Err(e) if e.to_string().contains("not found") => {
            (StatusCode::NOT_FOUND, e.to_string()).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Fremdkosten REST API (§ 147 AO / GoBD external cost pass-through, BO4E typed) ──

/// `PUT /api/v1/billing/fremdkosten/{draft_id}`
///
/// **§ 147 AO / GoBD — typed external cost pass-through.**
///
/// Associates a typed `rubo4e::current::Fremdkosten` + `FremdkostenBlock` +
/// `FremdkostenPosition` payload with an existing INVOIC draft.
///
/// External fees (ÜNB balancing charges, third-party MSB charges) currently appear
/// as free-text `ZusatzAttribut` positions in INVOIC 31002.  This endpoint stores
/// them as typed BO4E objects — on dispatch the `fremdkosten_json` is merged into
/// the `Rechnung.zusatzAttribute` so the LF receives the full breakdown.
///
/// **Idempotent** — subsequent PUT replaces the existing record for this draft.
pub async fn put_fremdkosten(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<NetzbilanzConfig>>,
    Path(draft_id): Path<Uuid>,
    Json(req): Json<UpsertFremdkostenRequest>,
) -> impl IntoResponse {
    // Validate _typ when present.
    if let Some(typ) = req.fremdkosten_json.get("_typ").and_then(|v| v.as_str())
        && typ.to_uppercase() != "FREMDKOSTEN"
    {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("expected _typ FREMDKOSTEN, got {typ:?}"),
        )
            .into_response();
    }
    match upsert_fremdkosten(&pool, &cfg.tenant, draft_id, &req).await {
        Ok(id) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "id": id,
                "draft_id": draft_id,
                "total_eur": req.total_eur.to_string(),
                "bezeichnung": req.bezeichnung,
            })),
        )
            .into_response(),
        Err(e) if e.to_string().contains("violates foreign key") => {
            (StatusCode::NOT_FOUND, format!("draft {draft_id} not found")).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/billing/fremdkosten/{draft_id}`
///
/// Retrieve the typed `Fremdkosten` record for an invoice draft.
pub async fn get_fremdkosten(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<NetzbilanzConfig>>,
    Path(draft_id): Path<Uuid>,
) -> impl IntoResponse {
    match fetch_fremdkosten(&pool, draft_id, &cfg.tenant).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── POST /api/v1/billing/drafts/dispatch-batch ────────────────────────────────

/// Request body for `POST /api/v1/billing/drafts/dispatch-batch`.
#[derive(Debug, Deserialize)]
pub struct DispatchBatchRequest {
    /// List of draft UUIDs to dispatch.
    pub draft_ids: Vec<Uuid>,
}

/// `POST /api/v1/billing/drafts/dispatch-batch`
///
/// Dispatch multiple approved drafts in a single operation.
/// Each draft is dispatched independently — partial failures are reported
/// without blocking remaining dispatches.
///
/// Returns a summary with `succeeded` count and a list of failures.
pub async fn post_dispatch_batch(
    Extension(pool): Extension<PgPool>,
    Extension(makod): Extension<Arc<MakodClient>>,
    Json(req): Json<DispatchBatchRequest>,
) -> impl IntoResponse {
    if req.draft_ids.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "draft_ids must not be empty" })),
        )
            .into_response();
    }
    if req.draft_ids.len() > 500 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "batch size must not exceed 500" })),
        )
            .into_response();
    }

    match dispatch_batch(&pool, &makod, &req.draft_ids).await {
        Ok((succeeded, failures)) => (
            if failures.is_empty() {
                StatusCode::OK
            } else {
                StatusCode::MULTI_STATUS
            },
            Json(serde_json::json!({
                "succeeded": succeeded,
                "failed": failures.len(),
                "total": req.draft_ids.len(),
                "failures": failures.iter().map(|(id, reason)| serde_json::json!({
                    "draft_id": id,
                    "reason": reason,
                })).collect::<Vec<_>>(),
            })),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── GET /api/v1/billing/malo/{malo_id} ───────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct MaloBillingHistoryQuery {
    pub limit: Option<i64>,
}

/// `GET /api/v1/billing/malo/{malo_id}`
///
/// Returns the billing history for a MaLo (lightweight — no Rechnung JSONB).
/// Useful for ERP reconciliation and per-MaLo payment status checks.
pub async fn get_malo_billing_history(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<NetzbilanzConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<MaloBillingHistoryQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(100).min(1000);
    match billing_history_for_malo(&pool, &cfg.tenant, &malo_id, limit).await {
        Ok(rows) => Json(serde_json::json!({
            "malo_id": malo_id,
            "count": rows.len(),
            "records": rows,
        }))
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── REMADV payment lifecycle ──────────────────────────────────────────────────

/// Request body for `PUT /api/v1/billing/drafts/{id}/mark-paid`.
#[derive(Debug, Deserialize)]
pub struct MarkPaidRequest {
    /// EDIFACT reference from the REMADV 33001/33003/33004 message.
    pub remadv_ref: String,
}

/// `PUT /api/v1/billing/drafts/{id}/mark-paid`
///
/// **REMADV 33001/33003/33004 — Zahlungsbestätigung.**
///
/// Updates `invoice_drafts.status` → `'paid'`.  Called by ERP or `makod` outbox
/// when a REMADV indicating payment is received from the LF.
///
/// Regulatory basis: INVOIC AHB 1.0 §3 — NB must track payment status for
/// § 147 AO / GoBD 3-year retention and BNetzA audit readiness.
pub async fn mark_paid(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<NetzbilanzConfig>>,
    Extension(http_client): Extension<Arc<reqwest::Client>>,
    Path(id): Path<Uuid>,
    Json(req): Json<MarkPaidRequest>,
) -> impl IntoResponse {
    match mark_draft_paid(&pool, id, &req.remadv_ref).await {
        Ok(true) => {
            if let Some(ref url) = cfg.erp_webhook_url {
                emit_cloud_event(
                    Arc::clone(&http_client),
                    url.clone(),
                    "de.netzbilanz.invoic.paid",
                    serde_json::json!({
                        "draft_id": id,
                        "remadv_ref": req.remadv_ref,
                        "tenant": cfg.tenant,
                    }),
                );
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "draft_id": id,
                    "status": "paid",
                    "remadv_ref": req.remadv_ref,
                })),
            )
                .into_response()
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            format!("draft {id} not found or not in 'dispatched' status"),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Request body for `PUT /api/v1/billing/drafts/{id}/mark-disputed`.
#[derive(Debug, Deserialize)]
pub struct MarkDisputedRequest {
    /// EDIFACT ERC reason code from REMADV 33002 (e.g. "Z32", "Z34", "Z35").
    pub erc_code: String,
    /// Free-text reason from the LF.
    pub reason: String,
}

/// `PUT /api/v1/billing/drafts/{id}/mark-disputed`
///
/// **REMADV 33002 — Zahlungsablehnung.**
///
/// Updates `invoice_drafts.check_outcome` → `'Dispute'` and stores the ERC code.
/// The NB can then issue a COMDIS 29001 via makod for formal escalation.
pub async fn mark_disputed(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<NetzbilanzConfig>>,
    Extension(http_client): Extension<Arc<reqwest::Client>>,
    Path(id): Path<Uuid>,
    Json(req): Json<MarkDisputedRequest>,
) -> impl IntoResponse {
    match mark_draft_disputed(&pool, id, &req.erc_code, &req.reason).await {
        Ok(true) => {
            if let Some(ref url) = cfg.erp_webhook_url {
                emit_cloud_event(
                    Arc::clone(&http_client),
                    url.clone(),
                    "de.netzbilanz.invoic.disputed",
                    serde_json::json!({
                        "draft_id": id,
                        "erc_code": req.erc_code,
                        "reason": req.reason,
                        "tenant": cfg.tenant,
                    }),
                );
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "draft_id": id,
                    "check_outcome": "Dispute",
                    "erc_code": req.erc_code,
                    "hint": "Use makod COMDIS 29001 for formal escalation.",
                })),
            )
                .into_response()
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            format!("draft {id} not found or not in 'dispatched' status"),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── GET /api/v1/billing/summary ───────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SummaryQuery {
    pub year: Option<i32>,
    pub month: Option<u8>,
}

/// `GET /api/v1/billing/summary?year=&month=`
///
/// Monthly billing totals by PID and status.  Used for ERP month-end reconciliation
/// and BNetzA §20 reporting.  Also exposed via the `get_billing_summary` MCP tool.
pub async fn get_billing_summary_rest(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<NetzbilanzConfig>>,
    Query(q): Query<SummaryQuery>,
) -> impl IntoResponse {
    let now = time::OffsetDateTime::now_utc();
    let year = q.year.unwrap_or(now.year());
    let month = q.month.unwrap_or(now.month() as u8);
    if !(1..=12).contains(&month) {
        return (StatusCode::BAD_REQUEST, "month must be 1–12").into_response();
    }
    match billing_summary(&pool, &cfg.tenant, year, month).await {
        Ok(rows) => {
            let total: i64 = rows.iter().map(|r| r.total_gross_eur_units).sum();
            Json(serde_json::json!({
                "year": year, "month": month,
                "total_gross_eur": format!("{:.5}", total as f64 / 100_000.0),
                "by_pid_status": rows,
            }))
            .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── GET /api/v1/billing/audit ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct AuditExportQuery {
    pub from: Option<String>, // ISO date YYYY-MM-DD
    pub to: Option<String>,   // ISO date YYYY-MM-DD
    pub pid: Option<i32>,
    pub status: Option<String>,
    pub limit: Option<i64>,
}

/// `GET /api/v1/billing/audit`
///
/// **§ 147 AO / GoBD BNetzA audit export.**
///
/// Returns all invoice records (lightweight, no Rechnung JSONB) filtered by
/// date range, PID, and status.  Used for:
/// - BNetzA § 147 AO / GoBD 3-year retention audit (Prüfung der Abrechnungsunterlagen)
/// - Annual NNE portfolio reconciliation
/// - Automated ERP import jobs
///
/// Note: full Rechnung JSONB is retrievable via `GET /api/v1/billing/drafts/{id}`.
pub async fn get_billing_audit(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<NetzbilanzConfig>>,
    Query(q): Query<AuditExportQuery>,
) -> impl IntoResponse {
    let from = q.from.as_deref().and_then(parse_date_opt);
    let to = q.to.as_deref().and_then(parse_date_opt);
    let query = AuditQuery {
        tenant: cfg.tenant.clone(),
        from,
        to,
        pid: q.pid,
        status: q.status.clone(),
        limit: q.limit.unwrap_or(10_000).min(50_000),
    };
    match list_audit(&pool, query).await {
        Ok(rows) => Json(serde_json::json!({
            "count": rows.len(),
            "records": rows,
            "regulatory_note": "§ 147 AO / GoBD: 3-year retention; full Rechnung via GET /api/v1/billing/drafts/{id}",
        })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

fn parse_date_opt(s: &str) -> Option<time::Date> {
    use time::format_description::well_known::Iso8601;
    time::Date::parse(s, &Iso8601::DEFAULT).ok()
}

// ── POST /api/v1/webhooks/remadv ──────────────────────────────────────────────

/// CloudEvent body for REMADV ingest from makod/ERP.
#[derive(Debug, Deserialize)]
pub struct RemadvWebhookBody {
    /// CloudEvent `type` — expected: `de.invoic.receipt.settled` or `de.invoic.receipt.disputed`.
    #[serde(rename = "type")]
    pub ce_type: String,
    pub data: serde_json::Value,
}

/// `POST /api/v1/webhooks/remadv`
///
/// **REMADV CloudEvent ingest.**
///
/// Receives `de.invoic.receipt.settled` (REMADV 33001/33003/33004) and
/// `de.invoic.receipt.disputed` (REMADV 33002) CloudEvents from `makod` or
/// an ERP webhook bridge.
///
/// Updates `invoice_drafts.status` accordingly.
/// The `data.draft_id` field must contain the UUID of the invoice draft.
///
/// Used to close the NB payment lifecycle without manual operator intervention.
pub async fn post_remadv_webhook(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<NetzbilanzConfig>>,
    Extension(http_client): Extension<Arc<reqwest::Client>>,
    Json(body): Json<RemadvWebhookBody>,
) -> impl IntoResponse {
    let id_str = body
        .data
        .get("draft_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let Ok(id) = id_str.parse::<Uuid>() else {
        return (
            StatusCode::BAD_REQUEST,
            "data.draft_id must be a valid UUID",
        )
            .into_response();
    };

    let result = match body.ce_type.as_str() {
        "de.invoic.receipt.settled" | "de.netzbilanz.invoic.paid" => {
            let remadv_ref = body
                .data
                .get("remadv_ref")
                .and_then(|v| v.as_str())
                .unwrap_or("webhook");
            mark_draft_paid(&pool, id, remadv_ref).await
        }
        "de.invoic.receipt.disputed" | "de.netzbilanz.invoic.disputed" => {
            let erc = body
                .data
                .get("erc_code")
                .and_then(|v| v.as_str())
                .unwrap_or("Z00");
            let reason = body
                .data
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("REMADV dispute");
            mark_draft_disputed(&pool, id, erc, reason).await
        }
        other => {
            tracing::debug!(
                ce_type = other,
                "netzbilanzd: unhandled REMADV CloudEvent type"
            );
            return StatusCode::NO_CONTENT.into_response();
        }
    };

    match result {
        Ok(true) => {
            // Emit downstream CloudEvent
            let ce_type = if body.ce_type.contains("settled") {
                "de.netzbilanz.invoic.paid"
            } else {
                "de.netzbilanz.invoic.disputed"
            };
            if let Some(ref url) = cfg.erp_webhook_url {
                emit_cloud_event(
                    Arc::clone(&http_client),
                    url.clone(),
                    ce_type,
                    serde_json::json!({ "draft_id": id, "tenant": cfg.tenant }),
                );
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            format!("draft {id} not found or wrong status"),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod kostenblatt_tests {
    use super::decimal_from_json_value;
    use rust_decimal::dec;

    // ── decimal_from_json_value ───────────────────────────────────────────────

    #[test]
    fn decimal_from_string_value() {
        let v = serde_json::json!("1.25");
        assert_eq!(decimal_from_json_value(&v), Some(dec!(1.25)));
    }

    #[test]
    fn decimal_from_numeric_value() {
        let v = serde_json::json!(2.5);
        assert_eq!(decimal_from_json_value(&v), Some(dec!(2.5)));
    }

    #[test]
    fn decimal_from_invalid_value_returns_none() {
        let v = serde_json::json!({ "wert": "1.25" });
        assert!(decimal_from_json_value(&v).is_none());
    }

    // ── KostenblattComputeRequest validation ──────────────────────────────────

    #[test]
    fn activation_window_parse_rfc3339() {
        // Verify that typical Redispatch timestamps round-trip correctly.
        let start = "2026-01-15T10:00:00Z";
        let end = "2026-01-15T10:15:00Z";
        let s = time::OffsetDateTime::parse(start, &time::format_description::well_known::Rfc3339);
        let e = time::OffsetDateTime::parse(end, &time::format_description::well_known::Rfc3339);
        assert!(s.is_ok(), "activation_start_utc must parse as RFC 3339");
        assert!(e.is_ok(), "activation_end_utc must parse as RFC 3339");
        assert!(e.unwrap() > s.unwrap(), "end must be after start");
    }

    #[test]
    fn activation_window_15min_gap() {
        let start = time::OffsetDateTime::parse(
            "2026-01-15T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let end = time::OffsetDateTime::parse(
            "2026-01-15T10:15:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let gap = end - start;
        assert_eq!(
            gap.whole_minutes(),
            15,
            "typical Redispatch activation = 15 min"
        );
    }

    // ── Lastgang response parsing ─────────────────────────────────────────────

    #[test]
    fn sum_lastgang_array_format() {
        // Array format: [{ "wert": "1.25", "timestamp_utc": "..." }, ...]
        let response = serde_json::json!([
            { "wert": "1.25", "timestamp_utc": "2026-01-15T10:00:00Z" },
            { "wert": "1.30", "timestamp_utc": "2026-01-15T10:15:00Z" },
            { "wert": "0.80", "timestamp_utc": "2026-01-15T10:30:00Z" }
        ]);
        let intervals: Vec<&serde_json::Value> = response.as_array().unwrap().iter().collect();
        let total: rust_decimal::Decimal = intervals
            .iter()
            .filter_map(|v| v.get("wert").and_then(decimal_from_json_value))
            .sum();
        assert_eq!(total, dec!(3.35));
    }

    #[test]
    fn sum_lastgang_bo4e_format() {
        // BO4E Lastgang: { "werteliste": [{ "wert": "...", ... }] }
        let response = serde_json::json!({
            "zeitIntervallLaenge": "PT15M",
            "werteliste": [
                { "wert": "2.50", "zeitstempel": "2026-01-15T10:00:00Z", "status": "67" },
                { "wert": "2.75", "zeitstempel": "2026-01-15T10:15:00Z", "status": "67" }
            ]
        });
        let intervals: Vec<&serde_json::Value> = response
            .get("werteliste")
            .and_then(|v| v.as_array())
            .unwrap()
            .iter()
            .collect();
        let total: rust_decimal::Decimal = intervals
            .iter()
            .filter_map(|v| v.get("wert").and_then(decimal_from_json_value))
            .sum();
        assert_eq!(total, dec!(5.25));
    }

    #[test]
    fn empty_lastgang_returns_zero() {
        let response = serde_json::json!([]);
        let intervals: Vec<&serde_json::Value> = response.as_array().unwrap().iter().collect();
        let total: rust_decimal::Decimal = intervals
            .iter()
            .filter_map(|v| v.get("wert").and_then(decimal_from_json_value))
            .sum();
        assert_eq!(total, dec!(0));
    }

    // ── Einsatzkosten calculation ─────────────────────────────────────────────

    #[test]
    fn einsatzkosten_calculation_precision() {
        // 2.5 kWh × 0.12345 EUR/kWh = 0.30863 EUR
        let dispatch_kwh = dec!(2.5);
        let arbeitspreis = dec!(0.12345);
        let einsatzkosten = dispatch_kwh * arbeitspreis;
        assert_eq!(einsatzkosten, dec!(0.308625));
        // Verify no floating-point drift
        assert_ne!(einsatzkosten.to_string(), "0.3086249");
    }

    #[test]
    fn dispatch_source_values_are_canonical() {
        // Canonical dispatch_source values match DB CHECK constraint
        let valid = &["lastgang_sum", "billing_period", "manual_override"];
        for v in valid {
            assert!(
                !v.is_empty(),
                "dispatch_source value must not be empty: {v}"
            );
            assert_eq!(
                *v,
                v.to_lowercase(),
                "dispatch_source must be lowercase: {v}"
            );
        }
    }
}

// ── §13a EnWG Vergütung (Redispatch 2.0 compensation) ─────────────────────────

/// Request body for `POST /api/v1/redispatch/verguetung/{activation_id}/compute`.
#[derive(Debug, serde::Deserialize)]
pub struct VerguetungComputeRequest {
    /// 11-digit MaLo-ID of the affected resource's grid connection.
    pub malo_id: String,
    /// UTC activation start — RFC 3339.
    pub activation_start_utc: String,
    /// UTC activation end — RFC 3339.
    pub activation_end_utc: String,
    /// Z01 EEG / Z02 KWKG / Z03 sonstige (Redispatch Stammdaten).
    pub verguetungsart: grid_billing::RedispatchVerguetungsart,
    /// EEG/KWKG plants: the anzulegender Wert in ct/kWh — the lost statutory
    /// remuneration basis (§13a Abs. 2 S. 3 Nr. 5 EnWG). Ignored when
    /// `entgangene_einnahmen_eur_override` is supplied.
    #[serde(default)]
    pub anzulegender_wert_ct_per_kwh: Option<rust_decimal::Decimal>,
    /// Proven lost revenue in EUR (Nr. 3) — required for Z03, optional
    /// override for Z01/Z02.
    #[serde(default)]
    pub entgangene_einnahmen_eur_override: Option<rust_decimal::Decimal>,
    /// Zusätzliche Aufwendungen in EUR (Nr. 1/2/4). Default 0.
    #[serde(default)]
    pub zusaetzliche_aufwendungen_eur: rust_decimal::Decimal,
    /// Ersparte Aufwendungen in EUR (Satz 4). Default 0.
    #[serde(default)]
    pub ersparte_aufwendungen_eur: rust_decimal::Decimal,
    /// Manual Ausfallarbeit override — when set, edmd is not queried.
    #[serde(default)]
    pub ausfallarbeit_kwh_override: Option<rust_decimal::Decimal>,
}

/// `POST /api/v1/redispatch/verguetung/{activation_id}/compute`
///
/// §13a Abs. 2 EnWG: compute the angemessene Vergütung for one redispatch
/// activation. The Ausfallarbeit comes from the same edmd Lastgang window
/// resolution the Kostenblatt uses (15-min interval sum over the activation
/// window); the compensation arithmetic is `grid_billing::redispatch_verguetung`
/// (entgangene Einnahmen + zusätzliche Aufwendungen − ersparte Aufwendungen).
///
/// This is a **calculation endpoint** — the figure and its per-component
/// trace are returned for the operator's payment run; nothing is persisted.
pub async fn post_verguetung_compute(
    Extension(cfg): Extension<Arc<crate::config::NetzbilanzConfig>>,
    Extension(http_client): Extension<Arc<reqwest::Client>>,
    Path(activation_id): Path<String>,
    Json(req): Json<VerguetungComputeRequest>,
) -> impl IntoResponse {
    use rust_decimal::Decimal;
    use time::format_description::well_known::Rfc3339;

    if time::OffsetDateTime::parse(&req.activation_start_utc, &Rfc3339).is_err()
        || time::OffsetDateTime::parse(&req.activation_end_utc, &Rfc3339).is_err()
    {
        return (
            StatusCode::BAD_REQUEST,
            "activation_start_utc/activation_end_utc must be RFC 3339",
        )
            .into_response();
    }

    // Ausfallarbeit: override → edmd Lastgang window sum.
    let (ausfallarbeit_kwh, source) = if let Some(kwh) = req.ausfallarbeit_kwh_override {
        (kwh, "manual_override")
    } else {
        let Some(edmd_url) = cfg.edmd_url.as_deref().map(|u| u.trim_end_matches('/')) else {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "edmd_url not configured — supply ausfallarbeit_kwh_override",
            )
                .into_response();
        };
        let shim = KostenblattComputeRequest {
            tr_id: String::new(),
            malo_id: req.malo_id.clone(),
            period_year: 0,
            period_month: 1,
            uenb_mp_id: String::new(),
            vnb_mp_id: String::new(),
            activation_start_utc: req.activation_start_utc.clone(),
            activation_end_utc: req.activation_end_utc.clone(),
            arbeitspreis_eur_per_kwh: Decimal::ZERO,
            dispatch_kwh_override: None,
        };
        match fetch_dispatch_kwh_from_lastgang(&http_client, edmd_url, &req.malo_id, &cfg, &shim)
            .await
        {
            Some(kwh) => (kwh, "lastgang_sum"),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    format!(
                        "no Lastgang data for MaLo {} in the activation window — ingest \
                         Redispatch MSCONS (PIDs 13020–13023, 13026) or supply \
                         ausfallarbeit_kwh_override",
                        req.malo_id
                    ),
                )
                    .into_response();
            }
        }
    };

    // Entgangene Einnahmen basis by Vergütungsart.
    let entgangene = match (
        req.entgangene_einnahmen_eur_override,
        req.anzulegender_wert_ct_per_kwh,
        req.verguetungsart,
    ) {
        (Some(eur), _, _) => eur,
        (None, Some(aw_ct), _) => grid_billing::eeg_entgangene_einnahmen(ausfallarbeit_kwh, aw_ct),
        (None, None, grid_billing::RedispatchVerguetungsart::Sonstige) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                "Z03 (sonstige) requires entgangene_einnahmen_eur_override — lost market \
                 revenue must be proven, not derived (§13a Abs. 2 S. 3 Nr. 3 EnWG)",
            )
                .into_response();
        }
        (None, None, _) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                "EEG/KWKG plants require anzulegender_wert_ct_per_kwh (or an explicit \
                 entgangene_einnahmen_eur_override)",
            )
                .into_response();
        }
    };

    let input = grid_billing::RedispatchVerguetungInput {
        ausfallarbeit_kwh,
        verguetungsart: req.verguetungsart,
        entgangene_einnahmen_eur: entgangene,
        zusaetzliche_aufwendungen_eur: req.zusaetzliche_aufwendungen_eur,
        ersparte_aufwendungen_eur: req.ersparte_aufwendungen_eur,
    };
    match grid_billing::redispatch_verguetung(&input) {
        Ok(v) => Json(serde_json::json!({
            "activation_id": activation_id,
            "malo_id": req.malo_id,
            "ausfallarbeit_source": source,
            "verguetung": v,
            "legal_basis": "§13a Abs. 2 EnWG",
        }))
        .into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

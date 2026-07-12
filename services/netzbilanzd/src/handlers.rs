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
use crate::pg::{
    UpsertKostenblattRequest, approve_and_dispatch, fetch_draft, fetch_kostenblatt, list_drafts_pg,
    list_kostenblatt, mark_kostenblatt_submitted, reject_draft_pg, upsert_kostenblatt,
};

// ── POST /api/v1/billing/run ──────────────────────────────────────────────────

/// `POST /api/v1/billing/run`
///
/// Generates invoice drafts for the given MaLos in the specified billing period.
/// Each draft is stored with `status = 'draft'` and validated against
/// `invoic-checker` checks 1–3 before storage.
pub async fn run_billing(
    Extension(pool): Extension<PgPool>,
    Extension(marktd): Extension<Arc<MarktdClient>>,
    Json(req): Json<BillingRunRequest>,
) -> impl IntoResponse {
    match run_billing_internal(&pool, &marktd, req).await {
        Ok(ids) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "draft_ids": ids })),
        )
            .into_response(),
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
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match approve_and_dispatch(&pool, &makod, id).await {
        Ok(ref_id) => (
            StatusCode::OK,
            Json(serde_json::json!({ "dispatch_ref": ref_id })),
        )
            .into_response(),
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
    let mut http_req = reqwest::Client::new().get(&url);
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

    match run_billing_internal(&pool, &marktd, billing_req).await {
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
    Path(activation_id): Path<String>,
    Json(req): Json<UpsertKostenblattRequest>,
) -> impl IntoResponse {
    let einsatzkosten = req.dispatch_kwh * req.arbeitspreis_eur_per_kwh;
    match upsert_kostenblatt(&pool, "default", &activation_id, &req).await {
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
    Path(activation_id): Path<String>,
) -> impl IntoResponse {
    match fetch_kostenblatt(&pool, &activation_id, "default").await {
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
    Query(q): Query<KostenblattListQuery>,
) -> impl IntoResponse {
    match list_kostenblatt(&pool, "default", q.year, q.month, q.status.as_deref()).await {
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
    Path((year, month)): Path<(i16, i16)>,
) -> impl IntoResponse {
    use rust_decimal::Decimal;

    let pending = match list_kostenblatt(&pool, "default", year, month, Some("pending")).await {
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

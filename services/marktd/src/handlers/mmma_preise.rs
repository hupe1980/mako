//! Handlers for MMMA Gas and MMM Strom settlement price endpoints.
//!
//! Both tables are shared by `netzbilanzd` (NB billing) and `invoicd` (LF
//! plausibility validation) — they cannot share a database, so `marktd` acts
//! as the single source of truth for B2B settlement prices.
//!
//! ## Endpoints
//!
//! | Method | Path | Action | Description |
//! |--------|------|--------|-------------|
//! | `PUT` | `/api/v1/mmma-preise/gas/{year}/{month}` | `write-preisblatt` | Upsert Gas MMM price pair (THE publication) |
//! | `GET` | `/api/v1/mmma-preise/gas/{year}/{month}` | `read-preisblatt` | Fetch Gas MMM prices for a billing month |
//! | `GET` | `/api/v1/mmma-preise/gas` | `read-preisblatt` | List all Gas MMM price records (newest first) |
//! | `PUT` | `/api/v1/mmm-preise/strom/{year}/{month}` | `write-preisblatt` | Upsert Strom MMM price pair (ÜNB publication) |
//! | `GET` | `/api/v1/mmm-preise/strom/{year}/{month}` | `read-preisblatt` | Fetch Strom MMM prices |

use std::sync::Arc;

use axum::{
    Json,
    extract::{Extension, Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::repository::{MmmPreisStromRepository, MmmaPreisGasRepository};
use mako_service::cedar::CedarEnforcer;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use time::Date;
use tracing::warn;

use super::{Claims, TenantGln};
use crate::pg::mmma_preise::{PgMmmPreisStromRepository, PgMmmaPreisGasRepository};

pub type MmmaGasRepoExt = Arc<PgMmmaPreisGasRepository>;
pub type MmmStromRepoExt = Arc<PgMmmPreisStromRepository>;

// ── Gas ───────────────────────────────────────────────────────────────────────

/// Path parameters for Gas MMM price endpoints.
#[derive(Debug, Deserialize)]
pub struct YearMonthPath {
    pub year: i32,
    pub month: u8,
}

impl YearMonthPath {
    /// Convert to the first day of the billing month.
    fn to_date(&self) -> Result<Date, String> {
        let month = time::Month::try_from(self.month)
            .map_err(|_| format!("invalid month: {}", self.month))?;
        Date::from_calendar_date(self.year, month, 1)
            .map_err(|e| format!("invalid date {}/{}: {e}", self.year, self.month))
    }
}

/// Response body for Gas MMM price queries.
#[derive(Debug, Serialize)]
pub struct MmmaGasResponse {
    pub price_month: String,
    pub marktgebiet: String,
    /// Mehrmengen (Überschuss) Ausgleichsenergiepreis ct/kWh.
    pub mehr_ct_kwh: Decimal,
    /// Mindermengen (Defizit) Ausgleichsenergiepreis ct/kWh.
    pub minder_ct_kwh: Decimal,
    pub source: String,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Request body for upserting Gas MMM prices.
#[derive(Debug, Deserialize)]
pub struct MmmaGasUpsertRequest {
    /// Marktgebiet — defaults to `"THE"`.
    #[serde(default = "default_marktgebiet")]
    pub marktgebiet: String,
    pub mehr_ct_kwh: Decimal,
    pub minder_ct_kwh: Decimal,
    /// Import source: `"manual"` | `"the-api"` | `"csv-import"`. Defaults to `"manual"`.
    #[serde(default = "default_source_manual")]
    pub source: String,
}

fn default_marktgebiet() -> String {
    "THE".to_owned()
}

fn default_source_manual() -> String {
    "manual".to_owned()
}

/// `PUT /api/v1/mmma-preise/gas/{year}/{month}`
///
/// Upsert the Gas MMM Abrechnungspreise for a billing month. Published monthly
/// by Trading Hub Europe (THE). The `netzbilanzd` billing run auto-fetches these
/// instead of requiring manual ERP input.
pub async fn put_mmma_gas(
    Extension(repo): Extension<MmmaGasRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(path): Path<YearMonthPath>,
    Json(req): Json<MmmaGasUpsertRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-preisblatt", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }
    let price_month = match path.to_date() {
        Ok(d) => d,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    match repo
        .upsert_gas(
            price_month,
            &req.marktgebiet,
            req.mehr_ct_kwh,
            req.minder_ct_kwh,
            &req.source,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/mmma-preise/gas/{year}/{month}`
pub async fn get_mmma_gas(
    Extension(repo): Extension<MmmaGasRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(path): Path<YearMonthPath>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-preisblatt", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }
    let price_month = match path.to_date() {
        Ok(d) => d,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    match repo.find_gas(price_month, "THE").await {
        Ok(Some(r)) => Json(MmmaGasResponse {
            price_month: r.price_month.to_string(),
            marktgebiet: r.marktgebiet,
            mehr_ct_kwh: r.mehr_ct_kwh,
            minder_ct_kwh: r.minder_ct_kwh,
            source: r.source,
            updated_at: r.updated_at,
        })
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            format!("No MMMA Gas prices for {}/{}", path.year, path.month),
        )
            .into_response(),
        Err(e) => {
            warn!("mmma_gas GET error: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

/// Query parameters for the list endpoint.
#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    24
}

/// `GET /api/v1/mmma-preise/gas` — list all Gas MMM price records (newest first).
pub async fn list_mmma_gas(
    Extension(repo): Extension<MmmaGasRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-preisblatt", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }
    match repo.list_gas(q.limit.min(120)).await {
        Ok(records) => {
            let resp: Vec<_> = records
                .into_iter()
                .map(|r| MmmaGasResponse {
                    price_month: r.price_month.to_string(),
                    marktgebiet: r.marktgebiet,
                    mehr_ct_kwh: r.mehr_ct_kwh,
                    minder_ct_kwh: r.minder_ct_kwh,
                    source: r.source,
                    updated_at: r.updated_at,
                })
                .collect();
            Json(resp).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Strom ─────────────────────────────────────────────────────────────────────

/// Response body for Strom MMM price queries.
#[derive(Debug, Serialize)]
pub struct MmmStromResponse {
    pub price_month: String,
    pub unb_mp_id: String,
    pub mehr_ct_kwh: Decimal,
    pub minder_ct_kwh: Decimal,
    pub source: String,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Request body for upserting Strom MMM prices.
#[derive(Debug, Deserialize)]
pub struct MmmStromUpsertRequest {
    /// ÜNB MP-ID (BDEW-Codenummer, `99…`): 50Hertz, TenneT, Amprion, TransnetBW.
    pub unb_mp_id: String,
    pub mehr_ct_kwh: Decimal,
    pub minder_ct_kwh: Decimal,
    #[serde(default = "default_source_manual")]
    pub source: String,
}

/// `PUT /api/v1/mmm-preise/strom/{year}/{month}`
///
/// Upsert the Strom MMM Ausgleichsenergie prices for a billing month + ÜNB.
pub async fn put_mmm_strom(
    Extension(repo): Extension<MmmStromRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(path): Path<YearMonthPath>,
    Json(req): Json<MmmStromUpsertRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-preisblatt", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }
    let price_month = match path.to_date() {
        Ok(d) => d,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    match repo
        .upsert_strom(
            price_month,
            &req.unb_mp_id,
            req.mehr_ct_kwh,
            req.minder_ct_kwh,
            &req.source,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/mmm-preise/strom/{year}/{month}?unb_mp_id=9900...`
pub async fn get_mmm_strom(
    Extension(repo): Extension<MmmStromRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(path): Path<YearMonthPath>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-preisblatt", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }
    let unb_mp_id = match q.get("unb_mp_id") {
        Some(id) => id.clone(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "unb_mp_id query parameter required",
            )
                .into_response();
        }
    };
    let price_month = match path.to_date() {
        Ok(d) => d,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };
    match repo.find_strom(price_month, &unb_mp_id).await {
        Ok(Some(r)) => Json(MmmStromResponse {
            price_month: r.price_month.to_string(),
            unb_mp_id: r.unb_mp_id,
            mehr_ct_kwh: r.mehr_ct_kwh,
            minder_ct_kwh: r.minder_ct_kwh,
            source: r.source,
            updated_at: r.updated_at,
        })
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            format!(
                "No MMM Strom prices for {}/{} ÜNB {unb_mp_id}",
                path.year, path.month
            ),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── B12: Manual import trigger ────────────────────────────────────────────────

/// Extension type for MMMA import config (shared via axum Extension layer).
pub type MmmaImportCfgExt = Arc<crate::config::MmmaImportConfig>;

/// Optional query: override year/month for the import.
#[derive(Debug, Deserialize)]
pub struct ImportTriggerQuery {
    pub year: Option<i32>,
    pub month: Option<u8>,
}

/// `POST /api/v1/mmma-preise/import-trigger[?year=YYYY&month=MM]`
///
/// Manually trigger a MMMA Gas / MMM Strom price import cycle.
///
/// Uses the same configurable URLs as the background worker.
/// Useful for catch-up after service downtime, testing, or ERP-driven imports.
///
/// Requires `write-preisblatt` Cedar action.
///
/// ## Response
///
/// ```json
/// {
///   "year": 2026, "month": 7,
///   "results": [
///     { "commodity": "gas",   "success": true,  "error": null },
///     { "commodity": "strom", "success": false, "error": "HTTP 503 from ..." }
///   ]
/// }
/// ```
#[allow(clippy::too_many_arguments)]
pub async fn post_import_trigger(
    Extension(gas_repo): Extension<MmmaGasRepoExt>,
    Extension(strom_repo): Extension<MmmStromRepoExt>,
    Extension(import_cfg): Extension<MmmaImportCfgExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(event_tx): Extension<tokio::sync::mpsc::UnboundedSender<serde_json::Value>>,
    claims: Claims,
    Query(q): Query<ImportTriggerQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-preisblatt", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    let now = time::OffsetDateTime::now_utc();
    let year = q.year.unwrap_or_else(|| now.year());
    let month = q.month.unwrap_or_else(|| now.month() as u8);

    let results = crate::mmma_worker::run_import_cycle(
        year,
        month,
        &import_cfg.gas_url,
        &import_cfg.strom_url,
        &gas_repo,
        &strom_repo,
        &tenant_gln,
        &event_tx,
    )
    .await;

    let results_json: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            serde_json::json!({
                "commodity": r.commodity,
                "success":   r.success,
                "error":     r.error,
            })
        })
        .collect();

    let all_ok = results.iter().all(|r| r.success);
    let status = if all_ok {
        StatusCode::OK
    } else {
        StatusCode::MULTI_STATUS
    };

    (
        status,
        Json(serde_json::json!({
            "year": year,
            "month": month,
            "import_enabled": import_cfg.enabled,
            "results": results_json,
        })),
    )
        .into_response()
}

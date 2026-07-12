//! NB Energiemix authority endpoints.
//!
//! Routes:
//!   PUT  /api/v1/energiemix/{nb_mp_id}               — NB publishes annual grid-area mix
//!   GET  /api/v1/energiemix/{nb_mp_id}               — LF/portald reads current mix
//!   GET  /api/v1/energiemix/{nb_mp_id}/history       — all years
//!
//! ## Regulatory context (§42 EnWG)
//!
//! The NB is the authoritative source for the renewable energy mix in their
//! grid area, derived from local EEG plant feed-in data. Lieferanten use this
//! for §42 Abs. 5 EnWG Reststrommix disclosure on customer bills and for
//! Ökostrom / green-tariff labelling in `tarifbd`.
//!
//! ## Validation
//!
//! PUT body: `rubo4e::current::Energiemix` COM JSON (camelCase).
//! - Deserialized via rubo4e to validate `erzeugungsart` enum values.
//! - Re-serialised to canonical camelCase before storage.
//! - No `_typ` required (Energiemix is a COM, not a BO).
//!
//! ## Hard cut
//!
//! No version history in the API — GET always returns the most recent year.
//! `/history` returns all available years for audit purposes.

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use rubo4e::current::Energiemix;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row as _};
use time::OffsetDateTime;
use utoipa::IntoParams;

use super::TenantGln;

// ── DTOs ─────────────────────────────────────────────────────────────────────

/// Query parameters for `GET /api/v1/energiemix/{nb_mp_id}`.
#[derive(Debug, Deserialize, IntoParams)]
pub struct NbEnergiemixQuery {
    /// Calendar year to fetch.  Defaults to the most recent available year.
    pub year: Option<i16>,
}

/// Request body for `PUT /api/v1/energiemix/{nb_mp_id}`.
#[derive(Debug, Deserialize)]
pub struct PutNbEnergiemixRequest {
    /// `rubo4e::current::Energiemix` COM JSON.
    pub energiemix: serde_json::Value,
    /// Calendar year this mix is valid for.  Defaults to current year (UTC).
    pub gueltig_fuer: Option<i16>,
    /// Total EEG feed-in into this grid area in kWh (optional informational).
    pub eeg_einspeisung_kwh: Option<i64>,
    /// Total grid withdrawal (Gesamtentnahme) in kWh (optional informational).
    pub gesamtentnahme_kwh: Option<i64>,
}

/// Response body for `GET /api/v1/energiemix/{nb_mp_id}`.
#[derive(Debug, Serialize)]
pub struct NbEnergiemixResponse {
    pub nb_mp_id: String,
    pub gueltig_fuer: i16,
    pub energiemix: Energiemix,
    pub eeg_einspeisung_kwh: Option<i64>,
    pub gesamtentnahme_kwh: Option<i64>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `PUT /api/v1/energiemix/{nb_mp_id}`
///
/// Publish or update the annual grid-area `Energiemix` for a Netzbetreiber.
///
/// Body: `{ "energiemix": <rubo4e::current::Energiemix COM JSON>, "gueltig_fuer": 2026 }`
///
/// Validation:
/// - `energiemix` is deserialized via rubo4e to validate all `Erzeugungsart` enum values.
/// - Re-serialised to canonical camelCase before storage.
/// - `gueltig_fuer` defaults to the current calendar year.
///
/// §42 EnWG: NB must disclose the renewable energy share in their grid area
/// to Lieferanten annually.  LFs incorporate this into their §42 Abs. 5
/// Reststrommix statement on customer bills.
pub async fn put_nb_energiemix(
    Extension(pool): Extension<PgPool>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Path(nb_mp_id): Path<String>,
    Json(req): Json<PutNbEnergiemixRequest>,
) -> impl IntoResponse {
    // Validate + canonicalise via rubo4e deserialization.
    let typed: Energiemix = match serde_json::from_value(req.energiemix) {
        Ok(e) => e,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": format!("invalid Energiemix payload: {e}")
                })),
            )
                .into_response();
        }
    };
    let canonical = serde_json::to_value(&typed).unwrap_or_default();

    let year = req
        .gueltig_fuer
        .unwrap_or_else(|| OffsetDateTime::now_utc().year() as i16);

    match sqlx::query(
        r"INSERT INTO nb_energiemix
              (nb_mp_id, tenant, gueltig_fuer, energiemix,
               eeg_einspeisung_kwh, gesamtentnahme_kwh, updated_at)
          VALUES ($1, $2, $3, $4, $5, $6, now())
          ON CONFLICT (tenant, nb_mp_id, gueltig_fuer) DO UPDATE
          SET energiemix            = EXCLUDED.energiemix,
              eeg_einspeisung_kwh   = COALESCE(EXCLUDED.eeg_einspeisung_kwh, nb_energiemix.eeg_einspeisung_kwh),
              gesamtentnahme_kwh    = COALESCE(EXCLUDED.gesamtentnahme_kwh,  nb_energiemix.gesamtentnahme_kwh),
              updated_at            = now()",
    )
    .bind(&nb_mp_id)
    .bind(&tenant)
    .bind(year)
    .bind(&canonical)
    .bind(req.eeg_einspeisung_kwh)
    .bind(req.gesamtentnahme_kwh)
    .execute(&pool)
    .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "nb_mp_id": nb_mp_id,
                "gueltig_fuer": year,
                "message": "Energiemix stored",
            })),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/energiemix/{nb_mp_id}`
///
/// Retrieve the current (or specific year's) grid-area `Energiemix` for a NB.
///
/// Used by:
/// - `tarifbd` to attach NB Reststrommix to STROM products for §42 EnWG disclosure
/// - `portald` to display the local grid-area energy mix to customers
/// - `agentd` for grid sustainability analytics
pub async fn get_nb_energiemix(
    Extension(pool): Extension<PgPool>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Path(nb_mp_id): Path<String>,
    Query(q): Query<NbEnergiemixQuery>,
) -> impl IntoResponse {
    let row = if let Some(year) = q.year {
        sqlx::query(
            r"SELECT nb_mp_id, gueltig_fuer, energiemix,
                     eeg_einspeisung_kwh, gesamtentnahme_kwh, updated_at
              FROM nb_energiemix
              WHERE nb_mp_id = $1 AND tenant = $2 AND gueltig_fuer = $3",
        )
        .bind(&nb_mp_id)
        .bind(&tenant)
        .bind(year)
        .fetch_optional(&pool)
        .await
    } else {
        // Return the most recent available year.
        sqlx::query(
            r"SELECT nb_mp_id, gueltig_fuer, energiemix,
                     eeg_einspeisung_kwh, gesamtentnahme_kwh, updated_at
              FROM nb_energiemix
              WHERE nb_mp_id = $1 AND tenant = $2
              ORDER BY gueltig_fuer DESC
              LIMIT 1",
        )
        .bind(&nb_mp_id)
        .bind(&tenant)
        .fetch_optional(&pool)
        .await
    };

    match row {
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("no Energiemix found for NB {nb_mp_id}")
            })),
        )
            .into_response(),
        Ok(Some(r)) => {
            let energiemix_json: serde_json::Value = r.try_get("energiemix").unwrap_or_default();
            let typed: Energiemix =
                serde_json::from_value(energiemix_json.clone()).unwrap_or_default();
            let resp = NbEnergiemixResponse {
                nb_mp_id: r.try_get("nb_mp_id").unwrap_or_default(),
                gueltig_fuer: r.try_get("gueltig_fuer").unwrap_or_default(),
                energiemix: typed,
                eeg_einspeisung_kwh: r.try_get("eeg_einspeisung_kwh").unwrap_or(None),
                gesamtentnahme_kwh: r.try_get("gesamtentnahme_kwh").unwrap_or(None),
                updated_at: r
                    .try_get("updated_at")
                    .unwrap_or_else(|_| OffsetDateTime::now_utc()),
            };
            Json(resp).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/energiemix/{nb_mp_id}/history`
///
/// Return all available years of grid-area Energiemix for a NB.
/// Used for audit and multi-year trend analysis.
pub async fn get_nb_energiemix_history(
    Extension(pool): Extension<PgPool>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Path(nb_mp_id): Path<String>,
) -> impl IntoResponse {
    let rows = sqlx::query(
        r"SELECT nb_mp_id, gueltig_fuer, energiemix,
                 eeg_einspeisung_kwh, gesamtentnahme_kwh, updated_at
          FROM nb_energiemix
          WHERE nb_mp_id = $1 AND tenant = $2
          ORDER BY gueltig_fuer DESC",
    )
    .bind(&nb_mp_id)
    .bind(&tenant)
    .fetch_all(&pool)
    .await;

    match rows {
        Ok(rows) => {
            let items: Vec<serde_json::Value> = rows
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "nb_mp_id":              r.try_get::<String, _>("nb_mp_id").unwrap_or_default(),
                        "gueltig_fuer":          r.try_get::<i16, _>("gueltig_fuer").unwrap_or_default(),
                        "energiemix":            r.try_get::<serde_json::Value, _>("energiemix").unwrap_or_default(),
                        "eeg_einspeisung_kwh":   r.try_get::<Option<i64>, _>("eeg_einspeisung_kwh").unwrap_or(None),
                        "gesamtentnahme_kwh":    r.try_get::<Option<i64>, _>("gesamtentnahme_kwh").unwrap_or(None),
                        "updated_at":            r.try_get::<OffsetDateTime, _>("updated_at")
                                                    .map(|t| t.to_string())
                                                    .unwrap_or_default(),
                    })
                })
                .collect();
            Json(serde_json::json!({ "nb_mp_id": nb_mp_id, "history": items })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

//! `GET|PUT /api/v1/netzbetreiber/{nb_mp_id}/grundzustaendiger-msb` — the
//! MP-ID under which the grundzuständige Messstellenbetreiber of a
//! Netzbetreiber's grid acts.
//!
//! The grundzuständige Messstellenbetreiber is the Netzbetreiber as an
//! undertaking, unless it has transferred the Grundzuständigkeit (§ 2 Nr. 4
//! MsbG). In the market communication it still acts in the role MSB, and every
//! role has its own MP-ID: „Für jede Sparte (Gas bzw. Strom) und jede Rolle hat
//! ein Marktteilnehmer genau eine MP-ID" (Identifikatoren AWH V1.2 Kap. 2.1).
//! So the MSB code is a recorded fact, never derived from the NB's — and the
//! `PreisblattMessung` that prices a supply point in the grid is filed under
//! it (`/api/v1/preisblaetter-messung/{msb_mp_id}`).
//!
//! The NB's MP-ID is already per Sparte, so the record is keyed on it alone,
//! with a validity: a transfer of the Grundzuständigkeit for moderne
//! Messeinrichtungen and intelligente Messsysteme (§§ 41, 43 MsbG) names a new
//! gMSB from a day on. A `PUT` records the link from `valid_from`, closing the
//! one before it on that day in the same transaction; a `GET` answers the link
//! in force `at` a day (today by default).
//!
//! ## Access control
//!
//! - `GET` — `read-malo-grid`, any authenticated caller of the tenant
//! - `PUT` — `write-malo-grid`, NB role only: it is the NB's grid topology

use std::sync::Arc;

use axum::{
    Extension,
    extract::{Path, Query},
    http::StatusCode,
};
use mako_service::{ApiError, Json, cedar::CedarEnforcer};
use rubo4e::identifiers::MarktpartnerId;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::handlers::{Claims, Tenant};

/// `PUT` body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutGrundzustaendigerMsbBody {
    /// MP-ID the grundzuständige MSB of the grid acts under.
    pub gmsb_mp_id: String,
    /// The first day the link holds (`YYYY-MM-DD`).
    pub valid_from: String,
}

/// `GET` query.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtQuery {
    /// The day asked about (`YYYY-MM-DD`); today in Berlin when absent.
    pub at: Option<String>,
}

/// `GET` answer.
#[derive(Debug, Serialize, Deserialize)]
pub struct GrundzustaendigerMsb {
    pub nb_mp_id: String,
    pub gmsb_mp_id: String,
    /// The first day the link holds.
    pub valid_from: String,
    /// The day it no longer holds; `None` while open.
    pub valid_to: Option<String>,
}

/// A path or body value that is no MP-ID is `422 mp_id_ungueltig`.
fn mp_id(feld: &str, wert: &str) -> Result<(), ApiError> {
    MarktpartnerId::new(wert).map(drop).map_err(|e| {
        ApiError::unprocessable(format!("{feld} {wert:?}: {e}")).with_code("mp_id_ungueltig")
    })
}

/// `GET /api/v1/netzbetreiber/{nb_mp_id}/grundzustaendiger-msb`
#[utoipa::path(
    get,
    path = "/api/v1/netzbetreiber/{nb_mp_id}/grundzustaendiger-msb",
    tag = "malo-grid",
    params(
        ("nb_mp_id" = String, Path, description = "13-digit MP-ID of the Netzbetreiber"),
        ("at" = Option<String>, Query, description = "The day asked about, `YYYY-MM-DD`; today when absent"),
    ),
    responses(
        (status = 200, description = "The gMSB MP-ID of the grid that day: `{ nb_mp_id, gmsb_mp_id, valid_from, valid_to }`", body = serde_json::Value),
        (status = 401, description = "No valid credentials", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 404, description = "No gMSB recorded for this Netzbetreiber on that day", body = ApiError),
    )
)]
pub async fn get_grundzustaendiger_msb(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(nb_mp_id): Path<String>,
    Query(q): Query<AtQuery>,
) -> Result<Json<GrundzustaendigerMsb>, ApiError> {
    enforcer
        .check(&claims.principal(), "read-malo-grid", &tenant)
        .map_err(|e| ApiError::forbidden(e.to_string()))?;
    let at = match q.at.as_deref() {
        Some(s) => mako_service::dates::parse_iso_date("at", s)?,
        None => mako_fristen::heute(),
    };
    let row: Option<(String, time::Date, Option<time::Date>)> = sqlx::query_as(
        "SELECT gmsb_mp_id, valid_from, valid_to FROM grundzustaendige_msb
          WHERE tenant = $1 AND nb_mp_id = $2
            AND valid_from <= $3 AND (valid_to IS NULL OR valid_to > $3)",
    )
    .bind(&tenant)
    .bind(&nb_mp_id)
    .bind(at)
    .fetch_optional(&pool)
    .await
    .map_err(ApiError::internal)?;
    let (gmsb_mp_id, valid_from, valid_to) = row.ok_or_else(|| {
        ApiError::not_found(format!(
            "no grundzuständiger Messstellenbetreiber recorded for Netzbetreiber {nb_mp_id} on {at}"
        ))
    })?;
    Ok(Json(GrundzustaendigerMsb {
        nb_mp_id,
        gmsb_mp_id,
        valid_from: valid_from.to_string(),
        valid_to: valid_to.map(|d| d.to_string()),
    }))
}

/// `PUT /api/v1/netzbetreiber/{nb_mp_id}/grundzustaendiger-msb` — record it.
#[utoipa::path(
    put,
    path = "/api/v1/netzbetreiber/{nb_mp_id}/grundzustaendiger-msb",
    tag = "malo-grid",
    params(("nb_mp_id" = String, Path, description = "13-digit MP-ID of the Netzbetreiber")),
    request_body(content = serde_json::Value, description = "`PutGrundzustaendigerMsbBody`: `{ gmsb_mp_id, valid_from }`"),
    responses(
        (status = 204, description = "Written"),
        (status = 401, description = "No valid credentials", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 422, description = "`mp_id_ungueltig`: a value is no MP-ID; or `valid_from` is no date", body = ApiError),
    )
)]
pub async fn put_grundzustaendiger_msb(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(nb_mp_id): Path<String>,
    Json(body): Json<PutGrundzustaendigerMsbBody>,
) -> Result<StatusCode, ApiError> {
    enforcer
        .check(&claims.principal(), "write-malo-grid", &tenant)
        .map_err(|e| ApiError::forbidden(e.to_string()))?;
    mp_id("nb_mp_id", &nb_mp_id)?;
    mp_id("gmsb_mp_id", &body.gmsb_mp_id)?;
    let valid_from = mako_service::dates::parse_iso_date("valid_from", &body.valid_from)?;
    // One transaction: the link from `valid_from` replaces whatever was
    // recorded from that day on, and the one before it ends that day.
    let mut tx = pool.begin().await.map_err(ApiError::internal)?;
    sqlx::query(
        "DELETE FROM grundzustaendige_msb
          WHERE tenant = $1 AND nb_mp_id = $2 AND valid_from >= $3",
    )
    .bind(&tenant)
    .bind(&nb_mp_id)
    .bind(valid_from)
    .execute(&mut *tx)
    .await
    .map_err(ApiError::internal)?;
    sqlx::query(
        "UPDATE grundzustaendige_msb SET valid_to = $3, updated_at = now()
          WHERE tenant = $1 AND nb_mp_id = $2 AND valid_from < $3
            AND (valid_to IS NULL OR valid_to > $3)",
    )
    .bind(&tenant)
    .bind(&nb_mp_id)
    .bind(valid_from)
    .execute(&mut *tx)
    .await
    .map_err(ApiError::internal)?;
    sqlx::query(
        "INSERT INTO grundzustaendige_msb (tenant, nb_mp_id, gmsb_mp_id, valid_from)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(&tenant)
    .bind(&nb_mp_id)
    .bind(&body.gmsb_mp_id)
    .bind(valid_from)
    .execute(&mut *tx)
    .await
    .map_err(ApiError::internal)?;
    tx.commit().await.map_err(ApiError::internal)?;
    tracing::info!(%nb_mp_id, gmsb_mp_id = %body.gmsb_mp_id, %valid_from, "marktd: grundzuständiger MSB recorded");
    Ok(StatusCode::NO_CONTENT)
}

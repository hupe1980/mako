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
//! The NB's MP-ID is already per Sparte, so the record is keyed on it alone.
//!
//! ## Access control
//!
//! - `GET` — `read-malo-grid`, any authenticated caller of the tenant
//! - `PUT` — `write-malo-grid`, NB role only: it is the NB's grid topology

use std::sync::Arc;

use axum::{Extension, extract::Path, http::StatusCode};
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
}

/// `GET` answer.
#[derive(Debug, Serialize, Deserialize)]
pub struct GrundzustaendigerMsb {
    pub nb_mp_id: String,
    pub gmsb_mp_id: String,
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
    params(("nb_mp_id" = String, Path, description = "13-digit MP-ID of the Netzbetreiber")),
    responses(
        (status = 200, description = "The gMSB MP-ID of the grid: `{ nb_mp_id, gmsb_mp_id }`", body = serde_json::Value),
        (status = 401, description = "No valid credentials", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 404, description = "No gMSB recorded for this Netzbetreiber", body = ApiError),
    )
)]
pub async fn get_grundzustaendiger_msb(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(nb_mp_id): Path<String>,
) -> Result<Json<GrundzustaendigerMsb>, ApiError> {
    enforcer
        .check(&claims.principal(), "read-malo-grid", &tenant)
        .map_err(|e| ApiError::forbidden(e.to_string()))?;
    let gmsb: Option<String> = sqlx::query_scalar(
        "SELECT gmsb_mp_id FROM grundzustaendige_msb WHERE tenant = $1 AND nb_mp_id = $2",
    )
    .bind(&tenant)
    .bind(&nb_mp_id)
    .fetch_optional(&pool)
    .await
    .map_err(ApiError::internal)?;
    let gmsb_mp_id = gmsb.ok_or_else(|| {
        ApiError::not_found(format!(
            "no grundzuständiger Messstellenbetreiber recorded for Netzbetreiber {nb_mp_id}"
        ))
    })?;
    Ok(Json(GrundzustaendigerMsb {
        nb_mp_id,
        gmsb_mp_id,
    }))
}

/// `PUT /api/v1/netzbetreiber/{nb_mp_id}/grundzustaendiger-msb` — record it.
#[utoipa::path(
    put,
    path = "/api/v1/netzbetreiber/{nb_mp_id}/grundzustaendiger-msb",
    tag = "malo-grid",
    params(("nb_mp_id" = String, Path, description = "13-digit MP-ID of the Netzbetreiber")),
    request_body(content = serde_json::Value, description = "`PutGrundzustaendigerMsbBody`: `{ gmsb_mp_id }`"),
    responses(
        (status = 204, description = "Written"),
        (status = 401, description = "No valid credentials", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 422, description = "`mp_id_ungueltig`: a value is no MP-ID", body = ApiError),
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
    sqlx::query(
        r"INSERT INTO grundzustaendige_msb (tenant, nb_mp_id, gmsb_mp_id, updated_at)
          VALUES ($1, $2, $3, now())
          ON CONFLICT (tenant, nb_mp_id)
          DO UPDATE SET gmsb_mp_id = EXCLUDED.gmsb_mp_id, updated_at = now()",
    )
    .bind(&tenant)
    .bind(&nb_mp_id)
    .bind(&body.gmsb_mp_id)
    .execute(&pool)
    .await
    .map_err(ApiError::internal)?;
    tracing::info!(%nb_mp_id, gmsb_mp_id = %body.gmsb_mp_id, "marktd: grundzuständiger MSB recorded");
    Ok(StatusCode::NO_CONTENT)
}

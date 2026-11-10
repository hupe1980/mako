//! `GET /api/v1/malos/{id}/bundesland` — the Land a Marktlokation lies in.

use std::sync::Arc;

use axum::{
    Extension,
    extract::{Path, State},
    response::IntoResponse,
};
use mako_markt::{domain::MaloId, error::MdmError};
use mako_service::ApiError;
use mako_service::cedar::CedarEnforcer;
use rubo4e::current::Marktlokation;

use super::{Claims, IntoMdmResponse as _};
use crate::bundesland::{PlzBefund, QUELLE, der_plz};
use crate::state::AppState;

/// The Bundesland of a Marktlokation, from the Postleitzahl of its
/// `lokationsadresse` (`crate::bundesland`).
///
/// `404` when marktd holds no such MaLo; `422` naming the reason when the
/// MaLo has no address with a PLZ, the PLZ is not in the table, or it lies in
/// more than one Land — a Land is never guessed.
pub async fn get_malo_bundesland(
    State(state): State<Arc<AppState>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-malo", &state.tenant)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }
    let malo_id = match id.parse::<MaloId>() {
        Ok(id) => id,
        Err(e) => {
            return MdmError::InvalidMaloId {
                id,
                reason: e.to_string(),
            }
            .into_response();
        }
    };
    let record = match state.malo_repo.find(&malo_id, mako_fristen::heute()).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return MdmError::NotFound {
                resource_type: "malo",
                id: malo_id.to_string(),
            }
            .into_response();
        }
        Err(e) => return e.into_response(),
    };
    let plz = serde_json::from_value::<Marktlokation>(record.data.clone())
        .ok()
        .and_then(|m| m.lokationsadresse)
        .and_then(|a| a.postleitzahl);
    let Some(plz) = plz else {
        return ApiError::unprocessable(format!(
            "MaLo {malo_id} has no lokationsadresse with a postleitzahl — no Bundesland"
        ))
        .with_code("KEINE_POSTLEITZAHL")
        .into_response();
    };
    match der_plz(&plz) {
        PlzBefund::Eindeutig(land) => mako_service::Json(serde_json::json!({
            "malo_id": malo_id.to_string(),
            "bundesland": land.as_str(),
            "postleitzahl": plz,
            "quelle": QUELLE,
        }))
        .into_response(),
        PlzBefund::Mehrdeutig(laender) => ApiError::unprocessable(format!(
            "PLZ {plz} of MaLo {malo_id} lies in {} — no single Bundesland",
            laender
                .iter()
                .map(|l| l.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .with_code("PLZ_MEHRDEUTIG")
        .into_response(),
        PlzBefund::Unbekannt => ApiError::unprocessable(format!(
            "PLZ {plz} of MaLo {malo_id} is not in the PLZ table — no Bundesland"
        ))
        .with_code("PLZ_UNBEKANNT")
        .into_response(),
    }
}

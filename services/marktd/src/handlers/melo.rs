//! MeLo (Messlokation) REST handlers.
//!
//! Routes:
//!   PUT  /api/v1/melo/:id
//!   GET  /api/v1/melo/:id

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_markt::{
    cloudevents::{EventExtensions, MarktEvent},
    domain::{MaloId, MeloId},
    error::MdmError,
    repository::{
        AppState, ContractRepository, CorrelationIndex, MaloRepository, MeloRepository,
        PartnerRepository, SubscriptionRepository,
    },
};
use rubo4e::current::{Messlokation, Standorteigenschaften};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{Claims, IntoMdmResponse as _, etag, parse_if_match};

// ── BO4E validation helpers ──────────────────────────────────────────────────────────

/// Validate and normalise a `Messlokation` payload (L4 hard cut).
fn normalize_messlokation(
    mut data: serde_json::Value,
) -> Result<serde_json::Value, (axum::http::StatusCode, serde_json::Value)> {
    if let Some(obj) = data.as_object_mut() {
        obj.entry("_typ")
            .or_insert_with(|| serde_json::json!("MESSLOKATION"));
    }
    if let Some(typ) = data.get("_typ").and_then(|v| v.as_str())
        && typ.to_uppercase() != "MESSLOKATION"
    {
        return Err((
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "error": format!("expected _typ MESSLOKATION, got '{typ}'") }),
        ));
    }
    let melo: Messlokation = serde_json::from_value(data).map_err(|e| {
        (
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "error": format!("invalid Messlokation payload: {e}") }),
        )
    })?;
    Ok(serde_json::to_value(&melo).unwrap_or_default())
}

/// Deserialise stored JSONB as `Messlokation`. Returns `None` on schema drift.
fn deserialize_stored_melo(data: serde_json::Value, melo_id: &str) -> Option<Messlokation> {
    serde_json::from_value::<Messlokation>(data)
        .map_err(|e| {
            tracing::error!(
                melo_id,
                error = %e,
                "schema drift: stored MeLo data is not a valid Messlokation — \
                 re-PUT with a valid BO4E payload"
            );
        })
        .ok()
}

// ── DTOs ──────────────────────────────────────────────────────────────────────

fn default_bo4e_version() -> String {
    "v202607.0.0".to_owned()
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct MeloUpsertRequest {
    /// Associated MaLo-ID (optional).
    pub malo_id: Option<String>,
    /// Full BO4E MESSLOKATION payload.
    pub data: serde_json::Value,
    /// BO4E schema version of `data` (e.g. `"v202607.0.0"`). Defaults to current.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MeloResponse {
    pub melo_id: String,
    pub malo_id: Option<String>,
    pub version: i64,
    /// Validated BO4E `Messlokation` payload in canonical camelCase form.
    /// `_typ` is auto-injected on write; enum fields validated on write.
    #[schema(value_type = Object)]
    pub data: Messlokation,
    /// Voltage/pressure level at the metering point (`Messlokation.netzebeneMessung`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub netzebene_messung: Option<String>,
    /// Regelzone EIC code extracted from `standorteigenschaften.eigenschaftenStrom[0].regelzone`.
    /// Maps this MeLo to the \u00dcNB for Redispatch 2.0 Stammdaten routing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regelzone: Option<String>,
    /// Full BO4E `Standorteigenschaften` JSONB — carries `StandorteigenschaftenStrom`
    /// (regelzone, bilanzierungsgebietEic) and `StandorteigenschaftenGas` (druckstufe).
    /// Required for Redispatch 2.0 `NetworkConstraintDocument` and Gas billing zones.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub standorteigenschaften: Option<serde_json::Value>,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `PUT /api/v1/melo/:id`
#[utoipa::path(
    put,
    path = "/api/v1/melo/{id}",
    tag = "melo",
    params(("id" = String, Path, description = "MeLo-ID (DE + 31 chars)")),
    request_body = MeloUpsertRequest,
    responses(
        (status = 200, description = "Updated"),
        (status = 201, description = "Created"),
        (status = 409, description = "Version conflict"),
    )
)]
pub async fn put_melo<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    headers: HeaderMap,
    _claims: Claims,
    Path(id): Path<String>,
    Json(req): Json<MeloUpsertRequest>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    let melo_id = match id.parse::<MeloId>() {
        Ok(id) => id,
        Err(e) => {
            return MdmError::InvalidMeloId {
                id,
                reason: e.to_string(),
            }
            .into_response();
        }
    };

    let malo_id = match req
        .malo_id
        .as_deref()
        .map(|s| {
            s.parse::<MaloId>().map_err(|e| MdmError::InvalidMaloId {
                id: s.to_owned(),
                reason: e.to_string(),
            })
        })
        .transpose()
    {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let if_match = parse_if_match(&headers);
    let exists = state
        .melo_repo
        .find(&melo_id)
        .await
        .ok()
        .flatten()
        .is_some();

    // L4 hard cut: validate and normalise the incoming BO4E Messlokation payload.
    let canonical_data = match normalize_messlokation(req.data) {
        Ok(v) => v,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };

    match state
        .melo_repo
        .upsert(
            &melo_id,
            malo_id.as_ref(),
            canonical_data,
            if_match,
            &req.bo4e_version,
        )
        .await
    {
        Ok(version) => {
            // Emit de.markt.melo.updated so ERP subscribers and edmd get notified of
            // Standorteigenschaften / zaehlwerke changes (required for WiM Stammdaten
            // auto-update and Redispatch 2.0 NetworkConstraintDocument cross-references).
            let melo_id_str = melo_id.to_string();
            let evt = MarktEvent::new(
                &state.tenant_gln,
                "de.markt.melo.updated",
                melo_id_str,
                serde_json::json!({ "version": version }),
            )
            .with_extensions(EventExtensions {
                marktmeloid: Some(melo_id.to_string()),
                marktmaloid: req.malo_id.clone(),
                ..Default::default()
            });
            if let Ok(payload) = serde_json::to_value(&evt) {
                let _ = state.event_tx.send(payload);
            }

            let status = if exists {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            };
            (
                status,
                [(axum::http::header::ETAG, etag(version))],
                axum::Json(serde_json::json!({ "version": version })),
            )
                .into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/melo/:id`
#[utoipa::path(
    get,
    path = "/api/v1/melo/{id}",
    tag = "melo",
    params(("id" = String, Path, description = "MeLo-ID")),
    responses(
        (status = 200, description = "Found", body = MeloResponse),
        (status = 404, description = "Not found"),
    )
)]
pub async fn get_melo<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    _claims: Claims,
    Path(id): Path<String>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    let melo_id = match id.parse::<MeloId>() {
        Ok(id) => id,
        Err(e) => {
            return MdmError::InvalidMeloId {
                id,
                reason: e.to_string(),
            }
            .into_response();
        }
    };

    match state.melo_repo.find(&melo_id).await {
        Ok(Some(r)) => {
            let data = match deserialize_stored_melo(r.data, r.melo_id.as_ref()) {
                Some(v) => v,
                None => return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            };
            let resp = MeloResponse {
                melo_id: r.melo_id.to_string(),
                malo_id: r.malo_id.map(|id| id.to_string()),
                version: r.version,
                data,
                netzebene_messung: r.netzebene_messung,
                regelzone: r.regelzone,
                standorteigenschaften: r.standorteigenschaften,
            };
            (
                StatusCode::OK,
                [(axum::http::header::ETAG, etag(r.version))],
                axum::Json(resp),
            )
                .into_response()
        }
        Ok(None) => mako_markt::error::MdmError::NotFound {
            resource_type: "resource",
            id,
        }
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/melos/:id/standorteigenschaften`
///
/// Returns the typed BO4E `Standorteigenschaften` for the MeLo — carrying
/// `StandorteigenschaftenStrom` (regelzone EIC, bilanzierungsgebietEic) and/or
/// `StandorteigenschaftenGas` (druckstufe). Required for Redispatch 2.0
/// `NetworkConstraintDocument` cross-references and Gas billing zone routing.
///
/// Returns 404 when the MeLo has no `standorteigenschaften` column populated yet.
/// Use `PUT /api/v1/melo/{id}` with a `data.standorteigenschaften` field to populate it,
/// or wait for `nis-syncd` / WiM Stammdaten auto-population (Roadmap N3).
#[utoipa::path(
    get,
    path = "/api/v1/melos/{id}/standorteigenschaften",
    tag = "melo",
    params(("id" = String, Path, description = "MeLo-ID (DE + 31 chars)")),
    responses(
        (status = 200, description = "Standorteigenschaften", body = Object),
        (status = 404, description = "MeLo not found or no Standorteigenschaften"),
    )
)]
pub async fn get_melo_standorteigenschaften<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    _claims: Claims,
    Path(id): Path<String>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    let melo_id = match id.parse::<MeloId>() {
        Ok(id) => id,
        Err(e) => {
            return MdmError::InvalidMeloId {
                id,
                reason: e.to_string(),
            }
            .into_response();
        }
    };

    match state.melo_repo.find(&melo_id).await {
        Ok(Some(r)) => {
            match r.standorteigenschaften {
                Some(raw) => {
                    // Attempt to deserialize as typed `Standorteigenschaften`.
                    // Falls back to returning raw JSONB when the stored JSON doesn't
                    // match the typed schema (e.g. legacy or non-standard data).
                    match serde_json::from_value::<Standorteigenschaften>(raw.clone()) {
                        Ok(typed) => (
                            StatusCode::OK,
                            axum::Json(serde_json::to_value(&typed).unwrap_or(raw)),
                        )
                            .into_response(),
                        Err(_) => (StatusCode::OK, axum::Json(raw)).into_response(),
                    }
                }
                None => mako_markt::error::MdmError::NotFound {
                    resource_type: "standorteigenschaften",
                    id,
                }
                .into_response(),
            }
        }
        Ok(None) => mako_markt::error::MdmError::NotFound {
            resource_type: "melo",
            id,
        }
        .into_response(),
        Err(e) => e.into_response(),
    }
}

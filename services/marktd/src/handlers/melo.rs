//! MeLo (Messlokation) REST handlers.
//!
//! Routes:
//!   PUT  /api/v1/melos/:id
//!   GET  /api/v1/melos/:id

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_markt::{
    cloudevents::{EventExtensions, MarktEvent},
    domain::{MaloId, MeloId},
    error::MdmError,
    repository::{
        AppState, CorrelationIndex, MaloRepository, MeloRepository, PartnerRepository,
        SubscriptionRepository,
    },
};
use rubo4e::current::{Messlokation, Standorteigenschaften};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use mako_service::cedar::CedarEnforcer;

use super::{Claims, IfMatch, IntoMdmResponse as _, etag, malformed_if_match, parse_if_match};

// ── BO4E validation helpers ──────────────────────────────────────────────────────────

/// Validate and normalise a `Messlokation` payload through the BO4E gate.
///
/// The BO4E-stated rule this adds for a `Messlokation` is that at most one of
/// `messadresse`, `geoadresse` and `katasterinformation` may be present.
fn normalize_messlokation(
    data: serde_json::Value,
) -> Result<Messlokation, (axum::http::StatusCode, serde_json::Value)> {
    mako_markt::bo4e::decode(data)
        .map_err(|e| (axum::http::StatusCode::UNPROCESSABLE_ENTITY, e.to_json()))
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
    mako_markt::bo4e::schema_version()
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct MeloUpsertRequest {
    /// Associated MaLo-ID (optional).
    pub malo_id: Option<String>,
    /// Full BO4E MESSLOKATION payload.
    pub data: serde_json::Value,
    /// BO4E schema version this payload is interpreted under. Server-derived;
    /// a value sent by the client is recorded but never changes how `data` is
    /// parsed, so prefer omitting it.
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
    /// Lokationsbündel object code (`Messlokation.lokationsbuendelObjektcode`,
    /// UTILMD Lokationsbündelstruktur).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lokationsbuendel_objektcode: Option<String>,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `PUT /api/v1/melos/:id`
///
/// # Single-write-path invariant (MaLo ↔ MeLo)
///
/// This PUT is the only writer of the `melo.malo_id` FK, and the repository
/// maintains the corresponding `melo → malo` edge in the temporal
/// `lokationszuordnungen` graph in the same transaction: the graph is always a
/// superset of the FK (the FK is a derived convenience for "current parent"),
/// and reparenting closes the previous open edge (`valid_to`). See
/// `marktd::pg::melo` for the reconciliation rules.
#[utoipa::path(
    put,
    path = "/api/v1/melos/{id}",
    tag = "melo",
    params(("id" = String, Path, description = "MeLo-ID (DE + 31 chars)")),
    request_body = MeloUpsertRequest,
    responses(
        (status = 200, description = "Updated"),
        (status = 201, description = "Created"),
        (status = 409, description = "Version conflict"),
    )
)]
pub async fn put_melo<Ma, Me, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Su, Ci, Pa>>>,
    Extension(pool): Extension<sqlx::PgPool>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    headers: HeaderMap,
    claims: Claims,
    Path(id): Path<String>,
    Json(req): Json<MeloUpsertRequest>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    if enforcer
        .check(&claims.principal(), "write-melo", &state.tenant)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "write-melo denied",
        }
        .into_response();
    }

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

    let if_match = match parse_if_match(&headers) {
        IfMatch::Absent | IfMatch::Any => None,
        IfMatch::Version(v) => Some(v),
        // Refuse rather than fall back to an unconditional write: the caller
        // asked for a conditional one and would otherwise be told it succeeded.
        IfMatch::Malformed => return malformed_if_match(),
    };
    // Not `.ok().flatten().is_some()`: that reported "does not exist" both for a
    // storage fault and for a row that exists but no longer deserialises, so a
    // PUT over an existing MeLo answered `201 Created`. Unlike `malo`, the MeLo
    // upsert takes its version from the caller rather than returning a
    // post-increment one, so the answer cannot be derived from the write and
    // this probe has to stay — but it has to be honest about not knowing.
    let exists = match state.melo_repo.find(&melo_id).await {
        Ok(found) => found.is_some(),
        Err(e) => {
            tracing::error!(melo_id = %melo_id, error = %e, "put_melo: existence probe failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "could not determine whether the MeLo exists"})),
            )
                .into_response();
        }
    };

    // L4 hard cut: validate and normalise the incoming BO4E Messlokation payload.
    let melo = match normalize_messlokation(req.data) {
        Ok(v) => v,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };

    match state
        .melo_repo
        .upsert(
            &melo_id,
            malo_id.as_ref(),
            &melo,
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
                &state.tenant,
                mako_events::markt::MELO_UPDATED,
                melo_id_str,
                serde_json::json!({ "version": version }),
            )
            .with_extensions(EventExtensions {
                marktmeloid: Some(melo_id.to_string()),
                marktmaloid: req.malo_id.clone(),
                ..Default::default()
            });
            if let Err(e) = crate::outbox::enqueue(&pool, &evt, &state.notify).await {
                tracing::error!(error = %e, "melo: durable enqueue failed");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": "event enqueue failed"})),
                )
                    .into_response();
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

/// `GET /api/v1/melos/:id`
#[utoipa::path(
    get,
    path = "/api/v1/melos/{id}",
    tag = "melo",
    params(("id" = String, Path, description = "MeLo-ID")),
    responses(
        (status = 200, description = "Found", body = MeloResponse),
        (status = 404, description = "Not found"),
    )
)]
pub async fn get_melo<Ma, Me, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Path(id): Path<String>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    if enforcer
        .check(&claims.principal(), "read-melo", &state.tenant)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "read-melo denied",
        }
        .into_response();
    }

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
                lokationsbuendel_objektcode: r.lokationsbuendel_objektcode,
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
/// Use `PUT /api/v1/melos/{id}` with a `data.standorteigenschaften` field to populate it,
/// or wait for WiM Stammdaten auto-population (Roadmap N3).
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
pub async fn get_melo_standorteigenschaften<Ma, Me, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Path(id): Path<String>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    if enforcer
        .check(&claims.principal(), "read-melo", &state.tenant)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "read-melo denied",
        }
        .into_response();
    }

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

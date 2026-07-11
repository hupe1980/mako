//! VersorgungsStatus REST handlers.
//!
//! Routes:
//!   GET  /api/v1/versorgung/:malo_id            — current supply state (or `?at=YYYY-MM-DD`)
//!   GET  /api/v1/versorgung/:malo_id/history    — full supply-state change history
//!   PUT  /api/v1/versorgung/:malo_id            — upsert supply state (ERP / processd)

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_markt::{
    domain::MaloId,
    error::MdmError,
    repository::{
        AppState, ContractRepository, CorrelationIndex, LieferStatus, MaloRepository,
        MeloRepository, PartnerRepository, SubscriptionRepository, VersorgungsStatusHistoryRecord,
        VersorgungsStatusRecord, VersorgungsStatusRepository,
    },
};
use mako_service::cedar::CedarEnforcer;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::{Claims, IntoMdmResponse as _, etag, parse_if_match};

// ── DTOs ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct VersorgungsStatusResponse {
    pub malo_id: String,
    #[schema(value_type = String, example = "Beliefert")]
    pub lieferstatus: String,
    pub lf_mp_id: Option<String>,
    pub lf_mp_id_next: Option<String>,
    pub lf_next_lieferbeginn: Option<String>,
    pub lieferbeginn: Option<String>,
    pub lieferende: Option<String>,
    pub msb_mp_id: Option<String>,
    pub nb_mp_id: String,
    pub last_process_id: Option<String>,
    pub updated_at: String,
    pub version: i64,
}

impl From<VersorgungsStatusRecord> for VersorgungsStatusResponse {
    fn from(r: VersorgungsStatusRecord) -> Self {
        Self {
            malo_id: r.malo_id.as_ref().to_owned(),
            lieferstatus: r.lieferstatus.to_string(),
            lf_mp_id: r.lf_mp_id,
            lf_mp_id_next: r.lf_mp_id_next,
            lf_next_lieferbeginn: r.lf_next_lieferbeginn.map(|d| d.to_string()),
            lieferbeginn: r.lieferbeginn.map(|d| d.to_string()),
            lieferende: r.lieferende.map(|d| d.to_string()),
            msb_mp_id: r.msb_mp_id,
            nb_mp_id: r.nb_mp_id,
            last_process_id: r.last_process_id.map(|u| u.to_string()),
            updated_at: r.updated_at.to_string(),
            version: r.version,
        }
    }
}

/// Single supply-state history entry (response DTO).
#[derive(Debug, Serialize, ToSchema)]
pub struct VersorgungsStatusHistoryResponse {
    pub id: i64,
    pub malo_id: String,
    #[schema(value_type = String, example = "Beliefert")]
    pub lieferstatus: String,
    pub lf_mp_id: Option<String>,
    pub lf_mp_id_next: Option<String>,
    pub lf_next_lieferbeginn: Option<String>,
    pub lieferbeginn: Option<String>,
    pub lieferende: Option<String>,
    pub msb_mp_id: Option<String>,
    pub nb_mp_id: String,
    pub last_process_id: Option<String>,
    pub version: i64,
    /// UTC instant when this state became active.
    pub valid_from: String,
}

impl From<VersorgungsStatusHistoryRecord> for VersorgungsStatusHistoryResponse {
    fn from(r: VersorgungsStatusHistoryRecord) -> Self {
        Self {
            id: r.id,
            malo_id: r.malo_id.as_ref().to_owned(),
            lieferstatus: r.lieferstatus.to_string(),
            lf_mp_id: r.lf_mp_id,
            lf_mp_id_next: r.lf_mp_id_next,
            lf_next_lieferbeginn: r.lf_next_lieferbeginn.map(|d| d.to_string()),
            lieferbeginn: r.lieferbeginn.map(|d| d.to_string()),
            lieferende: r.lieferende.map(|d| d.to_string()),
            msb_mp_id: r.msb_mp_id,
            nb_mp_id: r.nb_mp_id,
            last_process_id: r.last_process_id.map(|u| u.to_string()),
            version: r.version,
            valid_from: r.valid_from.to_string(),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct VersorgungsStatusUpsertRequest {
    #[schema(value_type = String, example = "Beliefert")]
    pub lieferstatus: String,
    pub lf_mp_id: Option<String>,
    pub lf_mp_id_next: Option<String>,
    pub lf_next_lieferbeginn: Option<String>,
    pub lieferbeginn: Option<String>,
    pub lieferende: Option<String>,
    pub msb_mp_id: Option<String>,
    pub nb_mp_id: String,
    pub last_process_id: Option<uuid::Uuid>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct VersorgungQuery {
    /// Point-in-time date in `YYYY-MM-DD` format (German local time, i.e. CET/CEST).
    ///
    /// When present, returns the supply state as it was at end-of-day on this date,
    /// reconstructed from the history log.  Omit for the current state.
    #[param(example = "2025-04-01")]
    pub at: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct HistoryQuery {
    #[param(example = 0)]
    #[serde(default)]
    pub page: u32,
    #[param(example = 50)]
    #[serde(default = "default_history_size")]
    pub size: u32,
}

fn default_history_size() -> u32 {
    50
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// GET /api/v1/versorgung/:malo_id
///
/// Returns the current supply state.  Add `?at=YYYY-MM-DD` to query the state
/// as of a specific calendar date (German local time, CET/CEST).
#[allow(clippy::type_complexity)]
pub async fn get_versorgungsstatus<Ma, Me, Co, Su, Ci, Pa, Vs>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Path(malo_id): Path<String>,
    Query(query): Query<VersorgungQuery>,
    Extension(vs_repo): Extension<Arc<Vs>>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
    Vs: VersorgungsStatusRepository + Send + Sync,
{
    if enforcer
        .check(
            &claims.principal(),
            "read-versorgungsstatus",
            &state.tenant_gln,
        )
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }
    let malo_id = match malo_id.parse::<MaloId>() {
        Ok(id) => id,
        Err(e) => {
            return MdmError::InvalidMaloId {
                id: malo_id,
                reason: e.to_string(),
            }
            .into_response();
        }
    };

    // If `?at=` is present, delegate to the history-based point-in-time query.
    if let Some(at_str) = &query.at {
        let at = match time::Date::parse(
            at_str,
            &time::format_description::well_known::Iso8601::DEFAULT,
        ) {
            Ok(d) => d,
            Err(e) => {
                return MdmError::Unprocessable {
                    reason: format!("invalid ?at date '{at_str}': {e}"),
                }
                .into_response();
            }
        };
        return match vs_repo.find_at(&malo_id, &state.tenant_gln, at).await {
            Ok(Some(rec)) => {
                let version = rec.version;
                let mut resp_headers = HeaderMap::new();
                resp_headers.insert("ETag", etag(version).parse().unwrap());
                (
                    StatusCode::OK,
                    resp_headers,
                    Json(VersorgungsStatusResponse::from(rec)),
                )
                    .into_response()
            }
            Ok(None) => StatusCode::NOT_FOUND.into_response(),
            Err(e) => e.into_response(),
        };
    }

    match vs_repo.find(&malo_id, &state.tenant_gln).await {
        Ok(Some(rec)) => {
            let version = rec.version;
            let mut resp_headers = HeaderMap::new();
            resp_headers.insert("ETag", etag(version).parse().unwrap());
            (
                StatusCode::OK,
                resp_headers,
                Json(VersorgungsStatusResponse::from(rec)),
            )
                .into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => e.into_response(),
    }
}

/// GET /api/v1/versorgung/:malo_id/history
///
/// Returns the full supply-state change history for a MaLo, newest first.
/// Backed by the `versorgungsstatus_history` table.
#[allow(clippy::type_complexity)]
pub async fn get_versorgungsstatus_history<Ma, Me, Co, Su, Ci, Pa, Vs>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Path(malo_id): Path<String>,
    Query(query): Query<HistoryQuery>,
    Extension(vs_repo): Extension<Arc<Vs>>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
    Vs: VersorgungsStatusRepository + Send + Sync,
{
    if enforcer
        .check(
            &claims.principal(),
            "read-versorgungsstatus",
            &state.tenant_gln,
        )
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }
    let malo_id = match malo_id.parse::<MaloId>() {
        Ok(id) => id,
        Err(e) => {
            return MdmError::InvalidMaloId {
                id: malo_id,
                reason: e.to_string(),
            }
            .into_response();
        }
    };
    match vs_repo
        .find_history(&malo_id, &state.tenant_gln, query.page, query.size)
        .await
    {
        Ok(page) => Json(serde_json::json!({
            "items": page.items.into_iter().map(VersorgungsStatusHistoryResponse::from).collect::<Vec<_>>(),
            "total": page.total,
            "page":  page.page,
            "size":  page.size,
        }))
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// PUT /api/v1/versorgung/:malo_id
#[allow(clippy::type_complexity)]
pub async fn put_versorgungsstatus<Ma, Me, Co, Su, Ci, Pa, Vs>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Path(malo_id): Path<String>,
    Extension(vs_repo): Extension<Arc<Vs>>,
    headers: HeaderMap,
    Json(body): Json<VersorgungsStatusUpsertRequest>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
    Vs: VersorgungsStatusRepository + Send + Sync,
{
    if enforcer
        .check(
            &claims.principal(),
            "write-versorgungsstatus",
            &state.tenant_gln,
        )
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }
    let if_version = parse_if_match(&headers);
    let malo_id_str = malo_id.clone();
    let malo_id = match malo_id.parse::<MaloId>() {
        Ok(id) => id,
        Err(e) => {
            return MdmError::InvalidMaloId {
                id: malo_id_str,
                reason: e.to_string(),
            }
            .into_response();
        }
    };
    let lieferstatus: LieferStatus = match body.lieferstatus.parse() {
        Ok(s) => s,
        Err(reason) => return MdmError::Unprocessable { reason }.into_response(),
    };
    let lieferbeginn = body
        .lieferbeginn
        .as_deref()
        .map(|s| {
            time::Date::parse(s, &time::format_description::well_known::Iso8601::DEFAULT)
                .map_err(|e| format!("invalid lieferbeginn: {e}"))
        })
        .transpose();
    let lieferbeginn = match lieferbeginn {
        Ok(d) => d,
        Err(reason) => return MdmError::Unprocessable { reason }.into_response(),
    };
    let lieferende = body
        .lieferende
        .as_deref()
        .map(|s| {
            time::Date::parse(s, &time::format_description::well_known::Iso8601::DEFAULT)
                .map_err(|e| format!("invalid lieferende: {e}"))
        })
        .transpose();
    let lieferende = match lieferende {
        Ok(d) => d,
        Err(reason) => return MdmError::Unprocessable { reason }.into_response(),
    };
    let rec = VersorgungsStatusRecord {
        malo_id,
        lieferstatus,
        lf_mp_id: body.lf_mp_id,
        lf_mp_id_next: body.lf_mp_id_next,
        lf_next_lieferbeginn: body
            .lf_next_lieferbeginn
            .as_deref()
            .map(|s| time::Date::parse(s, &time::format_description::well_known::Iso8601::DEFAULT))
            .transpose()
            .unwrap_or(None),
        lieferbeginn,
        lieferende,
        msb_mp_id: body.msb_mp_id,
        nb_mp_id: body.nb_mp_id,
        last_process_id: body.last_process_id,
        updated_at: time::OffsetDateTime::now_utc(),
        tenant: state.tenant_gln.clone(),
        version: 0,
    };
    match vs_repo.upsert(rec, if_version).await {
        Ok(new_version) => {
            let mut resp_headers = HeaderMap::new();
            resp_headers.insert("ETag", etag(new_version).parse().unwrap());
            (StatusCode::OK, resp_headers).into_response()
        }
        Err(MdmError::VersionConflict { .. }) => StatusCode::PRECONDITION_FAILED.into_response(),
        Err(e) => e.into_response(),
    }
}

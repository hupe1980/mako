//! MaLo (Marktlokation) REST handlers.
//!
//! Routes:
//!   PUT    /api/v1/malo/:id
//!   GET    /api/v1/malo/:id
//!   GET    /api/v1/malo           (list / query)

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_mdm::{
    domain::{MaloId, Sparte},
    error::MdmError,
    repository::{
        AppState, ContractRepository, CorrelationIndex, Lokationszuordnung, MaloFilter,
        MaloRepository, MeloRepository, PageResult, PartnerRepository, SubscriptionRepository,
    },
};
use serde::{Deserialize, Serialize};
use time::Date;
use utoipa::{IntoParams, ToSchema};

use super::{Claims, IntoMdmResponse as _, etag, parse_if_match};

// ── DTOs ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct MaloUpsertRequest {
    /// "STROM" or "GAS"
    #[schema(value_type = String, example = "STROM")]
    pub sparte: Sparte,
    /// Full BO4E MARKTLOKATION payload.
    pub data: serde_json::Value,
    #[serde(default)]
    #[schema(value_type = Vec<Object>)]
    pub lokationszuordnung: Vec<Lokationszuordnung>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MaloResponse {
    pub malo_id: String,
    #[schema(value_type = String, example = "STROM")]
    pub sparte: Sparte,
    pub version: i64,
    pub data: serde_json::Value,
    #[schema(value_type = Vec<Object>)]
    pub lokationszuordnung: Vec<Lokationszuordnung>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListQuery {
    #[param(example = "STROM")]
    #[serde(default)]
    pub sparte: Option<String>,
    /// Filter by `zuordnungstyp` in active role assignments (e.g. `NB`, `LF`).
    #[serde(default)]
    pub zuordnungstyp: Option<String>,
    /// Filter by `rollencodenummer` (GLN) in active role assignments.
    #[serde(default)]
    pub rollencodenummer: Option<String>,
    #[param(example = 0)]
    #[serde(default)]
    pub page: u32,
    #[param(example = 50)]
    #[serde(default = "default_size")]
    pub size: u32,
}

fn default_size() -> u32 {
    50
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `PUT /api/v1/malo/:id`
#[utoipa::path(
    put,
    path = "/api/v1/malo/{id}",
    tag = "malo",
    params(("id" = String, Path, description = "11-digit MaLo-ID")),
    request_body = MaloUpsertRequest,
    responses(
        (status = 200, description = "Updated"),
        (status = 201, description = "Created"),
        (status = 409, description = "Version conflict"),
        (status = 422, description = "Validation error"),
    )
)]
pub async fn put_malo<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    headers: HeaderMap,
    _claims: Claims,
    Path(id): Path<String>,
    Json(req): Json<MaloUpsertRequest>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
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

    let if_match = parse_if_match(&headers);
    let exists = state
        .malo_repo
        .find(&malo_id, today_berlin())
        .await
        .ok()
        .flatten()
        .is_some();

    match state
        .malo_repo
        .upsert(
            &malo_id,
            req.sparte,
            req.data,
            req.lokationszuordnung,
            if_match,
        )
        .await
    {
        Ok(version) => {
            let status = if exists {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            };
            (
                status,
                [(axum::http::header::ETAG, etag(version))],
                Json(serde_json::json!({ "version": version })),
            )
                .into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/malo/:id`
#[utoipa::path(
    get,
    path = "/api/v1/malo/{id}",
    tag = "malo",
    params(("id" = String, Path, description = "11-digit MaLo-ID")),
    responses(
        (status = 200, description = "Found", body = MaloResponse),
        (status = 404, description = "Not found"),
    )
)]
pub async fn get_malo<Ma, Me, Co, Su, Ci, Pa>(
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

    match state.malo_repo.find(&malo_id, today_berlin()).await {
        Ok(Some(r)) => {
            let resp = MaloResponse {
                malo_id: r.malo_id.to_string(),
                sparte: r.sparte,
                version: r.version,
                data: r.data,
                lokationszuordnung: r.lokationszuordnung,
            };
            (
                StatusCode::OK,
                [(axum::http::header::ETAG, etag(r.version))],
                Json(resp),
            )
                .into_response()
        }
        Ok(None) => mako_mdm::error::MdmError::NotFound {
            resource_type: "malo",
            id,
        }
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/malo`
#[utoipa::path(
    get,
    path = "/api/v1/malo",
    tag = "malo",
    params(ListQuery),
    responses(
        (status = 200, description = "List of Marktlokationen", body = Vec<MaloResponse>),
    )
)]
pub async fn list_malo<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    _claims: Claims,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    let sparte = q.sparte.as_deref().and_then(|s| s.parse::<Sparte>().ok());
    let filter = MaloFilter {
        sparte,
        zuordnungstyp: q.zuordnungstyp,
        rollencodenummer: q.rollencodenummer,
        page: q.page,
        size: q.size.min(500),
    };

    match state.malo_repo.list(filter, today_berlin()).await {
        Ok(page) => {
            let items: Vec<MaloResponse> = page
                .items
                .into_iter()
                .map(|r| MaloResponse {
                    malo_id: r.malo_id.to_string(),
                    sparte: r.sparte,
                    version: r.version,
                    data: r.data,
                    lokationszuordnung: r.lokationszuordnung,
                })
                .collect();
            Json(PageResult {
                items,
                total: page.total,
                page: page.page,
                size: page.size,
            })
            .into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// Returns today's date in German local time (CET/CEST via Europe/Berlin).
///
/// Regulatory deadlines and `lokationszuordnung` validity queries must use
/// German local time, not UTC.  Using UTC causes off-by-one errors around
/// midnight during winter/summer transitions.
fn today_berlin() -> Date {
    let tz = jiff::tz::TimeZone::get("Europe/Berlin")
        .expect("jiff bundles IANA tz data; Europe/Berlin always present");
    let zoned = jiff::Timestamp::now().to_zoned(tz);
    let d = zoned.date();
    time::Date::from_calendar_date(
        d.year() as i32,
        time::Month::try_from(d.month() as u8).expect("jiff month 1..=12 always valid"),
        d.day() as u8,
    )
    .expect("jiff date maps to a valid time::Date")
}

//! VersorgungsStatus REST handlers.
//!
//! Routes:
//!   GET  /api/v1/versorgung/:malo_id            — current supply state (or `?at=YYYY-MM-DD`)
//!   GET  /api/v1/versorgung/:malo_id/history    — full supply-state change history
//!   PUT  /api/v1/versorgung/:malo_id            — upsert supply state (ERP / processd)

use crate::state::AppState;
use std::sync::Arc;

use axum::{
    Extension,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_markt::{
    cloudevents::{EventExtensions, MarktEvent},
    domain::MaloId,
    error::MdmError,
    repository::{LfZuordnung, LieferStatus, VersorgungsStatusRecord, ZuordnungsStatus},
};
use mako_service::cedar::CedarEnforcer;
use mako_service::{ApiError, ApiResult, Json};
use serde::Deserialize;

use crate::pg::PgVersorgungsStatusRepository;
use utoipa::{IntoParams, ToSchema};

use super::{Claims, IfMatch, IntoMdmResponse as _, etag, malformed_if_match, parse_if_match};

// ── DTOs ──────────────────────────────────────────────────────────────────────

pub use crate::reads::{VersorgungsStatusHistorie, VersorgungsStatusResponse};

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VersorgungsStatusUpsertRequest {
    #[schema(value_type = String, example = "Beliefert")]
    pub lieferstatus: String,
    /// The assignment list, written wholesale — what is sent replaces what is
    /// stored, so an omitted assignment is a removed one.
    ///
    /// This is the only way to state a tranchierte Marktlokation. The scalar
    /// `lf_mp_id` / `lf_mp_id_next` below are a shorthand for the ordinary
    /// one-supplier case and are ignored when `zuordnungen` is present.
    #[schema(value_type = Vec<Object>)]
    #[serde(default)]
    pub zuordnungen: Option<Vec<LfZuordnung>>,
    pub lf_mp_id: Option<String>,
    pub lf_mp_id_next: Option<String>,
    pub lf_next_lieferbeginn: Option<String>,
    pub lieferbeginn: Option<String>,
    pub lieferende: Option<String>,
    pub msb_mp_id: Option<String>,
    pub nb_mp_id: String,
    /// Start of the Ersatz-/Grundversorgung (ISO date). Required when
    /// `lieferstatus` is `Ersatzversorgung`/`Grundversorgung`, forbidden
    /// otherwise (DB CHECK).
    pub eog_seit: Option<String>,
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

/// `GET /api/v1/versorgung/{malo_id}` — the current supply state, or with
/// `?at=YYYY-MM-DD` the state as of that German calendar day.
#[utoipa::path(
    get,
    path = "/api/v1/versorgung/{malo_id}",
    tag = "versorgung",
    operation_id = "get_versorgung_at",
    params(("malo_id" = String, Path, description = "11-digit Marktlokations-ID"), VersorgungQuery),
    responses(
        (status = 200, description = "The supply state; `ETag` carries its version", body = VersorgungsStatusResponse),
        (status = 403, description = "Missing read-versorgungsstatus"),
        (status = 404, description = "`versorgung_not_found`"),
        (status = 422, description = "`invalid_malo_id` or an invalid `at`"),
    )
)]
pub async fn get_versorgungsstatus(
    State(state): State<Arc<AppState>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<sqlx::PgPool>,
    claims: Claims,
    Path(malo_id): Path<String>,
    Query(query): Query<VersorgungQuery>,
) -> ApiResult<impl IntoResponse> {
    enforcer
        .check(&claims.principal(), "read-versorgungsstatus", &state.tenant)
        .map_err(|_| ApiError::forbidden("access denied"))?;
    let status =
        crate::reads::versorgung(&pool, &state.tenant, &malo_id, query.at.as_deref()).await?;
    let mut resp_headers = HeaderMap::new();
    if let Ok(v) = etag(status.version).parse() {
        resp_headers.insert("ETag", v);
    }
    Ok((StatusCode::OK, resp_headers, Json(status)))
}

/// `GET /api/v1/versorgung/{malo_id}/history` — the supply-state change
/// history, newest first, paged.
#[utoipa::path(
    get,
    path = "/api/v1/versorgung/{malo_id}/history",
    tag = "versorgung",
    operation_id = "get_versorgungsstatus_history",
    params(("malo_id" = String, Path, description = "11-digit Marktlokations-ID"), HistoryQuery),
    responses(
        (status = 200, description = "One page of the history", body = VersorgungsStatusHistorie),
        (status = 403, description = "Missing read-versorgungsstatus"),
        (status = 422, description = "`invalid_malo_id`"),
    )
)]
pub async fn get_versorgungsstatus_history(
    State(state): State<Arc<AppState>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<sqlx::PgPool>,
    claims: Claims,
    Path(malo_id): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> ApiResult<Json<VersorgungsStatusHistorie>> {
    enforcer
        .check(&claims.principal(), "read-versorgungsstatus", &state.tenant)
        .map_err(|_| ApiError::forbidden("access denied"))?;
    crate::reads::versorgungsstatus_history(&pool, &state.tenant, &malo_id, query.page, query.size)
        .await
        .map(Json)
}

/// PUT /api/v1/versorgung/:malo_id
#[utoipa::path(
    put,
    path = "/api/v1/versorgung/{malo_id}",
    tag = "versorgung",
    params(("malo_id" = String, Path, description = "11-digit Marktlokations-ID")),
    request_body = VersorgungsStatusUpsertRequest,
    responses(
        (status = 200, description = "OK", body = serde_json::Value),
        (status = 401, description = "No valid credentials", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 422, description = "The request was refused; `code` names the rule", body = ApiError),
    )
)]
pub async fn put_versorgungsstatus(
    State(state): State<Arc<AppState>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<sqlx::PgPool>,
    claims: Claims,
    Path(malo_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<VersorgungsStatusUpsertRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(
            &claims.principal(),
            "write-versorgungsstatus",
            &state.tenant,
        )
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }
    let if_version = match parse_if_match(&headers) {
        IfMatch::Absent | IfMatch::Any => None,
        IfMatch::Version(v) => Some(v),
        // Refuse rather than fall back to an unconditional write: the caller
        // asked for a conditional one and would otherwise be told it succeeded.
        IfMatch::Malformed => return malformed_if_match(),
    };
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
    // `zuordnungen` wins when present; otherwise the scalar shorthand is
    // expanded into the one- or two-assignment list it stands for.
    let zuordnungen = body.zuordnungen.unwrap_or_else(|| {
        let lf_next_lieferbeginn = body
            .lf_next_lieferbeginn
            .as_deref()
            .map(|s| time::Date::parse(s, &time::format_description::well_known::Iso8601::DEFAULT))
            .transpose()
            .unwrap_or(None);
        let mut list = Vec::new();
        if let Some(lf) = body.lf_mp_id {
            list.push(LfZuordnung {
                zuordnungsbeginn: lieferbeginn,
                process_id: body.last_process_id,
                ..LfZuordnung::ganz(lf, ZuordnungsStatus::Aktiv)
            });
        }
        if let Some(lf) = body.lf_mp_id_next {
            list.push(LfZuordnung {
                zuordnungsbeginn: lf_next_lieferbeginn,
                process_id: body.last_process_id,
                ..LfZuordnung::ganz(lf, ZuordnungsStatus::Angekuendigt)
            });
        }
        list
    });
    let rec = VersorgungsStatusRecord {
        malo_id,
        lieferstatus,
        zuordnungen,
        lieferende,
        msb_mp_id: body.msb_mp_id,
        nb_mp_id: body.nb_mp_id,
        eog_seit: body
            .eog_seit
            .as_deref()
            .map(|s| time::Date::parse(s, &time::format_description::well_known::Iso8601::DEFAULT))
            .transpose()
            .unwrap_or(None),
        last_process_id: body.last_process_id,
        updated_at: time::OffsetDateTime::now_utc(),
        tenant: state.tenant.clone(),
        version: 0,
    };
    // Persist-before-dispatch: the state change and the de.mako.markt.versorgung.changed
    // outbox row commit in ONE transaction, so a crash can never change the supply
    // state without emitting the event.
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "versorgung: begin failed");
            return MdmError::Internal(e.to_string()).into_response();
        }
    };

    let new_version =
        match PgVersorgungsStatusRepository::upsert_tx(&mut tx, &rec, if_version).await {
            Ok(v) => v,
            Err(e) => return e.into_response(),
        };

    // Emit de.mako.markt.versorgung.changed so ERP subscribers (vertragd, billingd)
    // are notified of supply-state transitions (Lieferbeginn, Lieferende,
    // Lieferant changes) in near-real-time without polling.
    // The Sparte a subscriber filters on lives on the MaLo, not on the supply
    // row. Read it in the same transaction so this event carries the same
    // `marktsparte` as its EDIFACT-driven twin in `event_ingest`; a path that
    // omits it bypasses every `sparten` filter.
    // `malo` is tenant-global — the Marktlokation is the market's object, not a
    // tenant's — so the lookup keys on the MaLo-ID alone. A failure here is not
    // "no Sparte": it aborts the surrounding transaction, so it must surface
    // rather than resolve to `None`.
    let sparte: Option<String> =
        match sqlx::query_scalar("SELECT sparte FROM malo WHERE malo_id = $1")
            .bind(&malo_id_str)
            .fetch_optional(&mut *tx)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                return ApiError::internal(
                    anyhow::Error::from(e)
                        .context(format!("versorgung: Sparte lookup for MaLo {malo_id_str}")),
                )
                .into_response();
            }
        };

    let evt = MarktEvent::new(
        &state.tenant,
        mako_events::markt::VERSORGUNG_CHANGED,
        malo_id_str.clone(),
        serde_json::json!({
            "malo_id": malo_id_str,
            "lieferstatus": body.lieferstatus,
            "sparte": sparte,
            "version": new_version,
        }),
    )
    .with_extensions(EventExtensions {
        marktmaloid: Some(malo_id_str.clone()),
        marktsparte: sparte,
        ..Default::default()
    });
    if let Err(e) = crate::outbox::enqueue(&mut tx, &evt).await {
        return ApiError::internal(anyhow::Error::from(e).context(format!(
            "versorgung: durable enqueue for MaLo {malo_id_str}"
        )))
        .into_response();
    }
    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, "versorgung: commit failed");
        return MdmError::Internal(e.to_string()).into_response();
    }

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert("ETag", etag(new_version).parse().unwrap());
    (StatusCode::OK, resp_headers).into_response()
}

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
    cloudevents::{EventExtensions, MarktEvent},
    domain::MaloId,
    error::MdmError,
    repository::{
        AppState, CorrelationIndex, LfZuordnung, LieferStatus, MaloRepository, MeloRepository,
        PartnerRepository, SubscriptionRepository, VersorgungsStatusHistoryRecord,
        VersorgungsStatusRecord, VersorgungsStatusRepository, ZuordnungsStatus,
    },
};
use mako_service::cedar::CedarEnforcer;
use serde::{Deserialize, Serialize};

use crate::pg::PgVersorgungsStatusRepository;
use time::format_description::well_known::Rfc3339;
use utoipa::{IntoParams, ToSchema};

use super::{Claims, IfMatch, IntoMdmResponse as _, etag, malformed_if_match, parse_if_match};

// ── DTOs ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct VersorgungsStatusResponse {
    pub malo_id: String,
    #[schema(value_type = String, example = "Beliefert")]
    pub lieferstatus: String,
    /// Every Lieferant holding a share of this Marktlokation — the authoritative
    /// answer to „who supplies it". A tranchierte Marktlokation carries several.
    #[schema(value_type = Vec<Object>)]
    pub zuordnungen: Vec<LfZuordnung>,
    /// The single active Lieferant, when there is exactly one.
    ///
    /// Derived from `zuordnungen` and **absent on a tranchierte
    /// Marktlokation**, where no single supplier exists — read `zuordnungen`
    /// there rather than treating the omission as „unsupplied".
    pub lf_mp_id: Option<String>,
    /// The single announced Lieferant, when exactly one Anmeldung is pending.
    /// Derived, with the same caveat as `lf_mp_id`.
    pub lf_mp_id_next: Option<String>,
    pub lf_next_lieferbeginn: Option<String>,
    pub lieferbeginn: Option<String>,
    pub lieferende: Option<String>,
    pub msb_mp_id: Option<String>,
    pub nb_mp_id: String,
    /// Start of the running Ersatz-/Grundversorgung (§38/§36 EnWG), if any.
    pub eog_seit: Option<String>,
    pub last_process_id: Option<String>,
    pub updated_at: String,
    pub version: i64,
}

impl From<VersorgungsStatusRecord> for VersorgungsStatusResponse {
    fn from(r: VersorgungsStatusRecord) -> Self {
        Self {
            malo_id: r.malo_id.as_ref().to_owned(),
            lieferstatus: r.lieferstatus.to_string(),
            lf_mp_id: r.lf_mp_id().map(ToOwned::to_owned),
            lf_mp_id_next: r.lf_mp_id_next().map(ToOwned::to_owned),
            lf_next_lieferbeginn: r.lf_next_lieferbeginn().map(|d| d.to_string()),
            lieferbeginn: r.lieferbeginn().map(|d| d.to_string()),
            zuordnungen: r.zuordnungen,
            lieferende: r.lieferende.map(|d| d.to_string()),
            msb_mp_id: r.msb_mp_id,
            nb_mp_id: r.nb_mp_id,
            eog_seit: r.eog_seit.map(|d| d.to_string()),
            last_process_id: r.last_process_id.map(|u| u.to_string()),
            updated_at: r
                .updated_at
                .format(&Rfc3339)
                .unwrap_or_else(|_| r.updated_at.to_string()),
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
    /// The assignment list as it stood at `valid_from`.
    #[schema(value_type = Vec<Object>)]
    pub zuordnungen: Vec<LfZuordnung>,
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
            zuordnungen: r.zuordnungen,
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

/// GET /api/v1/versorgung/:malo_id
///
/// Returns the current supply state.  Add `?at=YYYY-MM-DD` to query the state
/// as of a specific calendar date (German local time, CET/CEST).
#[expect(clippy::type_complexity)]
pub async fn get_versorgungsstatus<Ma, Me, Su, Ci, Pa, Vs>(
    State(state): State<Arc<AppState<Ma, Me, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Path(malo_id): Path<String>,
    Query(query): Query<VersorgungQuery>,
    Extension(vs_repo): Extension<Arc<Vs>>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
    Vs: VersorgungsStatusRepository + Send + Sync,
{
    if enforcer
        .check(&claims.principal(), "read-versorgungsstatus", &state.tenant)
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
        return match vs_repo.find_at(&malo_id, &state.tenant, at).await {
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

    match vs_repo.find(&malo_id, &state.tenant).await {
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
#[expect(clippy::type_complexity)]
pub async fn get_versorgungsstatus_history<Ma, Me, Su, Ci, Pa, Vs>(
    State(state): State<Arc<AppState<Ma, Me, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Path(malo_id): Path<String>,
    Query(query): Query<HistoryQuery>,
    Extension(vs_repo): Extension<Arc<Vs>>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
    Vs: VersorgungsStatusRepository + Send + Sync,
{
    if enforcer
        .check(&claims.principal(), "read-versorgungsstatus", &state.tenant)
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
        .find_history(&malo_id, &state.tenant, query.page, query.size)
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
#[expect(clippy::type_complexity)]
pub async fn put_versorgungsstatus<Ma, Me, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<sqlx::PgPool>,
    claims: Claims,
    Path(malo_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<VersorgungsStatusUpsertRequest>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
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
    // Persist-before-dispatch: the state change and the de.markt.versorgung.changed
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
            Err(MdmError::VersionConflict { .. }) => {
                return StatusCode::PRECONDITION_FAILED.into_response();
            }
            Err(e) => return e.into_response(),
        };

    // Emit de.markt.versorgung.changed so ERP subscribers (vertragd, billingd)
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
    let sparte: Option<String> = match sqlx::query_scalar(
        "SELECT sparte FROM malo WHERE malo_id = $1",
    )
    .bind(&malo_id_str)
    .fetch_optional(&mut *tx)
    .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, malo = %malo_id_str, "versorgung: Sparte lookup failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
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
        marktmaloid: Some(malo_id_str),
        marktsparte: sparte,
        ..Default::default()
    });
    if let Err(e) = crate::outbox::enqueue(&mut *tx, &evt, &state.notify).await {
        tracing::error!(error = %e, "versorgung: durable enqueue failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, "versorgung: commit failed");
        return MdmError::Internal(e.to_string()).into_response();
    }

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert("ETag", etag(new_version).parse().unwrap());
    (StatusCode::OK, resp_headers).into_response()
}

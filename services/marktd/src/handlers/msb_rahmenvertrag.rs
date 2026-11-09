//! MSB-Rahmenvertrag registry API, both Sparten.
//!
//! The contract the NB checks before it confirms an Anmeldung MSB (WiM Teil 1
//! Kap. 2.3.2 Nr. 2 Ziff. 3; AWH WiM Gas 2.0 Kap. 3.5.2 Nr. 2 Ziff. 3), per
//! (Sparte, NB, MSB) — see [`crate::pg::msb_rahmenvertrag`] for the Gas-only
//! Fassung and adaptation duty of GeLi Gas 3.0 Tenor 13–15. Every change
//! emits `de.markt.msb-rahmenvertrag.updated` in the write's transaction;
//! `GET /api/v1/msb-rahmenvertraege/check` is the lookup `processd`'s
//! Anmeldung MSB arm reads.

use mako_service::ApiError;
use std::sync::Arc;

use axum::{
    Extension,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::domain::{MarktpartnerId, Sparte};
use mako_markt::{cloudevents::MarktEvent, error::MdmError};
use mako_service::Json;
use mako_service::cedar::CedarEnforcer;
use serde::Deserialize;
use uuid::Uuid;

use crate::pg::PgMsbRahmenvertragRepository;
use crate::pg::msb_rahmenvertrag::{
    MsbRahmenvertrag, MsbRvFilter, MsbRvPruefung, MsbRvStatus, MsbRvUpsertRequest,
};

use super::{Claims, IntoMdmResponse as _, Tenant};

/// Injected `Arc<PgMsbRahmenvertragRepository>`.
pub type MsbRvRepoExt = Arc<PgMsbRahmenvertragRepository>;

/// `PUT /api/v1/msb-rahmenvertraege` — upsert a conclusion record.
///
/// Idempotent on the natural key `(sparte, nb_mp_id, msb_mp_id, valid_from)`;
/// the record id stays stable across re-submits. A non-zero `version` in the
/// body acts as an optimistic-locking guard. The row and its CloudEvent
/// commit in one transaction.
#[utoipa::path(
    put,
    path = "/api/v1/msb-rahmenvertraege",
    tag = "msb-rahmenvertraege",
    request_body = MsbRvUpsertRequest,
    responses(
        (status = 200, description = "Upserted; returns stable id and new version"),
        (status = 403, description = "Missing write-msb-rv scope"),
        (status = 412, description = "Supplied version does not match the stored version"),
        (status = 422, description = "An invalid window, a Strom row with a Gas-only status, or a window overlapping another contract of the same parties and Sparte"),
    )
)]
pub async fn upsert_msb_rv(
    claims: Claims,
    Extension(repo): Extension<MsbRvRepoExt>,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Json(rec): Json<MsbRvUpsertRequest>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "write-msb-rv", &tenant) {
        tracing::warn!(error = %e, "marktd: Cedar denied write-msb-rv");
        return ApiError::forbidden("write-msb-rv denied").into_response();
    }
    if let Err(e) = rec.validate() {
        return e.into_response();
    }
    let mut tx = match repo.pool().begin().await {
        Ok(tx) => tx,
        Err(e) => return MdmError::Internal(e.to_string()).into_response(),
    };
    let (id, version) = match PgMsbRahmenvertragRepository::upsert_tx(&mut tx, &tenant, &rec).await
    {
        Ok(out) => out,
        Err(e) => return e.into_response(),
    };
    let evt = MarktEvent::new(
        &tenant,
        mako_events::markt::MSB_RAHMENVERTRAG_UPDATED,
        id.to_string(),
        serde_json::json!({
            "id": id,
            "sparte": rec.sparte.to_string(),
            "nb_mp_id": &*rec.nb_mp_id,
            "msb_mp_id": &*rec.msb_mp_id,
            "fassung": rec.effective_fassung(),
            "status": rec.status,
            "version": version,
            "valid_from": rec.valid_from,
            "valid_to": rec.valid_to,
            "signed_at": rec.signed_at.and_then(|t| {
                t.format(&time::format_description::well_known::Rfc3339).ok()
            }),
        }),
    );
    if let Err(e) = crate::outbox::enqueue(&mut *tx, &evt).await {
        return ApiError::internal(anyhow::Error::from(e).context(format!(
            "msb_rv: durable enqueue for MSB-Rahmenvertrag {id}"
        )))
        .into_response();
    }
    if let Err(e) = tx.commit().await {
        return MdmError::Internal(e.to_string()).into_response();
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({ "id": id, "version": version })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    #[serde(default)]
    pub sparte: Option<Sparte>,
    #[serde(default)]
    pub nb_mp_id: Option<String>,
    #[serde(default)]
    pub msb_mp_id: Option<String>,
    #[serde(default)]
    pub status: Option<MsbRvStatus>,
}

/// `GET /api/v1/msb-rahmenvertraege` — list conclusion records.
#[utoipa::path(
    get,
    path = "/api/v1/msb-rahmenvertraege",
    tag = "msb-rahmenvertraege",
    params(
        ("sparte" = Option<String>, Query, description = "Filter by Sparte (STROM / GAS)"),
        ("nb_mp_id" = Option<String>, Query, description = "Filter by NB MP-ID"),
        ("msb_mp_id" = Option<String>, Query, description = "Filter by MSB MP-ID"),
        ("status" = Option<String>, Query, description = "Filter by lifecycle status"),
    ),
    responses(
        (status = 200, description = "Conclusion records, newest valid_from first", body = [MsbRahmenvertrag]),
        (status = 403, description = "Missing read-msb-rv scope"),
    )
)]
pub async fn list_msb_rv(
    claims: Claims,
    Extension(repo): Extension<MsbRvRepoExt>,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-msb-rv", &tenant) {
        tracing::warn!(error = %e, "marktd: Cedar denied read-msb-rv");
        return ApiError::forbidden("read-msb-rv denied").into_response();
    }
    let filter = MsbRvFilter {
        sparte: q.sparte,
        nb_mp_id: q.nb_mp_id.as_deref(),
        msb_mp_id: q.msb_mp_id.as_deref(),
        status: q.status,
    };
    match repo.list(&tenant, &filter).await {
        Ok(rows) => (StatusCode::OK, Json(serde_json::json!({ "data": rows }))).into_response(),
        Err(e) => e.into_response(),
    }
}

/// The query of `GET /api/v1/msb-rahmenvertraege/check` — every field
/// required, because each one selects which contract answers.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckQuery {
    pub sparte: Sparte,
    pub nb_mp_id: MarktpartnerId,
    pub msb_mp_id: MarktpartnerId,
    /// The day the contract must be valid on — for an Anmeldung MSB, the
    /// Zuordnungsbeginn.
    pub datum: time::Date,
}

/// `GET /api/v1/msb-rahmenvertraege/check` — is a contract `abgeschlossen`
/// and valid on `datum`?
///
/// Always `200` with a Befund: `abgeschlossen: false` names why (no contract,
/// only offered, to be adapted, ended). A database failure is `500`, never a
/// `false` — an outage is not evidence that no contract exists.
#[utoipa::path(
    get,
    path = "/api/v1/msb-rahmenvertraege/check",
    tag = "msb-rahmenvertraege",
    params(
        ("sparte" = String, Query, description = "STROM or GAS"),
        ("nb_mp_id" = String, Query, description = "The NB's MP-ID in that Sparte"),
        ("msb_mp_id" = String, Query, description = "The MSB's MP-ID"),
        ("datum" = String, Query, description = "The day the contract must be valid on (YYYY-MM-DD)"),
    ),
    responses(
        (status = 200, description = "The Befund for that day", body = MsbRvPruefung),
        (status = 400, description = "A missing or malformed query parameter"),
        (status = 403, description = "Missing read-msb-rv scope"),
    )
)]
pub async fn check_msb_rv(
    claims: Claims,
    Extension(repo): Extension<MsbRvRepoExt>,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Query(q): Query<CheckQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-msb-rv", &tenant) {
        tracing::warn!(error = %e, "marktd: Cedar denied read-msb-rv");
        return ApiError::forbidden("read-msb-rv denied").into_response();
    }
    match repo
        .pruefe(&tenant, q.sparte, &q.nb_mp_id, &q.msb_mp_id, q.datum)
        .await
    {
        Ok(p) => (StatusCode::OK, Json(p)).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/msb-rahmenvertraege/{id}` — fetch one record.
#[utoipa::path(
    get,
    path = "/api/v1/msb-rahmenvertraege/{id}",
    tag = "msb-rahmenvertraege",
    params(("id" = Uuid, Path, description = "Record id")),
    responses(
        (status = 200, description = "The record", body = MsbRahmenvertrag),
        (status = 403, description = "Missing read-msb-rv scope"),
        (status = 404, description = "Unknown id"),
    )
)]
pub async fn get_msb_rv(
    claims: Claims,
    Extension(repo): Extension<MsbRvRepoExt>,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-msb-rv", &tenant) {
        tracing::warn!(error = %e, "marktd: Cedar denied read-msb-rv");
        return ApiError::forbidden("read-msb-rv denied").into_response();
    }
    match repo.get(&tenant, id).await {
        Ok(Some(rec)) => (StatusCode::OK, Json(serde_json::json!({ "data": rec }))).into_response(),
        Ok(None) => {
            ApiError::not_found(format!("MSB-Rahmenvertrag {id} not found")).into_response()
        }
        Err(e) => e.into_response(),
    }
}

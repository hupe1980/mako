//! NB network contract REST handlers.
//!
//! Routes:
//!   PUT  /api/v1/nb-contracts/:id
//!   GET  /api/v1/nb-contracts/:id
//!   GET  /api/v1/nb-contracts?nb_mp_id=...
//!
//! NB contracts are typed records (netzebene, bilanzierungsmethode,
//! billing_schedule) stored in the `nb_contracts` table.  Unlike LF supply
//! contracts (opaque JSONB), NB contracts enable SQL queries by `netzebene`
//! and `bilanzierungsmethode` in `invoicd`.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::{
    domain::{MaloId, Sparte},
    error::MdmError,
    repository::{
        AppState, BillingSchedule, ContractRepository, CorrelationIndex, MaloRepository,
        MeloRepository, NbContractRecord, NbContractRepository, PartnerRepository,
        SubscriptionRepository,
    },
};
use mako_service::cedar::CedarEnforcer;
use serde::{Deserialize, Serialize};
use time::macros::format_description;
use utoipa::ToSchema;

use crate::pg::PgNbContractRepository;

use super::{Claims, IntoMdmResponse as _, TenantGln};

/// Extension alias — `PgNbContractRepository` is concrete so AFIT works.
pub type NbContractRepoExt = Arc<PgNbContractRepository>;

// ── DTOs ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct NbContractUpsertRequest {
    pub malo_id: String,
    pub nb_mp_id: String,
    #[schema(value_type = String, example = "STROM")]
    pub sparte: Sparte,
    /// Voltage / pressure level: NS | MS | MSP | HSP | HS | HöS | HöS/HS
    pub netzebene: String,
    /// RLM | SLP
    pub bilanzierungsmethode: String,
    /// MONTHLY | QUARTERLY | ANNUALLY
    pub billing_schedule: String,
    /// Contract start date (ISO 8601, e.g. `"2026-01-01"`)
    pub valid_from: String,
    #[serde(default)]
    pub valid_to: Option<String>,
    /// Tenant ID.  Defaults to the operator primary GLN if absent.
    #[serde(default)]
    pub tenant: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NbContractResponse {
    pub contract_id: String,
    pub malo_id: String,
    pub nb_mp_id: String,
    pub sparte: String,
    pub netzebene: String,
    pub bilanzierungsmethode: String,
    pub billing_schedule: String,
    pub valid_from: String,
    pub valid_to: Option<String>,
    pub version: i64,
    pub tenant: String,
}

#[derive(Debug, Deserialize)]
pub struct ListNbContractsQuery {
    pub nb_mp_id: Option<String>,
}

// ── Parse helpers ─────────────────────────────────────────────────────────────

fn parse_req(
    id: String,
    req: NbContractUpsertRequest,
    tenant_gln: &str,
) -> Result<NbContractRecord, (StatusCode, serde_json::Value)> {
    let date_fmt = format_description!("[year]-[month]-[day]");

    let valid_from = time::Date::parse(&req.valid_from, date_fmt).map_err(|_| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "error": "valid_from must be YYYY-MM-DD" }),
        )
    })?;

    let valid_to = req
        .valid_to
        .as_deref()
        .map(|s| time::Date::parse(s, date_fmt))
        .transpose()
        .map_err(|_| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                serde_json::json!({ "error": "valid_to must be YYYY-MM-DD" }),
            )
        })?;

    let billing_schedule = BillingSchedule::from_str_or_default(&req.billing_schedule);
    let tenant = req
        .tenant
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| tenant_gln.to_owned());

    let malo_id = req.malo_id.parse::<MaloId>().map_err(|e| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "error": format!("invalid malo_id: {e}") }),
        )
    })?;

    Ok(NbContractRecord {
        contract_id: id,
        malo_id,
        nb_mp_id: req.nb_mp_id,
        sparte: req.sparte,
        netzebene: req.netzebene,
        bilanzierungsmethode: req.bilanzierungsmethode,
        billing_schedule,
        valid_from,
        valid_to,
        tenant,
        version: 0,
    })
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `PUT /api/v1/nb-contracts/:id`
pub async fn put_nb_contract(
    Extension(repo): Extension<NbContractRepoExt>,
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(tenant_gln): Extension<TenantGln>,
    Path(id): Path<String>,
    Json(req): Json<NbContractUpsertRequest>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "write-nb-contract", &tenant_gln.0) {
        tracing::warn!(error = %e, "marktd: Cedar denied write-nb-contract");
        return StatusCode::FORBIDDEN.into_response();
    }

    let rec = match parse_req(id, req, &tenant_gln.0) {
        Ok(r) => r,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };

    match repo.upsert(rec).await {
        Ok(version) => Json(serde_json::json!({ "version": version })).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/nb-contracts/:id`
pub async fn get_nb_contract(
    Extension(repo): Extension<NbContractRepoExt>,
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(tenant_gln): Extension<TenantGln>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-nb-contract", &tenant_gln.0) {
        tracing::warn!(error = %e, "marktd: Cedar denied read-nb-contract");
        return StatusCode::FORBIDDEN.into_response();
    }

    match repo.find(&id).await {
        Ok(Some(r)) => Json(rec_to_response(r)).into_response(),
        Ok(None) => MdmError::NotFound {
            resource_type: "nb_contract",
            id,
        }
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/nb-contracts?nb_mp_id=...`
pub async fn list_nb_contracts(
    Extension(repo): Extension<NbContractRepoExt>,
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(tenant_gln): Extension<TenantGln>,
    Query(q): Query<ListNbContractsQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-nb-contract", &tenant_gln.0) {
        tracing::warn!(error = %e, "marktd: Cedar denied read-nb-contract");
        return StatusCode::FORBIDDEN.into_response();
    }

    let nb_mp_id = match q.nb_mp_id {
        Some(g) => g,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "nb_mp_id query parameter required" })),
            )
                .into_response();
        }
    };

    match repo.list_by_nb(&nb_mp_id, &tenant_gln.0).await {
        Ok(recs) => Json(recs.into_iter().map(rec_to_response).collect::<Vec<_>>()).into_response(),
        Err(e) => e.into_response(),
    }
}

// Silence unused-import warnings for generic bounds that are needed for
// the State extractor in other handlers but not used directly here.
#[allow(dead_code)]
fn _assert_bounds<Ma, Me, Co, Su, Ci, Pa>(_: &AppState<Ma, Me, Co, Su, Ci, Pa>)
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn rec_to_response(r: NbContractRecord) -> NbContractResponse {
    let date_fmt = format_description!("[year]-[month]-[day]");
    NbContractResponse {
        contract_id: r.contract_id,
        malo_id: r.malo_id.as_ref().to_owned(),
        nb_mp_id: r.nb_mp_id,
        sparte: r.sparte.to_string(),
        netzebene: r.netzebene,
        bilanzierungsmethode: r.bilanzierungsmethode,
        billing_schedule: r.billing_schedule.to_string(),
        valid_from: r.valid_from.format(date_fmt).unwrap_or_default(),
        valid_to: r.valid_to.map(|d| d.format(date_fmt).unwrap_or_default()),
        version: r.version,
        tenant: r.tenant,
    }
}

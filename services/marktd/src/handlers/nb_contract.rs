//! NB network contract REST handlers (L1 — Vertrag BO4E typing).
//!
//! Routes:
//!   PUT  /api/v1/nb-contracts/:id
//!   GET  /api/v1/nb-contracts/:id
//!   GET  /api/v1/nb-contracts?nb_mp_id=...
//!
//! NB contracts are stored as typed SQL columns (fast queries by `invoicd` and
//! `processd`) PLUS a full BO4E `Vertrag` JSONB payload for digital LRV exchange
//! with ERP systems.  The `vertragsart` and `vertragsstatus` columns are
//! extracted from `data` on every write.
//!
//! A `de.markt.nb-contract.updated` CloudEvent is emitted on every successful
//! upsert so subscribers can rebuild Vertrag caches without polling.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::{
    cloudevents::MarktEvent,
    domain::{MaloId, Sparte},
    error::MdmError,
    repository::{
        AppState, BillingSchedule, ContractRepository, CorrelationIndex, MaloRepository,
        MeloRepository, NbContractRecord, NbContractRepository, PartnerRepository,
        SubscriptionRepository,
    },
};
use mako_service::cedar::CedarEnforcer;
use rubo4e::current::Vertrag;
use serde::{Deserialize, Serialize};
use time::macros::format_description;
use utoipa::ToSchema;

use crate::pg::PgNbContractRepository;

use super::{Claims, IntoMdmResponse as _, TenantGln};

/// Extension alias — `PgNbContractRepository` is concrete so AFIT works.
pub type NbContractRepoExt = Arc<PgNbContractRepository>;

// ── Vertrag validation helper ─────────────────────────────────────────────────

/// Validate and normalise a `Vertrag` BO4E payload (L1 hard cut).
///
/// 1. Auto-inject `_typ: "VERTRAG"` when absent.
/// 2. Reject 422 if `_typ` is present but does not equal `"VERTRAG"`.
/// 3. Deserialise as `rubo4e::current::Vertrag` — validates all enum fields.
/// 4. Re-serialise to canonical camelCase form for durable storage.
fn normalize_vertrag(
    mut data: serde_json::Value,
) -> Result<(Vertrag, serde_json::Value), (StatusCode, serde_json::Value)> {
    if let Some(obj) = data.as_object_mut() {
        obj.entry("_typ")
            .or_insert_with(|| serde_json::json!("VERTRAG"));
    }
    if let Some(typ) = data.get("_typ").and_then(|v| v.as_str())
        && typ.to_uppercase() != "VERTRAG"
    {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "error": format!("expected _typ VERTRAG, got '{typ}'") }),
        ));
    }
    let vertrag: Vertrag = serde_json::from_value(data).map_err(|e| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "error": format!("invalid Vertrag payload: {e}") }),
        )
    })?;
    let canonical = serde_json::to_value(&vertrag).unwrap_or_default();
    Ok((vertrag, canonical))
}

// ── DTOs ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct NbContractUpsertRequest {
    pub malo_id: String,
    pub nb_mp_id: String,
    #[schema(value_type = String, example = "STROM")]
    pub sparte: Sparte,
    /// Voltage / pressure level: NS | MS | MSP | HSP | HS | HöS | HöS/HS (Strom)
    /// or GND | GMT | GHD (Gas)
    pub netzebene: String,
    /// Billing mode: RLM | SLP | IMS | TLP_GEMEINSAM | TLP_GETRENNT | PAUSCHAL
    pub bilanzierungsmethode: String,
    /// MONTHLY | QUARTERLY | ANNUALLY
    pub billing_schedule: String,
    /// Contract start date (ISO 8601, e.g. `"2026-01-01"`)
    pub valid_from: String,
    #[serde(default)]
    pub valid_to: Option<String>,
    /// Full BO4E `Vertrag` payload (L1).
    ///
    /// `_typ` is auto-injected to `"VERTRAG"` if absent.
    /// When omitted, a minimal `Vertrag` is auto-constructed from the other fields.
    /// Returns 422 if `_typ` is present but not `"VERTRAG"`, or if any typed
    /// field (e.g. `vertragsart`, `vertragsstatus`) contains an unknown enum value.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
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
    /// Full BO4E `Vertrag` payload in canonical camelCase form.
    /// `_typ: "VERTRAG"` is always present after a successful PUT.
    #[schema(value_type = Object)]
    pub data: serde_json::Value,
    /// BO4E `Vertragsart` extracted from `data.vertragsart`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vertragsart: Option<String>,
    /// BO4E `Vertragsstatus` lifecycle — extracted from `data.vertragsstatus`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vertragsstatus: Option<String>,
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

    // Validate and normalise the BO4E Vertrag payload.
    // When the caller omits `data`, auto-construct a minimal Vertrag so every
    // stored record is self-describing BO4E from day 1.
    let raw_data = req.data.unwrap_or_else(|| {
        serde_json::json!({
            "_typ": "VERTRAG",
            "vertragsart": "NETZNUTZUNGSVERTRAG",
            "vertragsstatus": "AKTIV"
        })
    });
    let (_, canonical_data) = normalize_vertrag(raw_data)?;

    // Extract typed columns from the canonical Vertrag JSON for fast SQL queries.
    let vertragsart = canonical_data
        .get("vertragsart")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let vertragsstatus = canonical_data
        .get("vertragsstatus")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

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
        data: canonical_data,
        vertragsart,
        vertragsstatus,
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
    Extension(event_tx): Extension<tokio::sync::mpsc::UnboundedSender<serde_json::Value>>,
    Path(id): Path<String>,
    Json(req): Json<NbContractUpsertRequest>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "write-nb-contract", &tenant_gln.0) {
        tracing::warn!(error = %e, "marktd: Cedar denied write-nb-contract");
        return StatusCode::FORBIDDEN.into_response();
    }

    let rec = match parse_req(id.clone(), req, &tenant_gln.0) {
        Ok(r) => r,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };

    let vertragsart = rec
        .vertragsart
        .clone()
        .unwrap_or_else(|| "NETZNUTZUNGSVERTRAG".into());
    let tenant = rec.tenant.clone();

    match repo.upsert(rec).await {
        Ok(version) => {
            // Emit de.markt.nb-contract.updated so ERP subscribers can rebuild
            // Vertrag caches without polling.
            let evt = MarktEvent::new(
                &tenant_gln.0,
                "de.markt.nb-contract.updated",
                id,
                serde_json::json!({
                    "version": version,
                    "vertragsart": vertragsart,
                    "tenant": tenant,
                }),
            );
            if let Ok(payload) = serde_json::to_value(&evt) {
                let _ = event_tx.send(payload);
            }
            Json(serde_json::json!({ "version": version })).into_response()
        }
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
        data: r.data,
        vertragsart: r.vertragsart,
        vertragsstatus: r.vertragsstatus,
        version: r.version,
        tenant: r.tenant,
    }
}

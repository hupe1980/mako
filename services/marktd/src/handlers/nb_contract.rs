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
    repository::{BillingSchedule, NbContractRecord, NbContractRepository, NetznutzerTyp},
};
use mako_service::cedar::CedarEnforcer;
use rubo4e::current::Vertrag;
use serde::{Deserialize, Serialize};
use time::macros::format_description;
use utoipa::ToSchema;

use crate::pg::PgNbContractRepository;

use super::{Claims, IntoMdmResponse as _, Tenant};

/// Extension alias — `PgNbContractRepository` is concrete so AFIT works.
pub type NbContractRepoExt = Arc<PgNbContractRepository>;

// ── Vertrag validation helper ─────────────────────────────────────────────────

/// Validate and normalise a `Vertrag` BO4E payload through the BO4E gate,
/// returning it alongside the canonical camelCase form durable storage takes.
///
/// The strict-enum stage is why `vertragsart` and `vertragsstatus` — the two
/// fields this endpoint's docs claim to validate — are actually validated:
/// serde alone decodes any unrecognised value to `Unknown` and stores it.
fn normalize_vertrag(
    data: serde_json::Value,
) -> Result<(Vertrag, serde_json::Value), (StatusCode, serde_json::Value)> {
    let vertrag: Vertrag = mako_markt::bo4e::decode(data)
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_json()))?;
    let canonical = super::serialise_or_500(&vertrag)?;
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
    /// MP-ID of the Netznutzer this contract is with — the LF in the ordinary
    /// case, the Letztverbraucher itself for a Selbstzahler.
    pub netznutzer_mp_id: String,
    /// `LIEFERANT` (default) | `LETZTVERBRAUCHER`.
    ///
    /// `LETZTVERBRAUCHER` marks a **Selbstzahler** — a „Netznutzer ohne
    /// All-Inklusiv-Vertrag" who pays the Netznutzung without an LF and steps
    /// into the LF role (GPKE Teil 1, Vorbemerkung).
    #[serde(default)]
    #[schema(value_type = String, example = "LIEFERANT")]
    pub netznutzer_typ: NetznutzerTyp,
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
    pub netznutzer_mp_id: String,
    pub netznutzer_typ: String,
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

#[derive(Debug, Deserialize)]
pub struct ByMaloQuery {
    /// Date the contract must be in force on. Defaults to today in Europe/Berlin.
    pub on: Option<String>,
}

// ── Parse helpers ─────────────────────────────────────────────────────────────

/// Build the record from the request.
///
/// `tenant` is the **deployment's own** identity, taken from the Axum extension
/// the Cedar check was made against. It is never read from the body: a request
/// that could name its own tenant would be authorised against one row scope and
/// written into another.
fn parse_req(
    id: String,
    req: NbContractUpsertRequest,
    tenant: &str,
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

    let malo_id = req.malo_id.parse::<MaloId>().map_err(|e| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "error": format!("invalid malo_id: {e}") }),
        )
    })?;

    // Validate and normalise the BO4E Vertrag payload.
    //
    // When the caller omits `data`, a minimal Vertrag is constructed so every
    // stored record is self-describing BO4E. It is built *typed*: the default
    // is mako's own value, not an untrusted payload, so it has no business
    // being spelled as JSON and re-parsed, and `..Default::default()` stamps
    // `_typ` and `_version`.
    let vertrag = match req.data {
        Some(data) => normalize_vertrag(data)?.0,
        None => Vertrag {
            vertragsart: Some(rubo4e::current::Vertragsart::Netznutzungsvertrag),
            vertragsstatus: Some(rubo4e::current::Vertragsstatus::Aktiv),
            ..Default::default()
        },
    };
    let canonical_data = super::serialise_or_500(&vertrag)?;

    // Typed columns for fast SQL queries, read off the *typed* value rather
    // than by string lookup on its JSON — a field that moves stops compiling.
    let vertragsart = vertrag.vertragsart.map(|v| v.as_wire().to_owned());
    let vertragsstatus = vertrag.vertragsstatus.map(|v| v.as_wire().to_owned());

    Ok(NbContractRecord {
        contract_id: id,
        malo_id,
        nb_mp_id: req.nb_mp_id,
        sparte: req.sparte,
        netzebene: req.netzebene,
        bilanzierungsmethode: req.bilanzierungsmethode,
        billing_schedule,
        netznutzer_mp_id: req.netznutzer_mp_id,
        netznutzer_typ: req.netznutzer_typ,
        valid_from,
        valid_to,
        data: canonical_data,
        vertragsart,
        vertragsstatus,
        tenant: tenant.to_owned(),
        version: 0,
    })
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `PUT /api/v1/nb-contracts/:id`
#[allow(clippy::too_many_arguments)]
pub async fn put_nb_contract(
    Extension(repo): Extension<NbContractRepoExt>,
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(tenant): Extension<Tenant>,
    Extension(pool): Extension<sqlx::PgPool>,
    Extension(notify): Extension<Arc<tokio::sync::Notify>>,
    Path(id): Path<String>,
    Json(req): Json<NbContractUpsertRequest>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "write-nb-contract", &tenant.0) {
        tracing::warn!(error = %e, "marktd: Cedar denied write-nb-contract");
        return StatusCode::FORBIDDEN.into_response();
    }

    let rec = match parse_req(id.clone(), req, &tenant.0) {
        Ok(r) => r,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };

    let vertragsart = rec
        .vertragsart
        .clone()
        .unwrap_or_else(|| "NETZNUTZUNGSVERTRAG".into());
    let sparte = rec.sparte.to_string();
    let evt_malo_id = rec.malo_id.to_string();

    match repo.upsert(rec).await {
        Ok(version) => {
            // Emit de.markt.nb-contract.updated so ERP subscribers can rebuild
            // Vertrag caches without polling.
            let evt = MarktEvent::new(
                &tenant.0,
                mako_events::markt::NB_CONTRACT_UPDATED,
                id,
                serde_json::json!({
                    "version": version,
                    "vertragsart": vertragsart,
                    "sparte": sparte,
                }),
            )
            .with_extensions(mako_markt::cloudevents::EventExtensions {
                marktmaloid: Some(evt_malo_id),
                marktsparte: Some(sparte),
                ..Default::default()
            });
            if let Err(e) = crate::outbox::enqueue(&pool, &evt, &notify).await {
                tracing::error!(error = %e, "nb_contract: durable enqueue failed");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": "event enqueue failed"})),
                )
                    .into_response();
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
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-nb-contract", &tenant.0) {
        tracing::warn!(error = %e, "marktd: Cedar denied read-nb-contract");
        return StatusCode::FORBIDDEN.into_response();
    }

    match repo.find(&id, &tenant.0).await {
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
    Extension(tenant): Extension<Tenant>,
    Query(q): Query<ListNbContractsQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-nb-contract", &tenant.0) {
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

    match repo.list_by_nb(&nb_mp_id, &tenant.0).await {
        Ok(recs) => Json(recs.into_iter().map(rec_to_response).collect::<Vec<_>>()).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/nb-contracts/by-malo/{malo_id}?on=YYYY-MM-DD`
///
/// The Netznutzungsvertrag in force for a MaLo on a date (default today). Read by
/// `processd`, which needs the Netznutzer and its type before it decides an
/// Anmeldung: a Selbstzahler („Netznutzer ohne All-Inklusiv-Vertrag") is an
/// ordinary LF in GPKE, with one exception — the LF's Lieferantenwechsel-Meldungen
/// (GPKE Teil 1, Vorbemerkung).
#[utoipa::path(
    get,
    path = "/api/v1/nb-contracts/by-malo/{malo_id}",
    params(
        ("malo_id" = String, Path, description = "11-digit Marktlokations-ID"),
        ("on" = Option<String>, Query, description = "Date the contract must be in force on (default today)"),
    ),
    responses(
        (status = 200, description = "Contract in force", body = NbContractResponse),
        (status = 404, description = "No contract in force on that date"),
        (status = 403, description = "Forbidden"),
    ),
)]
pub async fn get_nb_contract_by_malo(
    Extension(repo): Extension<NbContractRepoExt>,
    claims: Claims,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(tenant): Extension<Tenant>,
    Path(malo_id): Path<String>,
    Query(q): Query<ByMaloQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-nb-contract", &tenant.0) {
        tracing::warn!(error = %e, "marktd: Cedar denied read-nb-contract");
        return StatusCode::FORBIDDEN.into_response();
    }

    let date_fmt = format_description!("[year]-[month]-[day]");
    let on = match q.on.as_deref() {
        Some(raw) => match time::Date::parse(raw, date_fmt) {
            Ok(d) => d,
            Err(_) => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({ "error": "on must be YYYY-MM-DD" })),
                )
                    .into_response();
            }
        },
        None => super::malo::today_berlin(),
    };

    match repo.find_active(&malo_id, on, &tenant.0).await {
        Ok(Some(rec)) => Json(rec_to_response(rec)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => e.into_response(),
    }
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
        netznutzer_mp_id: r.netznutzer_mp_id,
        netznutzer_typ: r.netznutzer_typ.as_db_str().to_owned(),
        valid_from: r.valid_from.format(date_fmt).unwrap_or_default(),
        valid_to: r.valid_to.map(|d| d.format(date_fmt).unwrap_or_default()),
        data: r.data,
        vertragsart: r.vertragsart,
        vertragsstatus: r.vertragsstatus,
        version: r.version,
        tenant: r.tenant,
    }
}

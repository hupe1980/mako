//! MaLo (Marktlokation) REST handlers.
//!
//! Routes:
//!   PUT    /api/v1/malo/:id
//!   GET    /api/v1/malo/:id
//!   GET    /api/v1/malo           (list / query)

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_markt::{
    cloudevents::{EventExtensions, MarktEvent},
    domain::{MaloId, Sparte},
    error::MdmError,
    repository::{
        AppState, ContractRepository, CorrelationIndex, Lokationszuordnung, MaloFilter,
        MaloRepository, MeloRepository, PageResult, PartnerRepository, SubscriptionRepository,
    },
};
use mako_service::cedar::CedarEnforcer;
use rubo4e::current::Marktlokation;
use serde::{Deserialize, Serialize};
use time::Date;
use utoipa::{IntoParams, ToSchema};

use super::{Claims, IntoMdmResponse as _, etag, parse_if_match};

// ── BO4E validation helpers ──────────────────────────────────────────────────────────

/// Validate and normalise a `Marktlokation` payload (L4 hard cut).
///
/// 1. Auto-inject `_typ: "MARKTLOKATION"` when absent.
/// 2. Reject 422 if `_typ` is present but does not equal `MARKTLOKATION`.
/// 3. Deserialise as `rubo4e::current::Marktlokation` to validate all enum
///    fields (`bilanzierungsmethode`, `netzebene`, `gasqualitaet`, …).
/// 4. Re-serialise to canonical BO4E form (camelCase, correct `_typ`).
///
/// Non-standard keys (e.g. `fallgruppenzuordnung`) are preserved through the
/// `_additional` extension map (serde `flatten`) — round-trip is lossless.
fn normalize_marktlokation(
    mut data: serde_json::Value,
) -> Result<(Marktlokation, serde_json::Value), (StatusCode, serde_json::Value)> {
    if let Some(obj) = data.as_object_mut() {
        obj.entry("_typ")
            .or_insert_with(|| serde_json::json!("MARKTLOKATION"));
    }
    if let Some(typ) = data.get("_typ").and_then(|v| v.as_str())
        && typ.to_uppercase() != "MARKTLOKATION"
    {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "error": format!("expected _typ MARKTLOKATION, got '{typ}'") }),
        ));
    }
    let malo: Marktlokation = serde_json::from_value(data).map_err(|e| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "error": format!("invalid Marktlokation payload: {e}") }),
        )
    })?;
    let canonical = serde_json::to_value(&malo).unwrap_or_default();
    Ok((malo, canonical))
}

/// Deserialise stored JSONB as `Marktlokation`. Returns `None` and logs an
/// error on schema drift (operator must re-PUT the record to fix).
fn deserialize_stored_malo(data: serde_json::Value, malo_id: &str) -> Option<Marktlokation> {
    serde_json::from_value::<Marktlokation>(data)
        .map_err(|e| {
            tracing::error!(
                malo_id,
                error = %e,
                "schema drift: stored MaLo data is not a valid Marktlokation — \
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
pub struct MaloUpsertRequest {
    /// "STROM" or "GAS"
    #[schema(value_type = String, example = "STROM")]
    pub sparte: Sparte,
    /// Full BO4E MARKTLOKATION payload.
    pub data: serde_json::Value,
    #[serde(default)]
    #[schema(value_type = Vec<Object>)]
    pub lokationszuordnung: Vec<Lokationszuordnung>,
    /// BO4E schema version of `data` (e.g. `"v202607.0.0"`). Defaults to current.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MaloResponse {
    pub malo_id: String,
    #[schema(value_type = String, example = "STROM")]
    pub sparte: Sparte,
    pub version: i64,
    /// Validated BO4E `Marktlokation` payload in canonical camelCase form.
    /// Schema is enforced on every `PUT` — enum fields like `bilanzierungsmethode`
    /// and `netzebene` are rejected with 422 if they contain unknown values.
    #[schema(value_type = Object)]
    pub data: Marktlokation,
    /// Voltage/pressure level extracted from `data.netzebene` (e.g. `"NS"`, `"MS"`, `"HöS"`).
    /// Available immediately on write; no `nis-syncd` needed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub netzebene: Option<String>,
    /// Bilanzierungsgebiet EIC code extracted from `data.bilanzierungsgebiet`.
    /// Used by `processd` NB check 4 as primary source; falls back to `malo_grid` when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bilanzierungsgebiet: Option<String>,
    /// Gas quality extracted from `data.gasqualitaet` (`"HGas"` | `"LGas"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gasqualitaet: Option<String>,
    /// Energy direction (`"Aussp"` = generation, `"Einsp"` = consumption).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub energierichtung: Option<String>,
    /// Billing mode extracted from `Marktlokation.bilanzierungsmethode`.
    ///
    /// Values: `"RLM"` | `"SLP"` | `"TLP_GEMEINSAM"` | `"TLP_GETRENNT"` | `"PAUSCHAL"` | `"IMS"`.
    /// `"RLM"` → `netzbilanzd` includes Leistungspreis position (`spitzenleistung_kw` required).
    /// `"SLP"` → Arbeitspreis only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bilanzierungsmethode: Option<String>,
    /// Regelzone EIC code (`Marktlokation.regelzone`) — maps to the ÜNB for MABIS IFTSTA 21000
    /// routing and Redispatch 2.0 Stammdaten forwarding (VNB → ÜNB).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regelzone: Option<String>,
    /// Gas GaBi RLM Fallgruppe (`data["fallgruppenzuordnung"]`) — determines GaBi billing
    /// category. Values: `GABI_RLM_MIT_TAGESBAND` | `GABI_RLM_OHNE_TAGESBAND` |
    /// `GABI_RLM_IM_NOMINIERUNGSERSATZVERFAHREN`. Required for Gas MMM settlement routing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallgruppe: Option<String>,
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
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    headers: HeaderMap,
    claims: Claims,
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
    if enforcer
        .check(&claims.principal(), "write-malo", &state.tenant_gln)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }

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

    // L4 hard cut: validate and normalise the incoming BO4E payload.
    // Returns 422 on wrong _typ or invalid enum values (bilanzierungsmethode, netzebene, …).
    // Re-serialises to canonical camelCase form before storage.
    let (_, canonical_data) = match normalize_marktlokation(req.data) {
        Ok(v) => v,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };

    // Extract fields for the makod MaLo cache push from the canonical payload.
    let nb_mp_id = req
        .lokationszuordnung
        .iter()
        .find(|z| z.zuordnungstyp == "NB" || z.zuordnungstyp == "GNB")
        .map(|z| z.rollencodenummer.clone())
        .unwrap_or_else(|| state.tenant_gln.clone());
    let msb_mp_id = req
        .lokationszuordnung
        .iter()
        .find(|z| z.zuordnungstyp == "MSB" || z.zuordnungstyp == "GMSB")
        .map(|z| z.rollencodenummer.clone());
    let bilanzierungsgebiet = canonical_data
        .get("bilanzierungsgebiet")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let netzgebiet = canonical_data
        .get("netzgebietsnummer")
        .or_else(|| canonical_data.get("netzgebiet"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let sparte_str = req.sparte.to_string();
    let malo_id_str = malo_id.to_string();

    match state
        .malo_repo
        .upsert(
            &malo_id,
            req.sparte,
            canonical_data,
            req.lokationszuordnung,
            if_match,
            &req.bo4e_version,
        )
        .await
    {
        Ok(version) => {
            // Push to makod's MaLo cache so the engine can resolve NB/MSB GLNs
            // for outbound EDIFACT without the ERP having to call makod directly.
            // Best-effort: a failure here is logged but does NOT fail the API call —
            // the master record is already durably stored in marktd's PostgreSQL.
            let cache_record = mako_markt::makod_client::MaloIdentResultPositive {
                malo_id: malo_id_str.clone(),
                nb_mp_id,
                msb_mp_id,
                sender_market_partner_id: state.tenant_gln.clone(),
                bilanzierungsgebiet,
                netzgebiet,
                sparte: sparte_str,
            };
            if let Err(e) = state
                .makod_client
                .put_malo(&cache_record.malo_id, &cache_record)
                .await
            {
                tracing::warn!(
                    malo_id = %malo_id,
                    error   = %e,
                    "put_malo: makod cache push failed (non-fatal — marktd record saved)",
                );
            }

            // Emit de.markt.malo.updated so ERP subscribers and obsd get notified.
            let evt = MarktEvent::new(
                &state.tenant_gln,
                "de.markt.malo.updated",
                malo_id_str,
                serde_json::json!({ "version": version }),
            )
            .with_extensions(EventExtensions {
                marktmaloid: Some(malo_id.to_string()),
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
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
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
    if enforcer
        .check(&claims.principal(), "read-malo", &state.tenant_gln)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }
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
            let data = match deserialize_stored_malo(r.data, r.malo_id.as_ref()) {
                Some(v) => v,
                None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            };
            let resp = MaloResponse {
                malo_id: r.malo_id.to_string(),
                sparte: r.sparte,
                version: r.version,
                data,
                netzebene: r.netzebene,
                bilanzierungsgebiet: r.bilanzierungsgebiet,
                gasqualitaet: r.gasqualitaet,
                energierichtung: r.energierichtung,
                bilanzierungsmethode: r.bilanzierungsmethode,
                regelzone: r.regelzone,
                fallgruppe: r.fallgruppe,
                lokationszuordnung: r.lokationszuordnung,
            };
            (
                StatusCode::OK,
                [(axum::http::header::ETAG, etag(r.version))],
                Json(resp),
            )
                .into_response()
        }
        Ok(None) => mako_markt::error::MdmError::NotFound {
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
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
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
    if enforcer
        .check(&claims.principal(), "read-malo", &state.tenant_gln)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }

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
                .filter_map(|r| {
                    let malo_id_str = r.malo_id.to_string();
                    let data = deserialize_stored_malo(r.data, r.malo_id.as_ref())?;
                    Some(MaloResponse {
                        malo_id: malo_id_str,
                        sparte: r.sparte,
                        version: r.version,
                        data,
                        netzebene: r.netzebene,
                        bilanzierungsgebiet: r.bilanzierungsgebiet,
                        gasqualitaet: r.gasqualitaet,
                        energierichtung: r.energierichtung,
                        bilanzierungsmethode: r.bilanzierungsmethode,
                        regelzone: r.regelzone,
                        fallgruppe: r.fallgruppe,
                        lokationszuordnung: r.lokationszuordnung,
                    })
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
pub(crate) fn today_berlin() -> Date {
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

//! MaLo (Marktlokation) REST handlers.
//!
//! Routes:
//!   PUT    /api/v1/malos/:id
//!   GET    /api/v1/malos/:id
//!   GET    /api/v1/malos           (list / query)

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
        AppState, CorrelationIndex, MaloFilter, MaloRepository, MeloRepository, PageResult,
        PartnerRepository, Rollenzuordnung, SubscriptionRepository,
    },
};
use mako_service::cedar::CedarEnforcer;
use rubo4e::current::{Lastprofil, Marktlokation, Profilart};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::{Claims, IfMatch, IntoMdmResponse as _, etag, malformed_if_match, parse_if_match};

// ── BO4E validation helpers ──────────────────────────────────────────────────────────

/// Validate and normalise a `Marktlokation` payload through the BO4E gate.
///
/// `mako_markt::bo4e::decode` runs the four stages every BO4E endpoint runs —
/// `_typ` (injected when absent, refused when it names another BO), typed
/// deserialization, strict enums with their JSON-paths, and the BO4E-stated
/// rules (for a `Marktlokation`: at most one of `lokationsadresse`,
/// `geoadresse`, `katasterinformation`).
///
/// The caller stores the returned **value**, not the input: `PgMaloRepository`
/// serialises the BO itself and derives the typed columns from the same object,
/// so the canonical BO4E form (camelCase, correct `_typ`) is the only shape
/// that reaches the `data JSONB` column and a column cannot disagree with it.
///
/// Non-standard keys (e.g. `fallgruppenzuordnung`) are preserved through the
/// `_additional` extension map (serde `flatten`) — round-trip is lossless.
fn normalize_marktlokation(
    data: serde_json::Value,
) -> Result<Marktlokation, (StatusCode, serde_json::Value)> {
    mako_markt::bo4e::decode(data).map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_json()))
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
    mako_markt::bo4e::schema_version()
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
    pub rollenzuordnung: Vec<Rollenzuordnung>,
    /// BO4E schema version of `data` (e.g. `"202607.1.0"`). Defaults to current.
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
    /// Voltage/pressure level extracted from `data.netzebene` — a BO4E
    /// `Netzebene` wire value (`"NSP"`, `"MSP"`, `"HSP"`, `"HSS"`, `"MSP_NSP_UMSP"`, …).
    /// Available immediately on write; no separate grid provisioning needed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub netzebene: Option<String>,
    /// Bilanzierungsgebiet EIC code extracted from `data.bilanzierungsgebiet`.
    /// Used by `processd` NB check 4 as primary source; falls back to `malo_grid` when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bilanzierungsgebiet: Option<String>,
    /// Gas quality extracted from `data.gasqualitaet` (`"H_GAS"` | `"L_GAS"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gasqualitaet: Option<String>,
    /// BO4E `Energierichtung`, named from the grid's point of view:
    /// `"EINSP"` = generation (feeds the grid), `"AUSSP"` = consumption.
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
    /// Lokationsbündel object code (`Marktlokation.lokationsbuendelObjektcode`,
    /// UTILMD Lokationsbündelstruktur).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lokationsbuendel_objektcode: Option<String>,
    /// §14a EnWG „Status der Fernsteuerbarkeit" (`true` = technisch fernsteuerbar,
    /// `false` = nicht fernsteuerbar). Populated from UTILMD `CCI+7037` Z97/Z96.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fernsteuerbar: Option<bool>,
    #[schema(value_type = Vec<Object>)]
    pub rollenzuordnung: Vec<Rollenzuordnung>,
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
    /// Filter by Gas GaBi RLM Fallgruppe (e.g. `GABI_RLM_MIT_TAGESBAND`).
    /// Gas only — Strom MaLos have no Fallgruppe.
    #[serde(default)]
    pub fallgruppe: Option<String>,
    /// Filter by billing mode (e.g. `RLM`, `SLP`, `IMS`).
    #[serde(default)]
    pub bilanzierungsmethode: Option<String>,
    /// Filter by Regelzone EIC code (e.g. `10YDE-EON------1`).
    #[serde(default)]
    pub regelzone: Option<String>,
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

/// `PUT /api/v1/malos/:id`
#[utoipa::path(
    put,
    path = "/api/v1/malos/{id}",
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
pub async fn put_malo<Ma, Me, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<sqlx::PgPool>,
    headers: HeaderMap,
    claims: Claims,
    Path(id): Path<String>,
    Json(req): Json<MaloUpsertRequest>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    if enforcer
        .check(&claims.principal(), "write-malo", &state.tenant)
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

    let if_match = match parse_if_match(&headers) {
        IfMatch::Absent | IfMatch::Any => None,
        IfMatch::Version(v) => Some(v),
        // Refuse rather than fall back to an unconditional write: the caller
        // asked for a conditional one and would otherwise be told it succeeded.
        IfMatch::Malformed => return malformed_if_match(),
    };
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
    let malo = match normalize_marktlokation(req.data) {
        Ok(v) => v,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };

    // Extract fields for the makod MaLo cache push from the canonical payload.
    //
    // makod resolves outbound EDIFACT recipients from this cache, so the role
    // codes must be the ones valid *today* — an ERP PUT carrying the full role
    // history would otherwise push an expired NB/MSB GLN. Same half-open
    // `[valid_from, valid_to)` window as the SQL read path; latest start wins.
    let today = today_berlin();
    let current_role = |typ: &str, generic: &str| {
        req.rollenzuordnung
            .iter()
            .filter(|z| {
                (z.zuordnungstyp == typ || z.zuordnungstyp == generic)
                    && z.valid_from <= today
                    && z.valid_to.is_none_or(|to| to > today)
            })
            .max_by_key(|z| z.valid_from)
            .map(|z| z.rollencodenummer.clone())
    };
    let nb_mp_id = current_role("NB", "GNB").unwrap_or_else(|| state.tenant.clone());
    let msb_mp_id = current_role("MSB", "GMSB");
    // Read off the typed BO, not its JSON: the previous string lookups asked
    // for `netzgebietsnummer` / `netzgebiet`, neither of which is a BO4E field
    // name (the schema calls it `netzgebietsnr`), so the cache push carried a
    // permanent `None`.
    let bilanzierungsgebiet = malo.bilanzierungsgebiet.clone();
    let netzgebiet = malo.netzgebietsnr.clone();
    let sparte_str = req.sparte.to_string();
    let malo_id_str = malo_id.to_string();

    // Persist-before-dispatch: the master record and the de.markt.malo.updated
    // outbox row commit in ONE transaction, so a crash can never leave the
    // record changed without the event.
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "put_malo: begin failed");
            return MdmError::Internal(e.to_string()).into_response();
        }
    };

    let version = match crate::pg::PgMaloRepository::upsert_tx(
        &mut tx,
        &malo_id,
        req.sparte,
        &malo,
        req.rollenzuordnung,
        if_match,
        &req.bo4e_version,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    // Emit de.markt.malo.updated so ERP subscribers and obsd get notified.
    let evt = MarktEvent::new(
        &state.tenant,
        mako_events::markt::MALO_UPDATED,
        malo_id_str.clone(),
        serde_json::json!({ "version": version }),
    )
    .with_extensions(EventExtensions {
        marktmaloid: Some(malo_id_str.clone()),
        marktsparte: Some(sparte_str.clone()),
        ..Default::default()
    });
    if let Err(e) = crate::outbox::enqueue(&mut *tx, &evt, &state.notify).await {
        tracing::error!(error = %e, "malo: durable enqueue failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "event enqueue failed"})),
        )
            .into_response();
    }
    if let Err(e) = tx.commit().await {
        tracing::error!(error = %e, "put_malo: commit failed");
        return MdmError::Internal(e.to_string()).into_response();
    }

    // Push to makod's MaLo cache so the engine can resolve NB/MSB GLNs for
    // outbound EDIFACT without the ERP having to call makod directly.
    // Best-effort: a failure here is logged but does NOT fail the API call —
    // the master record is already durably stored in marktd's PostgreSQL.
    let cache_record = mako_markt::makod_client::MaloIdentResultPositive {
        malo_id: malo_id_str.clone(),
        nb_mp_id,
        msb_mp_id,
        sender_market_partner_id: state.tenant.clone(),
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

/// `GET /api/v1/malos/:id`
#[utoipa::path(
    get,
    path = "/api/v1/malos/{id}",
    tag = "malo",
    params(("id" = String, Path, description = "11-digit MaLo-ID")),
    responses(
        (status = 200, description = "Found", body = MaloResponse),
        (status = 404, description = "Not found"),
    )
)]
pub async fn get_malo<Ma, Me, Su, Ci, Pa>(
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
        .check(&claims.principal(), "read-malo", &state.tenant)
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
                lokationsbuendel_objektcode: r.lokationsbuendel_objektcode,
                fernsteuerbar: r.fernsteuerbar,
                rollenzuordnung: r.rollenzuordnung,
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

/// `GET /api/v1/malos`
#[utoipa::path(
    get,
    path = "/api/v1/malos",
    tag = "malo",
    params(ListQuery),
    responses(
        (status = 200, description = "List of Marktlokationen", body = Vec<MaloResponse>),
    )
)]
pub async fn list_malo<Ma, Me, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    if enforcer
        .check(&claims.principal(), "read-malo", &state.tenant)
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
        fallgruppe: q.fallgruppe,
        bilanzierungsmethode: q.bilanzierungsmethode,
        regelzone: q.regelzone,
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
                        lokationsbuendel_objektcode: r.lokationsbuendel_objektcode,
                        fernsteuerbar: r.fernsteuerbar,
                        rollenzuordnung: r.rollenzuordnung,
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

/// Today's date in German local time — [`mako_fristen::heute`].
///
/// Re-exported here because `rollenzuordnung` validity queries and every other
/// „which record is in force now" read in this service go through it.
pub(crate) use mako_fristen::heute as today_berlin;

// ── Lastprofil derivation ─────────────────────────────────────────────────────

/// `GET /api/v1/malos/{id}/lastprofil`
///
/// Returns the SLP `Lastprofil` COM array for a MaLo.
///
/// ## Resolution order
///
/// 1. If `Marktlokation.lastprofile` is populated in the stored JSONB data, the
///    stored values are returned verbatim (already typed BO4E).
/// 2. Otherwise a default profile is **derived** from `bilanzierungsmethode` +
///    `sparte` according to §12 StromNZV (bis 31.12.2025) / BDEW SLP registry:
///
/// | bilanzierungsmethode | sparte | derived profilschar | profilart |
/// |---|---|---|---|
/// | `SLP` | `STROM` | `H0` | `ART_STANDARDLASTPROFIL` |
/// | `SLP` | `GAS` | `G000` | `ART_STANDARDLASTPROFIL` |
/// | `RLM` | any | — | 404 — RLM meters use Lastgang, not SLP |
/// | `IMS` | any | — | 404 — iMSys uses measured values, not SLP |
///
/// ## Motivation
///
/// `billingd` uses the SLP profile to:
/// - Select the correct NNE tariff zone (H0 vs G0/G1–G6 vs L0 have different
///   NNE rates in some DSO grid areas).
/// - Verify Zählerstand plausibility against typical H0/G0 consumption curves.
///
/// Calling this endpoint from `billingd` replaces the current hard-coded
/// `Eintarif` assumption for all SLP meters.
#[utoipa::path(
    get,
    path = "/api/v1/malos/{id}/lastprofil",
    params(("id" = String, Path, description = "11-digit MaLo-ID")),
    responses(
        (status = 200, description = "Lastprofil array for this MaLo"),
        (status = 404, description = "MaLo not found or RLM/IMS meter (no SLP profile)"),
    )
)]
pub async fn get_malo_lastprofil<Ma, Me, Su, Ci, Pa>(
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
        .check(&claims.principal(), "read-malo", &state.tenant)
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

    let record = match state.malo_repo.find(&malo_id, today_berlin()).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return MdmError::NotFound {
                resource_type: "malo",
                id: malo_id.to_string(),
            }
            .into_response();
        }
        Err(e) => return e.into_response(),
    };

    // RLM and IMS meters use Lastgang / measured values — no SLP profile.
    let bilanzierungsmethode = record
        .bilanzierungsmethode
        .as_deref()
        .unwrap_or("")
        .to_uppercase();
    if bilanzierungsmethode == "RLM" || bilanzierungsmethode == "IMS" {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!(
                    "MaLo {malo_id} uses bilanzierungsmethode={bilanzierungsmethode}; \
                     no SLP Lastprofil applies (use edmd Lastgang for metered values)"
                )
            })),
        )
            .into_response();
    }

    // 1. Try to extract stored lastprofile from the JSONB data.
    let stored_profile: Vec<Lastprofil> = record
        .data
        .get("lastprofile")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    if !stored_profile.is_empty() {
        return Json(serde_json::json!({
            "malo_id": malo_id.to_string(),
            "bilanzierungsmethode": record.bilanzierungsmethode,
            "lastprofile": stored_profile,
            "source": "stored",
        }))
        .into_response();
    }

    // 2. Derive default SLP profile from sparte (§12 StromNZV (bis 31.12.2025) / BDEW).
    let profilschar = match record.sparte {
        Sparte::Gas => "G000", // DVGW G 685 — Standardlastprofil Erdgas
        _ => "H0",             // BDEW — Standardlastprofil Haushalt Strom
    };
    let derived = Lastprofil {
        profilschar: Some(profilschar.to_owned()),
        profilart: Some(Profilart::ArtStandardlastprofil),
        bezeichnung: Some(match record.sparte {
            Sparte::Gas => "Standardlastprofil Gas (G000 — DVGW G 685)".to_owned(),
            _ => "Standardlastprofil Haushalt Strom (H0 — BDEW)".to_owned(),
        }),
        ..Default::default()
    };

    Json(serde_json::json!({
        "malo_id": malo_id.to_string(),
        "bilanzierungsmethode": record.bilanzierungsmethode,
        "lastprofile": [derived],
        "source": "derived",
    }))
    .into_response()
}

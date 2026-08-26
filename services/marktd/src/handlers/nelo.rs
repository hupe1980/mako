//! NeLo (Netz-Element-Lokation) REST handlers.
//!
//! Routes:
//!   PUT  /api/v1/nelos/:id           — upsert a NeLo (schema-validated `Netzlokation` BO4E)
//!   GET  /api/v1/nelos/:id           — get a single NeLo — returns typed `Netzlokation`
//!   GET  /api/v1/nelos               — list NeLos (?nb_mp_id=… filters by Netzbetreiber)
//!
//! NeLos are network element locations used in BDEW Redispatch 2.0 processes.
//! The `nelo_id` is typically a 16-char EIC code (ENTSO-E) or a 13-digit BDEW
//! Codenummer.
//!
//! ## Hard cut — typed API (same pattern as Marktlokation)
//!
//! PUT body: `rubo4e::current::Netzlokation` JSON (camelCase).
//! GET returns: `NetzlokationResponse` with `data: rubo4e::current::Netzlokation`.
//!
//! Validation on PUT is [the BO4E gate](mako_markt::bo4e::decode) — `_typ`,
//! schema, out-of-schema enums by JSON-path, BO4E's own rules — followed by
//! this endpoint's mako profile: `sparte` is **required** despite BO4E making
//! it optional, and must be `STROM` or `GAS`, because market communication is a
//! two-commodity affair where BO4E's `Sparte` has seven values.
//!
//! Every typed SQL column is then derived from the object the gate returned.
//! Only `nb_mp_id` rides in the envelope, and only because `Netzlokation`
//! declares no Netzbetreiber field for it to duplicate.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_markt::{
    domain::Sparte,
    error::MdmError,
    repository::{NeLoRecord, NeLoRepository, PageResult},
};
use mako_service::cedar::CedarEnforcer;
use rubo4e::current::Netzlokation;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::pg::PgNeLoRepository;

use super::{
    Claims, IfMatch, IntoMdmResponse as _, TenantGln, etag, malformed_if_match, parse_if_match,
};

/// Extension alias — concrete type so AFIT dispatches statically.
pub type NeLoRepoExt = Arc<PgNeLoRepository>;

// ── BO4E validation helpers ───────────────────────────────────────────────────

/// Validate and normalise a `Netzlokation` payload through the BO4E gate,
/// returning it alongside its canonical serialization.
fn normalize_netzlokation(
    data: serde_json::Value,
) -> Result<(Netzlokation, serde_json::Value), (StatusCode, serde_json::Value)> {
    let nelo: Netzlokation = mako_markt::bo4e::decode(data)
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_json()))?;
    let canonical = super::serialise_or_500(&nelo, "Netzlokation")?;
    Ok((nelo, canonical))
}

/// Deserialise stored JSONB as `Netzlokation`. Returns `None` on schema drift.
fn deserialize_stored_nelo(data: serde_json::Value, nelo_id: &str) -> Option<Netzlokation> {
    serde_json::from_value::<Netzlokation>(data)
        .map_err(|e| {
            tracing::error!(
                nelo_id,
                error = %e,
                "schema drift: stored NeLo data is not a valid Netzlokation — \
                 re-PUT with a valid BO4E payload"
            );
        })
        .ok()
}

// ── DTOs ──────────────────────────────────────────────────────────────────────

/// PUT body — the BO4E `Netzlokation` in `data`, plus what the BO does not carry.
///
/// `nb_mp_id` is the one envelope field: it is an indexed column and
/// `Netzlokation` declares no Netzbetreiber. `sparte` is indexed too, but the
/// BO declares it, so it is derived from the payload and required by the
/// endpoint's profile.
#[derive(Debug, Deserialize, ToSchema)]
pub struct NeLoUpsertRequest {
    /// Owning Netzbetreiber MP-ID (indexed column, required for filtering).
    ///
    /// mako's own: `Netzlokation` declares no Netzbetreiber field, so this is
    /// not a restatement of anything the BO carries.
    pub nb_mp_id: String,
    /// Full `rubo4e::current::Netzlokation` payload (BO4E camelCase JSON).
    ///
    /// Crosses [the BO4E gate](mako_markt::bo4e::decode), and then this
    /// endpoint's own profile: `sparte` is **required** despite BO4E making it
    /// optional, and must be `STROM` or `GAS` — MaKo is a two-commodity market
    /// by regulation, while BO4E's `Sparte` also covers Fernwärme, Wasser and
    /// Abwasser.
    ///
    /// `sparte` is **not** an envelope field: the BO declares it, and two
    /// sources for one fact would let a caller file a Netzlokation under `GAS`
    /// while its stored document says `STROM`. The column is what every query
    /// reads.
    #[schema(value_type = Object)]
    pub data: serde_json::Value,
}

/// GET response — returns typed `Netzlokation` BO4E payload.
#[derive(Debug, Serialize, ToSchema)]
pub struct NetzlokationResponse {
    /// 16-char EIC code or 13-digit BDEW Codenummer.
    pub nelo_id: String,
    /// Owning Netzbetreiber MP-ID.
    pub nb_mp_id: String,
    /// Sparte extracted from the `Netzlokation` payload.
    #[schema(value_type = String, example = "STROM")]
    pub sparte: String,
    /// `true` if this NeLo has a Steuerkanal (Redispatch 2.0 remote-control).
    pub steuerkanal: Option<bool>,
    /// gMSB Marktrolle — `eigenschaftMsbLokation` in BO4E.
    pub eigenschaft_msb_lokation: Option<String>,
    /// gMSB MP-ID — `grundzustaendigerMsbCodenr` in BO4E.
    pub grundzustaendiger_msb_codenr: Option<String>,
    /// Full validated `rubo4e::current::Netzlokation` — canonical BO4E camelCase.
    #[schema(value_type = Object)]
    pub data: Netzlokation,
    pub version: i64,
    pub updated_at: String,
}

impl NetzlokationResponse {
    fn from_record(rec: NeLoRecord) -> Self {
        let nelo_id = rec.nelo_id.clone();
        let nelo = deserialize_stored_nelo(rec.data.clone(), &nelo_id).unwrap_or_else(|| {
            // Schema drift — return a minimal valid Netzlokation.
            // The operator must re-PUT to fix the stored data.
            Netzlokation::default()
        });
        Self {
            nelo_id,
            nb_mp_id: rec.nb_mp_id,
            sparte: rec.sparte.to_string(),
            steuerkanal: rec.steuerkanal,
            eigenschaft_msb_lokation: rec.eigenschaft_msb_lokation,
            grundzustaendiger_msb_codenr: rec.grundzustaendiger_msb_codenr,
            data: nelo,
            version: rec.version,
            updated_at: rec.updated_at.to_string(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NetzlokationListResponse {
    pub items: Vec<NetzlokationResponse>,
    pub total: u64,
    pub page: u32,
    pub size: u32,
}

impl From<PageResult<NeLoRecord>> for NetzlokationListResponse {
    fn from(p: PageResult<NeLoRecord>) -> Self {
        Self {
            items: p
                .items
                .into_iter()
                .map(NetzlokationResponse::from_record)
                .collect(),
            total: p.total,
            page: p.page,
            size: p.size,
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct NeLoListQuery {
    /// Filter by owning Netzbetreiber GLN.
    #[serde(default)]
    pub nb_mp_id: Option<String>,
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

/// PUT /api/v1/nelos/:id
///
/// Insert or update a Netz-Element-Lokation.
///
/// Body must be a valid `rubo4e::current::Netzlokation` JSON object (camelCase).
/// Returns 422 on wrong `_typ` or invalid enum values.
/// Supply `If-Match` header for optimistic concurrency; omit for unconditional upsert.
pub async fn put_nelo(
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(repo): Extension<NeLoRepoExt>,
    Path(nelo_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<NeLoUpsertRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-nelo", &tenant_gln)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }

    // Validate and canonicalise the BO4E Netzlokation payload.
    let (typed_nelo, canonical_data) = match normalize_netzlokation(body.data) {
        Ok(v) => v,
        Err((status, json)) => return (status, Json(json)).into_response(),
    };

    // The mako profile for this endpoint: BO4E makes `sparte` optional, marktd
    // does not — it is an indexed column every query filters on, and MaKo knows
    // only two commodities where BO4E knows seven.
    let sparte = match typed_nelo.sparte {
        Some(rubo4e::current::Sparte::Strom) => Sparte::Strom,
        Some(rubo4e::current::Sparte::Gas) => Sparte::Gas,
        Some(other) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": format!(
                        "sparte {} is a BO4E value, but market communication covers \
                         only STROM and GAS",
                        other.as_wire()
                    ),
                    "code": "mako.profile",
                    "field": "sparte",
                })),
            )
                .into_response();
        }
        None => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "sparte is required on a Netzlokation stored by marktd, \
                              though BO4E makes it optional — it is the indexed column \
                              every query filters on",
                    "code": "mako.profile",
                    "field": "sparte",
                })),
            )
                .into_response();
        }
    };

    // Typed SQL columns, derived from the validated BO4E struct — the single
    // derivation. `as_wire()` and not `format!("{r:?}")`: `Debug` renders
    // `Marktrolle::Nb` as `"Nb"`, not the BO4E wire value `"NB"`.
    let steuerkanal = typed_nelo.steuerkanal;
    let eigenschaft_msb_lokation = typed_nelo.eigenschaft_msb_lokation.map(|r| r.as_wire());
    let grundzustaendiger_msb_codenr = typed_nelo
        .grundzustaendiger_msb_codenr
        .as_ref()
        .map(ToString::to_string);

    let if_match = match parse_if_match(&headers) {
        IfMatch::Absent | IfMatch::Any => None,
        IfMatch::Version(v) => Some(v),
        // Refuse rather than fall back to an unconditional write: the caller
        // asked for a conditional one and would otherwise be told it succeeded.
        IfMatch::Malformed => return malformed_if_match(),
    };
    let rec = NeLoRecord {
        nelo_id,
        tenant: tenant_gln,
        name: None,
        sparte,
        netzebene: None,
        nb_mp_id: body.nb_mp_id,
        steuerkanal,
        eigenschaft_msb_lokation: eigenschaft_msb_lokation.map(ToOwned::to_owned),
        grundzustaendiger_msb_codenr,
        data: canonical_data,
        version: 0,
        updated_at: time::OffsetDateTime::now_utc(),
    };

    match repo.upsert(rec, if_match).await {
        Ok(new_version) => {
            let mut resp_headers = HeaderMap::new();
            resp_headers.insert("ETag", etag(new_version).parse().unwrap());
            (StatusCode::OK, resp_headers).into_response()
        }
        Err(MdmError::VersionConflict { .. }) => StatusCode::PRECONDITION_FAILED.into_response(),
        Err(e) => e.into_response(),
    }
}

/// GET /api/v1/nelos/:id
///
/// Retrieve a single NeLo. Returns a typed `NetzlokationResponse` with the
/// full `rubo4e::current::Netzlokation` BO4E payload in the `data` field.
pub async fn get_nelo(
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(repo): Extension<NeLoRepoExt>,
    Path(nelo_id): Path<String>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-nelo", &tenant_gln)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }
    match repo.find(&nelo_id, &tenant_gln).await {
        Ok(Some(rec)) => {
            let version = rec.version;
            let mut resp_headers = HeaderMap::new();
            resp_headers.insert("ETag", etag(version).parse().unwrap());
            (
                StatusCode::OK,
                resp_headers,
                Json(NetzlokationResponse::from_record(rec)),
            )
                .into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => e.into_response(),
    }
}

/// GET /api/v1/nelos
///
/// List NeLos for this tenant.  Pass `?nb_mp_id=<GLN>` to filter by Netzbetreiber.
pub async fn list_nelos(
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(repo): Extension<NeLoRepoExt>,
    Query(query): Query<NeLoListQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-nelo", &tenant_gln)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }
    let page_result = if let Some(nb_mp_id) = &query.nb_mp_id {
        repo.list_by_nb(nb_mp_id, &tenant_gln, query.page, query.size)
            .await
    } else {
        repo.list_by_tenant(&tenant_gln, query.page, query.size)
            .await
    };
    match page_result {
        Ok(page) => Json(NetzlokationListResponse::from(page)).into_response(),
        Err(e) => e.into_response(),
    }
}

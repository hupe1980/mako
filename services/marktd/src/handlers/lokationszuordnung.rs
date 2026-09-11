//! Handlers for the `Lokationszuordnung` location-graph endpoints (B5).
//!
//! Routes:
//!   GET    /api/v1/malos/{id}/lokationen               — recursive graph from a MaLo
//!   GET    /api/v1/melos/{id}/lokationen               — recursive graph from a MeLo
//!   PUT    /api/v1/lokationszuordnungen                — upsert a directed edge
//!   DELETE /api/v1/lokationszuordnungen/{von_id}/{nach_id} — hard-delete an edge pair

use std::sync::Arc;

use axum::{
    Extension,
    extract::{Path, Query},
    http::StatusCode,
};
use mako_markt::bo4e::Bo4e;
use mako_markt::domain::Lokationstyp;
use mako_markt::repository::{Buendelbefund, Lokationsbuendel, LokationszuordnungRepository};
use mako_service::cedar::CedarEnforcer;
use mako_service::{ApiError, ApiResult, Json};
use rubo4e::current::Lokationszuordnung;
use rubo4e::lokationsbuendel::LokationsbuendelExt as _;
use serde::{Deserialize, Serialize};

use crate::pg::PgLokationszuordnungRepository;

use super::{Claims, Tenant};

pub type LzRepoExt = Arc<PgLokationszuordnungRepository>;

// ── DTOs ─────────────────────────────────────────────────────────────────────

/// Request body for `PUT /api/v1/lokationszuordnungen`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertEdgeRequest {
    /// Source node ID (e.g. MaLo-ID).
    pub von_id: String,
    /// Source node type — [`Lokationstyp`] (`MALO`/`MELO`/`NELO`/`SR`/`TR`).
    pub von_typ: Lokationstyp,
    /// Target node ID.
    pub nach_id: String,
    /// Target node type — [`Lokationstyp`].
    pub nach_typ: Lokationstyp,
    /// Start of validity (`YYYY-MM-DD`). `null` = from epoch.
    pub valid_from: Option<String>,
    /// End of validity (`YYYY-MM-DD`). `null` = open-ended.
    pub valid_to: Option<String>,
    /// The BO4E `Lokationszuordnung` this edge belongs to.
    ///
    /// Crosses [the BO4E gate](mako_markt::bo4e::decode) as the request
    /// deserialises, and the **canonical round-trip** is what gets stored — the
    /// `lokationsbuendelcode` column is derived from it, so an unvalidated
    /// payload would put an unvalidated code in a typed column.
    ///
    /// Absent stores `{}`: an edge may be asserted on its own, before the
    /// bundle document that explains it exists.
    #[serde(default)]
    pub data: Option<Bo4e<Lokationszuordnung>>,
}

/// An ISO-8601 date, or the reason it is not one.
///
/// `Option<time::Date>` is the *answer* here — `valid_from: null` means "from
/// the beginning of time" and `valid_to: null` means "open-ended" — so a parse
/// failure must not collapse into it. `.ok()` did exactly that: `valid_from:
/// "2026-13-01"` stored an edge valid since the epoch, and `?at=garbage`
/// returned every edge regardless of validity, which on a temporal graph is a
/// different question answered without saying so.
fn parse_date(field: &str, s: &str) -> Result<time::Date, ApiError> {
    use time::format_description::well_known::Iso8601;
    time::Date::parse(s, &Iso8601::DEFAULT).map_err(|e| {
        ApiError::bad_request(format!(
            "`{field}` is not an ISO-8601 date (`YYYY-MM-DD`): {e}"
        ))
    })
}

/// `Some(date)` for a supplied value, `None` for an absent one — and a refusal
/// for a value that is present and unparseable.
fn opt_date(field: &str, s: Option<&str>) -> Result<Option<time::Date>, ApiError> {
    s.map(|v| parse_date(field, v)).transpose()
}

/// Query parameters for graph endpoints.
#[derive(Debug, Deserialize, Default)]
pub struct GraphQuery {
    /// Point-in-time filter (`YYYY-MM-DD`). Omit for all edges regardless of validity.
    pub at: Option<String>,
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /api/v1/malos/{id}/lokationen`
///
/// Recursively traverses the MaKo location graph starting at the given `MaLo-ID`.
/// Returns all reachable edges (MaLo → MeLo → NeLo → SR/TR) ordered by depth.
/// Pass `?at=YYYY-MM-DD` to filter to edges valid on a specific date.
pub async fn get_malo_lokationen(
    Extension(repo): Extension<LzRepoExt>,
    claims: Claims,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Path(malo_id): Path<String>,
    Query(q): Query<GraphQuery>,
) -> ApiResult<Json<Vec<mako_markt::repository::LokationszuordnungEdge>>> {
    enforcer
        .check(&claims.principal(), "read-lokationszuordnung", &tenant)
        .map_err(|_| ApiError::Forbidden)?;

    let at_date = opt_date("at", q.at.as_deref())?;
    let edges = repo
        .find_graph(&tenant, &malo_id, at_date)
        .await
        .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;
    Ok(Json(edges))
}

/// Response for `GET /api/v1/malos/{id}/buendel` — the projected Lokationsbündel,
/// the BDEW structure it declares, and everything that disagrees with it.
#[derive(Debug, Serialize)]
pub struct BuendelResponse {
    /// The projected bundle.
    #[serde(flatten)]
    pub buendel: Lokationsbuendel,
    /// `true` when the bundle carries at least one Messlokation.
    pub valid: bool,
    /// Human-readable integrity violation, when `valid` is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation_error: Option<String>,
    /// The published *Lokationsbündelstruktur* the declared
    /// `lokationsbuendelcode` names, in the BDEW codelist's own words.
    ///
    /// `null` when the bundle declares no code, or one the codelist does not
    /// publish — which [`befunde`](Self::befunde) then says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub struktur: Option<StrukturView>,
    /// Every way the bundle departs from the structure it declares.
    ///
    /// Empty is conformant. A **report**, not a refusal: BDEW requires none of
    /// this of a stored record, a bundle is legitimately incomplete mid-Einzug,
    /// and a `GET` that 500s on imperfect data tells an operator less than one
    /// that names the imperfection.
    pub befunde: Vec<String>,
}

/// The published structure a bundle declares, as the BDEW codelist states it.
#[derive(Debug, Serialize)]
pub struct StrukturView {
    /// The 13-digit BDEW *Lokationsbündelstrukturcode*.
    pub code: String,
    /// The codelist's own name for the structure.
    pub bezeichnung: String,
    /// How many levels deep the structure goes.
    pub max_ebene: u8,
}

/// `GET /api/v1/malos/{id}/buendel`
///
/// Returns the first-class [`Lokationsbuendel`] rooted at the given `MaLo-ID`,
/// projected from the typed location graph, together with its structural
/// integrity status (a bundle can be transiently incomplete mid-Einzug, so this
/// reports `valid: false` rather than failing the request).
pub async fn get_malo_buendel(
    Extension(repo): Extension<LzRepoExt>,
    claims: Claims,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Path(malo_id): Path<String>,
    Query(q): Query<GraphQuery>,
) -> ApiResult<Json<BuendelResponse>> {
    enforcer
        .check(&claims.principal(), "read-lokationszuordnung", &tenant)
        .map_err(|_| ApiError::Forbidden)?;

    let at_date = opt_date("at", q.at.as_deref())?;
    let buendel = repo
        .load_buendel(&tenant, &malo_id, at_date)
        .await
        .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;
    let (valid, validation_error) = match buendel.validate() {
        Ok(()) => (true, None),
        Err(e) => (false, Some(e.to_string())),
    };
    // The BDEW codelist's answer to "which structure is this, and does the
    // bundle match it" — neither of which an opaque `lokationsbuendelcode`
    // string on the response can give.
    let audit = buendel.audit_struktur();
    Ok(Json(BuendelResponse {
        buendel,
        valid,
        validation_error,
        struktur: audit.struktur.map(|s| StrukturView {
            code: s.as_code().to_string(),
            bezeichnung: s.bezeichnung.to_owned(),
            max_ebene: s.max_ebene(),
        }),
        befunde: audit.befunde.iter().map(Buendelbefund::to_string).collect(),
    }))
}

/// `GET /api/v1/melos/{id}/lokationen`
///
/// Recursively traverses the location graph starting at the given `MeLo-ID`.
/// Returns all reachable edges ordered by depth.
pub async fn get_melo_lokationen(
    Extension(repo): Extension<LzRepoExt>,
    claims: Claims,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Path(melo_id): Path<String>,
    Query(q): Query<GraphQuery>,
) -> ApiResult<Json<Vec<mako_markt::repository::LokationszuordnungEdge>>> {
    enforcer
        .check(&claims.principal(), "read-lokationszuordnung", &tenant)
        .map_err(|_| ApiError::Forbidden)?;

    let at_date = opt_date("at", q.at.as_deref())?;
    let edges = repo
        .find_graph(&tenant, &melo_id, at_date)
        .await
        .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;
    Ok(Json(edges))
}

/// `PUT /api/v1/lokationszuordnungen`
///
/// Upserts a directed edge in the location graph.  Idempotent.
pub async fn put_lokationszuordnung(
    Extension(repo): Extension<LzRepoExt>,
    claims: Claims,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Json(req): Json<UpsertEdgeRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    enforcer
        .check(&claims.principal(), "write-lokationszuordnung", &tenant)
        .map_err(|_| ApiError::Forbidden)?;

    let valid_from = opt_date("valid_from", req.valid_from.as_deref())?;
    let valid_to = opt_date("valid_to", req.valid_to.as_deref())?;
    if let (Some(from), Some(to)) = (valid_from, valid_to)
        && to < from
    {
        return Err(ApiError::unprocessable(format!(
            "`valid_to` {to} is before `valid_from` {from}"
        )));
    }

    // The canonical round-trip, never the request body: `upsert_edge` derives
    // the `lokationsbuendelcode` column from this document.
    //
    // This is also the **only** place the per-object-code audit can run. The
    // document carries its Marktlokationen, Messlokationen, Netzlokationen and
    // technischen Ressourcen inline, each with its own
    // `lokationsbuendelObjektcode`; the graph the read path projects from holds
    // ids grouped by `Lokationstyp` and no codes at all. So the fine audit
    // belongs here and the coarse one on the read — see
    // `Lokationsbuendel::audit_struktur`.
    let (data, befunde) = match req.data.as_ref() {
        Some(bo) => {
            // `StrukturcodeFehlt` is dropped here and kept on the read.
            //
            // The two paths ask different questions. `GET /buendel` asks *which
            // structure is this*, and "none is declared" is the answer — it
            // belongs in the report. A `PUT` asks *does this document match what
            // it declares*, and one declaring nothing matches vacuously. An edge
            // is routinely asserted with no bundle document at all (`data: {}`
            // is what the demos send), so reporting it here would put a finding
            // on the common case and train every reader to ignore the field.
            //
            // The rest of the audit still runs: `rubo4e` checks a malformed
            // object code and one filed under the wrong type without needing the
            // structure code, so dropping this one finding loses nothing else.
            let audit = bo.audit_buendel();
            let befunde: Vec<String> = audit
                .befunde
                .iter()
                .filter(|b| !matches!(b, rubo4e::lokationsbuendel::Befund::StrukturcodeFehlt))
                .map(ToString::to_string)
                .collect();
            (
                bo.canonical_json()
                    .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?,
                befunde,
            )
        }
        None => (
            serde_json::Value::Object(serde_json::Map::new()),
            Vec::new(),
        ),
    };
    if !befunde.is_empty() {
        // Reported, not refused. BDEW requires none of this of a stored record,
        // a bundle is legitimately incomplete mid-Einzug, and an edge is often
        // asserted before the objects around it exist. Logged so an operator
        // sees it without having to ask the API.
        tracing::info!(
            von_id = %req.von_id,
            nach_id = %req.nach_id,
            findings = befunde.len(),
            "marktd: Lokationsbündel departs from the structure it declares"
        );
    }

    let id = repo
        .upsert_edge(
            &tenant,
            &req.von_id,
            req.von_typ,
            &req.nach_id,
            req.nach_typ,
            valid_from,
            valid_to,
            data,
        )
        .await
        .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;
    Ok(Json(serde_json::json!({ "id": id, "befunde": befunde })))
}

/// `DELETE /api/v1/lokationszuordnungen/{von_id}/{nach_id}`
///
/// Hard-deletes all temporal variants of an edge pair.
pub async fn delete_lokationszuordnung(
    Extension(repo): Extension<LzRepoExt>,
    claims: Claims,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Path((von_id, nach_id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    enforcer
        .check(&claims.principal(), "write-lokationszuordnung", &tenant)
        .map_err(|_| ApiError::Forbidden)?;

    match repo
        .delete_edge(&tenant, &von_id, &nach_id)
        .await
        .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?
    {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::NotFound),
    }
}

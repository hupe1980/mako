//! edmd's documented REST reads and the OpenAPI 3.1 document describing them.
//!
//! Every route here is mounted through `utoipa-axum`'s `OpenApiRouter`, so it
//! cannot exist without its document entry; the document is served at
//! `GET /api/v1/openapi.json` and committed as `services/edmd/openapi.json`.
//! Each handler authorizes, then calls the shared read in [`crate::reads`] the
//! MCP tool of the same name calls, under the same Cedar action.

use std::sync::Arc;

use axum::{
    Extension,
    extract::{Path, Query, State},
};
use mako_service::cedar::CedarEnforcer;
use mako_service::oidc::Claims;
use mako_service::{ApiError, ApiResult, Json};
use serde::Deserialize;
use utoipa::{IntoParams, OpenApi};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::handler::HandlerState;
use crate::reads::{self, Abdeckung, Korrekturen, Messwerte, Qualitaetsbefund};

/// The document's frame; the routes add their paths and schemas.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "edmd — Energiedatenmanagement",
        description = "Meter data of the Marktlokationen: the version-resolved series, its quality, corrections and per-day coverage. Describes the routes edmd mounts through its documented router."
    ),
    tags((name = "messwerte", description = "Meter data reads"))
)]
struct ApiDoc;

fn authorize(
    claims: &Claims,
    enforcer: &CedarEnforcer,
    action: &str,
    tenant: &str,
) -> ApiResult<()> {
    enforcer
        .check(&claims.principal(), action, tenant)
        .map_err(|e| ApiError::forbidden(e.to_string()))
}

/// A window of RFC 3339 instants.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct Fenster {
    /// Window start, RFC 3339.
    pub from: String,
    /// Window end, RFC 3339.
    pub to: String,
}

/// An optional window of RFC 3339 instants.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OffenesFenster {
    /// Window start, RFC 3339; defaults per route.
    pub from: Option<String>,
    /// Window end, RFC 3339; defaults to now.
    pub to: Option<String>,
}

/// `get_correction_history`'s query.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct KorrekturFenster {
    /// Earliest `corrected_at`, RFC 3339; defaults to 90 days ago.
    pub from: Option<String>,
    /// Latest `corrected_at`, RFC 3339; defaults to now.
    pub to: Option<String>,
    /// At most this many (default 100, at most 1000).
    pub limit: Option<u32>,
}

/// `get_coverage`'s query.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct Tagesbereich {
    /// First day, `YYYY-MM-DD`.
    pub from: String,
    /// Last day, `YYYY-MM-DD` (inclusive).
    pub to: String,
    /// `STROM`, `GAS`, `WAERME` or `WASSER`; absent, the Sparte the MaLo's
    /// readings are stored under.
    pub sparte: Option<String>,
}

/// The version-resolved series of a Marktlokation (Cedar `read-timeseries`).
#[utoipa::path(
    get,
    path = "/api/v1/timeseries/{malo_id}",
    tag = "messwerte",
    params(("malo_id" = String, Path, description = "11-digit Marktlokations-ID"), Fenster),
    responses(
        (status = 200, description = "The series", body = Messwerte),
        (status = 400, description = "invalid_instant, zeitraum_ungueltig"),
        (status = 422, description = "zeitraum_zu_lang"),
    )
)]
pub async fn get_timeseries(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(q): Query<Fenster>,
) -> ApiResult<Json<Messwerte>> {
    authorize(&claims, &enforcer, "read-timeseries", &state.tenant)?;
    reads::timeseries(&state.repo, &state.tenant, &malo_id, &q.from, &q.to)
        .await
        .map(Json)
}

/// The quality verdict over a window (Cedar `read-timeseries`).
#[utoipa::path(
    get,
    path = "/api/v1/quality/{malo_id}",
    tag = "messwerte",
    params(("malo_id" = String, Path, description = "11-digit Marktlokations-ID"), OffenesFenster),
    responses(
        (status = 200, description = "The verdict", body = Qualitaetsbefund),
        (status = 400, description = "invalid_instant"),
        (status = 404, description = "no_data: nothing stored in the window"),
    )
)]
pub async fn get_quality(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(q): Query<OffenesFenster>,
) -> ApiResult<Json<Qualitaetsbefund>> {
    authorize(&claims, &enforcer, "read-timeseries", &state.tenant)?;
    reads::quality_warnings(
        &state.repo,
        &state.tenant,
        &malo_id,
        q.from.as_deref(),
        q.to.as_deref(),
    )
    .await
    .map(Json)
}

/// The corrections recorded for a Marktlokation (Cedar `read-corrections`).
#[utoipa::path(
    get,
    path = "/api/v1/corrections/{malo_id}",
    tag = "messwerte",
    params(("malo_id" = String, Path, description = "11-digit Marktlokations-ID"), KorrekturFenster),
    responses(
        (status = 200, description = "The corrections, newest first", body = Korrekturen),
        (status = 400, description = "invalid_instant"),
    )
)]
pub async fn get_corrections(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(q): Query<KorrekturFenster>,
) -> ApiResult<Json<Korrekturen>> {
    authorize(&claims, &enforcer, "read-corrections", &state.tenant)?;
    reads::correction_history(
        &state.repo,
        &state.tenant,
        &malo_id,
        q.from.as_deref(),
        q.to.as_deref(),
        q.limit,
    )
    .await
    .map(Json)
}

/// One state per day — gemessen, ersatzwert, korrigiert, fehlt — for a
/// Marktlokation (Cedar `read-timeseries`).
#[utoipa::path(
    get,
    path = "/api/v1/coverage/{malo_id}",
    tag = "messwerte",
    params(("malo_id" = String, Path, description = "11-digit Marktlokations-ID"), Tagesbereich),
    responses(
        (status = 200, description = "One state per day", body = Abdeckung),
        (status = 400, description = "invalid_date, zeitraum_ungueltig"),
        (status = 422, description = "zeitraum_zu_lang, sparte_unklar"),
    )
)]
pub async fn get_coverage(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(q): Query<Tagesbereich>,
) -> ApiResult<Json<Abdeckung>> {
    authorize(&claims, &enforcer, "read-timeseries", &state.tenant)?;
    reads::coverage(
        &state.repo,
        &state.tenant,
        &malo_id,
        &q.from,
        &q.to,
        q.sparte.as_deref(),
    )
    .await
    .map(Json)
}

/// The documented routes and their document.
pub fn routes() -> (axum::Router<HandlerState>, utoipa::openapi::OpenApi) {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(get_timeseries))
        .routes(routes!(get_quality))
        .routes(routes!(get_corrections))
        .routes(routes!(get_coverage))
        .split_for_parts()
}

/// The OpenAPI document edmd serves at `GET /api/v1/openapi.json`.
#[must_use]
pub fn document() -> utoipa::openapi::OpenApi {
    routes().1
}

/// The documented routes plus `GET /api/v1/openapi.json`.
pub fn router() -> axum::Router<HandlerState> {
    let (router, doc) = routes();
    router.route(
        "/api/v1/openapi.json",
        axum::routing::get(move || {
            let doc = doc.clone();
            async move { axum::Json(doc) }
        }),
    )
}

#[cfg(test)]
mod tests {
    /// The served document is the committed `services/edmd/openapi.json`
    /// (`BLESS_MCP_SNAPSHOT=1` rewrites it); integrators and the Desk generate
    /// their clients from the committed file.
    #[test]
    fn the_openapi_document_matches_the_committed_one() {
        let doc = serde_json::to_value(super::document()).expect("the document serializes");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json");
        if let Err(e) = mako_service::mcp_snapshot::compare_json(&doc, &path) {
            panic!("{e}");
        }
    }

    /// `GET /api/v1/corrections/{malo_id}` lives here while the `POST` of the
    /// same path stays on the server's router; the two merge into one path.
    #[test]
    fn the_documented_get_merges_beside_an_undocumented_post() {
        async fn post() {}
        let _ = axum::Router::<crate::handler::HandlerState>::new()
            .route("/api/v1/corrections/{malo_id}", axum::routing::post(post))
            .merge(super::router());
    }
}

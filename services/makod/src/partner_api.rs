//! Admin REST API for trading-partner master-data management.
//!
//! Mounted on the **existing `--http-addr` port** under `/admin/partners/`.
//! Protected by the same bearer token as `POST /edifact`.
//!
//! # ⚠️ Security
//!
//! This router **must never** be mounted on the public BDEW API-Webdienste
//! port (`--api-webdienste-addr`). Only the admin port is allowed. Restrict
//! network access to trusted ERP hosts via firewall rules or VPC subnets.
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | `GET` | `/admin/partners` | List all partners for the tenant |
//! | `GET` | `/admin/partners/{mp_id}` | Retrieve a single partner record |
//! | `PUT` | `/admin/partners/{mp_id}` | Create or update a partner record |
//! | `DELETE` | `/admin/partners/{mp_id}` | Remove a partner record |
//! | `POST` | `/admin/partners/import` | Import from raw PARTIN EDIFACT |
//!
//! # Bootstrap flow
//!
//! On startup `main.rs` calls [`seed_from_config`] to populate the store from
//! the `[as4] partners = ["GLN=URL", …]` config list.  Individual records can
//! then be updated at runtime via `PUT` or via inbound PARTIN messages that
//! trigger the `POST /admin/partners/import` endpoint.
//!
//! # PARTIN import
//!
//! `POST /admin/partners/import` accepts a raw EDIFACT interchange
//! (`Content-Type: text/plain; charset=utf-8`).  The body is parsed as a
//! PARTIN message using [`edi_energy::Platform`]; each participating party
//! is extracted and upserted into the `PartnerStore`.
//!
//! Until full PARTIN segment extraction is implemented the endpoint returns
//! `501 Not Implemented`.

use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{delete, get, post, put},
};
use edi_energy::{AnyMessage, EdiEnergyMessage as _, Platform};
use mako_engine::{
    ids::TenantId,
    partner::{PartnerRecord, PartnerStore as _},
    store_slatedb::SlateDbPartnerStore,
    types::MarktpartnerCode,
};
use serde::{Deserialize, Serialize};
use tracing::info;
use utoipa::ToSchema;

use crate::cedar_authz::{CedarAuthorizer, MakoAction, PartnerResource};

// ── State ─────────────────────────────────────────────────────────────────────

/// Shared state for the partner admin API.
pub struct PartnerAdminState {
    pub store: SlateDbPartnerStore,
    pub tenant_id: TenantId,
    /// Cedar-based authorization engine.
    pub cedar: Arc<CedarAuthorizer>,
    /// Shared EDIFACT platform for parsing PARTIN interchanges submitted to
    /// `POST /admin/partners/import`.
    pub platform: Arc<Platform>,
}

// ── Request / response types ──────────────────────────────────────────────────

/// Request body for `PUT /admin/partners/{mp_id}`.
///
/// Accepts a full [`PartnerRecord`] as JSON. The `gln` field in the body
/// must match the `{mp_id}` path parameter; a mismatch is rejected with `400`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpsertRequest {
    #[schema(value_type = Object)]
    #[serde(flatten)]
    pub record: PartnerRecord,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct PartnerResponse {
    #[schema(value_type = Object)]
    #[serde(flatten)]
    record: PartnerRecord,
    updated_at: String,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct ListResponse {
    #[schema(value_type = Vec<Object>)]
    partners: Vec<PartnerRecord>,
    count: usize,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct DeleteResponse {
    #[schema(example = "9904829000001")]
    mp_id: String,
    deleted: bool,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct ImportResponse {
    /// Number of PARTIN records successfully upserted.
    #[schema(example = 3)]
    upserted: usize,
    /// Number of PARTIN messages that had no extractable GLN.
    #[schema(example = 0)]
    skipped: usize,
    /// GLNs that were upserted.
    glns: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct ErrorResponse {
    error: String,
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Build the axum [`Router`] for the partner admin API.
///
/// Mount this on the admin port (same as `--http-addr`) — the
/// `/admin/partners/` prefix is already part of the route definitions.
pub fn router(state: Arc<PartnerAdminState>) -> Router {
    Router::new()
        .route("/admin/partners", get(handle_list))
        .route("/admin/partners/import", post(handle_import))
        .route("/admin/partners/{mp_id}", get(handle_get))
        .route("/admin/partners/{mp_id}", put(handle_put))
        .route("/admin/partners/{mp_id}", delete(handle_delete))
        .with_state(state)
}

// ── Auth helper ───────────────────────────────────────────────────────────────

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
}

fn forbidden() -> Response {
    (StatusCode::FORBIDDEN, "Forbidden").into_response()
}

fn internal_error(e: impl std::fmt::Display) -> Response {
    tracing::error!(error = %e, "partner admin API error");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: "internal error".to_string(),
        }),
    )
        .into_response()
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `GET /admin/partners` — list all partner records for this tenant.
#[utoipa::path(
    get,
    path = "/admin/partners",
    tag = "admin",
    responses(
        (status = 200, description = "List of partners", body = ListResponse),
        (status = 401, description = "Missing or invalid bearer token"),
    ),
    security((), ("bearer_token" = []))
)]
pub(crate) async fn handle_list(
    headers: HeaderMap,
    State(state): State<Arc<PartnerAdminState>>,
) -> Response {
    let identity = match state.cedar.authenticate(&headers) {
        Some(id) => id,
        None => return unauthorized(),
    };
    if !state.cedar.authorize_partner(
        &identity,
        MakoAction::AdminPartnerRead,
        &PartnerResource {
            tenant: &state.tenant_id.to_string(),
            mp_id: None,
        },
    ) {
        return forbidden();
    }
    match state.store.list(state.tenant_id).await {
        Ok(partners) => {
            let count = partners.len();
            Json(ListResponse { partners, count }).into_response()
        }
        Err(e) => internal_error(e),
    }
}

/// `GET /admin/partners/{mp_id}` — retrieve a single partner record.
#[utoipa::path(
    get,
    path = "/admin/partners/{mp_id}",
    tag = "admin",
    params(("mp_id" = String, Path, description = "13-digit GLN")),
    responses(
        (status = 200, description = "Partner record", body = PartnerResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 404, description = "Partner not found"),
    ),
    security((), ("bearer_token" = []))
)]
pub(crate) async fn handle_get(
    headers: HeaderMap,
    State(state): State<Arc<PartnerAdminState>>,
    Path(gln_str): Path<String>,
) -> Response {
    let identity = match state.cedar.authenticate(&headers) {
        Some(id) => id,
        None => return unauthorized(),
    };
    if !state.cedar.authorize_partner(
        &identity,
        MakoAction::AdminPartnerRead,
        &PartnerResource {
            tenant: &state.tenant_id.to_string(),
            mp_id: Some(&gln_str),
        },
    ) {
        return forbidden();
    }
    let mp_id = MarktpartnerCode::from(gln_str.as_str());
    match state.store.get(state.tenant_id, &mp_id).await {
        Ok(Some(record)) => {
            let updated_at = record.updated_at.to_string();
            Json(PartnerResponse { record, updated_at }).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "Not Found").into_response(),
        Err(e) => internal_error(e),
    }
}

/// `PUT /admin/partners/{mp_id}` — create or update a partner record.
///
/// The `gln` in the path must match `record.mp_id` in the body; a mismatch is
/// rejected with `400 Bad Request`.
#[utoipa::path(
    put,
    path = "/admin/partners/{mp_id}",
    tag = "admin",
    params(("mp_id" = String, Path, description = "13-digit GLN")),
    request_body(content = UpsertRequest, content_type = "application/json"),
    responses(
        (status = 200, description = "Upserted", body = PartnerResponse),
        (status = 400, description = "GLN mismatch"),
        (status = 401, description = "Missing or invalid bearer token"),
    ),
    security((), ("bearer_token" = []))
)]
pub(crate) async fn handle_put(
    headers: HeaderMap,
    State(state): State<Arc<PartnerAdminState>>,
    Path(gln_str): Path<String>,
    Json(body): Json<UpsertRequest>,
) -> Response {
    let identity = match state.cedar.authenticate(&headers) {
        Some(id) => id,
        None => return unauthorized(),
    };
    if !state.cedar.authorize_partner(
        &identity,
        MakoAction::AdminPartnerWrite,
        &PartnerResource {
            tenant: &state.tenant_id.to_string(),
            mp_id: Some(&gln_str),
        },
    ) {
        return forbidden();
    }
    let path_gln = MarktpartnerCode::from(gln_str.as_str());
    if body.record.mp_id != path_gln {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!(
                    "GLN in path ({path_gln}) does not match GLN in body ({})",
                    body.record.mp_id
                ),
            }),
        )
            .into_response();
    }
    match state.store.upsert(state.tenant_id, &body.record).await {
        Ok(()) => {
            info!(mp_id = %path_gln, tenant = %state.tenant_id, "partner upserted via admin API");
            let updated_at = body.record.updated_at.to_string();
            (
                StatusCode::OK,
                Json(PartnerResponse {
                    record: body.record,
                    updated_at,
                }),
            )
                .into_response()
        }
        Err(e) => internal_error(e),
    }
}

/// `DELETE /admin/partners/{mp_id}` — remove a partner record.
#[utoipa::path(
    delete,
    path = "/admin/partners/{mp_id}",
    tag = "admin",
    params(("mp_id" = String, Path, description = "13-digit GLN")),
    responses(
        (status = 200, description = "Deletion result", body = DeleteResponse),
        (status = 401, description = "Missing or invalid bearer token"),
    ),
    security((), ("bearer_token" = []))
)]
pub(crate) async fn handle_delete(
    headers: HeaderMap,
    State(state): State<Arc<PartnerAdminState>>,
    Path(gln_str): Path<String>,
) -> Response {
    let identity = match state.cedar.authenticate(&headers) {
        Some(id) => id,
        None => return unauthorized(),
    };
    if !state.cedar.authorize_partner(
        &identity,
        MakoAction::AdminPartnerDelete,
        &PartnerResource {
            tenant: &state.tenant_id.to_string(),
            mp_id: Some(&gln_str),
        },
    ) {
        return forbidden();
    }
    let mp_id = MarktpartnerCode::from(gln_str.as_str());
    match state.store.remove(state.tenant_id, &mp_id).await {
        Ok(()) => {
            info!(%mp_id, tenant = %state.tenant_id, "partner removed via admin API");
            Json(DeleteResponse {
                mp_id: mp_id.to_string(),
                deleted: true,
            })
            .into_response()
        }
        Err(e) => internal_error(e),
    }
}

/// `POST /admin/partners/import` — import partners from a raw PARTIN EDIFACT
/// interchange.
///
/// Accepts a raw EDIFACT interchange (`Content-Type: text/plain; charset=utf-8`
/// or `application/edifact`). Each PARTIN message in the interchange is parsed,
/// the sender's communication data (GLN, AS4 endpoint, email, phone) is
/// extracted, and the result is upserted into the [`PartnerStore`].
///
/// This endpoint is idempotent — reimporting the same interchange is safe and
/// will update existing records according to the `valid_from` merge rules of
/// [`PartnerRecord::merge_from_partin`].
///
/// Returns a JSON summary of how many records were upserted/skipped.
#[utoipa::path(
    post,
    path = "/admin/partners/import",
    tag = "admin",
    request_body(content = String, description = "Raw EDIFACT PARTIN interchange", content_type = "text/plain; charset=utf-8"),
    responses(
        (status = 200, description = "Import result", body = ImportResponse),
        (status = 400, description = "Empty body"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 422, description = "EDIFACT parse error"),
    ),
    security((), ("bearer_token" = []))
)]
pub(crate) async fn handle_import(
    headers: HeaderMap,
    State(state): State<Arc<PartnerAdminState>>,
    body: Bytes,
) -> Response {
    let identity = match state.cedar.authenticate(&headers) {
        Some(id) => id,
        None => return unauthorized(),
    };
    if !state.cedar.authorize_partner(
        &identity,
        MakoAction::AdminPartnerImport,
        &PartnerResource {
            tenant: &state.tenant_id.to_string(),
            mp_id: None,
        },
    ) {
        return forbidden();
    }
    if body.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "request body is empty".to_string(),
            }),
        )
            .into_response();
    }

    let mut upserted = 0usize;
    let mut skipped = 0usize;
    let mut glns = Vec::new();

    for result in state
        .platform
        .parse_interchange(std::io::Cursor::new(&body[..]))
    {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, "PARTIN import: parse error — skipping message");
                skipped += 1;
                continue;
            }
        };
        if let AnyMessage::Partin(partin) = msg {
            let pid = partin.detect_pruefidentifikator().ok().map(|p| p.as_u32());
            match crate::edifact_api::partin_to_partner_record(&partin, pid) {
                Some(record) => {
                    let gln_str = record.mp_id.to_string();
                    match state.store.upsert(state.tenant_id, &record).await {
                        Ok(()) => {
                            info!(mp_id = %gln_str, "PARTIN import: partner upserted");
                            glns.push(gln_str);
                            upserted += 1;
                        }
                        Err(e) => {
                            tracing::warn!(mp_id = %gln_str, error = %e, "PARTIN import: upsert failed");
                            skipped += 1;
                        }
                    }
                }
                None => {
                    tracing::debug!("PARTIN import: no sender GLN — skipping");
                    skipped += 1;
                }
            }
        } else {
            // Non-PARTIN message in interchange — ignore
            skipped += 1;
        }
    }

    (
        StatusCode::OK,
        Json(ImportResponse {
            upserted,
            skipped,
            glns,
        }),
    )
        .into_response()
}

// ── Bootstrap helper ──────────────────────────────────────────────────────────

/// Seed the partner store from `[as4] partners = ["GLN=URL", …]` config pairs.
///
/// Called once at daemon startup before the HTTP server begins serving.
/// Records created here carry only a GLN and an AS4 endpoint URL; they are
/// upgraded in-place when the partner later sends a PARTIN message.
///
/// # Errors
///
/// Returns an error if a config pair is malformed or if a store write fails.
pub async fn seed_from_config(
    store: &SlateDbPartnerStore,
    tenant_id: TenantId,
    pairs: &[String],
) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use mako_engine::partner::PartnerStore as _;

    let records = PartnerRecord::from_cli_pairs(pairs).context("parsing [as4] partners config")?;

    for record in &records {
        store
            .upsert(tenant_id, record)
            .await
            .with_context(|| format!("seeding partner {}", record.mp_id))?;
    }

    if !records.is_empty() {
        info!(
            count  = records.len(),
            tenant = %tenant_id,
            "seeded partner store from config"
        );
    }
    Ok(())
}

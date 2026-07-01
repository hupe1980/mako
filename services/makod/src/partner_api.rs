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
//! | `GET` | `/admin/partners/{gln}` | Retrieve a single partner record |
//! | `PUT` | `/admin/partners/{gln}` | Create or update a partner record |
//! | `DELETE` | `/admin/partners/{gln}` | Remove a partner record |
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
//! is extracted and upserted into the [`PartnerStore`].
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
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tracing::info;

// ── State ─────────────────────────────────────────────────────────────────────

/// Shared state for the partner admin API.
pub struct PartnerAdminState {
    pub store: SlateDbPartnerStore,
    pub tenant_id: TenantId,
    pub optional_token: Option<SecretString>,
    /// Shared EDIFACT platform for parsing PARTIN interchanges submitted to
    /// `POST /admin/partners/import`.
    pub platform: Arc<Platform>,
}

// ── Request / response types ──────────────────────────────────────────────────

/// Request body for `PUT /admin/partners/{gln}`.
///
/// Accepts a full [`PartnerRecord`] as JSON. The `gln` field in the body
/// must match the `{gln}` path parameter; a mismatch is rejected with `400`.
#[derive(Debug, Deserialize)]
pub struct UpsertRequest {
    #[serde(flatten)]
    pub record: PartnerRecord,
}

#[derive(Serialize)]
struct PartnerResponse {
    #[serde(flatten)]
    record: PartnerRecord,
    updated_at: String,
}

#[derive(Serialize)]
struct ListResponse {
    partners: Vec<PartnerRecord>,
    count: usize,
}

#[derive(Serialize)]
struct DeleteResponse {
    gln: String,
    deleted: bool,
}

#[derive(Serialize)]
struct ImportResponse {
    /// Number of PARTIN records successfully upserted.
    upserted: usize,
    /// Number of PARTIN messages that had no extractable GLN.
    skipped: usize,
    /// GLNs that were upserted.
    glns: Vec<String>,
}

#[derive(Serialize)]
struct ErrorResponse {
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
        .route("/admin/partners/{gln}", get(handle_get))
        .route("/admin/partners/{gln}", put(handle_put))
        .route("/admin/partners/{gln}", delete(handle_delete))
        .with_state(state)
}

// ── Auth helper ───────────────────────────────────────────────────────────────

fn check_auth(headers: &HeaderMap, optional_token: &Option<SecretString>) -> bool {
    let Some(expected) = optional_token else {
        return true;
    };
    let provided = headers
        .get("authorization")
        .or_else(|| headers.get("Authorization"))
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    provided
        .as_bytes()
        .ct_eq(expected.expose_secret().as_bytes())
        .into()
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
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
async fn handle_list(headers: HeaderMap, State(state): State<Arc<PartnerAdminState>>) -> Response {
    if !check_auth(&headers, &state.optional_token) {
        return unauthorized();
    }
    match state.store.list(state.tenant_id).await {
        Ok(partners) => {
            let count = partners.len();
            Json(ListResponse { partners, count }).into_response()
        }
        Err(e) => internal_error(e),
    }
}

/// `GET /admin/partners/{gln}` — retrieve a single partner record.
async fn handle_get(
    headers: HeaderMap,
    State(state): State<Arc<PartnerAdminState>>,
    Path(gln_str): Path<String>,
) -> Response {
    if !check_auth(&headers, &state.optional_token) {
        return unauthorized();
    }
    let gln = MarktpartnerCode::from(gln_str.as_str());
    match state.store.get(state.tenant_id, &gln).await {
        Ok(Some(record)) => {
            let updated_at = record.updated_at.to_string();
            Json(PartnerResponse { record, updated_at }).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "Not Found").into_response(),
        Err(e) => internal_error(e),
    }
}

/// `PUT /admin/partners/{gln}` — create or update a partner record.
///
/// The `gln` in the path must match `record.gln` in the body; a mismatch is
/// rejected with `400 Bad Request`.
async fn handle_put(
    headers: HeaderMap,
    State(state): State<Arc<PartnerAdminState>>,
    Path(gln_str): Path<String>,
    Json(body): Json<UpsertRequest>,
) -> Response {
    if !check_auth(&headers, &state.optional_token) {
        return unauthorized();
    }
    let path_gln = MarktpartnerCode::from(gln_str.as_str());
    if body.record.gln != path_gln {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!(
                    "GLN in path ({path_gln}) does not match GLN in body ({})",
                    body.record.gln
                ),
            }),
        )
            .into_response();
    }
    match state.store.upsert(state.tenant_id, &body.record).await {
        Ok(()) => {
            info!(gln = %path_gln, tenant = %state.tenant_id, "partner upserted via admin API");
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

/// `DELETE /admin/partners/{gln}` — remove a partner record.
async fn handle_delete(
    headers: HeaderMap,
    State(state): State<Arc<PartnerAdminState>>,
    Path(gln_str): Path<String>,
) -> Response {
    if !check_auth(&headers, &state.optional_token) {
        return unauthorized();
    }
    let gln = MarktpartnerCode::from(gln_str.as_str());
    match state.store.remove(state.tenant_id, &gln).await {
        Ok(()) => {
            info!(%gln, tenant = %state.tenant_id, "partner removed via admin API");
            Json(DeleteResponse {
                gln: gln.to_string(),
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
async fn handle_import(
    headers: HeaderMap,
    State(state): State<Arc<PartnerAdminState>>,
    body: Bytes,
) -> Response {
    if !check_auth(&headers, &state.optional_token) {
        return unauthorized();
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
                    let gln_str = record.gln.to_string();
                    match state.store.upsert(state.tenant_id, &record).await {
                        Ok(()) => {
                            info!(gln = %gln_str, "PARTIN import: partner upserted");
                            glns.push(gln_str);
                            upserted += 1;
                        }
                        Err(e) => {
                            tracing::warn!(gln = %gln_str, error = %e, "PARTIN import: upsert failed");
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
            .with_context(|| format!("seeding partner {}", record.gln))?;
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

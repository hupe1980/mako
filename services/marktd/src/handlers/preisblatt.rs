//! Handlers for `GET|PUT /api/v1/preisblaetter/{nb_mp_id}`.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::{
    cloudevents::MarktEvent,
    repository::{PreisblattRepository, PreisblattSource, PriCatRepository as _},
};
use mako_service::cedar::CedarEnforcer;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;
use utoipa::{IntoParams, ToSchema};

use crate::pg::{PgPreisblattRepository, PgPriCatRepository};

use super::{Claims, TenantGln};

// ── Type alias ────────────────────────────────────────────────────────────────

/// The preisblatt repo extension type injected via Axum `Extension`.
pub type PreisblattRepoExt = Arc<PgPreisblattRepository>;

/// PRICAT version history + dispatch repo extension type.
pub type PriCatRepoExt = Arc<PgPriCatRepository>;

// ── DTOs ─────────────────────────────────────────────────────────────────────

/// Query parameters for `GET /api/v1/preisblaetter/{nb_mp_id}`.
#[derive(Debug, Deserialize, IntoParams)]
pub struct PreisblattQuery {
    /// Billing date (`YYYY-MM-DD`) used for temporal validity lookup.
    /// Defaults to today (UTC) when absent.
    pub date: Option<String>,
}

/// Request body for `PUT /api/v1/preisblaetter/{nb_mp_id}`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PreisblattUpsertRequest {
    /// Full BO4E `PreisblattNetznutzung` payload.
    pub data: serde_json::Value,
    /// BO4E schema version of `data` (e.g. `"v202501.0.0"`). Defaults to current.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
}

fn default_bo4e_version() -> String {
    "v202501.0.0".to_owned()
}

/// Response body for `GET /api/v1/preisblaetter/{nb_mp_id}`.
///
/// Wraps the BO4E payload with metadata so callers can distinguish
/// operator API uploads from engine-ingested sheets.
#[derive(Debug, Serialize, ToSchema)]
pub struct PreisblattResponse {
    /// The full BO4E `PreisblattNetznutzung` payload.
    pub data: serde_json::Value,
    /// How this record entered the system.
    /// `"api"` — operator REST upload; `"mako"` — PRICAT 27003 engine ingest.
    pub source: String,
    /// BO4E schema version of `data`.
    pub bo4e_version: String,
    /// Wall-clock time (UTC) when this sheet was last written.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /api/v1/preisblaetter/{nb_mp_id}?date={billing_date}`
///
/// Returns the `PreisblattNetznutzung` for the NB GLN valid on `date`.
/// When `date` is absent, today's UTC date is used.
/// Returns 404 when no matching price sheet is found.
#[utoipa::path(
    get,
    path = "/api/v1/preisblaetter/{nb_mp_id}",
    params(
        ("nb_mp_id" = String, Path, description = "NB GLN (13-digit BDEW/DVGW code)"),
        PreisblattQuery,
    ),
    responses(
        (status = 200, description = "PreisblattNetznutzung JSON", body = PreisblattResponse),
        (status = 404, description = "No price sheet found for this NB on the given date"),
    ),
)]
pub async fn get_preisblatt(
    Extension(repo): Extension<PreisblattRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(nb_mp_id): Path<String>,
    Query(query): Query<PreisblattQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-preisblatt", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    let billing_date = query.date.unwrap_or_else(today_iso);

    match repo.find_for_date(&nb_mp_id, &billing_date).await {
        Ok(Some(record)) => Json(PreisblattResponse {
            data: record.data,
            source: record.source.to_string(),
            bo4e_version: record.bo4e_version,
            updated_at: record.updated_at,
        })
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            format!("No Preisblatt found for NB GLN {nb_mp_id} on {billing_date}"),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `PUT /api/v1/preisblaetter/{nb_mp_id}`
///
/// Upsert a `PreisblattNetznutzung` for the given NB GLN.
///
/// This endpoint always writes with `source = "api"`.  An API-sourced sheet is
/// never silently overwritten by a later mako engine ingest (operator override
/// protection).  Returns `204 No Content` on success.
#[utoipa::path(
    put,
    path = "/api/v1/preisblaetter/{nb_mp_id}",
    params(
        ("nb_mp_id" = String, Path, description = "NB GLN (13-digit BDEW/DVGW code)"),
    ),
    request_body = PreisblattUpsertRequest,
    responses(
        (status = 204, description = "Price sheet stored"),
        (status = 400, description = "Bad request"),
        (status = 403, description = "Forbidden"),
    ),
)]
#[allow(clippy::too_many_arguments)]
pub async fn put_preisblatt(
    Extension(repo): Extension<PreisblattRepoExt>,
    Extension(pricat_repo): Extension<PriCatRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(event_tx): Extension<UnboundedSender<serde_json::Value>>,
    claims: Claims,
    Path(nb_mp_id): Path<String>,
    Json(req): Json<PreisblattUpsertRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-preisblatt", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    // REST API calls always count as source='api' — operator override protection.
    let data = req.data;
    let bo4e_version = req.bo4e_version;
    match repo
        .upsert(
            &nb_mp_id,
            data.clone(),
            &bo4e_version,
            PreisblattSource::Api,
        )
        .await
    {
        Ok(()) => {
            // Phase 2: also store a versioned snapshot in pricat_versions and
            // emit de.markt.pricat.published so ERP subscribers and the dispatch
            // background task can react.
            //
            // This is best-effort: a failure here is logged but does NOT fail the
            // API call — the preisblaetter record is already durably stored.
            let nb_gln2 = nb_mp_id.clone();
            let tenant2 = tenant_gln.clone();
            let data2 = data.clone();
            let bo4e2 = bo4e_version.clone();
            tokio::spawn(async move {
                // Extract validity dates from the BO4E payload.
                let valid_from = data2
                    .pointer("/gueltigkeit/startdatum")
                    .and_then(|v| v.as_str())
                    .and_then(|s| {
                        let parts: Vec<&str> = s.splitn(4, '-').collect();
                        if parts.len() >= 3 {
                            let y: i32 = parts[0].parse().ok()?;
                            let m: u8 = parts[1].parse().ok()?;
                            let d: u8 = parts[2].parse().ok()?;
                            let month = time::Month::try_from(m).ok()?;
                            time::Date::from_calendar_date(y, month, d).ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| time::OffsetDateTime::now_utc().date());

                let valid_to = data2
                    .pointer("/gueltigkeit/enddatum")
                    .and_then(|v| v.as_str())
                    .and_then(|s| {
                        let parts: Vec<&str> = s.splitn(4, '-').collect();
                        if parts.len() >= 3 {
                            let y: i32 = parts[0].parse().ok()?;
                            let m: u8 = parts[1].parse().ok()?;
                            let d: u8 = parts[2].parse().ok()?;
                            let month = time::Month::try_from(m).ok()?;
                            time::Date::from_calendar_date(y, month, d).ok()
                        } else {
                            None
                        }
                    });

                match pricat_repo
                    .upsert_version(
                        &nb_gln2,
                        &tenant2,
                        valid_from,
                        valid_to,
                        data2,
                        &bo4e2,
                        PreisblattSource::Api,
                    )
                    .await
                {
                    Ok(version_id) => {
                        // Emit de.markt.pricat.published so ERP webhook
                        // subscribers and the obsd observability daemon are notified.
                        let evt = MarktEvent::new(
                            &tenant2,
                            "de.markt.pricat.published",
                            nb_gln2.clone(),
                            serde_json::json!({
                                "nb_mp_id": nb_gln2,
                                "version_id": version_id,
                                "valid_from": valid_from.to_string(),
                            }),
                        );
                        if let Ok(payload) = serde_json::to_value(&evt) {
                            let _ = event_tx.send(payload);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            nb_mp_id = %nb_gln2,
                            error  = %e,
                            "put_preisblatt: pricat_versions upsert failed (non-fatal)",
                        );
                    }
                }
            });

            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Helper ────────────────────────────────────────────────────────────────────

/// Returns today's UTC date as `"YYYY-MM-DD"`.
fn today_iso() -> String {
    let now = time::OffsetDateTime::now_utc();
    let d = now.date();
    format!("{:04}-{:02}-{:02}", d.year(), d.month() as u8, d.day())
}

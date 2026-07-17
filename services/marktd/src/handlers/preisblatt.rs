//! Handlers for `GET|PUT /api/v1/preisblaetter/{nb_mp_id}` and
//! `GET|PUT /api/v1/preisblaetter-messung/{msb_mp_id}`.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::{
    cloudevents::MarktEvent,
    repository::{
        PreisblattDienstleistungRepository, PreisblattHardwareRepository, PreisblattKaRepository,
        PreisblattMessungRepository, PreisblattRepository, PreisblattSource, PriCatRepository as _,
    },
};
use mako_service::cedar::CedarEnforcer;
use rubo4e::current::{
    LastvariablePreisposition, PreisblattMessung, PreisblattNetznutzung, ZeitvariablePreisposition,
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;
use utoipa::{IntoParams, ToSchema};

use crate::pg::{
    PgPreisblattDienstleistungRepository, PgPreisblattHardwareRepository, PgPreisblattKaRepository,
    PgPreisblattMessungRepository, PgPreisblattRepository, PgPriCatRepository,
};

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
    /// BO4E schema version of `data` (e.g. `"v202607.0.0"`). Defaults to current.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
}

fn default_bo4e_version() -> String {
    "v202607.0.0".to_owned()
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
    /// §14a Modul 2 time-variable NNE price positions (BNetzA BK6-22-300).
    ///
    /// Extracted from `data.zeitvariablePreispositionen` for explicit typed access.
    /// Contains ToU (time-of-use) discount bands for controllable loads
    /// (heat pumps, EV charging, §14a-eligible assets).
    ///
    /// Mandatory for all NB deployments since 01.01.2024.  Consumers
    /// (`invoicd` check 4, `netzbilanzd` Modul-2 billing) MUST check this
    /// list before falling back to static `Leistungstyp` positions.
    ///
    /// `null` / empty = no ToU bands configured (pure static NNE tariff).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub zeitvariable_preispositionen: Vec<serde_json::Value>,
    /// §14a Modul 3 load-variable NNE pricing formula (BNetzA BK6-22-300 Anlage 2 §3).
    ///
    /// Extracted from `data.lastvariablePreispositionen` for typed ERP consumption.
    /// Each element is a `LastvariablePreisposition` BO4E COM describing the
    /// spot-price-linked NNE formula for one pricing band:
    /// - `tarifkalkulationsmethode = SPOTPREIS`
    /// - `preisreferenz = ENERGIEMENGE`
    /// - `preisBezugseinheit = KWH`
    /// - `preisstaffeln` — formula parameters (multiplier, floor/ceiling rates)
    ///
    /// Used by `netzbilanzd` Modul-3 billing to build per-interval `NneArbeitModul3`
    /// positions in `GridSettlement`. `billingd` embeds these in `Rechnungsposition.
    /// zusatzAttribute` for portal display.
    ///
    /// `null` / empty = Modul 3 not offered by this NB (most deployments pre-2025).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lastvariable_preispositionen: Vec<serde_json::Value>,
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
        Ok(Some(record)) => {
            // Extract §14a Modul 2 time-variable positions from the stored JSONB.
            // The NB PUTs them under `zeitvariablePreispositionen` (BO4E camelCase).
            // Expose at top-level for explicit typed consumption by invoicd / netzbilanzd.
            let zeitvariable = record
                .data
                .get("zeitvariablePreispositionen")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            // Extract §14a Modul 3 load-variable NNE pricing formula positions.
            // Validated as `rubo4e::current::LastvariablePreisposition` on PUT.
            let lastvariable = record
                .data
                .get("lastvariablePreispositionen")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            Json(PreisblattResponse {
                data: record.data,
                source: record.source.to_string(),
                bo4e_version: record.bo4e_version,
                updated_at: record.updated_at,
                zeitvariable_preispositionen: zeitvariable,
                lastvariable_preispositionen: lastvariable,
            })
            .into_response()
        }
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
    // Validate the payload against rubo4e::current::PreisblattNetznutzung (B15).
    // This catches wrong `_typ`, invalid enum values in `preispositionen`, and
    // malformed `zeitvariablePreispositionen.zaehlzeitregister` before DB insert.
    if let Err(e) = serde_json::from_value::<PreisblattNetznutzung>(req.data.clone()) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!("invalid PreisblattNetznutzung: {e}")
            })),
        )
            .into_response();
    }

    // ── M7: Validate lastvariablePreispositionen (§14a Modul 3, BK6-22-300 Anlage 2 §3) ──
    //
    // Validates each element against `rubo4e::current::LastvariablePreisposition`.
    // Rules enforced beyond the BO4E schema:
    //   1. Each element must deserialize as `LastvariablePreisposition`.
    //   2. `tarifkalkulationsmethode` must be `SPOTPREIS` for §14a Modul 3.
    //      Other methods (STAFFELN, INDEXIERT, …) are not Modul-3-compliant.
    //   3. `preisBezugseinheit` must be `KWH` (NNE is always per kWh for controllable loads).
    //
    // Unlike `zeitvariablePreispositionen`, `lastvariablePreispositionen` is an extension
    // field not in the standard BO4E schema — stored in `_additional` extension map.
    if let Some(lvp_arr) = req
        .data
        .get("lastvariablePreispositionen")
        .and_then(|v| v.as_array())
    {
        for (i, item) in lvp_arr.iter().enumerate() {
            if let Err(e) = serde_json::from_value::<LastvariablePreisposition>(item.clone()) {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": format!(
                            "lastvariablePreispositionen[{i}] is not a valid LastvariablePreisposition: {e}"
                        )
                    })),
                )
                    .into_response();
            }
            // M7.2: tarifkalkulationsmethode must be SPOTPREIS for §14a Modul 3 NNE
            if let Some(method) = item
                .get("tarifkalkulationsmethode")
                .and_then(|v| v.as_str())
                && method != "SPOTPREIS"
            {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": format!(
                            "lastvariablePreispositionen[{i}].tarifkalkulationsmethode must be \
                             \"SPOTPREIS\" for §14a Modul 3 NNE (got \"{method}\")"
                        )
                    })),
                )
                    .into_response();
            }
            // M7.3: preisBezugseinheit must be KWH
            if let Some(unit) = item.get("preisBezugseinheit").and_then(|v| v.as_str())
                && unit != "KWH"
            {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": format!(
                            "lastvariablePreispositionen[{i}].preisBezugseinheit must be \
                             \"KWH\" for §14a Modul 3 (got \"{unit}\")"
                        )
                    })),
                )
                    .into_response();
            }
        }
    }

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

// ── PreisblattMessung (MSB) — B5 ─────────────────────────────────────────────

/// Extension type for the MSB preisblatt repo.
pub type PreisblattMessungRepoExt = Arc<PgPreisblattMessungRepository>;

/// Response body for `GET /api/v1/preisblaetter-messung/{msb_mp_id}`.
///
/// ## M5 — typed `zeitvariablePreispositionen`
///
/// `zeitvariable_preispositionen` is exposed as a typed `Vec<ZeitvariablePreisposition>`
/// (deserialized from stored JSONB). This enables consumers (`invoicd` check 4,
/// `invoic-checker` MSB position validation) to directly access the `zaehlzeitregister`
/// band code without JSON path traversal.
///
/// `schema_drift` counts elements that could not be deserialized (indicates stale data
/// written before M5 validation — re-PUT to normalize).
#[derive(Debug, Serialize, ToSchema)]
pub struct PreisblattMessungResponse {
    /// The full BO4E `PreisblattMessung` payload.
    pub data: serde_json::Value,
    /// How this record entered the system (`"api"` or `"mako"`).
    pub source: String,
    /// BO4E schema version of `data`.
    pub bo4e_version: String,
    /// Wall-clock time (UTC) when this sheet was last written.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
    /// Price supplements and discounts (BO4E `AufAbschlag`).
    ///
    /// Extracted from `data.aufAbschlaege` for explicit typed access.
    /// Contains conditional supplements and discounts on the MSB base price:
    /// - remote-read surcharge (`AufAbschlagstyp::AUFSCHLAG`)
    /// - §14a Modul 2/3 ToU discount bands (`AufAbschlagstyp::ABSCHLAG`)
    /// - hardware rental add-ons
    ///
    /// An empty list means no supplements/discounts are configured.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub auf_abschlaege: Vec<serde_json::Value>,
    /// §14a Modul 2 time-variable MSB price positions.
    ///
    /// Typed `Vec<ZeitvariablePreisposition>` deserialized from stored JSONB.
    /// Each element carries `zaehlzeitregister` (e.g. `"HT"`, `"NT"`, `"ST"`) which
    /// identifies the TOU band.  Used by `invoicd` check 4 for INVOIC 31009 MSB
    /// tariff validation and by `billingd` §14a Modul 2/3 billing.
    ///
    /// Empty = no TOU bands configured (pure flat MSB tariff).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = Vec<Object>)]
    pub zeitvariable_preispositionen: Vec<ZeitvariablePreisposition>,
    /// Number of `zeitvariablePreispositionen` elements that failed schema validation.
    /// Non-zero indicates stale pre-M5 data — re-PUT the sheet to normalize.
    #[serde(skip_serializing_if = "is_zero")]
    pub schema_drift_count: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// `GET /api/v1/preisblaetter-messung/{msb_mp_id}?date={billing_date}`
///
/// Returns the `PreisblattMessung` for the MSB MP-ID valid on `date`.
/// When `date` is absent, today's UTC date is used.  Returns 404 when not found.
///
/// Used by `invoicd` for PID 31009 tariff plausibility checks (positions 4+5).
#[utoipa::path(
    get,
    path = "/api/v1/preisblaetter-messung/{msb_mp_id}",
    params(
        ("msb_mp_id" = String, Path, description = "MSB MP-ID (13-digit BDEW code)"),
        ("date" = Option<String>, Query, description = "Billing date YYYY-MM-DD; defaults to today"),
    ),
    responses(
        (status = 200, description = "PreisblattMessung JSON", body = PreisblattMessungResponse),
        (status = 404, description = "No price sheet found"),
    ),
)]
pub async fn get_preisblatt_messung(
    Extension(repo): Extension<PreisblattMessungRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(msb_mp_id): Path<String>,
    Query(query): Query<PreisblattQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-preisblatt", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    let billing_date = query.date.unwrap_or_else(today_iso);

    match repo.find_messung_for_date(&msb_mp_id, &billing_date).await {
        Ok(Some(record)) => {
            let auf_abschlaege = record
                .data
                .get("aufAbschlaege")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            // M5: Deserialize `zeitvariablePreispositionen` as typed
            // `Vec<ZeitvariablePreisposition>` for downstream typed consumption.
            let raw_zvp = record
                .data
                .get("zeitvariablePreispositionen")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let mut schema_drift_count = 0u32;
            let zeitvariable_preispositionen: Vec<ZeitvariablePreisposition> = raw_zvp
                .into_iter()
                .filter_map(|item| {
                    serde_json::from_value::<ZeitvariablePreisposition>(item)
                        .map_err(|e| {
                            schema_drift_count += 1;
                            tracing::warn!(
                                msb_mp_id = %msb_mp_id,
                                error = %e,
                                "schema drift: stored ZeitvariablePreisposition cannot be \
                                 deserialised — re-PUT the PreisblattMessung to normalize (M5)"
                            );
                        })
                        .ok()
                })
                .collect();

            Json(PreisblattMessungResponse {
                data: record.data,
                source: record.source.to_string(),
                bo4e_version: record.bo4e_version,
                updated_at: record.updated_at,
                auf_abschlaege,
                zeitvariable_preispositionen,
                schema_drift_count,
            })
            .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            format!("No PreisblattMessung found for MSB MP-ID {msb_mp_id} on {billing_date}"),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Request body for `PUT /api/v1/preisblaetter-messung/{msb_mp_id}`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PreisblattMessungUpsertRequest {
    /// Full BO4E `PreisblattMessung` payload.
    pub data: serde_json::Value,
    /// BO4E schema version of `data`. Defaults to current.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
}

/// `PUT /api/v1/preisblaetter-messung/{msb_mp_id}`
///
/// Upsert a `PreisblattMessung` for the given MSB MP-ID.
/// Always sets `source = "api"`.  Returns `204 No Content` on success.
#[utoipa::path(
    put,
    path = "/api/v1/preisblaetter-messung/{msb_mp_id}",
    params(
        ("msb_mp_id" = String, Path, description = "MSB MP-ID (13-digit BDEW code)"),
    ),
    request_body = PreisblattMessungUpsertRequest,
    responses(
        (status = 204, description = "Price sheet stored"),
        (status = 400, description = "Bad request"),
        (status = 403, description = "Forbidden"),
    ),
)]
pub async fn put_preisblatt_messung(
    Extension(repo): Extension<PreisblattMessungRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(msb_mp_id): Path<String>,
    Json(req): Json<PreisblattMessungUpsertRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-preisblatt", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    // ── Basic BO4E schema validation ─────────────────────────────────────────
    // Catches wrong `_typ`, missing required fields, and invalid enum values.
    // `ZeitvariablePreisposition` elements are stored in `_additional` extension
    // data of `PreisblattMessung` — they are validated separately below (M5).
    if let Err(e) = serde_json::from_value::<PreisblattMessung>(req.data.clone()) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!("invalid PreisblattMessung: {e}")
            })),
        )
            .into_response();
    }

    // ── M5: Full ZeitvariablePreisposition validation ────────────────────────
    //
    // The basic `PreisblattMessung` schema check above validates the outer envelope.
    // Now extract `zeitvariablePreispositionen` from the raw JSON and apply
    // BDEW business rules that are NOT enforced by the BO4E schema:
    //
    // 1. Each element must deserialize as `ZeitvariablePreisposition`.
    // 2. `zaehlzeitregister` MUST be non-empty (§14a Modul 2, BK6-22-300):
    //    An MSB with ToU pricing MUST identify each band code (e.g. "HT", "NT", "ST").
    //    Without it, `invoic-checker` cannot match INVOIC 31009 positions against bands.
    // 3. Reject `bandNummer` (does NOT exist in BO4E v202607 — pre-standardization field).
    //    Operators who accidentally set `bandNummer` instead of `zaehlzeitregister` would
    //    get silent failures in invoic-checker check 4.

    let mut data = req.data;
    if let Some(zvp_arr) = data
        .get("zeitvariablePreispositionen")
        .and_then(|v| v.as_array())
        .cloned()
    {
        let mut validated: Vec<serde_json::Value> = Vec::with_capacity(zvp_arr.len());
        for (i, item) in zvp_arr.iter().enumerate() {
            // Reject `bandNummer` — it does not exist in BO4E v202607.
            // Fail loudly so operators fix their PRICAT import tooling.
            if item.get("bandNummer").is_some() {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": format!(
                            "zeitvariablePreispositionen[{}]: 'bandNummer' is not a valid \
                             BO4E v202607 field — use 'zaehlzeitregister' instead \
                             (e.g. \"HT\", \"NT\", \"ST\")",
                            i
                        )
                    })),
                )
                    .into_response();
            }

            // Parse as ZeitvariablePreisposition to validate schema.
            let zvp = match serde_json::from_value::<ZeitvariablePreisposition>(item.clone()) {
                Ok(z) => z,
                Err(e) => {
                    return (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        Json(serde_json::json!({
                            "error": format!(
                                "zeitvariablePreispositionen[{}] is not a valid \
                                 ZeitvariablePreisposition: {}",
                                i, e
                            )
                        })),
                    )
                        .into_response();
                }
            };

            // Business rule (§14a Modul 2, BK6-22-300): `zaehlzeitregister` is mandatory.
            // Without it, `invoicd` / `invoic-checker` cannot route INVOIC positions to bands.
            match zvp.zaehlzeitregister.as_deref() {
                None | Some("") => {
                    return (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        Json(serde_json::json!({
                            "error": format!(
                                "zeitvariablePreispositionen[{}]: 'zaehlzeitregister' is \
                                 mandatory per §14a Modul 2 (BK6-22-300) — set it to the \
                                 TOU band code (e.g. \"HT\", \"NT\", \"ST\")",
                                i
                            )
                        })),
                    )
                        .into_response();
                }
                _ => {}
            }

            // Re-serialize to canonical BO4E camelCase.
            validated.push(serde_json::to_value(&zvp).unwrap_or(item.clone()));
        }

        // Replace the raw array with the validated canonical form.
        if let Some(obj) = data.as_object_mut() {
            obj.insert(
                "zeitvariablePreispositionen".to_owned(),
                serde_json::Value::Array(validated),
            );
        }
    }

    match repo
        .upsert_messung(&msb_mp_id, data, &req.bo4e_version, PreisblattSource::Api)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── PreisblattKonzessionsabgabe (B3) ─────────────────────────────────────────

/// Extension type for the KA preisblatt repo.
pub type PreisblattKaRepoExt = Arc<PgPreisblattKaRepository>;

/// Query parameters for `GET /api/v1/preisblaetter-ka/{nb_mp_id}`.
#[derive(Debug, Deserialize)]
pub struct PreisblattKaQuery {
    pub date: Option<String>,
    /// Filter by Kundengruppe: `"Tarifkunden"` | `"Sondervertragskunden"` | omit for both.
    pub kundengruppe: Option<String>,
    pub sparte: Option<String>,
}

/// Response body for `GET /api/v1/preisblaetter-ka/{nb_mp_id}`.
#[derive(Debug, Serialize, ToSchema)]
pub struct PreisblattKaResponse {
    pub data: serde_json::Value,
    pub sparte: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kundengruppe_ka: Option<String>,
    pub source: String,
    pub bo4e_version: String,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// `GET /api/v1/preisblaetter-ka/{nb_mp_id}?date=YYYY-MM-DD&sparte=STROM&kundengruppe=Tarifkunden`
///
/// Returns the `PreisblattKonzessionsabgabe` for the NB valid on `date`.
/// Used by `netzbilanzd` for KA positions in INVOIC 31001/31002 (§17 StromNZV).
pub async fn get_preisblatt_ka(
    Extension(repo): Extension<PreisblattKaRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(nb_mp_id): Path<String>,
    Query(query): Query<PreisblattKaQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-preisblatt", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    let billing_date = query.date.unwrap_or_else(today_iso);
    let sparte = query.sparte.as_deref().unwrap_or("STROM");

    match repo
        .find_ka_for_date(
            &nb_mp_id,
            sparte,
            query.kundengruppe.as_deref(),
            &billing_date,
        )
        .await
    {
        Ok(Some(r)) => Json(PreisblattKaResponse {
            data: r.data,
            sparte: r.sparte,
            kundengruppe_ka: r.kundengruppe_ka,
            source: r.source.to_string(),
            bo4e_version: r.bo4e_version,
            updated_at: r.updated_at,
        })
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            format!("No PreisblattKonzessionsabgabe for NB {nb_mp_id} on {billing_date}"),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Request body for `PUT /api/v1/preisblaetter-ka/{nb_mp_id}`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PreisblattKaUpsertRequest {
    pub data: serde_json::Value,
    /// `"STROM"` or `"GAS"`. Defaults to `"STROM"`.
    #[serde(default = "default_sparte")]
    pub sparte: String,
    /// `"Tarifkunden"` | `"Sondervertragskunden"` | omit for both.
    pub kundengruppe_ka: Option<String>,
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
}

fn default_sparte() -> String {
    "STROM".to_owned()
}

/// `PUT /api/v1/preisblaetter-ka/{nb_mp_id}`
///
/// Upsert a `PreisblattKonzessionsabgabe` for the given NB.
/// Always sets `source = "api"`. Returns `204 No Content` on success.
pub async fn put_preisblatt_ka(
    Extension(repo): Extension<PreisblattKaRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(nb_mp_id): Path<String>,
    Json(req): Json<PreisblattKaUpsertRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-preisblatt", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    match repo
        .upsert_ka(
            &nb_mp_id,
            &req.sparte,
            req.kundengruppe_ka.as_deref(),
            req.data,
            &req.bo4e_version,
            PreisblattSource::Api,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── PreisblattDienstleistung ──────────────────────────────────────────────────

pub type PreisblattDlRepoExt = Arc<PgPreisblattDienstleistungRepository>;

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct PreisblattDlResponse {
    pub data: serde_json::Value,
    pub source: String,
    pub bo4e_version: String,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct PreisblattDlUpsertRequest {
    pub data: serde_json::Value,
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
}

pub async fn get_preisblatt_dienstleistung(
    Extension(repo): Extension<PreisblattDlRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(msb_mp_id): Path<String>,
    Query(query): Query<PreisblattQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-preisblatt", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }
    let billing_date = query.date.unwrap_or_else(today_iso);
    match repo
        .find_dienstleistung_for_date(&msb_mp_id, &billing_date)
        .await
    {
        Ok(Some(r)) => Json(PreisblattDlResponse {
            data: r.data,
            source: r.source.to_string(),
            bo4e_version: r.bo4e_version,
            updated_at: r.updated_at,
        })
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            format!("No PreisblattDienstleistung for {msb_mp_id} on {billing_date}"),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn put_preisblatt_dienstleistung(
    Extension(repo): Extension<PreisblattDlRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(msb_mp_id): Path<String>,
    Json(req): Json<PreisblattDlUpsertRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-preisblatt", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }
    match repo
        .upsert_dienstleistung(
            &msb_mp_id,
            req.data,
            &req.bo4e_version,
            PreisblattSource::Api,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── PreisblattHardware ────────────────────────────────────────────────────────

pub type PreisblattHwRepoExt = Arc<PgPreisblattHardwareRepository>;

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct PreisblattHwResponse {
    pub data: serde_json::Value,
    pub source: String,
    pub bo4e_version: String,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct PreisblattHwUpsertRequest {
    pub data: serde_json::Value,
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
}

pub async fn get_preisblatt_hardware(
    Extension(repo): Extension<PreisblattHwRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(msb_mp_id): Path<String>,
    Query(query): Query<PreisblattQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-preisblatt", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }
    let billing_date = query.date.unwrap_or_else(today_iso);
    match repo.find_hardware_for_date(&msb_mp_id, &billing_date).await {
        Ok(Some(r)) => Json(PreisblattHwResponse {
            data: r.data,
            source: r.source.to_string(),
            bo4e_version: r.bo4e_version,
            updated_at: r.updated_at,
        })
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            format!("No PreisblattHardware for {msb_mp_id} on {billing_date}"),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn put_preisblatt_hardware(
    Extension(repo): Extension<PreisblattHwRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(msb_mp_id): Path<String>,
    Json(req): Json<PreisblattHwUpsertRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-preisblatt", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }
    match repo
        .upsert_hardware(
            &msb_mp_id,
            req.data,
            &req.bo4e_version,
            PreisblattSource::Api,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

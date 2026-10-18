//! Handlers for `GET|PUT /api/v1/preisblaetter/{nb_mp_id}` and
//! `GET|PUT /api/v1/preisblaetter-messung/{msb_mp_id}`.

use mako_service::ApiError;
use std::sync::Arc;

use axum::{
    Extension,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::bo4e::Bo4e;
use mako_markt::error::MdmError;
use mako_markt::{
    cloudevents::MarktEvent,
    repository::{
        PreisblattDienstleistungRepository, PreisblattHardwareRepository, PreisblattKaRepository,
        PreisblattMessungRepository, PreisblattRepository, PreisblattSource,
    },
};
use mako_service::Json;
use mako_service::cedar::CedarEnforcer;
use rubo4e::current::{
    LastvariablePreisposition, PreisblattDienstleistung, PreisblattHardware,
    PreisblattKonzessionsabgabe, PreisblattMessung, PreisblattNetznutzung,
    ZeitvariablePreisposition,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::pg::{
    PgPreisblattDienstleistungRepository, PgPreisblattHardwareRepository, PgPreisblattKaRepository,
    PgPreisblattMessungRepository, PgPreisblattRepository, PgPriCatRepository,
};

use super::{Claims, Tenant};

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
#[serde(deny_unknown_fields)]
pub struct PreisblattUpsertRequest {
    /// Full BO4E `PreisblattNetznutzung` payload.
    ///
    /// `bo4e_version` is **not** an envelope field: it is provenance, and only
    /// the server knows which schema series it parsed the payload under. A
    /// caller-supplied one let a row be stamped with a version mako never read
    /// it as.
    pub data: serde_json::Value,
}

fn default_bo4e_version() -> String {
    mako_markt::bo4e::schema_version()
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
    /// The § 14a Modul 3 zeitvariables Netzentgelt (BK8-22/010-A Tenor 3.).
    ///
    /// Extracted from `data.zeitvariablePreispositionen` for explicit typed
    /// access: one `ZeitvariablePreisposition` per Tarifstufe, named by its
    /// `zaehlzeitregister` — `HT`, `ST` and `NT`, all three present (refused on
    /// PUT otherwise). `invoicd` check 4 and `netzbilanzd` read the band prices
    /// from here before falling back to static `Leistungstyp` positions.
    ///
    /// Empty = no zeitvariables Netzentgelt on this sheet.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub zeitvariable_preispositionen: Vec<serde_json::Value>,
    /// Quantity-tiered price positions (`LastvariablePreisposition`).
    ///
    /// Extracted from `data.lastvariablePreispositionen`. Each element is
    /// priced by its `tarifkalkulationsmethode` (`STAFFELN`, `ZONEN`, …) over
    /// its `preisstaffeln`. Not a § 14a module.
    ///
    /// Empty = no quantity-tiered positions on this sheet.
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
    Extension(Tenant(tenant)): Extension<Tenant>,
    claims: Claims,
    Path(nb_mp_id): Path<String>,
    Query(query): Query<PreisblattQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-preisblatt", &tenant)
        .is_err()
    {
        return ApiError::forbidden("access denied").into_response();
    }

    let billing_date = query.date.unwrap_or_else(today_iso);

    match repo.find_for_date(&nb_mp_id, &billing_date).await {
        Ok(Some(record)) => {
            // The § 14a Modul 3 Tarifstufen, exposed at top level for invoicd and
            // netzbilanzd. Validated on PUT.
            let zeitvariable = record
                .data
                .get("zeitvariablePreispositionen")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            // Quantity-tiered positions, validated on PUT.
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
        Ok(None) => ApiError::NotFound.into_response(),
        Err(e) => failure(e),
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
pub async fn put_preisblatt(
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Extension(pool): Extension<sqlx::PgPool>,
    claims: Claims,
    Path(nb_mp_id): Path<String>,
    Json(req): Json<PreisblattUpsertRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-preisblatt", &tenant)
        .is_err()
    {
        return ApiError::forbidden("access denied").into_response();
    }

    // REST API calls always count as source='api' — operator override protection.
    // The BO4E gate: wrong `_typ`, invalid enum values anywhere in
    // `preispositionen`, and malformed `zeitvariablePreispositionen` are all
    // refused before the insert.
    if let Err(e) = mako_markt::bo4e::decode::<PreisblattNetznutzung>(req.data.clone()) {
        return super::bo4e_refusal(&e).into_response();
    }

    if let Err(e) = validate_netznutzung_positionen(&req.data) {
        return e.into_response();
    }

    let data = req.data;
    let bo4e_version = default_bo4e_version();

    // The durable price sheet, its PRICAT version snapshot and the
    // de.markt.pricat.published event commit in ONE transaction. A detached
    // best-effort task would let the two stores diverge and drop the PRICAT
    // 27003 dispatch to the LFs while the operator still saw 204.
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return failure(e),
    };

    if let Err(e) = PgPreisblattRepository::upsert_tx(
        &mut tx,
        &nb_mp_id,
        &data,
        &bo4e_version,
        PreisblattSource::Api,
    )
    .await
    {
        return failure(e);
    }

    // `pricat_versions.valid_from` is NOT NULL — an open-started sheet
    // (`preisblaetter.valid_from IS NULL`) is snapshotted as starting today in
    // German civil time.
    let valid_from = gueltigkeit_date(&data, "/gueltigkeit/startdatum")
        .unwrap_or_else(super::malo::today_berlin);
    let valid_to = gueltigkeit_date(&data, "/gueltigkeit/enddatum");

    let version_id = match PgPriCatRepository::upsert_version_tx(
        &mut tx,
        &nb_mp_id,
        &tenant,
        valid_from,
        valid_to,
        &data,
        &bo4e_version,
        PreisblattSource::Api,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => return failure(e),
    };

    // Emit de.markt.pricat.published so ERP webhook subscribers and the PRICAT
    // dispatch worker are notified.
    let evt = MarktEvent::new(
        &tenant,
        mako_events::markt::PRICAT_PUBLISHED,
        nb_mp_id.clone(),
        serde_json::json!({
            "nb_mp_id": nb_mp_id,
            "version_id": version_id,
            "valid_from": valid_from.to_string(),
        }),
    );
    if let Err(e) = crate::outbox::enqueue(&mut *tx, &evt).await {
        tracing::error!(error = %e, "put_preisblatt: pricat.published enqueue failed");
        return failure(e);
    }

    if let Err(e) = tx.commit().await {
        return failure(e);
    }

    StatusCode::NO_CONTENT.into_response()
}

// ── Helper ────────────────────────────────────────────────────────────────────

/// A failure, answered with the status [`MdmError`] maps it to.
///
/// A database refusal is the caller's to fix — an overlapping validity window
/// (`*_no_overlap`, SQLSTATE `23P01`) or a broken CHECK is `422` — and only an
/// outage is `500`. Formatting every error into a `500` tuple throws that
/// mapping away and tells the operator the server broke when what they need to
/// change is a date.
/// The three Tarifstufen of the § 14a Modul 3 zeitvariables Netzentgelt:
/// BK8-22/010-A Tenor 3. b) obliges one „mit drei Tarifstufen gemäß der
/// Anlage", and a `ZeitvariablePreisposition` names its Stufe in
/// `zaehlzeitregister`.
const MODUL3_TARIFSTUFEN: [&str; 3] = ["HT", "ST", "NT"];

/// Checks the price positions a `PreisblattNetznutzung` carries in its BO4E
/// extension map, which the envelope's own decode does not reach.
///
/// - `zeitvariablePreispositionen` is the § 14a Modul 3 zeitvariables
///   Netzentgelt. Every element names one of [`MODUL3_TARIFSTUFEN`] and the
///   sheet carries all three, so `invoic-checker` and `netzbilanzd` can price
///   each band: a sheet missing a Stufe would leave its energy unpriced.
/// - `lastvariablePreispositionen` is quantity-tiered pricing (`STAFFELN`,
///   `ZONEN`, …). It is not a § 14a module — BK8-22/010-A defines three and none
///   is spot-linked — so it passes the BO4E gate and nothing more.
#[must_use = "a refusal that is not returned stores the sheet anyway"]
fn validate_netznutzung_positionen(data: &serde_json::Value) -> Result<(), ApiError> {
    let at = |list: &str, i: usize, e: &mako_markt::bo4e::Bo4eRejection| {
        let mut detail = e.detail();
        detail.insert("at".into(), format!("{list}[{i}]").into());
        ApiError::unprocessable_with(e.to_string(), serde_json::Value::Object(detail))
    };

    if let Some(zvp) = data
        .get("zeitvariablePreispositionen")
        .and_then(|v| v.as_array())
    {
        let mut stufen = Vec::with_capacity(MODUL3_TARIFSTUFEN.len());
        for (i, item) in zvp.iter().enumerate() {
            if item.get("bandNummer").is_some() {
                return Err(ApiError::unprocessable(format!(
                    "zeitvariablePreispositionen[{i}]: 'bandNummer' is not a BO4E v202607 \
                     field — name the Tarifstufe in 'zaehlzeitregister' (\"HT\", \"ST\" or \"NT\")"
                )));
            }
            // A nested COM need not stamp `_typ`; the gate injects the one the
            // enclosing PreisblattNetznutzung already settled.
            let position = mako_markt::bo4e::decode::<ZeitvariablePreisposition>(item.clone())
                .map_err(|e| at("zeitvariablePreispositionen", i, &e))?;
            match position.zaehlzeitregister.as_deref() {
                Some(stufe) if MODUL3_TARIFSTUFEN.contains(&stufe) => stufen.push(stufe.to_owned()),
                other => {
                    return Err(ApiError::unprocessable(format!(
                        "zeitvariablePreispositionen[{i}].zaehlzeitregister must name a \
                         § 14a Modul 3 Tarifstufe — \"HT\", \"ST\" or \"NT\" \
                         (BK8-22/010-A Tenor 3. b)); got {other:?}"
                    )));
                }
            }
        }
        let fehlend: Vec<&str> = MODUL3_TARIFSTUFEN
            .into_iter()
            .filter(|s| !stufen.iter().any(|have| have == s))
            .collect();
        if !zvp.is_empty() && !fehlend.is_empty() {
            return Err(ApiError::unprocessable(format!(
                "zeitvariablePreispositionen must price all three § 14a Modul 3 \
                 Tarifstufen HT, ST and NT (BK8-22/010-A Tenor 3. b)); missing {}",
                fehlend.join(", ")
            )));
        }
    }

    if let Some(lvp) = data
        .get("lastvariablePreispositionen")
        .and_then(|v| v.as_array())
    {
        for (i, item) in lvp.iter().enumerate() {
            mako_markt::bo4e::decode::<LastvariablePreisposition>(item.clone())
                .map_err(|e| at("lastvariablePreispositionen", i, &e))?;
        }
    }
    Ok(())
}

fn failure(e: impl Into<Failure>) -> axum::response::Response {
    super::mdm_error(e.into().0).into_response()
}

/// Every error a Preisblatt handler meets, as the [`MdmError`] it answers.
struct Failure(MdmError);

impl From<MdmError> for Failure {
    fn from(e: MdmError) -> Self {
        Self(e)
    }
}

impl From<sqlx::Error> for Failure {
    fn from(e: sqlx::Error) -> Self {
        Self(crate::pg::write_error(e))
    }
}

impl From<mako_markt::bo4e::Bo4eSerialiseError> for Failure {
    fn from(e: mako_markt::bo4e::Bo4eSerialiseError) -> Self {
        Self(MdmError::Internal(e.to_string()))
    }
}

/// Today's German calendar date as `"YYYY-MM-DD"` — the day a Preisblatt
/// validity window is measured against.
fn today_iso() -> String {
    let d = mako_fristen::heute();
    format!("{:04}-{:02}-{:02}", d.year(), d.month() as u8, d.day())
}

/// Read a BO4E `Zeitraum` boundary (`gueltigkeit.startdatum` / `.enddatum`) as a
/// civil date. Accepts a plain ISO date and the leading date of an ISO datetime,
/// matching the `::date` cast the `preisblaetter` upsert applies to the same field.
fn gueltigkeit_date(data: &serde_json::Value, pointer: &str) -> Option<time::Date> {
    let s = data.pointer(pointer).and_then(|v| v.as_str())?;
    time::Date::parse(
        s.get(..10)?,
        time::macros::format_description!("[year]-[month]-[day]"),
    )
    .ok()
}

// ── PreisblattMessung (MSB) — B5 ─────────────────────────────────────────────

/// Extension type for the MSB preisblatt repo.
pub type PreisblattMessungRepoExt = Arc<PgPreisblattMessungRepository>;

/// Response body for `GET /api/v1/preisblaetter-messung/{msb_mp_id}`.
///
/// ## Typed `zeitvariablePreispositionen`
///
/// `zeitvariable_preispositionen` is exposed as a typed `Vec<ZeitvariablePreisposition>`
/// (deserialized from stored JSONB). This enables consumers (`invoicd` check 4,
/// `invoic-checker` MSB position validation) to directly access the `zaehlzeitregister`
/// band code without JSON path traversal.
///
/// `schema_drift` counts elements that could not be deserialized (indicates stale data
/// written before the band-code validation — re-PUT to normalize).
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
    /// - discounts (`AufAbschlagstyp::ABSCHLAG`)
    /// - hardware rental add-ons
    ///
    /// An empty list means no supplements/discounts are configured.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub auf_abschlaege: Vec<serde_json::Value>,
    /// Time-of-use MSB price positions.
    ///
    /// Typed `Vec<ZeitvariablePreisposition>` deserialized from stored JSONB.
    /// Each element carries `zaehlzeitregister` (e.g. `"HT"`, `"NT"`), which
    /// identifies the band. Used by `invoicd` check 4 for INVOIC 31009 MSB
    /// tariff validation. A Messstellenbetrieb price is not a Netzentgelt, so
    /// § 14a does not number these bands.
    ///
    /// Empty = no TOU bands configured (pure flat MSB tariff).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = Vec<Object>)]
    pub zeitvariable_preispositionen: Vec<ZeitvariablePreisposition>,
    /// Number of `zeitvariablePreispositionen` elements that failed schema validation.
    /// Non-zero means a stored element no longer decodes under the current
    /// schema — re-PUT the sheet to normalize it.
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
    Extension(Tenant(tenant)): Extension<Tenant>,
    claims: Claims,
    Path(msb_mp_id): Path<String>,
    Query(query): Query<PreisblattQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-preisblatt", &tenant)
        .is_err()
    {
        return ApiError::forbidden("access denied").into_response();
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

            // Deserialize `zeitvariablePreispositionen` as typed
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
                                 deserialised — re-PUT the PreisblattMessung to normalize"
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
        Ok(None) => ApiError::NotFound.into_response(),
        Err(e) => failure(e),
    }
}

/// Request body for `PUT /api/v1/preisblaetter-messung/{msb_mp_id}`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PreisblattMessungUpsertRequest {
    /// Full BO4E `PreisblattMessung` payload.
    ///
    /// `bo4e_version` is **not** an envelope field: it is provenance, and only
    /// the server knows which schema series it parsed the payload under. A
    /// caller-supplied one let a row be stamped with a version mako never read
    /// it as.
    pub data: serde_json::Value,
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
    Extension(Tenant(tenant)): Extension<Tenant>,
    claims: Claims,
    Path(msb_mp_id): Path<String>,
    Json(req): Json<PreisblattMessungUpsertRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-preisblatt", &tenant)
        .is_err()
    {
        return ApiError::forbidden("access denied").into_response();
    }

    // ── The BO4E gate ────────────────────────────────────────────────────────
    // Catches wrong `_typ`, values the type cannot hold, and any enum outside
    // the schema — without which a `preistyp` typo decodes to the `Unknown`
    // catch-all and `invoic-checker` later matches INVOIC positions against a
    // price sheet whose price type nothing can resolve.
    // `ZeitvariablePreisposition` elements live in the `_additional` extension
    // map of `PreisblattMessung`, which typed deserialization does not reach —
    // they are validated separately below.
    if let Err(e) = mako_markt::bo4e::decode::<PreisblattMessung>(req.data.clone()) {
        return super::bo4e_refusal(&e).into_response();
    }

    // ── Full ZeitvariablePreisposition validation ────────────────────────
    //
    // The basic `PreisblattMessung` schema check above validates the outer envelope.
    // Now extract `zeitvariablePreispositionen` from the raw JSON and apply
    // BDEW business rules that are NOT enforced by the BO4E schema:
    //
    // 1. Each element must deserialize as `ZeitvariablePreisposition`.
    // 2. `zaehlzeitregister` MUST be non-empty: an MSB with time-of-use pricing
    //    names each band code (e.g. "HT", "NT"). Without it, `invoic-checker`
    //    cannot match INVOIC 31009 positions against bands.
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
                return ApiError::unprocessable(format!(
                    "zeitvariablePreispositionen[{}]: 'bandNummer' is not a valid \
                             BO4E v202607 field — use 'zaehlzeitregister' instead \
                             (e.g. \"HT\", \"NT\", \"ST\")",
                    i
                ))
                .into_response();
            }

            // The BO4E gate. A nested COM need not stamp `_typ`; the gate
            // injects the one the enclosing PreisblattMessung already settled.
            let zvp = match mako_markt::bo4e::decode::<ZeitvariablePreisposition>(item.clone()) {
                Ok(z) => z,
                Err(e) => {
                    let mut detail = e.detail();
                    detail.insert(
                        "at".into(),
                        format!("zeitvariablePreispositionen[{i}]").into(),
                    );
                    return ApiError::unprocessable_with(
                        e.to_string(),
                        serde_json::Value::Object(detail),
                    )
                    .into_response();
                }
            };

            // `zaehlzeitregister` is mandatory: without it, `invoicd` /
            // `invoic-checker` cannot route INVOIC positions to bands.
            match zvp.zaehlzeitregister.as_deref() {
                None | Some("") => {
                    return ApiError::unprocessable(format!(
                        "zeitvariablePreispositionen[{}]: 'zaehlzeitregister' is \
                                 mandatory — set it to the band code (e.g. \"HT\", \"NT\")",
                        i
                    ))
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
        .upsert_messung(
            &msb_mp_id,
            data,
            &default_bo4e_version(),
            PreisblattSource::Api,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => failure(e),
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
/// Used by `netzbilanzd` for KA positions in INVOIC 31001/31002 (KAV §2).
pub async fn get_preisblatt_ka(
    Extension(repo): Extension<PreisblattKaRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    claims: Claims,
    Path(nb_mp_id): Path<String>,
    Query(query): Query<PreisblattKaQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-preisblatt", &tenant)
        .is_err()
    {
        return ApiError::forbidden("access denied").into_response();
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
        Ok(None) => ApiError::NotFound.into_response(),
        Err(e) => failure(e),
    }
}

/// Request body for `PUT /api/v1/preisblaetter-ka/{nb_mp_id}`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PreisblattKaUpsertRequest {
    /// The BO4E `PreisblattKonzessionsabgabe`, crossing
    /// [the gate](mako_markt::bo4e::decode) as the request deserialises.
    #[schema(value_type = Object)]
    pub data: Bo4e<PreisblattKonzessionsabgabe>,
    /// `"STROM"` or `"GAS"`. Defaults to `"STROM"`.
    #[serde(default = "default_sparte")]
    pub sparte: String,
    /// `"Tarifkunden"` | `"Sondervertragskunden"` | omit for both.
    pub kundengruppe_ka: Option<String>,
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
    Extension(Tenant(tenant)): Extension<Tenant>,
    claims: Claims,
    Path(nb_mp_id): Path<String>,
    Json(req): Json<PreisblattKaUpsertRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-preisblatt", &tenant)
        .is_err()
    {
        return ApiError::forbidden("access denied").into_response();
    }

    match repo
        .upsert_ka(
            &nb_mp_id,
            &req.sparte,
            req.kundengruppe_ka.as_deref(),
            match req.data.canonical_json() {
                Ok(v) => v,
                Err(e) => {
                    return failure(e);
                }
            },
            &mako_markt::bo4e::schema_version(),
            PreisblattSource::Api,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => failure(e),
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
#[serde(deny_unknown_fields)]
pub struct PreisblattDlUpsertRequest {
    /// The BO4E `PreisblattDienstleistung`, crossing
    /// [the gate](mako_markt::bo4e::decode) as the request deserialises.
    #[schema(value_type = Object)]
    pub data: Bo4e<PreisblattDienstleistung>,
}

pub async fn get_preisblatt_dienstleistung(
    Extension(repo): Extension<PreisblattDlRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    claims: Claims,
    Path(msb_mp_id): Path<String>,
    Query(query): Query<PreisblattQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-preisblatt", &tenant)
        .is_err()
    {
        return ApiError::forbidden("access denied").into_response();
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
        Ok(None) => ApiError::NotFound.into_response(),
        Err(e) => failure(e),
    }
}

pub async fn put_preisblatt_dienstleistung(
    Extension(repo): Extension<PreisblattDlRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    claims: Claims,
    Path(msb_mp_id): Path<String>,
    Json(req): Json<PreisblattDlUpsertRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-preisblatt", &tenant)
        .is_err()
    {
        return ApiError::forbidden("access denied").into_response();
    }
    match repo
        .upsert_dienstleistung(
            &msb_mp_id,
            match req.data.canonical_json() {
                Ok(v) => v,
                Err(e) => {
                    return failure(e);
                }
            },
            &mako_markt::bo4e::schema_version(),
            PreisblattSource::Api,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => failure(e),
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
#[serde(deny_unknown_fields)]
pub struct PreisblattHwUpsertRequest {
    /// The BO4E `PreisblattHardware`, crossing
    /// [the gate](mako_markt::bo4e::decode) as the request deserialises.
    #[schema(value_type = Object)]
    pub data: Bo4e<PreisblattHardware>,
}

pub async fn get_preisblatt_hardware(
    Extension(repo): Extension<PreisblattHwRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    claims: Claims,
    Path(msb_mp_id): Path<String>,
    Query(query): Query<PreisblattQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-preisblatt", &tenant)
        .is_err()
    {
        return ApiError::forbidden("access denied").into_response();
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
        Ok(None) => ApiError::NotFound.into_response(),
        Err(e) => failure(e),
    }
}

pub async fn put_preisblatt_hardware(
    Extension(repo): Extension<PreisblattHwRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    claims: Claims,
    Path(msb_mp_id): Path<String>,
    Json(req): Json<PreisblattHwUpsertRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-preisblatt", &tenant)
        .is_err()
    {
        return ApiError::forbidden("access denied").into_response();
    }
    match repo
        .upsert_hardware(
            &msb_mp_id,
            match req.data.canonical_json() {
                Ok(v) => v,
                Err(e) => {
                    return failure(e);
                }
            },
            &mako_markt::bo4e::schema_version(),
            PreisblattSource::Api,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => failure(e),
    }
}

#[cfg(test)]
mod tests {
    use super::validate_netznutzung_positionen;
    use serde_json::json;

    fn stufe(register: &str) -> serde_json::Value {
        json!({
            "zaehlzeitregister": register,
            "preisreferenz": "ENERGIEMENGE",
            "preis": { "wert": "8.35", "einheit": "CT", "bezugswert": "KWH" }
        })
    }

    fn sheet(zeitvariabel: Vec<serde_json::Value>) -> serde_json::Value {
        json!({ "_typ": "PREISBLATTNETZNUTZUNG", "zeitvariablePreispositionen": zeitvariabel })
    }

    fn refusal(data: &serde_json::Value) -> String {
        format!(
            "{:?}",
            validate_netznutzung_positionen(data).expect_err("must be refused")
        )
    }

    /// A § 14a Modul 3 sheet prices the three Tarifstufen HT, ST and NT
    /// (BK8-22/010-A Tenor 3. b)) and is stored.
    #[test]
    fn a_modul_3_sheet_with_ht_st_nt_is_accepted() {
        let data = sheet(vec![stufe("HT"), stufe("ST"), stufe("NT")]);
        assert!(validate_netznutzung_positionen(&data).is_ok());
    }

    /// Two Stufen leave the third band's energy with no price to bill at.
    #[test]
    fn a_modul_3_sheet_missing_a_tarifstufe_is_refused() {
        let msg = refusal(&sheet(vec![stufe("HT"), stufe("NT")]));
        assert!(msg.contains("missing ST"), "{msg}");
    }

    /// A register outside HT/ST/NT, or none, names no Modul 3 Tarifstufe.
    #[test]
    fn a_register_that_is_no_tarifstufe_is_refused() {
        for bad in [stufe("SuperHT"), json!({ "preisreferenz": "ENERGIEMENGE" })] {
            let msg = refusal(&sheet(vec![stufe("HT"), stufe("ST"), stufe("NT"), bad]));
            assert!(msg.contains("Tarifstufe"), "{msg}");
        }
    }

    /// `bandNummer` is no BO4E v202607 field.
    #[test]
    fn a_band_nummer_is_refused() {
        let mut ht = stufe("HT");
        ht["bandNummer"] = json!(1);
        let msg = refusal(&sheet(vec![ht, stufe("ST"), stufe("NT")]));
        assert!(msg.contains("bandNummer"), "{msg}");
    }

    /// A quantity-tiered position is priced by its Staffeln and is not a
    /// § 14a module, so `STAFFELN` is stored.
    #[test]
    fn a_staffel_priced_lastvariable_position_is_accepted() {
        let data = json!({
            "_typ": "PREISBLATTNETZNUTZUNG",
            "lastvariablePreispositionen": [{
                "tarifkalkulationsmethode": "STAFFELN",
                "preisreferenz": "ENERGIEMENGE",
                "preisBezugseinheit": "KWH"
            }]
        });
        assert!(validate_netznutzung_positionen(&data).is_ok());
    }

    /// `SPOTPREIS` is no `Tarifkalkulationsmethode`, so the BO4E gate
    /// refuses it, naming the element.
    #[test]
    fn a_spotpreis_method_is_refused_by_the_gate() {
        let data = json!({
            "_typ": "PREISBLATTNETZNUTZUNG",
            "lastvariablePreispositionen": [{ "tarifkalkulationsmethode": "SPOTPREIS" }]
        });
        let msg = refusal(&data);
        assert!(msg.contains("lastvariablePreispositionen[0]"), "{msg}");
    }
}

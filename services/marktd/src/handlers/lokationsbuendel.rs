//! `GET /api/v1/malos/{malo_id}/lokationsbuendel?stichtag=YYYY-MM-DD` — the
//! Lokationsbündel of a Marktlokation on a Stichtag, and
//! `PUT /api/v1/malos/{malo_id}/lokationsbuendel-angaben` — the NB's Angaben
//! of it no BO4E object carries.
//!
//! The NB answers a Geschäftsdatenanfrage Stammdaten Strom (ORDERS 17132) with
//! UTILMD 55035 / 55095 / 55060, whose columns (UTILMD AHB Strom 2.2 Kap. 9.5,
//! S. 599–665) are the Lokationsbündel: every object in `SG5 LOC`, the
//! Lokationsbündelstruktur in `SG8 SEQ+ZD5`, each object's position in
//! `SEQ+ZD6`, then each object's „Informative Daten". makod reads the bündel
//! here, at its boundary, beside the Marktlokation's UTILMD Stammdaten, rather
//! than taking it from whoever sends the command.
//!
//! ## Access control
//!
//! - `GET` — `read-utilmd-stammdaten`: the bündel is part of the same record;
//!   `GET /api/v1/lokationsbuendelstrukturen`, the Codeliste naming its code, too
//! - `PUT` — `write-netznutzungsdaten`: NB role only

use std::sync::Arc;

use axum::{
    Extension,
    extract::{Path, Query},
    http::StatusCode,
};
use mako_markt::domain::MaloId;
use mako_markt::lokationsbuendel::{
    BuendelAngaben, NnaPosition, ObisAngabe, Profilverfahren, RegisterAngabe, verwendungszweck,
};
use mako_service::cedar::CedarEnforcer;
use mako_service::{ApiError, ApiResult, Json};
use serde::Deserialize;
use tracing::info;

use crate::handlers::utilmd_stammdaten::StichtagQuery;
use crate::handlers::{Claims, Tenant, mdm_error};
use crate::pg::PgLokationsbuendelRepository;

/// Extension alias — concrete type so AFIT dispatches statically.
pub type LokationsbuendelRepoExt = Arc<PgLokationsbuendelRepository>;

/// Body of `PUT /api/v1/malos/{malo_id}/lokationsbuendel-angaben`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutAngabenBody {
    /// First day the row applies (`YYYY-MM-DD`).
    pub valid_from: String,
    /// First day it no longer applies (`YYYY-MM-DD`); absent = open.
    #[serde(default)]
    pub valid_to: Option<String>,
    /// MP-ID of the Übertragungsnetzbetreiber (`CAV+Z90`).
    #[serde(default)]
    pub uenb_mp_id: Option<String>,
    /// Technische Einrichtungen vorhanden (`CAV+ZH7`) or not (`ZH8`).
    #[serde(default)]
    pub technische_einrichtungen: Option<bool>,
    /// Code des normierten Profils.
    #[serde(default)]
    pub standardlastprofil: Option<String>,
    /// `SYNTHETISCH` or `ANALYTISCH`.
    #[serde(default)]
    pub profilverfahren: Option<Profilverfahren>,
    /// The Netznutzungsabrechnungspositionen.
    #[serde(default)]
    pub nna_positionen: Vec<NnaPosition>,
    /// The OBIS-Kennzahlen of the Marktlokation with their Verwendungszwecke.
    #[serde(default)]
    pub obis: Vec<ObisAngabe>,
    /// The Wertegranularität of each Zähler register.
    #[serde(default)]
    pub register: Vec<RegisterAngabe>,
}

fn malo_id(raw: &str) -> ApiResult<MaloId> {
    raw.parse()
        .map_err(|e| ApiError::bad_request(format!("invalid malo_id: {e}")))
}

/// `GET /api/v1/malos/{id}/lokationsbuendel?stichtag=YYYY-MM-DD`.
#[utoipa::path(
    get,
    path = "/api/v1/malos/{id}/lokationsbuendel",
    tag = "malo",
    operation_id = "get_lokationsbuendel",
    params(
        ("id" = String, Path, description = "11-digit Marktlokations-ID"),
        ("stichtag" = String, Query, description = "The date the bündel is resolved for (YYYY-MM-DD)"),
    ),
    responses(
        (status = 200, description = "The Lokationsbündel on the Stichtag", body = crate::reads::Lokationsbuendel),
        (status = 403, description = "Missing read-utilmd-stammdaten"),
        (status = 404, description = "`malo_not_found`"),
        (status = 422, description = "`invalid_malo_id` or an invalid `stichtag`"),
    )
)]
pub async fn get_lokationsbuendel(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<sqlx::PgPool>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(raw): Path<String>,
    Query(q): Query<StichtagQuery>,
) -> ApiResult<Json<crate::reads::Lokationsbuendel>> {
    enforcer
        .check(&claims.principal(), "read-utilmd-stammdaten", &tenant)
        .map_err(|e| ApiError::forbidden(e.to_string()))?;
    crate::reads::lokationsbuendel(&pool, &tenant, &raw, &q.stichtag)
        .await
        .map(Json)
}

/// `GET /api/v1/lokationsbuendelstrukturen` — every published
/// Lokationsbündelstruktur with its Bezeichnung, so a reader names a
/// `lokationsbuendelcode` without a copy of the Codeliste.
#[utoipa::path(
    get,
    path = "/api/v1/lokationsbuendelstrukturen",
    tag = "malo",
    operation_id = "list_lokationsbuendelstrukturen",
    responses(
        (status = 200, description = "The Codeliste der Lokationsbündelstrukturen", body = crate::reads::Lokationsbuendelstrukturen),
        (status = 403, description = "Missing read-utilmd-stammdaten"),
    )
)]
pub async fn list_lokationsbuendelstrukturen(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
) -> ApiResult<Json<crate::reads::Lokationsbuendelstrukturen>> {
    enforcer
        .check(&claims.principal(), "read-utilmd-stammdaten", &tenant)
        .map_err(|e| ApiError::forbidden(e.to_string()))?;
    Ok(Json(crate::reads::lokationsbuendelstrukturen()))
}

/// `PUT /api/v1/malos/{malo_id}/lokationsbuendel-angaben` — store one dated
/// row.
pub async fn put_lokationsbuendel_angaben(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<LokationsbuendelRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(raw): Path<String>,
    Json(body): Json<PutAngabenBody>,
) -> ApiResult<StatusCode> {
    enforcer
        .check(&claims.principal(), "write-netznutzungsdaten", &tenant)
        .map_err(|e| ApiError::forbidden(e.to_string()))?;
    let malo = malo_id(&raw)?;
    let valid_from = mako_service::dates::parse_iso_date("valid_from", &body.valid_from)?;
    let valid_to = body
        .valid_to
        .as_deref()
        .map(|s| mako_service::dates::parse_iso_date("valid_to", s))
        .transpose()?;
    pruefe(&body).map_err(ApiError::unprocessable)?;
    let row = BuendelAngaben {
        valid_from,
        valid_to,
        uenb_mp_id: body.uenb_mp_id,
        technische_einrichtungen: body.technische_einrichtungen,
        standardlastprofil: body.standardlastprofil,
        profilverfahren: body.profilverfahren,
        nna_positionen: body.nna_positionen,
        obis: body.obis,
        register: body.register,
    };
    info!(malo_id = %malo, %valid_from, "marktd: storing Lokationsbündel-Angaben");
    repo.upsert_angaben(&tenant, malo.as_ref(), &row)
        .await
        .map_err(mdm_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// What the schema's `CHECK`s cannot see inside the JSONB lists: a
/// Verwendungszweck the Codeliste der Verwendungszwecke 1.0 (Kap. 2, S. 5–7)
/// does not admit for a Marktlokation and that Marktrolle, a Zu-/Abschlag
/// with more than two decimals (UTILMD AHB Strom 2.2 `QTY+Z34`/`Z35`,
/// Bedingung `[930]`), and a list naming one OBIS-Kennzahl twice.
fn pruefe(body: &PutAngabenBody) -> Result<(), String> {
    for p in &body.nna_positionen {
        if p.artikel_id.trim().is_empty() {
            return Err("nna_positionen: artikel_id must not be empty".to_owned());
        }
        if p.zu_abschlag_prozent.scale() > 2 {
            return Err(format!(
                "nna_positionen[{}]: zu_abschlag_prozent {} has more than two decimals \
                 (QTY+Z34/Z35, Bedingung [930])",
                p.artikel_id, p.zu_abschlag_prozent
            ));
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for o in &body.obis {
        if !seen.insert(o.obis_kennzahl.as_str()) {
            return Err(format!("obis: {} is listed twice", o.obis_kennzahl));
        }
        for (rolle, code, zugelassen) in [
            (
                "nb",
                o.verwendungszweck_nb.as_deref(),
                verwendungszweck_nb as fn(&str) -> bool,
            ),
            ("lf", o.verwendungszweck_lf.as_deref(), verwendungszweck_lf),
            (
                "uenb",
                o.verwendungszweck_uenb.as_deref(),
                verwendungszweck_uenb,
            ),
        ] {
            if let Some(code) = code
                && !zugelassen(code)
            {
                return Err(format!(
                    "obis[{}].verwendungszweck_{rolle} {code:?} is no code of the Codeliste der \
                     Verwendungszwecke 1.0 admitted for a Marktlokation and that Marktrolle \
                     (Kap. 2, S. 5–7)",
                    o.obis_kennzahl
                ));
            }
        }
    }
    Ok(())
}

fn verwendungszweck_nb(code: &str) -> bool {
    verwendungszweck(code).is_some_and(|v| v.nb)
}

fn verwendungszweck_lf(code: &str) -> bool {
    verwendungszweck(code).is_some_and(|v| v.lf)
}

fn verwendungszweck_uenb(code: &str) -> bool {
    verwendungszweck(code).is_some_and(|v| v.uenb)
}

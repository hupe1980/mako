//! `GET /api/v1/malos/{malo_id}/utilmd-stammdaten?stichtag=YYYY-MM-DD` — the
//! UTILMD Stammdaten of a Marktlokation on a Stichtag, and
//! `PUT /api/v1/malos/{malo_id}/netznutzungsdaten` — the NB's facts of that
//! record no BO4E object carries.
//!
//! An Anmeldung EoG (UTILMD 55013 Strom, 44013 Gas) carries the Marktlokation's
//! record whole — GeLi Gas 3.0 Kap. 3.3.2: „Der Netzbetreiber teilt dem
//! Ersatz-/ Grundversorger unverzüglich sowohl den Beginn des
//! Zuordnungswechsels als auch, sofern ihm diese bekannt sind, Namen und
//! Adressen des Anschlussnehmers und des Anschlussnutzers, das Ende der
//! Zuordnung, sowie die Information, ob der an der Marktlokation versorgte
//! Letztverbraucher ein „Haushaltskunde“ ist, mit." makod reads the record
//! here, at its boundary, rather than taking it from whoever sends the command.
//!
//! The read assembles what marktd holds — the BO4E Marktlokation (Netzebene,
//! Gasqualität, Marktgebiet, Lokationsadresse, Endkunde, Zählwerke with their
//! Konzessionsabgabe), the Bilanzierung of the Stichtag, the Messlokationen the
//! Lokationszuordnung graph assigns, each with its MSB and its Zähler, and the
//! Netznutzungsdaten row of the Stichtag. A fact no row holds is `null`; makod
//! refuses a command whose message needs it, naming it.
//!
//! ## Access control
//!
//! - `GET` — `read-utilmd-stammdaten`: any authenticated caller of the same
//!   tenant, as for the Marktlokation itself
//! - `PUT` — `write-netznutzungsdaten`: NB role only

use std::sync::Arc;

use axum::{
    Extension,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::domain::MaloId;
use mako_markt::utilmd_stammdaten::{Abrechnungsvariante, Betriebszustand, Netznutzungsdaten};
use mako_service::cedar::CedarEnforcer;
use mako_service::{ApiError, ApiResult, Json};
use rust_decimal::Decimal;
use serde::Deserialize;
use tracing::info;

use crate::handlers::{Claims, Tenant, mdm_error};
use crate::pg::PgUtilmdStammdatenRepository;

/// Extension alias — concrete type so AFIT dispatches statically.
pub type UtilmdStammdatenRepoExt = Arc<PgUtilmdStammdatenRepository>;

/// `?stichtag=` of the read.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StichtagQuery {
    /// The date the record is resolved for (`YYYY-MM-DD`). Required: an
    /// Anmeldung EoG may be retroactive, so „today" is not a default.
    pub stichtag: String,
}

/// Body of `PUT /api/v1/malos/{malo_id}/netznutzungsdaten`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutNetznutzungsdatenBody {
    /// First day the row applies (`YYYY-MM-DD`).
    pub valid_from: String,
    /// First day it no longer applies (`YYYY-MM-DD`); absent = open.
    #[serde(default)]
    pub valid_to: Option<String>,
    /// Gruppenzuordnung nach EnWG — Haushaltskunde (§ 3 Nr. 22 EnWG).
    #[serde(default)]
    pub haushaltskunde: Option<bool>,
    /// `REGELBETRIEB` or `GESPERRT`.
    #[serde(default)]
    pub betriebszustand: Option<Betriebszustand>,
    /// Termin der Netznutzungsabrechnung, `MMDD`.
    #[serde(default)]
    pub abrechnungstermin: Option<String>,
    /// Jahr der nächsten Netznutzungsabrechnung, `CCYY`.
    #[serde(default)]
    pub naechste_abrechnung_jahr: Option<i32>,
    /// Netznutzungsabrechnungsintervall des NB in Monaten: 1, 3, 6 or 12.
    #[serde(default)]
    pub abrechnungsintervall_monate: Option<i16>,
    /// `ARBEITSPREIS_GRUNDPREIS` or `ARBEITSPREIS_LEISTUNGSPREIS`.
    #[serde(default)]
    pub abrechnungsvariante: Option<Abrechnungsvariante>,
    /// Gemeinderabatt in Prozent, 0 – 10 with at most two decimals.
    #[serde(default)]
    pub gemeinderabatt_prozent: Option<Decimal>,
}

fn malo_id(raw: &str) -> ApiResult<MaloId> {
    raw.parse()
        .map_err(|e| ApiError::bad_request(format!("invalid malo_id: {e}")))
}

/// `GET /api/v1/malos/{malo_id}/utilmd-stammdaten?stichtag=YYYY-MM-DD`.
pub async fn get_utilmd_stammdaten(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<UtilmdStammdatenRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(raw): Path<String>,
    Query(q): Query<StichtagQuery>,
) -> ApiResult<impl IntoResponse> {
    enforcer
        .check(&claims.principal(), "read-utilmd-stammdaten", &tenant)
        .map_err(|e| ApiError::forbidden(e.to_string()))?;
    let malo = malo_id(&raw)?;
    let stichtag = mako_service::dates::parse_iso_date("stichtag", &q.stichtag)?;
    match repo
        .find_at(&tenant, malo.as_ref(), stichtag)
        .await
        .map_err(mdm_error)?
    {
        Some(record) => Ok(Json(record)),
        None => Err(ApiError::not_found("no Marktlokation of this ID")
            .with_fields(serde_json::json!({ "malo_id": malo.as_ref() }))),
    }
}

/// `PUT /api/v1/malos/{malo_id}/netznutzungsdaten` — store one dated row.
pub async fn put_netznutzungsdaten(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<UtilmdStammdatenRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(raw): Path<String>,
    Json(body): Json<PutNetznutzungsdatenBody>,
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
    if let Some(p) = body.gemeinderabatt_prozent
        && p.scale() > 2
    {
        return Err(ApiError::unprocessable(format!(
            "gemeinderabatt_prozent {p} has more than two decimals (UTILMD QTY+Z16, Bedingung [930])"
        )));
    }
    let row = Netznutzungsdaten {
        valid_from,
        valid_to,
        haushaltskunde: body.haushaltskunde,
        betriebszustand: body.betriebszustand,
        abrechnungstermin: body.abrechnungstermin,
        naechste_abrechnung_jahr: body.naechste_abrechnung_jahr,
        abrechnungsintervall_monate: body.abrechnungsintervall_monate,
        abrechnungsvariante: body.abrechnungsvariante,
        gemeinderabatt_prozent: body.gemeinderabatt_prozent,
    };
    info!(malo_id = %malo, %valid_from, "marktd: storing Netznutzungsdaten");
    repo.upsert_netznutzung(&tenant, malo.as_ref(), &row)
        .await
        .map_err(mdm_error)?;
    Ok(StatusCode::NO_CONTENT)
}

//! The reads marktd serves through two doors: each is one function returning
//! one type, behind a documented REST `GET` and the MCP tool of the same name.
//!
//! The type derives both `utoipa::ToSchema` (the OpenAPI document) and
//! `schemars::JsonSchema` (the tool's `outputSchema`), so an integrator, the
//! Desk and an agent read one contract; `contract_tests` in `mcp_server` holds
//! the two derives to each other.
//!
//! | Read | REST | MCP tool | Cedar |
//! |---|---|---|---|
//! | [`versorgung`] | `GET /api/v1/versorgung/{malo_id}[?at=]` | `get_versorgung_at` | `read-versorgungsstatus` |
//! | [`versorgungsstatus_history`] | `GET /api/v1/versorgung/{malo_id}/history` | `get_versorgungsstatus_history` | `read-versorgungsstatus` |
//! | [`lokationsbuendel`] | `GET /api/v1/malos/{id}/lokationsbuendel?stichtag=` | `get_lokationsbuendel` | `read-utilmd-stammdaten` |
//! | [`lokationsbuendelstrukturen`] | `GET /api/v1/lokationsbuendelstrukturen` | — | `read-utilmd-stammdaten` |

use mako_markt::{
    domain::{Lokationstyp, MaloId},
    lokationsbuendel::{
        BuendelAngaben, BuendelObjekt, BuendelZaehler, BuendelZuordnung, MaloLokationsbuendel,
        NnaPosition, ObisAngabe, Profilverfahren, RegisterAngabe, Wertegranularitaet,
    },
    repository::{
        LfZuordnung, LieferStatus, VersorgungsStatusHistoryRecord, VersorgungsStatusRecord,
        ZuordnungsStatus,
    },
};
use mako_service::{ApiError, ApiResult};
use schemars::JsonSchema;
use serde::Serialize;
use sqlx::PgPool;
use time::format_description::well_known::Rfc3339;
use utoipa::ToSchema;

use crate::handlers::mdm_error;
use crate::pg::{PgLokationsbuendelRepository, PgVersorgungsStatusRepository};

// ── Versorgung ────────────────────────────────────────────────────────────────

/// The Lieferstatus of a Marktlokation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
pub enum Lieferstatus {
    Beliefert,
    Unbeliefert,
    Grundversorgung,
    Ersatzversorgung,
    Ruhend,
    Stillgelegt,
}

impl From<LieferStatus> for Lieferstatus {
    fn from(s: LieferStatus) -> Self {
        match s {
            LieferStatus::Beliefert => Self::Beliefert,
            LieferStatus::Unbeliefert => Self::Unbeliefert,
            LieferStatus::Grundversorgung => Self::Grundversorgung,
            LieferStatus::Ersatzversorgung => Self::Ersatzversorgung,
            LieferStatus::Ruhend => Self::Ruhend,
            LieferStatus::Stillgelegt => Self::Stillgelegt,
        }
    }
}

/// Whether a Zuordnung is announced (Anmeldung in flight) or running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
pub enum Zuordnungsstatus {
    Angekuendigt,
    Aktiv,
}

impl From<ZuordnungsStatus> for Zuordnungsstatus {
    fn from(s: ZuordnungsStatus) -> Self {
        match s {
            ZuordnungsStatus::Angekuendigt => Self::Angekuendigt,
            ZuordnungsStatus::Aktiv => Self::Aktiv,
        }
    }
}

/// One Lieferant's share of a Marktlokation.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Zuordnung {
    /// MP-ID of the Lieferant holding this share.
    pub lf_mp_id: String,
    /// The share in percent — `100` for an untranchierte Marktlokation.
    #[schemars(extend("format" = "decimal"))]
    #[schema(format = "decimal")]
    pub prozent: String,
    /// Tranchen-ID of a tranchierte Marktlokation.
    pub tranche_id: Option<String>,
    pub status: Zuordnungsstatus,
    /// Zuordnungsbeginn — the Lieferbeginn of this assignment.
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub zuordnungsbeginn: Option<String>,
    /// Zuordnungsende, once one is agreed.
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub zuordnungsende: Option<String>,
    /// The process that wrote this assignment.
    pub process_id: Option<String>,
}

impl From<LfZuordnung> for Zuordnung {
    fn from(z: LfZuordnung) -> Self {
        Self {
            lf_mp_id: z.lf_mp_id,
            prozent: z.prozent.to_string(),
            tranche_id: z.tranche_id,
            status: z.status.into(),
            zuordnungsbeginn: z.zuordnungsbeginn.map(|d| d.to_string()),
            zuordnungsende: z.zuordnungsende.map(|d| d.to_string()),
            process_id: z.process_id.map(|u| u.to_string()),
        }
    }
}

/// The VersorgungsStatus of a Marktlokation — now, or as it stood on a date.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct VersorgungsStatusResponse {
    pub malo_id: String,
    pub lieferstatus: Lieferstatus,
    /// Every Lieferant holding a share of this Marktlokation — the authoritative
    /// answer to „who supplies it". A tranchierte Marktlokation carries several.
    pub zuordnungen: Vec<Zuordnung>,
    /// The single active Lieferant, when there is exactly one.
    ///
    /// Derived from `zuordnungen` and **absent on a tranchierte
    /// Marktlokation**, where no single supplier exists — read `zuordnungen`
    /// there rather than treating the omission as „unsupplied".
    pub lf_mp_id: Option<String>,
    /// The single announced Lieferant, when exactly one Anmeldung is pending.
    /// Derived, with the same caveat as `lf_mp_id`.
    pub lf_mp_id_next: Option<String>,
    /// Lieferbeginn of the announced Lieferant.
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub lf_next_lieferbeginn: Option<String>,
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub lieferbeginn: Option<String>,
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub lieferende: Option<String>,
    pub msb_mp_id: Option<String>,
    pub nb_mp_id: String,
    /// Start of the running Ersatz-/Grundversorgung (§38/§36 EnWG), if any.
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub eog_seit: Option<String>,
    pub last_process_id: Option<String>,
    /// When this state was written (RFC 3339) — for a state read `at` a date,
    /// when that historic state became active.
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub updated_at: String,
    /// Optimistic-locking version, also the `ETag`.
    pub version: i64,
}

impl From<VersorgungsStatusRecord> for VersorgungsStatusResponse {
    fn from(r: VersorgungsStatusRecord) -> Self {
        Self {
            malo_id: r.malo_id.as_ref().to_owned(),
            lieferstatus: r.lieferstatus.into(),
            lf_mp_id: r.lf_mp_id().map(ToOwned::to_owned),
            lf_mp_id_next: r.lf_mp_id_next().map(ToOwned::to_owned),
            lf_next_lieferbeginn: r.lf_next_lieferbeginn().map(|d| d.to_string()),
            lieferbeginn: r.lieferbeginn().map(|d| d.to_string()),
            zuordnungen: r.zuordnungen.into_iter().map(Zuordnung::from).collect(),
            lieferende: r.lieferende.map(|d| d.to_string()),
            msb_mp_id: r.msb_mp_id,
            nb_mp_id: r.nb_mp_id,
            eog_seit: r.eog_seit.map(|d| d.to_string()),
            last_process_id: r.last_process_id.map(|u| u.to_string()),
            updated_at: rfc3339(r.updated_at),
            version: r.version,
        }
    }
}

/// One supply-state change of a Marktlokation.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct VersorgungsStatusHistoryResponse {
    pub id: i64,
    pub malo_id: String,
    pub lieferstatus: Lieferstatus,
    /// The assignment list as it stood at `valid_from`.
    pub zuordnungen: Vec<Zuordnung>,
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub lieferende: Option<String>,
    pub msb_mp_id: Option<String>,
    pub nb_mp_id: String,
    pub last_process_id: Option<String>,
    pub version: i64,
    /// The instant this state became active (RFC 3339).
    #[schemars(extend("format" = "date-time"))]
    #[schema(format = DateTime)]
    pub valid_from: String,
}

impl From<VersorgungsStatusHistoryRecord> for VersorgungsStatusHistoryResponse {
    fn from(r: VersorgungsStatusHistoryRecord) -> Self {
        Self {
            id: r.id,
            malo_id: r.malo_id.as_ref().to_owned(),
            lieferstatus: r.lieferstatus.into(),
            zuordnungen: r.zuordnungen.into_iter().map(Zuordnung::from).collect(),
            lieferende: r.lieferende.map(|d| d.to_string()),
            msb_mp_id: r.msb_mp_id,
            nb_mp_id: r.nb_mp_id,
            last_process_id: r.last_process_id.map(|u| u.to_string()),
            version: r.version,
            valid_from: rfc3339(r.valid_from),
        }
    }
}

/// One page of a Marktlokation's supply-state history, newest first.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct VersorgungsStatusHistorie {
    pub items: Vec<VersorgungsStatusHistoryResponse>,
    /// Every change recorded for the Marktlokation, not only this page's.
    pub total: u64,
    /// Zero-based page index.
    pub page: u32,
    /// Page size (at most 500).
    pub size: u32,
}

fn rfc3339(t: time::OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_else(|_| t.to_string())
}

fn malo(raw: &str) -> ApiResult<MaloId> {
    raw.parse::<MaloId>().map_err(|e| {
        ApiError::unprocessable(format!("invalid malo_id '{raw}': {e}"))
            .with_code("invalid_malo_id")
    })
}

fn iso_date(field: &str, raw: &str) -> ApiResult<time::Date> {
    Ok(mako_service::dates::parse_iso_date(field, raw)?)
}

/// The VersorgungsStatus of `malo_id` now, or — with `at` (`YYYY-MM-DD`,
/// German calendar day) — as it stood at the end of that day, from the
/// history log.
///
/// # Errors
///
/// `invalid_malo_id`, an invalid `at`, and `versorgung_not_found` when no
/// state is recorded (on or before `at`).
pub async fn versorgung(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    at: Option<&str>,
) -> ApiResult<VersorgungsStatusResponse> {
    let id = malo(malo_id)?;
    let repo = PgVersorgungsStatusRepository::new(pool.clone());
    let found = match at {
        Some(raw) => {
            let at = iso_date("at", raw)?;
            repo.find_at(&id, tenant, at).await
        }
        None => repo.find(&id, tenant).await,
    }
    .map_err(mdm_error)?;
    found.map(VersorgungsStatusResponse::from).ok_or_else(|| {
        ApiError::not_found(match at {
            Some(at) => format!("no Versorgungsstatus for MaLo {malo_id} on or before {at}"),
            None => format!("no Versorgungsstatus for MaLo {malo_id}"),
        })
        .with_code("versorgung_not_found")
    })
}

/// One page of `malo_id`'s supply-state changes, newest first.
///
/// # Errors
///
/// `invalid_malo_id`; a storage failure.
pub async fn versorgungsstatus_history(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    page: u32,
    size: u32,
) -> ApiResult<VersorgungsStatusHistorie> {
    let id = malo(malo_id)?;
    let page = PgVersorgungsStatusRepository::new(pool.clone())
        .find_history(&id, tenant, page, size)
        .await
        .map_err(mdm_error)?;
    Ok(VersorgungsStatusHistorie {
        items: page
            .items
            .into_iter()
            .map(VersorgungsStatusHistoryResponse::from)
            .collect(),
        total: page.total,
        page: page.page,
        size: page.size,
    })
}

// ── Lokationsbündel ───────────────────────────────────────────────────────────

/// The kind of object in a Lokationsbündel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Objekttyp {
    Malo,
    Melo,
    Nelo,
    Sr,
    Tr,
}

impl From<Lokationstyp> for Objekttyp {
    fn from(t: Lokationstyp) -> Self {
        match t {
            Lokationstyp::Malo => Self::Malo,
            Lokationstyp::Melo => Self::Melo,
            Lokationstyp::Nelo => Self::Nelo,
            Lokationstyp::Sr => Self::Sr,
            Lokationstyp::Tr => Self::Tr,
        }
    }
}

/// One object of the bündel.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Objekt {
    pub typ: Objekttyp,
    pub id: String,
    /// BO4E `lokationsbuendelObjektcode` — the object's position in the
    /// Lokationsbündelstruktur.
    pub objektcode: Option<String>,
}

/// One edge of the bündel valid on the Stichtag, half-open
/// `[valid_from, valid_to)`.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Kante {
    pub von_id: String,
    pub von_typ: Objekttyp,
    pub nach_id: String,
    pub nach_typ: Objekttyp,
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub valid_from: Option<String>,
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub valid_to: Option<String>,
}

/// A Zähler of one of the bündel's Messlokationen.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Zaehler {
    pub melo_id: String,
    pub zaehler_id: String,
    /// BO4E `Zaehler.zaehlertypSpezifikation` (`EDL40`, `EDL21`,
    /// `SONSTIGER_EHZ`, `MME_STANDARD`, `MME_MEDA`).
    pub zaehlertyp_spezifikation: Option<String>,
}

/// The Verfahren of a normiertes Profil.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verfahren {
    Synthetisch,
    Analytisch,
}

impl From<Profilverfahren> for Verfahren {
    fn from(p: Profilverfahren) -> Self {
        match p {
            Profilverfahren::Synthetisch => Self::Synthetisch,
            Profilverfahren::Analytisch => Self::Analytisch,
        }
    }
}

/// The Wertegranularität of a register.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Granularitaet {
    Jaehrlich,
    Halbjaehrlich,
    Quartalsweise,
    Monatlich,
}

impl From<Wertegranularitaet> for Granularitaet {
    fn from(w: Wertegranularitaet) -> Self {
        match w {
            Wertegranularitaet::Jaehrlich => Self::Jaehrlich,
            Wertegranularitaet::Halbjaehrlich => Self::Halbjaehrlich,
            Wertegranularitaet::Quartalsweise => Self::Quartalsweise,
            Wertegranularitaet::Monatlich => Self::Monatlich,
        }
    }
}

/// One Netznutzungsabrechnungsposition.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Nna {
    pub artikel_id: String,
    /// Zuschlag (> 0) or Abschlag (< 0) in Prozent.
    #[schemars(extend("format" = "decimal"))]
    #[schema(format = "decimal")]
    pub zu_abschlag_prozent: String,
}

/// One OBIS-Kennzahl of the Marktlokation with its Verwendungszwecke.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Obis {
    pub obis_kennzahl: String,
    pub verwendungszweck_nb: Option<String>,
    pub verwendungszweck_lf: Option<String>,
    pub verwendungszweck_uenb: Option<String>,
    pub wertegranularitaet: Option<Granularitaet>,
}

/// The Wertegranularität of one register of a Zähler.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Register {
    pub zaehler_id: String,
    pub obis_kennzahl: String,
    pub wertegranularitaet: Granularitaet,
}

/// The NB's Angaben of the bündel no BO4E object carries — one dated row.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Angaben {
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub valid_from: String,
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub valid_to: Option<String>,
    pub uenb_mp_id: Option<String>,
    pub technische_einrichtungen: Option<bool>,
    pub standardlastprofil: Option<String>,
    pub profilverfahren: Option<Verfahren>,
    pub nna_positionen: Vec<Nna>,
    pub obis: Vec<Obis>,
    pub register: Vec<Register>,
}

/// The Lokationsbündel of a Marktlokation on a Stichtag — the read model
/// behind the Geschäftsdatenanfrage answers 55035 / 55095 / 55060 (UTILMD AHB
/// Strom 2.2 Kap. 9.5). Its wire is [`MaloLokationsbuendel`]'s, which makod
/// reads with `deny_unknown_fields`, so it carries no field that type lacks;
/// the Lokationsbündelstruktur's wording is [`lokationsbuendelstrukturen`].
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Lokationsbuendel {
    pub malo_id: String,
    /// The date every edge and the Angaben row are resolved for.
    #[schemars(extend("format" = "date"))]
    #[schema(format = Date)]
    pub stichtag: String,
    /// BO4E `Marktlokation.energierichtung` — `AUSSP` verbrauchend, `EINSP`
    /// erzeugend.
    pub energierichtung: Option<String>,
    /// Every distinct `lokationsbuendelcode` the bündel's edges carry; more
    /// than one is a graph that disagrees with itself.
    pub lokationsbuendelcodes: Vec<String>,
    /// Every object reached, the Marktlokation itself first.
    pub objekte: Vec<Objekt>,
    /// Every edge valid on the Stichtag that reached them.
    pub zuordnungen: Vec<Kante>,
    /// The NB's Angaben row covering the Stichtag.
    pub angaben: Option<Angaben>,
    /// The Zähler of the bündel's Messlokationen.
    pub zaehler: Vec<Zaehler>,
}

/// A BO4E enum's wire value.
fn wire<T: Serialize>(value: &T) -> Option<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(ToOwned::to_owned))
}

impl From<MaloLokationsbuendel> for Lokationsbuendel {
    fn from(b: MaloLokationsbuendel) -> Self {
        Self {
            malo_id: b.malo_id,
            stichtag: b.stichtag.to_string(),
            energierichtung: b.energierichtung.as_ref().and_then(wire),
            lokationsbuendelcodes: b.lokationsbuendelcodes,
            objekte: b
                .objekte
                .into_iter()
                .map(
                    |BuendelObjekt {
                         typ,
                         id,
                         objektcode,
                     }| Objekt {
                        typ: typ.into(),
                        id,
                        objektcode,
                    },
                )
                .collect(),
            zuordnungen: b
                .zuordnungen
                .into_iter()
                .map(|z: BuendelZuordnung| Kante {
                    von_id: z.von_id,
                    von_typ: z.von_typ.into(),
                    nach_id: z.nach_id,
                    nach_typ: z.nach_typ.into(),
                    valid_from: z.valid_from.map(|d| d.to_string()),
                    valid_to: z.valid_to.map(|d| d.to_string()),
                })
                .collect(),
            angaben: b.angaben.map(Angaben::from),
            zaehler: b
                .zaehler
                .into_iter()
                .map(|z: BuendelZaehler| Zaehler {
                    melo_id: z.melo_id,
                    zaehler_id: z.zaehler_id,
                    zaehlertyp_spezifikation: z.zaehlertyp_spezifikation.as_ref().and_then(wire),
                })
                .collect(),
        }
    }
}

impl From<BuendelAngaben> for Angaben {
    fn from(a: BuendelAngaben) -> Self {
        Self {
            valid_from: a.valid_from.to_string(),
            valid_to: a.valid_to.map(|d| d.to_string()),
            uenb_mp_id: a.uenb_mp_id,
            technische_einrichtungen: a.technische_einrichtungen,
            standardlastprofil: a.standardlastprofil,
            profilverfahren: a.profilverfahren.map(Verfahren::from),
            nna_positionen: a
                .nna_positionen
                .into_iter()
                .map(
                    |NnaPosition {
                         artikel_id,
                         zu_abschlag_prozent,
                     }| Nna {
                        artikel_id,
                        zu_abschlag_prozent: zu_abschlag_prozent.to_string(),
                    },
                )
                .collect(),
            obis: a
                .obis
                .into_iter()
                .map(|o: ObisAngabe| Obis {
                    obis_kennzahl: o.obis_kennzahl,
                    verwendungszweck_nb: o.verwendungszweck_nb,
                    verwendungszweck_lf: o.verwendungszweck_lf,
                    verwendungszweck_uenb: o.verwendungszweck_uenb,
                    wertegranularitaet: o.wertegranularitaet.map(Granularitaet::from),
                })
                .collect(),
            register: a
                .register
                .into_iter()
                .map(|r: RegisterAngabe| Register {
                    zaehler_id: r.zaehler_id,
                    obis_kennzahl: r.obis_kennzahl,
                    wertegranularitaet: r.wertegranularitaet.into(),
                })
                .collect(),
        }
    }
}

/// The Lokationsbündel of `malo_id` on `stichtag` (`YYYY-MM-DD`).
///
/// # Errors
///
/// `invalid_malo_id`, an invalid `stichtag`, `malo_not_found`.
pub async fn lokationsbuendel(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    stichtag: &str,
) -> ApiResult<Lokationsbuendel> {
    let id = malo(malo_id)?;
    let stichtag = iso_date("stichtag", stichtag)?;
    PgLokationsbuendelRepository::new(pool.clone())
        .find_at(tenant, id.as_ref(), stichtag)
        .await
        .map_err(mdm_error)?
        .map(Lokationsbuendel::from)
        .ok_or_else(|| {
            ApiError::not_found("no Marktlokation of this ID")
                .with_fields(serde_json::json!({ "code": "malo_not_found", "malo_id": malo_id }))
        })
}

/// One published Lokationsbündelstruktur.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Lokationsbuendelstruktur {
    /// The `lokationsbuendelcode`.
    pub code: String,
    /// Its Bezeichnung in the Codeliste der Lokationsbündelstrukturen.
    pub bezeichnung: String,
}

/// The Codeliste der Lokationsbündelstrukturen (EDI@Energy, BDEW V1.0,
/// 31.03.2023), as `rubo4e::lokationsbuendel::STRUKTUREN` carries it.
#[derive(Debug, Clone, Serialize, JsonSchema, ToSchema)]
pub struct Lokationsbuendelstrukturen {
    pub strukturen: Vec<Lokationsbuendelstruktur>,
}

/// Every published Lokationsbündelstruktur with its Bezeichnung — what a
/// `lokationsbuendelcode` is called.
#[must_use]
pub fn lokationsbuendelstrukturen() -> Lokationsbuendelstrukturen {
    Lokationsbuendelstrukturen {
        strukturen: rubo4e::lokationsbuendel::STRUKTUREN
            .iter()
            .map(|s| Lokationsbuendelstruktur {
                code: s.code.to_owned(),
                bezeichnung: s.bezeichnung.to_owned(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// makod reads `GET …/lokationsbuendel` as a [`MaloLokationsbuendel`]
    /// with `deny_unknown_fields`, so the shared read's type must serialize
    /// to exactly that wire: a field the view adds, renames or re-encodes is
    /// a Geschäftsdatenanfrage makod can no longer answer.
    #[test]
    fn the_lokationsbuendel_view_is_the_wire_makod_reads() {
        let buendel: MaloLokationsbuendel = serde_json::from_value(serde_json::json!({
            "malo_id": "51238696012",
            "stichtag": "2026-11-01",
            "energierichtung": "AUSSP",
            "lokationsbuendelcodes": ["9992000000026"],
            "objekte": [
                { "typ": "MALO", "id": "51238696012", "objektcode": "9992000001016" },
                { "typ": "MELO", "id": "DE00056266802AO6G56M11SN51G21M24S", "objektcode": null }
            ],
            "zuordnungen": [{
                "von_id": "DE00056266802AO6G56M11SN51G21M24S", "von_typ": "MELO",
                "nach_id": "51238696012", "nach_typ": "MALO",
                "valid_from": "2026-01-01", "valid_to": null
            }],
            "angaben": {
                "valid_from": "2026-01-01", "valid_to": null,
                "uenb_mp_id": "9900000000002", "technische_einrichtungen": false,
                "standardlastprofil": "H0", "profilverfahren": "SYNTHETISCH",
                "nna_positionen": [{ "artikel_id": "9990001000053", "zu_abschlag_prozent": "-2.5" }],
                "obis": [{ "obis_kennzahl": "1-0:1.8.0", "verwendungszweck_nb": "Z84",
                           "verwendungszweck_lf": null, "verwendungszweck_uenb": null,
                           "wertegranularitaet": "JAEHRLICH" }],
                "register": [{ "zaehler_id": "Z1", "obis_kennzahl": "1-0:1.8.0",
                               "wertegranularitaet": "MONATLICH" }]
            },
            "zaehler": [{ "melo_id": "DE00056266802AO6G56M11SN51G21M24S", "zaehler_id": "Z1",
                          "zaehlertyp_spezifikation": "MME_STANDARD" }]
        }))
        .expect("a stored bündel");
        let view = serde_json::to_value(Lokationsbuendel::from(buendel.clone())).expect("view");
        let read_back: MaloLokationsbuendel =
            serde_json::from_value(view).expect("makod reads the view");
        assert_eq!(read_back, buendel);
    }

    /// Every published structure is named, `9992000000026` as the Codeliste
    /// names it.
    #[test]
    fn a_lokationsbuendelcode_is_named_from_the_codeliste() {
        let k = lokationsbuendelstrukturen();
        assert!(!k.strukturen.is_empty());
        let standard = k
            .strukturen
            .iter()
            .find(|s| s.code == "9992000000026")
            .expect("9992000000026 is published");
        assert_eq!(
            standard.bezeichnung,
            "Verbrauch mit einer Messlokation (Standard)"
        );
    }
}

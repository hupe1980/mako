#![allow(clippy::doc_markdown)]
//! Repository traits for all `marktd` aggregate types.
//!
//! Every trait has exactly two implementations:
//! - Production: `Pg*Repository` in `services/marktd/src/pg/`
//! - Testing: `InMemory*` in `crates/mako-markt/src/testing.rs` (feature = "testing")
//!
//! All methods are `async` (AFIT, stable since Rust 1.75).
//! All methods return `Result<_, MdmError>` annotated `#[must_use]`.

use serde::{Deserialize, Serialize};
use time::Date;
use uuid::Uuid;

use std::future::Future;

use crate::{
    domain::{Lokationstyp, MaloId, MarktpartnerId, MeloId, ProcessStatus, Sparte},
    error::MdmError,
};

// ── Serde default helpers ─────────────────────────────────────────────────────

/// Default value for `updated_at` serde fields: UNIX epoch (1970-01-01T00:00:00Z).
/// Used when a PUT request body omits the field (server overwrites it on upsert).
fn unix_epoch() -> time::OffsetDateTime {
    time::OffsetDateTime::UNIX_EPOCH
}

// ── Date serde helpers (ISO 8601 "YYYY-MM-DD" ↔ time::Date) ─────────────────
mod date_iso {
    use serde::{Deserialize, Deserializer, Serializer};
    use time::Date;
    use time::macros::format_description;

    #[expect(clippy::trivially_copy_pass_by_ref)]
    pub fn serialize<S: Serializer>(date: &Date, s: S) -> Result<S::Ok, S::Error> {
        let fmt = format_description!("[year]-[month]-[day]");
        s.serialize_str(&date.format(fmt).map_err(serde::ser::Error::custom)?)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Date, D::Error> {
        let raw = String::deserialize(d)?;
        let fmt = format_description!("[year]-[month]-[day]");
        Date::parse(&raw, fmt).map_err(serde::de::Error::custom)
    }

    pub mod opt {
        use serde::{Deserialize, Deserializer, Serializer};
        use time::Date;
        use time::macros::format_description;

        #[expect(clippy::trivially_copy_pass_by_ref, clippy::ref_option)]
        pub fn serialize<S: Serializer>(date: &Option<Date>, s: S) -> Result<S::Ok, S::Error> {
            match date {
                Some(d) => {
                    let fmt = format_description!("[year]-[month]-[day]");
                    s.serialize_some(&d.format(fmt).map_err(serde::ser::Error::custom)?)
                }
                None => s.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Date>, D::Error> {
            let raw: Option<String> = Option::deserialize(d)?;
            match raw {
                Some(s) => {
                    let fmt = format_description!("[year]-[month]-[day]");
                    Date::parse(&s, fmt)
                        .map(Some)
                        .map_err(serde::de::Error::custom)
                }
                None => Ok(None),
            }
        }
    }
}

// ── Type aliases ──────────────────────────────────────────────────────────────

/// Full BO4E `MARKTLOKATION` payload (stored as JSONB; returned as-is to callers).
pub type MaloPayload = serde_json::Value;
/// Full BO4E `MESSLOKATION` payload.
pub type MeloPayload = serde_json::Value;

/// Default BO4E schema version for `#[serde(default = ...)]` on record structs.
///
/// Derived from the linked `rubo4e` — see [`crate::bo4e::SCHEMA_VERSION`] for
/// why the value is asked for rather than written down.
fn default_bo4e_version() -> String {
    crate::bo4e::schema_version()
}

// ── MaLo ─────────────────────────────────────────────────────────────────────

/// Point-in-time market-role assignment (`rollenzuordnung`) for a `MARKTLOKATION`:
/// which Marktpartner (NB, LF, MSB, …) holds which role, and for which validity
/// period.
///
/// The `malo_id` is implicit (always the parent `MaloRecord.malo_id`) and
/// is therefore not repeated here.
///
/// **Not** the BO4E `Lokationszuordnung` business object — that BO models the
/// MaLo/MeLo/NeLo/TR/SR location-bundle *graph* and lives in
/// [`LokationszuordnungEdge`] (table `lokationszuordnungen`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rollenzuordnung {
    pub zuordnungstyp: String,
    pub rollencodenummer: String,
    #[serde(with = "date_iso")]
    pub valid_from: Date,
    #[serde(default, with = "date_iso::opt")]
    pub valid_to: Option<Date>,
}

/// Stored MaLo record as returned by repository reads.
///
/// `rollenzuordnung` contains only the role assignments valid at the
/// `at` date passed to `MaloRepository::find` / `MaloRepository::list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaloRecord {
    pub malo_id: MaloId,
    pub sparte: Sparte,
    /// Voltage/pressure level — BO4E `Netzebene` wire value (`NSP`/`MSP`/`HSP`/`HSS`
    /// and their `*_UMSP` transformation levels for Strom; `HD`/`MD`/`ND` for Gas).
    /// `None` when the incoming BO4E payload did not carry the field.
    pub netzebene: Option<String>,
    /// Bilanzierungsgebiet EIC code (`LOC+237` in UTILMD) extracted from `Marktlokation`.
    /// Used by `processd` NB check 4 as fallback when `malo_grid` is not populated.
    pub bilanzierungsgebiet: Option<String>,
    /// Gas quality extracted from `Marktlokation.standorteigenschaften.gasqualitaet`.
    ///
    /// BO4E `Gasqualitaet` wire value: `"H_GAS"` | `"L_GAS"`. Those are the
    /// only two the schema defines, so those are the only two the API accepts;
    /// see `mako_geli_gas::gas_quality` for why no speculative H2 spelling is
    /// written here (that crate is not a dependency of this one, so the
    /// reference is deliberately not an intra-doc link).
    ///
    /// Used for:
    /// - Gas tariff routing in `billingd` (Brennwert/Zustandszahl defaults differ by quality)
    /// - Invoice audit annotation (`ZusatzAttribut.gasqualitaet` per § 147 AO / GoBD)
    pub gasqualitaet: Option<String>,
    /// BO4E `Energierichtung` wire value, named from the **grid's** point of
    /// view: `EINSP` (Einspeisung) is a *generating* location that feeds the
    /// grid, `AUSSP` (Ausspeisung) a *consuming* one that draws from it.
    pub energierichtung: Option<String>,
    /// Billing mode extracted from `Marktlokation.bilanzierungsmethode`.
    ///
    /// Values: `RLM` | `SLP` | `TLP_GEMEINSAM` | `TLP_GETRENNT` | `PAUSCHAL` | `IMS`.
    /// `RLM` → `netzbilanzd` must include Leistungspreis position (`spitzenleistung_kw` required).
    /// `SLP` → Arbeitspreis only; no `spitzenleistung_kw`.
    pub bilanzierungsmethode: Option<String>,
    /// Regelzone EIC code extracted from `Marktlokation.regelzone`.
    ///
    /// Maps the MaLo to an ÜNB (Transmission System Operator) for:
    /// - MABIS IFTSTA 21000 routing (Bilanzkreisabrechnung Strom, BKV↔ÜNB)
    /// - Redispatch 2.0 `Stammdaten` forwarding (VNB → ÜNB)
    pub regelzone: Option<String>,
    /// Gas GaBi RLM Fallgruppe — **denormalised current-value derived from the
    /// BO4E [`BilanzierungRecord`] resource** (`fallgruppenzuordnung`), which is
    /// the authoritative, temporal home. Unlike `bilanzierungsmethode`/
    /// `bilanzierungsgebiet` (genuine `Marktlokation` fields, BO #12), the GaBi
    /// Fallgruppe is a `Bilanzierung` field (BO #3, absent from `Marktlokation`);
    /// writing a Bilanzierung syncs this column.
    ///
    /// Values: `"GABI_RLM_MIT_TAGESBAND"` | `"GABI_RLM_OHNE_TAGESBAND"` |
    /// `"GABI_RLM_IM_NOMINIERUNGSERSATZVERFAHREN"`.
    ///
    /// Determines the GaBi billing category for Gas RLM MaLos.
    /// Required for `netzbilanzd` Gas MMM settlement routing.
    pub fallgruppe: Option<String>,
    /// Lokationsbündel object code extracted from
    /// `Marktlokation.lokationsbuendelObjektcode` — groups the locations that
    /// are bundled for market communication (UTILMD Lokationsbündelstruktur).
    #[serde(default)]
    pub lokationsbuendel_objektcode: Option<String>,
    /// §14a EnWG „Status der Fernsteuerbarkeit" of the Marktlokation.
    ///
    /// Extracted from UTILMD SG10 `CCI+7037`: `Z97` (technisch fernsteuerbar) →
    /// `true`, `Z96` (technisch nicht fernsteuerbar) → `false`. `None` when the
    /// message did not carry the characteristic. Relevant for the §14a EnWG
    /// netzorientierte Steuerung of controllable consumption devices.
    #[serde(default)]
    pub fernsteuerbar: Option<bool>,
    pub version: i64,
    pub data: MaloPayload,
    /// Role assignments valid at the requested reference date.
    pub rollenzuordnung: Vec<Rollenzuordnung>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
    /// BO4E schema version of the `data` payload (e.g. `"202607.1.0"`).
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
}

/// Stored MeLo record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeloRecord {
    pub melo_id: MeloId,
    pub malo_id: Option<MaloId>,
    /// Voltage/pressure level at the metering point, extracted from `Messlokation.netzebene_messung`.
    pub netzebene_messung: Option<String>,
    /// Regelzone EIC code extracted from
    /// `Messlokation.standorteigenschaften.eigenschaftenStrom[0].regelzone`.
    ///
    /// Maps this MeLo to the \u00dcNB (Transmission System Operator) for:
    /// - Redispatch 2.0 `Stammdaten` forwarding (VNB \u2192 \u00dcNB)
    /// - MABIS IFTSTA 21000 routing (Bilanzkreisabrechnung Strom, BKV\u2194\u00dcNB)
    pub regelzone: Option<String>,
    /// Full BO4E `Standorteigenschaften` payload as JSONB.
    ///
    /// Contains `StandorteigenschaftenStrom` (regelzone, bilanzierungsgebietEic)
    /// and `StandorteigenschaftenGas` (druckstufe). Required for:
    /// - Redispatch 2.0 `NetworkConstraintDocument` cross-references
    /// - Gas billing zone assignment (`druckstufe`) for GeLi Gas MMM
    /// - `mako-pruefung` check 5 (Bilanzierungszone at MeLo level)
    pub standorteigenschaften: Option<serde_json::Value>,
    /// Lokationsbündel object code extracted from
    /// `Messlokation.lokationsbuendelObjektcode` (UTILMD Lokationsbündelstruktur).
    #[serde(default)]
    pub lokationsbuendel_objektcode: Option<String>,
    pub version: i64,
    pub data: MeloPayload,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
    /// BO4E schema version of the `data` payload.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
}

/// Stored webhook subscription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub subscriber_id: String,
    pub webhook_url: String,
    /// Stored encrypted at rest by the repository implementation.
    pub webhook_secret: Option<String>,
    /// Empty = all roles.
    pub roles: Vec<String>,
    /// Empty = all event types.
    pub event_types: Vec<String>,
    /// Empty = all Sparten.
    pub sparten: Vec<String>,
    pub active: bool,
    pub version: i64,
}

/// Stored trading-partner record.
///
/// `gln` holds the 13-digit `MarktpartnerId` (Rollencodenummer).  The field
/// name `gln` is kept for backward-compatibility with the PostgreSQL column
/// name and existing EDIFACT serialization; semantically this value is a
/// Marktpartner-ID, which may be a BDEW-Codenummer, DVGW-Codenummer, or a
/// GS1 GLN — use [`crate::domain::nad_agency_code`] to determine the coding
/// authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartnerRecord {
    /// 13-digit Marktpartner-ID.
    pub mp_id: MarktpartnerId,
    pub display_name: Option<String>,
    /// BO4E market role (serialises as the BDEW code, e.g. `"LF"`, `"NB"`, `"MSB"`).
    pub marktrolle: Option<rubo4e::current::Marktrolle>,
    pub sparte: Option<Sparte>,
    /// Coding authority: `BDEW` | `DVGW` | `GLN` (BO4E `Rollencodetyp`).
    /// Derived from the MP-ID prefix; stored for fast AS4 routing lookups.
    pub rollencodetyp: Option<rubo4e::current::Rollencodetyp>,
    /// AS4 endpoint URL list from `Marktteilnehmer.makoadresse`.
    /// Used by `makod` for dynamic AS4 destination routing.
    pub makoadresse: Vec<String>,
    /// Raw JSON for additional channel details (certificate, etc.)
    pub channels: serde_json::Value,
    /// Optimistic-concurrency version.
    #[serde(default)]
    pub version: i64,
    #[serde(default = "unix_epoch", with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

impl PartnerRecord {
    /// Present the stored partner as a BO4E `Marktteilnehmer`.
    ///
    /// Field mapping:
    /// - `mp_id` → `rollencodenummer`
    /// - `marktrolle` / `rollencodetyp` → carried as-is (already typed)
    /// - `sparte` → BO4E `Sparte`
    /// - `makoadresse` → `makoadresse` (omitted when empty)
    /// - `display_name` → `geschaeftspartner.organisationsname`
    #[must_use]
    pub fn to_marktteilnehmer(&self) -> rubo4e::current::Marktteilnehmer {
        let geschaeftspartner = self.display_name.as_ref().map(|name| {
            Box::new(rubo4e::current::Geschaeftspartner {
                organisationsname: Some(name.clone()),
                ..Default::default()
            })
        });
        rubo4e::current::Marktteilnehmer {
            rollencodenummer: Some(self.mp_id.clone()),
            marktrolle: self.marktrolle,
            rollencodetyp: self.rollencodetyp,
            sparte: self.sparte.map(|s| match s {
                Sparte::Strom => rubo4e::current::Sparte::Strom,
                Sparte::Gas => rubo4e::current::Sparte::Gas,
            }),
            makoadresse: (!self.makoadresse.is_empty()).then(|| self.makoadresse.clone()),
            geschaeftspartner,
            ..Default::default()
        }
    }
}

/// Process correlation entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelationEntry {
    pub process_id: Uuid,
    pub workflow_name: Option<String>,
    pub pid: Option<i32>,
    pub malo_id: Option<MaloId>,
    pub melo_id: Option<MeloId>,
    pub contract_id: Option<String>,
    pub erp_contract_id: Option<String>,
    pub erp_order_id: Option<String>,
    pub edifact_conv_id: Option<Uuid>,
    pub marktrolle: Option<String>,
    pub format_version: Option<String>,
    pub status: ProcessStatus,
    #[serde(with = "time::serde::rfc3339")]
    pub initiated_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub completed_at: Option<time::OffsetDateTime>,
}

// ── Pagination ────────────────────────────────────────────────────────────────

/// A paged collection returned by list operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageResult<T> {
    pub items: Vec<T>,
    /// Total matching rows (without pagination).
    pub total: u64,
    /// Zero-based page index.
    pub page: u32,
    /// Page size requested.
    pub size: u32,
}

// ── Query filters ─────────────────────────────────────────────────────────────

/// Filters for `GET /api/v1/malos` listing.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MaloFilter {
    pub sparte: Option<Sparte>,
    /// Filter by `zuordnungstyp` in active `rollenzuordnung` (e.g. `"NB"`, `"LF"`).
    pub zuordnungstyp: Option<String>,
    /// Filter by `rollencodenummer` (GLN) in active `rollenzuordnung`.
    pub rollencodenummer: Option<String>,
    /// Filter by Gas GaBi RLM Fallgruppe (e.g. `"GABI_RLM_MIT_TAGESBAND"`).
    /// Applies to Gas MaLos only; Strom MaLos have no Fallgruppe.
    pub fallgruppe: Option<String>,
    /// Filter by `bilanzierungsmethode` (e.g. `"RLM"`, `"SLP"`, `"IMS"`).
    pub bilanzierungsmethode: Option<String>,
    /// Filter by `regelzone` EIC code (e.g. `"10YDE-EON------1"`).
    /// Maps to the controlling ÜNB for MABIS IFTSTA and Redispatch 2.0.
    pub regelzone: Option<String>,
    pub page: u32,
    pub size: u32,
}

/// Filters for `GET /api/v1/correlations`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CorrelationFilter {
    pub erp_order_id: Option<String>,
    pub malo_id: Option<MaloId>,
    pub status: Option<ProcessStatus>,
}

// ── Traits ────────────────────────────────────────────────────────────────────

/// Read/write access to `MARKTLOKATION` records.
#[allow(async_fn_in_trait)]
/// Lightweight read model returned by `MarktdClient::get_malo`.
///
/// Contains only the typed fields extracted from the `Marktlokation` JSONB — not
/// the full payload. Used by `processd` NB check 4 (Bilanzierungsgebiet) as the
/// primary source before falling back to the `malo_grid` side table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct MaloTypedFields {
    pub malo_id: String,
    /// Voltage/pressure level (e.g. `"NS"`, `"MS"`, `"HS"`).
    pub netzebene: Option<String>,
    /// Bilanzierungsgebiet EIC code — primary input for `processd` NB check 4.
    pub bilanzierungsgebiet: Option<String>,
    /// BO4E `Gasqualitaet` wire value: `"H_GAS"` | `"L_GAS"`.
    pub gasqualitaet: Option<String>,
    /// BO4E `Energierichtung` wire value — `EINSP` feeds the grid (generation),
    /// `AUSSP` draws from it (consumption).
    pub energierichtung: Option<String>,
    /// Billing mode — `"SLP"` | `"RLM"` | `"IMS"`.
    ///
    /// Derived from UTILMD `TM+EM` at supply-start and updated by `marktd`
    /// `patch_typenmerkmal()`.  Drives `netzbilanzd` MMM SLP variant selection
    /// (H0/G0/L0) and `processd` NB billing-mode check.
    pub bilanzierungsmethode: Option<String>,
    /// Gas GaBi RLM Fallgruppe.
    pub fallgruppe: Option<String>,
    /// Regelzone EIC code — maps MeLo to ÜNB for Redispatch 2.0 Stammdaten routing.
    pub regelzone: Option<String>,
}

/// A UTILMD Stammdatenänderung applied to the typed MaLo columns.
///
/// Each `Some` field is a new authoritative value from a GPKE Teil 4 / GeLi Gas
/// "Änderung Daten der MaLo" message; each `None` leaves the column untouched.
/// Populated by the `makod` adapter from the UTILMD SG8 `SEQ`/SG10 `CCI`/`CAV`
/// and `TM` segments and carried into the `de.mako.process.completed` payload
/// that `marktd` applies via [`MaloRepository::patch_stammdaten`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MaloStammdatenPatch {
    /// `netzebene` — BO4E `Netzebene` wire value (`NSP`/`MSP`/`HSP`/`HSS`,
    /// their `*_UMSP` transformation levels, or `HD`/`MD`/`ND` for Gas).
    pub netzebene: Option<String>,
    /// `bilanzierungsgebiet` EIC.
    pub bilanzierungsgebiet: Option<String>,
    /// Gas quality — BO4E `Gasqualitaet` wire value (`H_GAS`/`L_GAS`).
    pub gasqualitaet: Option<String>,
    /// `energierichtung` — BO4E `Energierichtung` wire value, from
    /// `CCI+Z30++Z06` (Erzeugung → `EINSP`) / `Z07` (Verbrauch → `AUSSP`).
    pub energierichtung: Option<String>,
    /// Bilanzierungsmethode (`RLM`/`SLP`/`IMS`/`TLP_*`).
    pub bilanzierungsmethode: Option<String>,
    /// Regelzone EIC (ÜNB assignment).
    pub regelzone: Option<String>,
    /// GaBi RLM Fallgruppe.
    pub fallgruppe: Option<String>,
    /// §14a EnWG „Status der Fernsteuerbarkeit" (`CCI+7037` `Z97`→`true` /
    /// `Z96`→`false`).
    pub fernsteuerbar: Option<bool>,
}

impl MaloStammdatenPatch {
    /// `true` when no column would change.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.netzebene.is_none()
            && self.bilanzierungsgebiet.is_none()
            && self.gasqualitaet.is_none()
            && self.energierichtung.is_none()
            && self.bilanzierungsmethode.is_none()
            && self.regelzone.is_none()
            && self.fallgruppe.is_none()
            && self.fernsteuerbar.is_none()
    }
}

/// Read/write access to `MARKTLOKATION` records.
#[allow(async_fn_in_trait)]
pub trait MaloRepository: Send + Sync {
    /// Insert or update a `MARKTLOKATION`.
    ///
    /// Validates optimistic concurrency via `if_match` (the caller's ETag).
    /// Pass `None` for unconditional upsert (first write).
    ///
    /// Returns the new version number.
    /// `data` is the **typed** BO — not a `serde_json::Value`. The repository
    /// serialises the JSONB and derives the shadow columns from it
    /// ([`MaloShadowColumns`](crate::bo4e::MaloShadowColumns)), so a payload
    /// that has not been through BO4E validation cannot reach storage and a
    /// column cannot disagree with the document it shadows.
    async fn upsert(
        &self,
        malo_id: &MaloId,
        sparte: Sparte,
        data: &rubo4e::current::Marktlokation,
        rollenzuordnung: Vec<Rollenzuordnung>,
        if_match: Option<i64>,
        bo4e_version: &str,
    ) -> Result<i64, MdmError>;

    /// Patch the `bilanzierungsmethode` and/or `fallgruppe` typed columns on an
    /// existing MaLo row **without** touching the JSONB payload or version.
    ///
    /// Called by `marktd` event_ingest when it receives
    /// `de.mako.process.initiated` (PID 55001/44001) carrying
    /// `bilanzierungsmethode` and/or `fallgruppe` extracted from the UTILMD
    /// `TM+EM` / `TM+Z10` segments by the `makod` adapter (L1/N1).
    ///
    /// No-ops silently when the MaLo row does not yet exist — the values will
    /// be set on the first `PUT /api/v1/malos` call instead.
    async fn patch_typenmerkmal(
        &self,
        malo_id: &MaloId,
        bilanzierungsmethode: Option<&str>,
        fallgruppe: Option<&str>,
    ) -> Result<(), MdmError>;

    /// Apply a UTILMD Stammdatenänderung (GPKE Teil 4 / GeLi Gas) to the typed
    /// MaLo columns — the granular counterpart of
    /// [`patch_typenmerkmal`](Self::patch_typenmerkmal) over the full
    /// changeable attribute set.
    ///
    /// Each `Some` field overwrites its column; each `None` leaves it unchanged
    /// (`COALESCE`). The JSONB payload and the optimistic `version` are **not**
    /// touched — a Stammdatenänderung is authoritative master data arriving over
    /// EDIFACT, not an operator edit. Returns `true` when a row was updated and
    /// `false` when the MaLo is not yet known locally (the change is then a
    /// no-op; the row is created by the next `PUT /api/v1/malos`).
    async fn patch_stammdaten(
        &self,
        malo_id: &MaloId,
        patch: &MaloStammdatenPatch,
    ) -> Result<bool, MdmError>;

    /// Return the `MARKTLOKATION` with `rollenzuordnung` valid at `at`.
    ///
    /// `at` defaults to today (German local date).
    async fn find(&self, malo_id: &MaloId, at: Date) -> Result<Option<MaloRecord>, MdmError>;

    /// Return a paged list filtered by the given predicates.
    ///
    /// `at` is the reference date for `rollenzuordnung` validity.
    async fn list(&self, filter: MaloFilter, at: Date) -> Result<PageResult<MaloRecord>, MdmError>;
}

/// A GPKE Teil 4 / GeLi Gas Stammdatenänderung applied to the typed
/// `MESSLOKATION` columns (`LOC+Z17`, „Änderung Daten der MeLo").
///
/// The `makod` adapter builds one object-agnostic attribute map from the SG8
/// `SEQ`/SG10 `CCI`/`CAV` groups; each object's patch struct picks the subset
/// its table can hold via `serde(rename)`. For the MeLo that is (defensively)
/// the metering point's Netzebene (`netzebene` → `netzebene_messung`) and the
/// Regelzone.
///
/// **Verified against UTILMD AHB Strom 2.2 Kap. 9.1.5 (2026-07):** the MeLo
/// Änderungsmeldung (`STS 9013=ZX7`) does **not** carry Netzebene/Regelzone
/// characteristics — its actual payload is the **MSB-Zuordnung** (SG10
/// `CCI 7037=ZB3` Zugeordneter Marktpartner, `CAV 7111=Z91` MSB / `Z39`
/// grundzuständig / `ZF0` gMSB + MP-ID) plus NNE-Abrechnung info. That belongs
/// on the dated `melo_msb_zuordnungen` timeline, not a typed-column `COALESCE`
/// patch, and its GPKE-vs-WiM-MSB-Wechsel semantics make auto-applying it a
/// deliberate follow-up (roadmap). These fields are retained for robustness /
/// forward-compatibility but rarely fire in practice.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MeloStammdatenPatch {
    /// Netzebene at the metering point (generic `netzebene` attribute →
    /// `melo.netzebene_messung`).
    #[serde(rename = "netzebene")]
    pub netzebene_messung: Option<String>,
    /// Regelzone EIC (ÜNB assignment).
    pub regelzone: Option<String>,
}

impl MeloStammdatenPatch {
    /// `true` when no column would change.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.netzebene_messung.is_none() && self.regelzone.is_none()
    }
}

/// Read/write access to `MESSLOKATION` records.
#[allow(async_fn_in_trait)]
pub trait MeloRepository: Send + Sync {
    /// Insert or update a `MESSLOKATION`.
    ///
    /// Takes the **typed** BO for the same reason
    /// [`MaloRepository::upsert`] does — see
    /// [`MeloShadowColumns`](crate::bo4e::MeloShadowColumns).
    ///
    /// Returns the new version number.
    async fn upsert(
        &self,
        melo_id: &MeloId,
        malo_id: Option<&MaloId>,
        data: &rubo4e::current::Messlokation,
        if_match: Option<i64>,
        bo4e_version: &str,
    ) -> Result<i64, MdmError>;

    /// Return the `MESSLOKATION` record.
    async fn find(&self, melo_id: &MeloId) -> Result<Option<MeloRecord>, MdmError>;

    /// Apply a UTILMD Stammdatenänderung (`LOC+Z17`) to the typed MeLo columns —
    /// the MeLo counterpart of [`MaloRepository::patch_stammdaten`].
    ///
    /// Each `Some` field overwrites its column via `COALESCE`; each `None` leaves
    /// it unchanged. The JSONB payload (`data`, `standorteigenschaften`) and the
    /// optimistic `version` are **not** touched. Returns `true` when a row was
    /// updated and `false` when the MeLo is not yet known locally (no-op).
    async fn patch_stammdaten(
        &self,
        melo_id: &MeloId,
        patch: &MeloStammdatenPatch,
    ) -> Result<bool, MdmError>;
}

/// Read/write access to ERP webhook subscriptions.
///
/// Unlike the other repositories here, every method returns an explicitly
/// `Send` future rather than using bare `async fn`. The fan-out worker
/// (`marktd::fanout`) is generic over this trait, and a bare AFIT future is
/// not `Send` in a generic context — which forced the worker onto a dedicated
/// OS thread with its own current-thread runtime and a `LocalSet`, an entire
/// thread spent on an accidental auto-trait bound. With `+ Send` the worker is
/// an ordinary `tokio::spawn`.
pub trait SubscriptionRepository: Send + Sync {
    /// Insert or update a subscription.
    ///
    /// `webhook_secret` is the HMAC signing key and is stored **in plaintext** —
    /// it is an integrity secret a subscriber uses to verify a delivery came
    /// from this hub, not a confidentiality key over customer data. Protect it
    /// with database-level controls (least-privilege grants on `subscriptions`,
    /// storage encryption); see the marktd README, "Webhook secret at rest".
    ///
    /// Returns the new version number.
    fn upsert(&self, sub: Subscription) -> impl Future<Output = Result<i64, MdmError>> + Send;

    /// Return a subscription by subscriber ID.
    fn find(
        &self,
        subscriber_id: &str,
    ) -> impl Future<Output = Result<Option<Subscription>, MdmError>> + Send;

    /// Deactivate a subscription so it stops matching future fan-outs.
    ///
    /// A soft delete by design: `event_delivery` rows reference the subscriber
    /// and are the § 147 AO / GoBD record that a market event was (or was not)
    /// delivered, so the row itself must survive. Returns `false` when no such
    /// subscription exists.
    fn deactivate(
        &self,
        subscriber_id: &str,
    ) -> impl Future<Output = Result<bool, MdmError>> + Send;

    /// List all active subscriptions.
    fn list_active(&self) -> impl Future<Output = Result<Vec<Subscription>, MdmError>> + Send;

    /// Return all active subscriptions that match a given event type and role.
    ///
    /// Used by the fan-out worker to select delivery targets.
    fn list_matching(
        &self,
        event_type: &str,
        role: &str,
        sparte: Option<&str>,
    ) -> impl Future<Output = Result<Vec<Subscription>, MdmError>> + Send;
}

/// Read/write access to the process correlation index.
#[allow(async_fn_in_trait)]
pub trait CorrelationIndex: Send + Sync {
    /// Insert a new correlation entry (idempotent — duplicate `process_id` is a no-op).
    async fn insert(&self, entry: CorrelationEntry) -> Result<(), MdmError>;

    /// Update status and `completed_at` for a process.
    async fn update_status(
        &self,
        process_id: Uuid,
        status: ProcessStatus,
        completed_at: Option<time::OffsetDateTime>,
    ) -> Result<(), MdmError>;

    /// Update `edifact_conv_id` when the first `de.mako.*` event is received.
    async fn update_edifact_conv_id(&self, process_id: Uuid, conv_id: Uuid)
    -> Result<(), MdmError>;

    /// Look up by ERP order ID (`Idempotency-Key` from command submission).
    async fn find_by_erp_order_id(
        &self,
        erp_order_id: &str,
    ) -> Result<Option<CorrelationEntry>, MdmError>;

    /// Look up by `process_id`.
    async fn find_by_process_id(
        &self,
        process_id: Uuid,
    ) -> Result<Option<CorrelationEntry>, MdmError>;

    /// Return correlations matching the filter.
    async fn list(&self, filter: CorrelationFilter) -> Result<Vec<CorrelationEntry>, MdmError>;
}

/// Read/write access to the trading-partner directory.
#[allow(async_fn_in_trait)]
pub trait PartnerRepository: Send + Sync {
    /// Insert or update a trading partner.
    ///
    /// Returns the new version number.
    async fn upsert(&self, partner: PartnerRecord) -> Result<i64, MdmError>;

    /// Return a partner by their 13-digit `MarktpartnerId`.
    async fn find(&self, id: &MarktpartnerId) -> Result<Option<PartnerRecord>, MdmError>;

    /// List all partners.
    async fn list(&self) -> Result<Vec<PartnerRecord>, MdmError>;
}

// ── Preisblatt ───────────────────────────────────────────────────────────────

/// Discriminates how a price sheet entered the system.
///
/// Used for audit trails and to enforce operator-override protection:
/// an `Api`-sourced sheet is never silently overwritten by a `Mako` ingest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PreisblattSource {
    /// Uploaded directly via the REST API (operator batch job or manual override).
    Api,
    /// Ingested automatically from a PRICAT 27003 message by the mako engine.
    Mako,
}

impl std::fmt::Display for PreisblattSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PreisblattSource::Api => f.write_str("api"),
            PreisblattSource::Mako => f.write_str("mako"),
        }
    }
}

impl std::str::FromStr for PreisblattSource {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "api" => Ok(PreisblattSource::Api),
            "mako" => Ok(PreisblattSource::Mako),
            other => Err(format!("unknown PreisblattSource: {other:?}")),
        }
    }
}

/// A stored `PreisblattNetznutzung` record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PreisblattRecord {
    /// GLN of the NB that published this price sheet.
    pub nb_mp_id: String,
    /// The full BO4E `PreisblattNetznutzung` payload (stored as JSONB).
    pub data: serde_json::Value,
    /// BO4E schema version of `data`.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
    /// How this record entered the system: `api` (operator upload) or `mako` (engine ingest).
    pub source: PreisblattSource,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Read/write access to NB price sheets.
#[allow(async_fn_in_trait)]
pub trait PreisblattRepository: Send + Sync {
    /// Upsert a `PreisblattNetznutzung` for the given NB GLN.
    ///
    /// Multiple records per GLN are stored; they are distinguished by the
    /// `gueltigkeit.startdatum` inside `data`.
    ///
    /// `source` tracks how the record entered the system: `Api` for operator
    /// REST uploads, `Mako` for engine-ingested PRICAT 27003 messages.
    /// An `Api`-sourced sheet is never overwritten by a `Mako` ingest unless
    /// `force = true`.
    async fn upsert(
        &self,
        nb_mp_id: &str,
        data: serde_json::Value,
        bo4e_version: &str,
        source: PreisblattSource,
    ) -> Result<(), MdmError>;

    /// Return the price sheet for `nb_mp_id` that was valid on `billing_date`
    /// (ISO 8601 date string, e.g. `"2025-06-15"`).
    ///
    /// Returns `None` when no matching entry is found.
    async fn find_for_date(
        &self,
        nb_mp_id: &str,
        billing_date: &str,
    ) -> Result<Option<PreisblattRecord>, MdmError>;
}

// ── PreisblattMessung (MSB metering price sheets — B5) ───────────────────────

/// A stored `PreisblattMessung` record from the MSB.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PreisblattMessungRecord {
    /// MP-ID (BDEW-Codenummer) of the Messstellenbetreiber that published this sheet.
    pub msb_mp_id: String,
    /// The full BO4E `PreisblattMessung` payload (stored as JSONB).
    pub data: serde_json::Value,
    /// BO4E schema version of `data`.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
    /// How this record entered the system: `api` (operator upload) or `mako` (engine ingest).
    pub source: PreisblattSource,
    /// Optional `AufAbschlag` list from the MSB PRICAT 27001–27003.
    ///
    /// `AufAbschlag` entries describe conditional price supplements and discounts
    /// (§14a ToU discounts, time-variable surcharges, etc.).  Each entry is a
    /// `rubo4e::current::AufAbschlag` JSONB object.
    ///
    /// `None` when the PRICAT does not carry any `AufAbschlag` entries (most
    /// conventional meters).  `invoic-checker` uses this field to validate
    /// whether a discount position in INVOIC 31009 is contractually authorised.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auf_abschlaege: Vec<serde_json::Value>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Read/write access to MSB (Messstellenbetreiber) metering price sheets.
///
/// Used by `invoicd` for PID 31009 (`MSB-Rechnung`) tariff plausibility checks:
/// positions 4 (Grundpreis Messung) and 5 (Arbeitspreis Messung).
///
/// Source: WiM AHB BK6-24-174.
#[allow(async_fn_in_trait)]
pub trait PreisblattMessungRepository: Send + Sync {
    /// Upsert a `PreisblattMessung` for the given MSB MP-ID.
    ///
    /// Conflicts on `(msb_mp_id, valid_from)` perform an in-place update.
    /// An `Api`-sourced sheet is never overwritten by a `Mako` ingest.
    async fn upsert_messung(
        &self,
        msb_mp_id: &str,
        data: serde_json::Value,
        bo4e_version: &str,
        source: PreisblattSource,
    ) -> Result<(), MdmError>;

    /// Return the `PreisblattMessung` for `msb_mp_id` valid on `billing_date`
    /// (ISO 8601 date string, e.g. `"2025-06-15"`).
    ///
    /// Returns `None` when no matching entry is found.
    async fn find_messung_for_date(
        &self,
        msb_mp_id: &str,
        billing_date: &str,
    ) -> Result<Option<PreisblattMessungRecord>, MdmError>;
}

// ── PreisblattKonzessionsabgabe (B3) ─────────────────────────────────────────

/// A stored `PreisblattKonzessionsabgabe` record.
///
/// KAV §2 requires the NB to include Konzessionsabgabe (KA) as a separate
/// tariff position in every NNE invoice. `kundengruppe_ka` differentiates between
/// Tarifkunden and Sondervertragskunden.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PreisblattKaRecord {
    /// NB MP-ID (BDEW-Codenummer) that published this price sheet.
    pub nb_mp_id: String,
    /// Energy commodity (`STROM` or `GAS`).
    pub sparte: String,
    /// Customer group classification — `None` means applies to all groups.
    pub kundengruppe_ka: Option<String>,
    /// The full BO4E `PreisblattKonzessionsabgabe` payload.
    pub data: serde_json::Value,
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
    /// How this record entered the system.
    pub source: PreisblattSource,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Read/write access to `PreisblattKonzessionsabgabe` records.
///
/// Used by `netzbilanzd` for INVOIC 31001/31002 KA tariff positions.
#[allow(async_fn_in_trait)]
pub trait PreisblattKaRepository: Send + Sync {
    /// Upsert a `PreisblattKonzessionsabgabe` for the given NB MP-ID.
    ///
    /// Conflicts on `(nb_mp_id, sparte, kundengruppe_ka, valid_from)` are updated in-place.
    /// `Api`-sourced sheets are never overwritten by `Mako` ingests.
    async fn upsert_ka(
        &self,
        nb_mp_id: &str,
        sparte: &str,
        kundengruppe_ka: Option<&str>,
        data: serde_json::Value,
        bo4e_version: &str,
        source: PreisblattSource,
    ) -> Result<(), MdmError>;

    /// Return the `PreisblattKonzessionsabgabe` valid on `billing_date` for the NB.
    ///
    /// Returns `None` when no matching entry is found.
    async fn find_ka_for_date(
        &self,
        nb_mp_id: &str,
        sparte: &str,
        kundengruppe_ka: Option<&str>,
        billing_date: &str,
    ) -> Result<Option<PreisblattKaRecord>, MdmError>;
}

// ── PreisblattDienstleistung (MSB service price sheets) ──────────────────────

/// A stored `PreisblattDienstleistung` record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PreisblattDienstleistungRecord {
    pub msb_mp_id: String,
    pub data: serde_json::Value,
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
    pub source: PreisblattSource,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Read/write access to MSB service price sheets.
///
/// Used by `invoic-checker` for INVOIC 31009 service position validation
/// and by `mako-wim` REQOTE/QUOTES (PIDs 35001/35002/35004/35005).
#[allow(async_fn_in_trait)]
pub trait PreisblattDienstleistungRepository: Send + Sync {
    async fn upsert_dienstleistung(
        &self,
        msb_mp_id: &str,
        data: serde_json::Value,
        bo4e_version: &str,
        source: PreisblattSource,
    ) -> Result<(), MdmError>;

    async fn find_dienstleistung_for_date(
        &self,
        msb_mp_id: &str,
        billing_date: &str,
    ) -> Result<Option<PreisblattDienstleistungRecord>, MdmError>;
}

// ── PreisblattHardware (MSB hardware rental price sheets) ────────────────────

/// A stored `PreisblattHardware` record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PreisblattHardwareRecord {
    pub msb_mp_id: String,
    pub data: serde_json::Value,
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
    pub source: PreisblattSource,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Read/write access to MSB hardware rental price sheets.
///
/// Required for NB → MSB settlement INVOIC 31009 hardware positions.
/// `invoic-checker` check 5 cannot validate hardware positions without it.
#[allow(async_fn_in_trait)]
pub trait PreisblattHardwareRepository: Send + Sync {
    async fn upsert_hardware(
        &self,
        msb_mp_id: &str,
        data: serde_json::Value,
        bo4e_version: &str,
        source: PreisblattSource,
    ) -> Result<(), MdmError>;

    async fn find_hardware_for_date(
        &self,
        msb_mp_id: &str,
        billing_date: &str,
    ) -> Result<Option<PreisblattHardwareRecord>, MdmError>;
}

// ── PriCat (versioned PreisblattNetznutzung history + dispatch) ──────────────

/// Dispatch state of a versioned PRICAT snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriCatDispatchState {
    /// Not yet dispatched to any LF partner.
    Pending,
    /// Dispatch task has picked this version up; may be in-flight.
    Queued,
    /// All active LF partners for this NB have been successfully sent PRICAT 27003.
    Done,
    /// Dispatch failed (see `dispatch_error`); will be retried on next poll.
    Error,
}

impl std::fmt::Display for PriCatDispatchState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Queued => write!(f, "queued"),
            Self::Done => write!(f, "done"),
            Self::Error => write!(f, "error"),
        }
    }
}

/// A single versioned PRICAT snapshot for an NB GLN.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PriCatVersion {
    /// Surrogate primary key (`UUID v4`).
    pub id: uuid::Uuid,
    /// GLN of the NB that published this price sheet.
    pub nb_mp_id: String,
    /// Tenant GLN (operator).
    pub tenant: String,
    /// Start of the validity period (extracted from `data.gueltigkeit.startdatum`).
    pub valid_from: time::Date,
    /// End of the validity period, `None` means open-ended.
    pub valid_to: Option<time::Date>,
    /// Full BO4E `PreisblattNetznutzung` payload (stored as JSONB).
    pub data: serde_json::Value,
    /// BO4E schema version of `data`.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
    /// How this version entered the system.
    pub source: PreisblattSource,
    /// Current dispatch state.
    pub dispatch_state: PriCatDispatchState,
    /// Last dispatch error message, if any.
    pub dispatch_error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// One row in the PRICAT dispatch audit log.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PriCatDispatchEntry {
    pub id: uuid::Uuid,
    pub pricat_version_id: uuid::Uuid,
    pub nb_mp_id: String,
    pub lf_mp_id: String,
    pub tenant: String,
    /// `makod` process ID returned by `MakodClient`, or `None` if dispatch failed.
    pub process_id: Option<uuid::Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub dispatched_at: time::OffsetDateTime,
    pub outcome: String,
    pub error_detail: Option<String>,
}

/// Read/write access to versioned PRICAT snapshots and the dispatch audit log.
#[allow(async_fn_in_trait)]
pub trait PriCatRepository: Send + Sync {
    /// Insert or update a versioned PRICAT snapshot.
    ///
    /// Conflicts on `(nb_mp_id, tenant, valid_from)` perform an in-place update of
    /// the payload and reset `dispatch_done_at` so the new version is re-dispatched.
    ///
    /// Returns the `UUID` of the upserted row.
    #[allow(clippy::too_many_arguments)]
    async fn upsert_version(
        &self,
        nb_mp_id: &str,
        tenant: &str,
        valid_from: time::Date,
        valid_to: Option<time::Date>,
        data: serde_json::Value,
        bo4e_version: &str,
        source: PreisblattSource,
    ) -> Result<uuid::Uuid, MdmError>;

    /// Return all PRICAT versions for the given NB GLN, newest first.
    async fn list_versions(
        &self,
        nb_mp_id: &str,
        tenant: &str,
    ) -> Result<Vec<PriCatVersion>, MdmError>;

    /// Return the single most-recent PRICAT version for the given NB.
    async fn find_latest(
        &self,
        nb_mp_id: &str,
        tenant: &str,
    ) -> Result<Option<PriCatVersion>, MdmError>;

    /// Return all versions whose dispatch has not yet completed (state ≠ Done).
    async fn list_pending(&self, tenant: &str) -> Result<Vec<PriCatVersion>, MdmError>;

    /// Mark a version as queued for dispatch.
    async fn mark_queued(&self, id: uuid::Uuid) -> Result<(), MdmError>;

    /// Mark a version as fully dispatched (all LF partners reached).
    async fn mark_done(&self, id: uuid::Uuid) -> Result<(), MdmError>;

    /// Mark a version dispatch as failed with an error message.
    async fn mark_error(&self, id: uuid::Uuid, error: &str) -> Result<(), MdmError>;

    /// Append a dispatch audit entry for one NB × LF dispatch attempt.
    async fn log_dispatch(&self, entry: PriCatDispatchEntry) -> Result<(), MdmError>;

    /// Return dispatch log entries for the given PRICAT version.
    async fn dispatch_log(
        &self,
        pricat_version_id: uuid::Uuid,
    ) -> Result<Vec<PriCatDispatchEntry>, MdmError>;
}

// ── NbContract (NB network contracts — typed, not opaque JSONB) ──────────────

/// Billing frequency for NB network contracts.
///
/// Governs when `invoicd` triggers selbstausgestellt INVOIC 31006 MMM billing runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BillingSchedule {
    /// Invoice once per calendar month.
    #[default]
    Monthly,
    /// Invoice every calendar quarter.
    Quarterly,
    /// Invoice once per calendar year.
    Annually,
}

impl std::fmt::Display for BillingSchedule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Monthly => write!(f, "MONTHLY"),
            Self::Quarterly => write!(f, "QUARTERLY"),
            Self::Annually => write!(f, "ANNUALLY"),
        }
    }
}

impl std::str::FromStr for BillingSchedule {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_uppercase().as_str() {
            "MONTHLY" => Ok(Self::Monthly),
            "QUARTERLY" => Ok(Self::Quarterly),
            "ANNUALLY" => Ok(Self::Annually),
            other => Err(format!("unknown BillingSchedule '{other}'")),
        }
    }
}

impl BillingSchedule {
    /// Infallible parse; returns `Monthly` on unknown input.
    #[must_use]
    pub fn from_str_or_default(s: &str) -> Self {
        s.parse().unwrap_or_default()
    }
}

/// The Netznutzungsvertrag as `marktd` serves it over REST.
///
/// A read-side projection of [`NbContractRecord`]: only the fields a consumer
/// decides on, with the dates as the wire strings marktd emits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NbContractView {
    /// ERP contract number or UUID.
    pub contract_id: String,
    /// 11-digit Marktlokations-ID.
    pub malo_id: String,
    /// 13-digit BDEW/DVGW GLN of the Netzbetreiber.
    pub nb_mp_id: String,
    /// MP-ID of the Netznutzer this contract is with.
    pub netznutzer_mp_id: String,
    /// `LIEFERANT` | `LETZTVERBRAUCHER`.
    #[serde(default)]
    pub netznutzer_typ: NetznutzerTyp,
    /// Voltage / pressure level.
    pub netzebene: String,
    /// Metering / balancing method.
    pub bilanzierungsmethode: String,
}

impl NbContractView {
    /// Whether the Netznutzer is the Letztverbraucher itself (Selbstzahler).
    #[must_use]
    pub const fn is_selbstzahler(&self) -> bool {
        self.netznutzer_typ.is_selbstzahler()
    }
}

/// Who holds the Netznutzungsvertrag.
///
/// GPKE Teil 1 (BK6-24-174 Anlage 1a), Vorbemerkung, assumes the Letztverbraucher
/// has an all-inclusive supply contract and the Lieferant acts as Netznutzer.
/// „Ist der Letztverbraucher selbst Netznutzer, so tritt er in die Rolle des
/// Lieferanten i.S. dieser Prozessbeschreibung, soweit diese Regelungen sinngemäß
/// auf ihn anwendbar sind. Eine Ausnahme bilden die Meldungen des Lieferanten im
/// Rahmen des Lieferantenwechsels."
///
/// A Selbstzahler is therefore an ordinary LF on the wire and needs no separate
/// message routing — but the NB has to know, because the Preisblatt and the
/// „sonstige Leistung" invoice go to him in that role (Teil 2 Kap. 3.4.4 / 3.4.5)
/// and the Lieferantenwechsel exception applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NetznutzerTyp {
    /// The ordinary case: an all-inclusive supply contract, the LF is Netznutzer.
    #[default]
    Lieferant,
    /// Selbstzahler — „Netznutzer ohne All-Inklusiv-Vertrag". The Letztverbraucher
    /// pays the Netznutzung itself and steps into the LF role.
    Letztverbraucher,
}

impl NetznutzerTyp {
    /// The DB token.
    #[must_use]
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::Lieferant => "LIEFERANT",
            Self::Letztverbraucher => "LETZTVERBRAUCHER",
        }
    }

    /// Parse a DB token.
    ///
    /// `None` on anything else rather than a fallback to the ordinary case: a
    /// Selbstzahler silently read as `Lieferant` goes back onto the automated
    /// Lieferantenwechsel path the flag exists to keep it off. The CHECK
    /// constraint makes an unknown token impossible in the first place.
    #[must_use]
    pub fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "LIEFERANT" => Some(Self::Lieferant),
            "LETZTVERBRAUCHER" => Some(Self::Letztverbraucher),
            _ => None,
        }
    }

    /// Whether this Netznutzer is the Letztverbraucher itself.
    #[must_use]
    pub const fn is_selbstzahler(self) -> bool {
        matches!(self, Self::Letztverbraucher)
    }
}

/// A typed NB (Netzbetreiber) network contract record.
///
/// Unlike LF supply contracts (stored as opaque `JSONB`), NB contracts are
/// fully typed so that `invoicd` can query by
/// `netzebene` and `bilanzierungsmethode` without JSON path expressions.
///
/// Stored in the `nb_contracts` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NbContractRecord {
    /// ERP contract number or UUID.
    pub contract_id: String,
    /// 11-digit Marktlokations-ID.
    pub malo_id: crate::domain::MaloId,
    /// 13-digit BDEW/DVGW GLN of the Netzbetreiber.
    pub nb_mp_id: String,
    /// Energy commodity.
    pub sparte: crate::domain::Sparte,
    /// Voltage / pressure level: `NS` | `MS` | `MSP` | `HSP` | `HS` | `HöS` | `HöS/HS`
    /// (Gas: `GND` / `GMT` / `GHD`).
    pub netzebene: String,
    /// Metering / balancing method: `RLM` | `SLP` | `IMS` | `TLP_GEMEINSAM` | …
    pub bilanzierungsmethode: String,
    /// How often the NB bills for network usage.
    pub billing_schedule: BillingSchedule,
    /// MP-ID of the Netznutzer this contract is with.
    pub netznutzer_mp_id: String,
    /// What kind of party the Netznutzer is.
    #[serde(default)]
    pub netznutzer_typ: NetznutzerTyp,
    /// Start of contract validity (local date in MEZ/MESZ).
    #[serde(with = "date_iso")]
    pub valid_from: time::Date,
    /// End of contract validity (`None` = currently active).
    #[serde(with = "date_iso::opt")]
    pub valid_to: Option<time::Date>,
    /// Full BO4E `Vertrag` payload (L1 — digital LRV exchange).
    ///
    /// `_typ` is auto-injected to `"VERTRAG"` on write.
    /// Rows created before L1 have `'{}'` (empty); re-PUT to populate.
    #[serde(default)]
    pub data: serde_json::Value,
    /// Contract type extracted from `data["vertragsart"]`.
    /// Default: `NETZNUTZUNGSVERTRAG`.
    #[serde(default)]
    pub vertragsart: Option<String>,
    /// Contract lifecycle status extracted from `data["vertragsstatus"]`.
    /// Default: `AKTIV`.
    #[serde(default)]
    pub vertragsstatus: Option<String>,
    /// Tenant ID for multi-tenant deployments.
    pub tenant: String,
    /// Optimistic-concurrency version counter.
    pub version: i64,
}

/// CRUD repository for NB network contracts.
#[allow(async_fn_in_trait)]
pub trait NbContractRepository: Send + Sync {
    /// Upsert a NB contract record.  Returns the new version number.
    #[must_use]
    async fn upsert(&self, rec: NbContractRecord) -> Result<i64, MdmError>;

    /// Find a contract by `contract_id`.
    #[must_use]
    async fn find(&self, contract_id: &str) -> Result<Option<NbContractRecord>, MdmError>;

    /// Find the contract active on `date` for `malo_id` within `tenant`.
    ///
    /// Returns the most recent contract whose `valid_from ≤ date < valid_to`
    /// (or `valid_to IS NULL`).
    #[must_use]
    async fn find_active(
        &self,
        malo_id: &str,
        date: time::Date,
        tenant: &str,
    ) -> Result<Option<NbContractRecord>, MdmError>;

    /// List all NB contracts for a given `nb_mp_id` and `tenant`.
    #[must_use]
    async fn list_by_nb(
        &self,
        nb_mp_id: &str,
        tenant: &str,
    ) -> Result<Vec<NbContractRecord>, MdmError>;
}

// ── VersorgungsStatus ─────────────────────────────────────────────────────────

/// Supply status of a Marktlokation.
///
/// Derived from `de.mako.process.completed` events by `marktd`'s
/// `event_ingest` handler and persisted in the `versorgungsstatus` table.
/// One row per MaLo per tenant — upserted on each relevant process completion.
///
/// Used by `processd` to drive the LF's automated answers to the NB-initiated
/// GPKE processes (inbound 55007 and 55010)
/// without ERP involvement (GPKE Teil 1 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum LieferStatus {
    /// Active supply — an LF is assigned to this MaLo.
    Beliefert,
    /// No supply — after Lieferende or before first Lieferbeginn.
    Unbeliefert,
    /// Basic supply under §36 EnWG (Grundversorgung).
    Grundversorgung,
    /// Emergency supply under §38 EnWG (Ersatzversorgung, max 3 months).
    Ersatzversorgung,
    /// MaKo participation suspended (Ruhend).
    Ruhend,
    /// Decommissioned — no further MaKo processes possible.
    Stillgelegt,
}

impl std::fmt::Display for LieferStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Beliefert => write!(f, "Beliefert"),
            Self::Unbeliefert => write!(f, "Unbeliefert"),
            Self::Grundversorgung => write!(f, "Grundversorgung"),
            Self::Ersatzversorgung => write!(f, "Ersatzversorgung"),
            Self::Ruhend => write!(f, "Ruhend"),
            Self::Stillgelegt => write!(f, "Stillgelegt"),
        }
    }
}

impl std::str::FromStr for LieferStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Beliefert" => Ok(Self::Beliefert),
            "Unbeliefert" => Ok(Self::Unbeliefert),
            "Grundversorgung" => Ok(Self::Grundversorgung),
            "Ersatzversorgung" => Ok(Self::Ersatzversorgung),
            "Ruhend" => Ok(Self::Ruhend),
            "Stillgelegt" => Ok(Self::Stillgelegt),
            other => Err(format!("unknown LieferStatus '{other}'")),
        }
    }
}

/// Per-MaLo supply state record persisted in `marktd`.
///
/// One row per `(malo_id, tenant)`. Upserted atomically on each relevant
/// `de.mako.process.completed` event with optimistic concurrency control
/// (`WHERE version = $expected`). On conflict: read-retry once (at-least-once
/// fan-out delivery guarantees convergence).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersorgungsStatusRecord {
    /// 11-digit Marktlokations-ID.
    pub malo_id: MaloId,
    /// Current supply state.
    pub lieferstatus: LieferStatus,
    /// GLN of the active Lieferant (set when `lieferstatus == Beliefert`).
    pub lf_mp_id: Option<String>,
    /// MP-ID of the announced future Lieferant (post UTILMD 55001/44001, pre confirmation).
    ///
    /// At most ONE pending Lieferbeginn per MaLo at any time — the NB rejects a second
    /// 55001 with GPKE rule A06 while `lf_mp_id_next IS NOT NULL`.
    pub lf_mp_id_next: Option<String>,
    /// Announced Lieferbeginn date of the future Lieferant — set together with `lf_mp_id_next`.
    ///
    /// Together these two fields form the complete "pending transition" record: WHO takes
    /// over (`lf_mp_id_next`) and WHEN (`lf_next_lieferbeginn`).  Both are cleared atomically
    /// when the transition is confirmed (55003/44003) or rejected (55004/44004).
    ///
    /// Used by the NB to schedule Ersatz/Grundversorgung gap-closure (§38 EnWG) and by
    /// `netzbilanzd` for billing-period alignment.
    #[serde(default, with = "date_iso::opt")]
    pub lf_next_lieferbeginn: Option<Date>,
    /// Agreed Lieferbeginn date (set when supply is confirmed).
    #[serde(default, with = "date_iso::opt")]
    pub lieferbeginn: Option<Date>,
    /// Agreed Lieferende date (set when termination is initiated).
    #[serde(default, with = "date_iso::opt")]
    pub lieferende: Option<Date>,
    /// GLN of the active Messstellenbetreiber.
    pub msb_mp_id: Option<String>,
    /// GLN of the Netzbetreiber responsible for this MaLo.
    pub nb_mp_id: String,
    /// Start date of the running Ersatz-/Grundversorgung (§38/§36 EnWG).
    ///
    /// Set by `begin_eog_supply` when `lieferstatus` transitions to
    /// `Ersatzversorgung` or `Grundversorgung`; cleared on any other
    /// transition. For `Ersatzversorgung` this anchors the statutory
    /// 3-month maximum (§38 Abs. 2 EnWG) enforced by the `processd`
    /// EoG timer.
    #[serde(default, with = "date_iso::opt")]
    pub eog_seit: Option<Date>,
    /// `process_id` of the last process that triggered a state change.
    pub last_process_id: Option<Uuid>,
    /// Last time this record was updated (RFC 3339).
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
    /// Tenant identifier — data-isolation key written to every database row.
    ///
    /// Typically the operator's BDEW- or DVGW-Codenummer, but any stable unique
    /// string is valid (UUID, slug, etc.).  This is **not** a GLN.
    ///
    /// Not returned in API responses: `marktd` is a single-tenant daemon — every
    /// SQL query is already scoped by `AppState::tenant_gln`, so the client only
    /// ever sees their own data and the value is implicit from the server config.
    #[serde(skip_serializing, default)]
    pub tenant: String,
    /// Optimistic concurrency version; incremented on each update.
    pub version: i64,
}

/// Single entry in the supply-state change history of a MaLo.
///
/// Populated by `VersorgungsStatusRepository::upsert` — each successful write
/// appends one row to `versorgungsstatus_history`.  Used by
/// `GET /api/v1/versorgung/{malo_id}/history` and the `?at=YYYY-MM-DD`
/// point-in-time query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersorgungsStatusHistoryRecord {
    /// Auto-incremented surrogate key (`BIGSERIAL`).
    pub id: i64,
    pub malo_id: MaloId,
    pub tenant: String,
    pub lieferstatus: LieferStatus,
    pub lf_mp_id: Option<String>,
    pub lf_mp_id_next: Option<String>,
    #[serde(default, with = "date_iso::opt")]
    pub lf_next_lieferbeginn: Option<Date>,
    #[serde(default, with = "date_iso::opt")]
    pub lieferbeginn: Option<Date>,
    #[serde(default, with = "date_iso::opt")]
    pub lieferende: Option<Date>,
    pub msb_mp_id: Option<String>,
    pub nb_mp_id: String,
    pub last_process_id: Option<Uuid>,
    /// Version of the `versorgungsstatus` row that this snapshot captures.
    pub version: i64,
    /// UTC instant when this state became active (set when the upsert commits).
    #[serde(with = "time::serde::rfc3339")]
    pub valid_from: time::OffsetDateTime,
}

/// Read/write access to `VersorgungsStatus` records.
///
/// Exactly one row per `(malo_id, tenant)`. All writes use optimistic
/// concurrency — callers must supply the version observed during the
/// last read. A `MdmError::Conflict` response means a concurrent update
/// won; retry after re-reading.
///
/// Every successful `upsert` atomically appends a row to
/// `versorgungsstatus_history`, enabling point-in-time queries via `find_at`.
#[allow(async_fn_in_trait)]
pub trait VersorgungsStatusRepository: Send + Sync {
    /// Insert (version 1) or update a `VersorgungsStatus` record.
    ///
    /// `if_version` is the caller's expected current version.
    /// Pass `None` on first insert.  Returns the new version.
    ///
    /// Returns `MdmError::Conflict` when `if_version` does not match the
    /// stored version (optimistic locking violation).
    ///
    /// Every successful write appends one row to `versorgungsstatus_history`.
    #[must_use]
    async fn upsert(
        &self,
        rec: VersorgungsStatusRecord,
        if_version: Option<i64>,
    ) -> Result<i64, MdmError>;

    /// Return the current `VersorgungsStatus` for a MaLo, or `None` if unknown.
    #[must_use]
    async fn find(
        &self,
        malo_id: &MaloId,
        tenant: &str,
    ) -> Result<Option<VersorgungsStatusRecord>, MdmError>;

    /// Return the supply state as it was on the given calendar date (German local
    /// time, i.e. CET/CEST).
    ///
    /// Uses the `versorgungsstatus_history` table. Returns `None` when no
    /// history exists on or before `at`.
    ///
    /// The SQL equivalent:
    /// ```sql
    /// SELECT * FROM versorgungsstatus_history
    /// WHERE malo_id = $1 AND tenant = $2
    ///   AND (valid_from AT TIME ZONE 'Europe/Berlin')::date <= $at
    /// ORDER BY valid_from DESC LIMIT 1
    /// ```
    #[must_use]
    async fn find_at(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        at: Date,
    ) -> Result<Option<VersorgungsStatusRecord>, MdmError>;

    /// Return the full supply-state change history for a MaLo, newest first.
    ///
    /// Backed by the `versorgungsstatus_history` table.
    #[must_use]
    async fn find_history(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        page: u32,
        size: u32,
    ) -> Result<PageResult<VersorgungsStatusHistoryRecord>, MdmError>;

    /// Return all records for a tenant (used for bulk replay / re-projection).
    #[must_use]
    async fn list_by_tenant(
        &self,
        tenant: &str,
        page: u32,
        size: u32,
    ) -> Result<PageResult<VersorgungsStatusRecord>, MdmError>;

    /// Record an announced incoming Lieferant (partial update).
    ///
    /// Called when a UTILMD 55001/44001 (`de.mako.process.initiated`, NB side)
    /// is received.  Sets `lf_mp_id_next` and `lf_next_lieferbeginn` without
    /// touching `lieferstatus`, `lf_mp_id`, `lieferbeginn`, or `lieferende`.
    ///
    /// Inserts a new row as `Unbeliefert` if none exists yet for this MaLo.
    /// Appends to `versorgungsstatus_history` on every successful write.
    #[must_use]
    async fn announce_lf_next(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        lf_mp_id_next: &str,
        lf_next_lieferbeginn: Option<Date>,
        nb_mp_id: &str,
        process_id: Option<Uuid>,
    ) -> Result<(), MdmError>;

    /// Promote the announced future Lieferant to the active one.
    ///
    /// Called when UTILMD 55003/44003 (`de.mako.process.completed`, NB side)
    /// is sent.  Atomically:
    /// - `lf_mp_id = lf_mp_id_next`
    /// - `lieferbeginn = lf_next_lieferbeginn`
    /// - `lieferstatus = Beliefert`
    /// - `lf_mp_id_next = NULL`, `lf_next_lieferbeginn = NULL`
    ///
    /// No-ops if `lf_mp_id_next` is already `NULL` (idempotent re-delivery).
    /// Appends to `versorgungsstatus_history` on every successful write.
    #[must_use]
    async fn confirm_supply(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        process_id: Option<Uuid>,
    ) -> Result<(), MdmError>;

    /// Mark a MaLo as `Unbeliefert` while preserving any pending announcement.
    ///
    /// Called when UTILMD 55013/44013 (`de.mako.process.completed`) is processed.
    /// The active LF has ended supply; clears `lf_mp_id` and `lieferbeginn` but
    /// leaves `lf_mp_id_next` / `lf_next_lieferbeginn` intact so a pending future
    /// Lieferant announcement is not lost.
    ///
    /// The NB is responsible for activating Ersatz/Grundversorgung (§38 EnWG)
    /// when `lieferstatus` becomes `Unbeliefert` and no `lf_mp_id_next` is set.
    /// Appends to `versorgungsstatus_history` on every successful write.
    #[must_use]
    async fn end_supply(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        nb_mp_id: &str,
        process_id: Option<Uuid>,
    ) -> Result<(), MdmError>;

    /// Clear a pending future-Lieferant announcement without touching the
    /// active supply.
    ///
    /// Invoked when a Lieferbeginn is cancelled or rejected (GPKE 55004 /
    /// GeLi Gas 44004): the previously announced `lf_mp_id_next` /
    /// `lf_next_lieferbeginn` must be reset so downstream consumers do not act
    /// on a supplier switch that will not happen. Idempotent: a no-op when no
    /// pending announcement exists.
    async fn clear_lf_next(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        process_id: Option<Uuid>,
    ) -> Result<(), MdmError>;

    /// Record the start of the statutory fallback supply (§38/§36 EnWG).
    ///
    /// Called when the EoG Zuordnung completes (UTILMD 55013/44013
    /// `de.mako.process.completed`): the Grundversorger becomes the supplier
    /// of record. Atomically sets
    /// - `lieferstatus = Ersatzversorgung` or `Grundversorgung`
    ///   (`eog_status` must be one of the two; any other value is an error),
    /// - `lf_mp_id = gv_mp_id`, `lieferbeginn = eog_seit`,
    /// - `eog_seit = start of the fallback supply` (anchors the §38 Abs. 2
    ///   3-month maximum for `Ersatzversorgung`),
    ///
    /// while preserving `lf_mp_id_next` / `lf_next_lieferbeginn` — a pending
    /// regular supplier switch ends the fallback supply on confirmation.
    /// Appends to `versorgungsstatus_history` on every successful write.
    #[must_use]
    #[allow(clippy::too_many_arguments)] // regulatory transition carries its full context
    async fn begin_eog_supply(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        gv_mp_id: &str,
        nb_mp_id: &str,
        eog_status: LieferStatus,
        eog_seit: Option<Date>,
        process_id: Option<Uuid>,
    ) -> Result<(), MdmError>;
}

// ── Grundversorger (§36 EnWG) ─────────────────────────────────────────────────

/// The Grundversorger determined for a Netzgebiet per §36 Abs. 2 EnWG.
///
/// The supplier with the most Haushaltskunden in the Netzgebiet, festgestellt
/// by the NB every three years. Master data — maintained by the operator (or
/// an import), read by the `processd` gap-closure automation to address the
/// UTILMD 55013/44013 EoG Zuordnung.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrundversorgerRecord {
    /// GLN of the Netzbetreiber whose Netzgebiet this entry covers.
    pub nb_mp_id: String,
    /// Commodity.
    pub sparte: Sparte,
    /// MP-ID of the Grundversorger (the LF addressed by the EoG Zuordnung).
    pub gv_mp_id: String,
    /// Date of the §36 Abs. 2 Feststellung (three-year cycle).
    #[serde(default, with = "date_iso::opt")]
    pub festgestellt_am: Option<Date>,
    /// Pre-deposited default Bilanzkreis for the E/G Zuordnung (GPKE Teil 4
    /// „Übermittlung von Informationen"). When an EoG completes without the
    /// E/G supplying its own Bilanzkreis in time (`ZugeordnetOhneAntwort`),
    /// this BK is the one the NB balances the MaLo against. `None` if the E/G
    /// has not deposited one.
    #[serde(default)]
    pub default_bilanzkreis: Option<String>,
    /// Last time this record was updated (RFC 3339).
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
    /// Tenant identifier (not serialized in API responses).
    #[serde(skip_serializing, default)]
    pub tenant: String,
}

/// Repository for the per-Netzgebiet Grundversorger determination (§36 EnWG).
#[allow(async_fn_in_trait)]
pub trait GrundversorgerRepository: Send + Sync {
    /// Fetch the Grundversorger for a Netzbetreiber and Sparte.
    async fn find(
        &self,
        tenant: &str,
        nb_mp_id: &str,
        sparte: Sparte,
    ) -> Result<Option<GrundversorgerRecord>, MdmError>;

    /// Insert or update the Grundversorger entry.
    async fn upsert(&self, record: &GrundversorgerRecord) -> Result<(), MdmError>;
}

// ── MSB-Zuordnung je Messlokation (dated timeline) ────────────────────────────

/// One dated MSB assignment for a Messlokation.
///
/// The per-MeLo MSB timeline is the authoritative source for point-in-time MSB
/// resolution — a WiM Teil 2 historical Werteanfrage (UC 4.1.1) must address the
/// MSB that served the MeLo **at the target period**, which MaLo-level MSB data
/// cannot answer when a MaLo bundles MeLos with divergent MSB history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeloMsbZuordnung {
    /// Messlokations-ID.
    pub melo_id: String,
    /// GLN of the Messstellenbetreiber.
    pub msb_mp_id: String,
    /// Assignment start (inclusive).
    #[serde(with = "date_iso")]
    pub valid_from: Date,
    /// Assignment end (exclusive); `None` = currently valid.
    #[serde(default, with = "date_iso::opt")]
    pub valid_to: Option<Date>,
    /// Tenant identifier (not serialized in API responses).
    #[serde(skip_serializing, default)]
    pub tenant: String,
}

/// Repository for the per-Messlokation dated MSB timeline (WiM Teil 2 UC 4.1.1).
#[allow(async_fn_in_trait)]
pub trait MeloMsbRepository: Send + Sync {
    /// Record a new MSB assignment effective `valid_from`, closing the
    /// previously-open assignment (`valid_to = valid_from`) atomically. A
    /// re-assignment on an existing `valid_from` overwrites that row.
    async fn assign_msb(
        &self,
        tenant: &str,
        melo_id: &str,
        msb_mp_id: &str,
        valid_from: Date,
    ) -> Result<(), MdmError>;

    /// The MSB responsible for `melo_id` on `at` (point-in-time). `None` when no
    /// assignment covers the date.
    async fn find_msb_at(
        &self,
        tenant: &str,
        melo_id: &str,
        at: Date,
    ) -> Result<Option<String>, MdmError>;

    /// Full assignment history for a MeLo, newest first.
    async fn history(&self, tenant: &str, melo_id: &str)
    -> Result<Vec<MeloMsbZuordnung>, MdmError>;
}

// ── Bilanzierung (BO4E BO #3) ──────────────────────────────────────────────────

/// A BO4E `Bilanzierung` — the balancing-relevant data of a Marktlokation, as a
/// first-class resource with **identity and temporal validity**.
///
/// This subsumes the balancing concept otherwise smeared across `MaloRecord`
/// columns (`bilanzierungsmethode`, `fallgruppe`, `bilanzierungsgebiet` — kept as
/// denormalised current-values) together with the load profile (Prognosegrundlage).
/// The full BO4E object lives in [`Self::data`]; the typed fields are extracted
/// for indexing and query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BilanzierungRecord {
    /// Marktlokations-ID this Bilanzierung belongs to (BO4E `marktlokationsId`).
    pub malo_id: String,
    /// Validity start (BO4E `bilanzierungsbeginn`).
    #[serde(with = "time::serde::rfc3339")]
    pub bilanzierungsbeginn: time::OffsetDateTime,
    /// Validity end, exclusive (BO4E `bilanzierungsende`). `None` = open-ended.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub bilanzierungsende: Option<time::OffsetDateTime>,
    /// Bilanzkreis EIC (BO4E `bilanzkreis`).
    #[serde(default)]
    pub bilanzkreis: Option<String>,
    /// Aggregationsverantwortung (`NB` / `ÜNB`).
    #[serde(default)]
    pub aggregationsverantwortung: Option<String>,
    /// Prognosegrundlage (`SLP` / `Prognose` / …).
    #[serde(default)]
    pub prognosegrundlage: Option<String>,
    /// GaBi Fallgruppenzuordnung.
    #[serde(default)]
    pub fallgruppenzuordnung: Option<String>,
    /// Full BO4E `Bilanzierung` object (round-trip-preserving).
    pub data: serde_json::Value,
    /// BO4E schema version the `data` was written against.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
    /// Tenant identifier (not serialized in API responses).
    #[serde(skip_serializing, default)]
    pub tenant: String,
    /// Last update (RFC 3339).
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Repository for the first-class temporal BO4E `Bilanzierung` resource.
#[allow(async_fn_in_trait)]
pub trait BilanzierungRepository: Send + Sync {
    /// Insert or update the Bilanzierung effective `bilanzierungsbeginn`; the
    /// natural key is `(tenant, malo_id, bilanzierungsbeginn)`.
    async fn upsert(&self, record: &BilanzierungRecord) -> Result<(), MdmError>;

    /// The Bilanzierung effective for `malo_id` at instant `at` (point-in-time):
    /// the newest one whose validity window contains `at`. `None` when none.
    async fn find_at(
        &self,
        tenant: &str,
        malo_id: &str,
        at: time::OffsetDateTime,
    ) -> Result<Option<BilanzierungRecord>, MdmError>;

    /// Full Bilanzierung history for a MaLo, newest validity-start first.
    async fn history(
        &self,
        tenant: &str,
        malo_id: &str,
    ) -> Result<Vec<BilanzierungRecord>, MdmError>;
}

// ── Netz-Element-Lokation (NeLo) ──────────────────────────────────────────────

/// Stored NeLo record.
///
/// A Netz-Element-Lokation (NeLo) is a network element location used in
/// BDEW Redispatch 2.0 processes.  The `nelo_id` is typically a 16-char
/// EIC code (ENTSO-E, NAD DE3055 = `ZEW`) or a 13-digit BDEW Codenummer.
///
/// Source: BDEW Redispatch 2.0 Implementierungsleitfaden v2.x.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeLoRecord {
    /// EIC or BDEW Codenummer.
    pub nelo_id: String,
    pub tenant: String,
    /// Human-readable Bezeichnung.
    pub name: Option<String>,
    pub sparte: Sparte,
    /// Voltage / pressure level — a BO4E `Netzebene` wire value
    /// (`NSP`/`MSP`/`HSP`/`HSS`, their `*_UMSP` transformation levels, or
    /// `HD`/`MD`/`ND` for Gas), same vocabulary as `malo` and `melo`.
    pub netzebene: Option<String>,
    /// Owning Netzbetreiber GLN.
    pub nb_mp_id: String,
    /// Whether this NeLo can be remote-controlled (Redispatch 2.0 `steuerkanal`).
    ///
    /// Required by DELORD/DELRES topology queries.
    pub steuerkanal: Option<bool>,
    /// `eigenschaftMsbLokation` — which Marktrolle is responsible for MSB at this NeLo.
    ///
    /// A BO4E `Marktrolle` **wire** value — `"NB"` (grundzuständiger MSB = NB)
    /// or `"MSB"` (wechselbar), not the Rust variant spelling. Used for WiM Gas
    /// gMSB routing.
    pub eigenschaft_msb_lokation: Option<String>,
    /// `grundzustaendigerMsbCodenr` — gMSB MP-ID (13-digit BDEW/DVGW Codenummer).
    pub grundzustaendiger_msb_codenr: Option<String>,
    /// Additional Redispatch 2.0 attributes (open-ended JSONB).
    pub data: serde_json::Value,
    pub version: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// A GPKE Teil 4 Stammdatenänderung applied to the typed `NETZLOKATION`
/// columns (`LOC+Z18`, „Änderung Daten der NeLo", §14a/Redispatch).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NeloStammdatenPatch {
    /// Voltage / pressure level (`netzebene` → `nelo.netzebene`).
    pub netzebene: Option<String>,
    /// §14a EnWG Steuerkanal presence (`nelo.steuerkanal`), from UTILMD
    /// `CCI+7059=Z49` / `CCI+7037` `ZF3` (vorhanden → `true`) / `ZF2` (kein → `false`).
    pub steuerkanal: Option<bool>,
}

impl NeloStammdatenPatch {
    /// `true` when no column would change.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.netzebene.is_none() && self.steuerkanal.is_none()
    }
}

/// Read/write access to `NeLo` records.
///
/// One row per `(nelo_id, tenant)`.
/// Writes use optimistic concurrency via `if_match` (ETag header version).
#[allow(async_fn_in_trait)]
pub trait NeLoRepository: Send + Sync {
    /// Insert or update a NeLo record.
    ///
    /// `if_match` = `None` for unconditional upsert (first write).
    /// Returns the new version number.
    #[must_use]
    async fn upsert(&self, rec: NeLoRecord, if_match: Option<i64>) -> Result<i64, MdmError>;

    /// Return a NeLo by `nelo_id`, or `None` if not found.
    #[must_use]
    async fn find(&self, nelo_id: &str, tenant: &str) -> Result<Option<NeLoRecord>, MdmError>;

    /// Apply a UTILMD Stammdatenänderung (`LOC+Z18`) to the typed NeLo columns.
    ///
    /// `COALESCE` per column; the JSONB payload and `version` are untouched.
    /// Returns `true` when a row was updated, `false` when the NeLo is unknown.
    async fn patch_stammdaten(
        &self,
        nelo_id: &str,
        tenant: &str,
        patch: &NeloStammdatenPatch,
    ) -> Result<bool, MdmError>;

    /// Return all NeLos owned by a Netzbetreiber GLN.
    #[must_use]
    async fn list_by_nb(
        &self,
        nb_mp_id: &str,
        tenant: &str,
        page: u32,
        size: u32,
    ) -> Result<PageResult<NeLoRecord>, MdmError>;

    /// Return all NeLos for a tenant (paged).
    #[must_use]
    async fn list_by_tenant(
        &self,
        tenant: &str,
        page: u32,
        size: u32,
    ) -> Result<PageResult<NeLoRecord>, MdmError>;
}

// ── Tranche ──────────────────────────────────────────────────────────────────

/// Stored Tranche record.
///
/// A **Tranche** is a share of a Marktlokation's energy assigned to a distinct
/// balancing responsibility (BO4E `Tranche`; GPKE Teil 4 „Daten der Tranche",
/// PIDs 55619/55642/55652/55662/55686). One row per `(tranche_id, tenant)`; the
/// parent MaLo is recorded for `list_by_malo` grouping.
///
/// Source: GPKE Teil 4 (BK6-22-024 Anlage 1d) §1.4; BO4E `Tranche`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrancheRecord {
    /// Tranche identifier (e.g. `<MaLo>-T01`).
    pub tranche_id: String,
    pub tenant: String,
    /// Parent Marktlokation this Tranche belongs to.
    pub malo_id: Option<String>,
    /// Bilanzierungsgebiet EIC (`LOC+237`).
    pub bilanzierungsgebiet: Option<String>,
    /// Netzebene (`netzebene`).
    pub netzebene: Option<String>,
    /// Energierichtung (`EINSPEISUNG` / `ENTNAHME`).
    pub energierichtung: Option<String>,
    /// Full BO4E `Tranche` payload (open-ended JSONB).
    pub data: serde_json::Value,
    pub version: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// A GPKE Teil 4 Stammdatenänderung applied to the typed `TRANCHE` columns
/// (`LOC+Z21`, „Änderung Daten der Tranche").
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrancheStammdatenPatch {
    /// Bilanzierungsgebiet EIC.
    pub bilanzierungsgebiet: Option<String>,
    /// Netzebene.
    pub netzebene: Option<String>,
    /// Energierichtung.
    pub energierichtung: Option<String>,
}

impl TrancheStammdatenPatch {
    /// `true` when no column would change.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bilanzierungsgebiet.is_none()
            && self.netzebene.is_none()
            && self.energierichtung.is_none()
    }
}

/// Read/write access to `Tranche` records.
///
/// One row per `(tranche_id, tenant)`.
#[allow(async_fn_in_trait)]
pub trait TrancheRepository: Send + Sync {
    /// Insert or update a Tranche record. `if_match` = `None` for unconditional
    /// upsert. Returns the new version number.
    async fn upsert(&self, rec: TrancheRecord, if_match: Option<i64>) -> Result<i64, MdmError>;

    /// Return a Tranche by `tranche_id`, or `None` if not found.
    async fn find(&self, tranche_id: &str, tenant: &str)
    -> Result<Option<TrancheRecord>, MdmError>;

    /// Return all Tranchen of a Marktlokation (paged).
    async fn list_by_malo(
        &self,
        malo_id: &str,
        tenant: &str,
        page: u32,
        size: u32,
    ) -> Result<PageResult<TrancheRecord>, MdmError>;

    /// Apply a UTILMD Stammdatenänderung (`LOC+Z21`) to the typed Tranche
    /// columns. `COALESCE` per column; JSONB and `version` untouched. Returns
    /// `true` when a row was updated, `false` when the Tranche is unknown.
    async fn patch_stammdaten(
        &self,
        tranche_id: &str,
        tenant: &str,
        patch: &TrancheStammdatenPatch,
    ) -> Result<bool, MdmError>;
}

/// Convenience bundle of all repositories, passed to handlers via `Arc<AppState<...>>`.
///
/// Uses concrete generic parameters (same pattern as `mako-engine`'s `EngineContext`)
/// so all trait methods are statically dispatched — AFIT is **not** dyn-compatible.
///
/// `services/marktd` instantiates this with the Postgres implementations:
/// ```text
/// AppState<PgMaloRepo, PgMeloRepo, PgSubscriptionRepo, PgCorrelationIndex, PgPartnerRepo>
/// ```
///
/// `testing` feature instantiates it with InMemory implementations.
#[derive(Clone)]
pub struct AppState<Ma, Me, Su, Ci, Pa>
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    pub malo_repo: Ma,
    pub melo_repo: Me,
    pub subscription_repo: Su,
    pub correlation_index: Ci,
    pub partner_repo: Pa,
    #[cfg(feature = "makod-client")]
    pub makod_client: std::sync::Arc<crate::makod_client::MakodClient>,
    /// Low-latency wake-up hint for the durable fan-out worker.
    ///
    /// Producers persist events to the `event_log` outbox (via
    /// `marktd::outbox::enqueue`) and then `notify_one()` this handle so the
    /// worker drains immediately instead of waiting for its next poll. It is a
    /// hint only — correctness rests on the outbox table, never on this signal.
    pub notify: std::sync::Arc<tokio::sync::Notify>,
    /// Operator primary GLN (matches `makod.toml` `[[party]] primary = true`).
    pub tenant_gln: String,
}

// ── MaloGridRecord ────────────────────────────────────────────────────────────

/// NB grid topology record for a single Marktlokation.
///
/// Written by the NB's **NIS/GIS adapter** (network information system) or
/// provisioned manually via `PUT /api/v1/malos/{id}/grid` on `marktd`.
/// Read by `processd` NB module for Anmeldung STP decisions (checks 1, 4).
///
/// NOTE: This is NOT MaStR data. MaStR (BNetzA) covers generation/consumption
/// units, not NB grid topology or Bilanzierungsgebiet assignments.
///
/// Without a grid record, `mako-pruefung` returns `NetzCheckResult::Escalate`
/// — the NB cannot auto-decide.
///
/// # STP impact
///
/// STP improves markedly when this record is present — provision it via the
/// NB-role `PUT /api/v1/malos/{malo_id}/grid` endpoint (manual / ERP integration).
/// Without a grid record, ~40 % of Anmeldungen escalate (missing grid records → cold cache).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaloGridRecord {
    /// 11-digit Marktlokations-ID (Strom) or Gas-MaLo-ID.
    pub malo_id: MaloId,
    /// GLN of the Netzbetreiber that owns this MaLo in their grid.
    pub nb_mp_id: String,
    /// Bilanzierungsgebiet-EIC (`LOC+237` in UTILMD), if known.
    ///
    /// `None` when the NIS has not yet provided this value.  Check 4 in
    /// `mako-pruefung` is skipped (not failed) when both this and the
    /// UTILMD value are absent.
    pub bilanzierungsgebiet: Option<String>,
    /// NB-internal Netzgebiet code (optional).
    pub netzgebiet: Option<String>,
    /// Energy commodity (`STROM` / `GAS`).
    pub sparte: Sparte,
    /// Source of this record (e.g. `"nis"`, `"manual"`).
    pub source: String,
    /// Last sync timestamp.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
    /// Tenant GLN (operator). Not included in the REST API response;
    /// defaults to empty string when deserializing from the marktd API.
    #[serde(default)]
    pub tenant: String,
}

/// Read/write access to NB grid topology records (`malo_grid` table).
///
/// One row per `(malo_id, tenant)`.  Written by the NB's NIS adapter
/// and by manual provisioning; read by `processd` NB module for Anmeldung STP evaluation.
#[allow(async_fn_in_trait)]
pub trait MaloGridRepository: Send + Sync {
    /// Insert or replace the grid record for a MaLo.
    ///
    /// Idempotent — subsequent writes overwrite the previous record.
    /// `updated_at` is set to `now()` by the repository implementation.
    #[must_use]
    async fn upsert(&self, rec: MaloGridRecord) -> Result<(), MdmError>;

    /// Return the grid record for a MaLo, or `None` if not yet synced.
    #[must_use]
    async fn find(
        &self,
        malo_id: &MaloId,
        tenant: &str,
    ) -> Result<Option<MaloGridRecord>, MdmError>;

    /// List all grid records for a given NB GLN and tenant (e.g. for bulk export).
    #[must_use]
    async fn list_by_nb(
        &self,
        nb_mp_id: &str,
        tenant: &str,
    ) -> Result<Vec<MaloGridRecord>, MdmError>;

    /// Delete a grid record (e.g. when MaStR signals decommissioning).
    #[must_use]
    async fn delete(&self, malo_id: &MaloId, tenant: &str) -> Result<(), MdmError>;
}

// ── SteuerbareRessource (B4b) ─────────────────────────────────────────────────

/// A stored `SteuerbareRessource` record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SteuerbareRessourceRecord {
    /// SR-ID (format: `C[A-Z0-9]{9}[0-9]`).
    pub sr_id: String,
    /// Tenant GLN.
    pub tenant: String,
    /// Associated MaLo-ID, if known.
    pub malo_id: Option<String>,
    /// Associated MeLo-ID, if known.
    pub melo_id: Option<String>,
    /// Full BO4E `SteuerbareRessource` payload (stored as JSONB).
    pub data: serde_json::Value,
    /// Contracted iMS control products (`Vec<Konfigurationsprodukt>` as JSONB array).
    ///
    /// `None` = not yet populated from WiM Stammdaten.
    /// `Some([])` = SR has no contracted control products.
    /// Required for pre-dispatch eligibility checks in `wim.steuerungsauftrag.bestaetigen`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub konfigurationsprodukte: Option<serde_json::Value>,
    /// BO4E schema version.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
    /// Monotonic version counter (incremented on update).
    pub version: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// A GPKE Teil 4 Stammdatenänderung applied to the typed `STEUERBARE_RESSOURCE`
/// columns (`LOC+Z19`, „Änderung Daten der SR", §14a).
///
/// The grounded attribute is the contracted **Konfigurationsprodukte** — the SG8
/// `SEQ+Z79` product groups (produktcode `PIA+5` DE7140, zugeordneter Marktpartner
/// `CAV+Z91`/`ZF0`, Produkteigenschaft `CCI+Z66`), extracted as a BO4E
/// `Vec<Konfigurationsprodukt>` JSONB array. Applied by **replacing** the whole
/// array (the AHB carries the full contracted set per change), not merging.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SteuerbareRessourceStammdatenPatch {
    /// Full contracted `Konfigurationsprodukte` array (BO4E), or `None` to leave
    /// the column untouched.
    pub konfigurationsprodukte: Option<serde_json::Value>,
}

impl SteuerbareRessourceStammdatenPatch {
    /// `true` when no column would change.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.konfigurationsprodukte.is_none()
    }
}

/// Persistent store for `SteuerbareRessource` registrations.
///
/// Populated by the WiM iMS Steuerungsauftrag process (PID 55168)
/// and by operator REST uploads.
#[allow(async_fn_in_trait)]
pub trait SteuerbareRessourceRepository: Send + Sync {
    /// Upsert a `SteuerbareRessource` for the given `sr_id` + tenant.
    #[allow(clippy::too_many_arguments)]
    async fn upsert_sr(
        &self,
        sr_id: &str,
        tenant: &str,
        malo_id: Option<&str>,
        melo_id: Option<&str>,
        data: serde_json::Value,
        bo4e_version: &str,
        konfigurationsprodukte: Option<serde_json::Value>,
    ) -> Result<(), MdmError>;

    /// Return the `SteuerbareRessource` for `sr_id`, or `None` if not found.
    async fn find_sr(
        &self,
        sr_id: &str,
        tenant: &str,
    ) -> Result<Option<SteuerbareRessourceRecord>, MdmError>;

    /// Return all `SteuerbareRessource` records for a MaLo.
    async fn list_sr_by_malo(
        &self,
        malo_id: &str,
        tenant: &str,
    ) -> Result<Vec<SteuerbareRessourceRecord>, MdmError>;

    /// Replace the `konfigurationsprodukte` array for an existing SR (M1).
    ///
    /// Returns `Ok(true)` when the SR was found and updated,
    /// `Ok(false)` when the SR does not exist (caller should return 404).
    async fn replace_sr_konfigurationsprodukte(
        &self,
        sr_id: &str,
        tenant: &str,
        konfigurationsprodukte: serde_json::Value,
    ) -> Result<bool, MdmError>;
}

// ── TechnischeRessource (B9) ─────────────────────────────────────────────────

/// A stored `TechnischeRessource` record.
///
/// Covers E-mobility charging points (`EMobilitaetsart`), generation units
/// (`Erzeugungsart`), and storage (`Speicherart`).  Linked to `MaLo`/`MeLo` via
/// `Lokationszuordnung`.  Required for WiM iMS Steuerungsauftrag and Redispatch 2.0.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TechnischeRessourceRecord {
    /// `TrId` — Technische-Ressource identifier.
    pub tr_id: String,
    pub tenant: String,
    /// Linked `MaLo` (`zugeordnete_marktlokation_id`).
    pub malo_id: Option<String>,
    /// Linked `MeLo` (`vorgelagerte_messlokation_id`).
    pub melo_id: Option<String>,
    /// BO4E `TechnischeRessourceNutzung`: `"STROMVERBRAUCHSART"` |
    /// `"STROMERZEUGUNGSART"` | `"SPEICHER"`.
    pub nutzung: Option<String>,
    /// BO4E `TechnischeRessourceVerbrauchsart` (only for `STROMVERBRAUCHSART`):
    /// `"KRAFT_LICHT"` | `"WAERME"` | `"E_MOBILITAET"` | `"STRASSENBELEUCHTUNG"`.
    pub verbrauchsart: Option<String>,
    /// Whether the resource can be remote-controlled (Redispatch 2.0 `ist_fernschaltbar`).
    pub ist_fernschaltbar: Option<bool>,
    /// Full BO4E `TechnischeRessource` payload.
    pub data: serde_json::Value,
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
    pub version: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// A GPKE Teil 4 Stammdatenänderung applied to the typed `TECHNISCHE_RESSOURCE`
/// columns (`LOC+Z20`, „Änderung Daten der TR", §14a/Redispatch).
///
/// The grounded attributes are:
/// - **Fernschaltbarkeit** — UTILMD `CAV+7111=Z58` (Fernschaltung) / `CAV+7110`
///   `Z06` (vorhanden → `true`) / `Z07` (nicht vorhanden → `false`).
/// - **Art und Nutzung der Technischen Ressource** — the BO4E `nutzung`
///   (`CCI+7059` `Z17` Stromverbrauchsart / `Z50` Stromerzeugungsart / `Z56`
///   Speicher) and, for Stromverbrauchsart, the `verbrauchsart` (`CAV+7111`
///   `Z64` Kraft/Licht / `Z65` Wärme / `ZE5` E-Mobilität / `ZA8`
///   Straßenbeleuchtung). Note: this is the TR object's own classification, not
///   the MaLo `CCI+7059=Z69` „technische Einrichtung" Verbrauchsart.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TechnischeRessourceStammdatenPatch {
    /// BO4E `TechnischeRessourceNutzung` (`tr.nutzung`).
    pub nutzung: Option<String>,
    /// BO4E `TechnischeRessourceVerbrauchsart` (`tr.verbrauchsart`).
    pub verbrauchsart: Option<String>,
    /// Fernschaltbarkeit (`tr.ist_fernschaltbar`).
    pub ist_fernschaltbar: Option<bool>,
}

impl TechnischeRessourceStammdatenPatch {
    /// `true` when no column would change.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nutzung.is_none() && self.verbrauchsart.is_none() && self.ist_fernschaltbar.is_none()
    }
}

/// Persistent store for `TechnischeRessource` registrations.
///
/// Populated by Redispatch 2.0 registration processes and by operator REST
/// uploads.  Used by iMS E-mobility `Steuerungsauftrag` routing and flex-market
/// clearing.
#[allow(async_fn_in_trait)]
pub trait TechnischeRessourceRepository: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    async fn upsert_tr(
        &self,
        tr_id: &str,
        tenant: &str,
        malo_id: Option<&str>,
        melo_id: Option<&str>,
        nutzung: Option<&str>,
        verbrauchsart: Option<&str>,
        ist_fernschaltbar: Option<bool>,
        data: serde_json::Value,
        bo4e_version: &str,
    ) -> Result<(), MdmError>;

    async fn find_tr(
        &self,
        tr_id: &str,
        tenant: &str,
    ) -> Result<Option<TechnischeRessourceRecord>, MdmError>;

    /// Return all `TechnischeRessource` records for a `MaLo`.
    async fn list_tr_by_malo(
        &self,
        malo_id: &str,
        tenant: &str,
    ) -> Result<Vec<TechnischeRessourceRecord>, MdmError>;

    /// Return all `TechnischeRessource` records for a `MeLo`.
    async fn list_tr_by_melo(
        &self,
        melo_id: &str,
        tenant: &str,
    ) -> Result<Vec<TechnischeRessourceRecord>, MdmError>;

    /// Apply a UTILMD Stammdatenänderung (`LOC+Z20`) to the typed TR columns.
    ///
    /// `COALESCE` per column; the JSONB payload and `version` are untouched.
    /// Returns `true` when a row was updated, `false` when the TR is unknown.
    async fn patch_stammdaten(
        &self,
        tr_id: &str,
        tenant: &str,
        patch: &TechnischeRessourceStammdatenPatch,
    ) -> Result<bool, MdmError>;
}

// ── Lokationszuordnung graph (B5) ────────────────────────────────────────────

/// One directed edge of the MaKo location graph.
///
/// The graph models: `MaLo ↔ MeLo ↔ NeLo ↔ SteuerbareRessource ↔ TechnischeRessource`
///
/// Temporal validity: `valid_from IS NULL` means "from the beginning of time";
/// `valid_to IS NULL` means "open-ended (currently active)".
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LokationszuordnungEdge {
    pub id: uuid::Uuid,
    pub tenant: String,
    /// Source node ID (e.g. MaLo-ID, MeLo-ID).
    pub von_id: String,
    /// Source node type ([`Lokationstyp`]: `MALO`/`MELO`/`NELO`/`SR`/`TR`).
    pub von_typ: Lokationstyp,
    /// Target node ID.
    pub nach_id: String,
    /// Target node type ([`Lokationstyp`]).
    pub nach_typ: Lokationstyp,
    pub valid_from: Option<time::Date>,
    /// `None` = open-ended (currently active).
    pub valid_to: Option<time::Date>,
    /// Lokationsbündelcode extracted from the BO4E payload
    /// (`data.lokationsbuendelcode`) on upsert — identifies the Lokationsbündel
    /// this edge belongs to (UTILMD Lokationsbündelstruktur).
    #[serde(default)]
    pub lokationsbuendelcode: Option<String>,
    /// Full BO4E `Lokationszuordnung` payload.
    pub data: serde_json::Value,
    /// BFS traversal depth from root (0 = direct edge from root).
    #[serde(default)]
    pub depth: i32,
}

/// Persistent store for the `Lokationszuordnung` location graph.
///
/// Enables single-query recursive traversal of the full MaLo → MeLo → NeLo →
/// SR/TR graph for topology-dependent operations (Redispatch 2.0, iMS E-mobility
/// Steuerungsauftrag routing, MSB Stammdaten hierarchy).
///
/// # Single-write-path invariant (MaLo ↔ MeLo)
///
/// The `melo → malo` edges of this graph are ALSO maintained by the MeLo write
/// path (`MeloRepository::upsert` in marktd's PG implementation) in the same
/// transaction that sets the `melo.malo_id` FK: the FK is a derived convenience
/// for "current parent", the graph is the authoritative temporal history, and
/// the two never contradict. Writers other than the MeLo PUT and the graph API
/// must not touch `melo.malo_id` directly.
#[allow(async_fn_in_trait)]
pub trait LokationszuordnungRepository: Send + Sync {
    /// Insert or replace a directed edge.
    ///
    /// For open-ended edges (`valid_from = None`), only one edge per
    /// `(tenant, von_id, nach_id)` pair is kept.  Dated edges
    /// (`valid_from = Some(date)`) allow temporal succession.
    #[allow(clippy::too_many_arguments)]
    async fn upsert_edge(
        &self,
        tenant: &str,
        von_id: &str,
        von_typ: Lokationstyp,
        nach_id: &str,
        nach_typ: Lokationstyp,
        valid_from: Option<time::Date>,
        valid_to: Option<time::Date>,
        data: serde_json::Value,
    ) -> Result<uuid::Uuid, MdmError>;

    /// Recursively traverse the full location graph reachable from `root_id`.
    ///
    /// Returns all edges BFS-ordered by depth (depth 0 = direct edges from root).
    /// Pass `at_date = None` to return all edges regardless of validity.
    /// Pass `at_date = Some(d)` to filter to edges valid on date `d`.
    ///
    /// Traversal is capped at depth 8 to prevent runaway queries on malformed data.
    async fn find_graph(
        &self,
        tenant: &str,
        root_id: &str,
        at_date: Option<time::Date>,
    ) -> Result<Vec<LokationszuordnungEdge>, MdmError>;

    /// Return direct (depth-0) edges FROM a given node, optionally filtered by date.
    async fn list_edges_from(
        &self,
        tenant: &str,
        von_id: &str,
        at_date: Option<time::Date>,
    ) -> Result<Vec<LokationszuordnungEdge>, MdmError>;

    /// Hard-delete an edge by `(tenant, von_id, nach_id)`.
    ///
    /// Removes all temporal variants of the edge pair.
    /// Returns `true` if at least one row was deleted.
    async fn delete_edge(
        &self,
        tenant: &str,
        von_id: &str,
        nach_id: &str,
    ) -> Result<bool, MdmError>;

    /// Load the [`Lokationsbuendel`] rooted at `malo_id`, projected from the
    /// typed edge graph (validity-filtered by `at_date`).
    ///
    /// Provided method — implementations get it for free on top of
    /// [`find_graph`](Self::find_graph).
    async fn load_buendel(
        &self,
        tenant: &str,
        malo_id: &str,
        at_date: Option<time::Date>,
    ) -> Result<Lokationsbuendel, MdmError> {
        let edges = self.find_graph(tenant, malo_id, at_date).await?;
        Ok(Lokationsbuendel::from_graph(malo_id, &edges))
    }
}

/// Error raised when a [`Lokationsbuendel`] violates a structural integrity rule.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuendelError {
    /// A consuming Marktlokation bundles no Messlokation.
    #[error(
        "Lokationsbündel for MaLo {malo_id} has no Messlokation \
         (a consuming MaLo must bundle at least one MeLo)"
    )]
    NoMesslokation { malo_id: String },
    /// The bundle's Messlokationen are operated by more than one MSB.
    #[error(
        "Lokationsbündel for MaLo {malo_id} spans divergent MSB assignments {msbs:?} \
         (all MeLos of a MaLo must share one Messstellenbetreiber)"
    )]
    DivergentMsb { malo_id: String, msbs: Vec<String> },
}

/// First-class **Lokationsbündel** (UTILMD Lokationsbündelstruktur) — the set of
/// locations bundled under one Marktlokation, projected from the typed
/// [`LokationszuordnungEdge`] graph. Its BO4E carrier is
/// `rubo4e::current::Lokationszuordnung`.
///
/// Modeling the bundle as an aggregate makes its integrity invariants
/// ([`validate`](Self::validate),
/// [`validate_msb_consistency`](Self::validate_msb_consistency)) enforceable at
/// the domain boundary rather than upheld only by the single-write-path
/// convention on `melo.malo_id`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Lokationsbuendel {
    /// Root Marktlokation the bundle hangs off.
    pub malo_id: String,
    /// Codeliste der Lokationsbündelstrukturen (edi-energy id=38), when carried.
    pub lokationsbuendelcode: Option<String>,
    /// Referenced Messlokationen (≥ 1 for a valid consuming bundle).
    pub messlokationen: Vec<String>,
    /// Referenced Netzlokationen.
    pub netzlokationen: Vec<String>,
    /// Referenced steuerbare Ressourcen (§14a control).
    pub steuerbare_ressourcen: Vec<String>,
    /// Referenced technische Ressourcen.
    pub technische_ressourcen: Vec<String>,
}

impl Lokationsbuendel {
    /// Project the bundle rooted at `malo_id` from a set of graph edges
    /// (typically the output of [`LokationszuordnungRepository::find_graph`]).
    ///
    /// Nodes are collected by [`Lokationstyp`] across every edge in `edges`; the
    /// root MaLo itself is excluded from the lists, and ids are de-duplicated.
    #[must_use]
    pub fn from_graph(malo_id: &str, edges: &[LokationszuordnungEdge]) -> Self {
        use std::collections::BTreeSet;
        let (mut melo, mut nelo, mut sr, mut tr) = (
            BTreeSet::new(),
            BTreeSet::new(),
            BTreeSet::new(),
            BTreeSet::new(),
        );
        let mut lokationsbuendelcode: Option<String> = None;
        for e in edges {
            if lokationsbuendelcode.is_none() {
                lokationsbuendelcode.clone_from(&e.lokationsbuendelcode);
            }
            for (id, typ) in [(&e.von_id, e.von_typ), (&e.nach_id, e.nach_typ)] {
                if id == malo_id {
                    continue;
                }
                // Exhaustive rather than wildcarded: mako owns `Lokationstyp`
                // now (BO4E removed it in v202607.1.0) and it carries no
                // `Unknown`, so a new node type must be placed here
                // deliberately instead of being silently discarded with `Malo`.
                match typ {
                    Lokationstyp::Melo => melo.insert(id.clone()),
                    Lokationstyp::Nelo => nelo.insert(id.clone()),
                    Lokationstyp::Sr => sr.insert(id.clone()),
                    Lokationstyp::Tr => tr.insert(id.clone()),
                    // The bundle is keyed on this MaLo; another MaLo on an edge
                    // is a sibling, not a member.
                    Lokationstyp::Malo => false,
                };
            }
        }
        Self {
            malo_id: malo_id.to_owned(),
            lokationsbuendelcode,
            messlokationen: melo.into_iter().collect(),
            netzlokationen: nelo.into_iter().collect(),
            steuerbare_ressourcen: sr.into_iter().collect(),
            technische_ressourcen: tr.into_iter().collect(),
        }
    }

    /// Structural integrity: a consuming Marktlokation must bundle at least one
    /// Messlokation.
    ///
    /// # Errors
    /// [`BuendelError::NoMesslokation`] when the bundle carries no MeLo.
    pub fn validate(&self) -> Result<(), BuendelError> {
        if self.messlokationen.is_empty() {
            return Err(BuendelError::NoMesslokation {
                malo_id: self.malo_id.clone(),
            });
        }
        Ok(())
    }

    /// MSB-consistency invariant: all Messlokationen of the bundle must be
    /// operated by the same Messstellenbetreiber. `msb_by_melo` maps each MeLo-ID
    /// to its current MSB MP-ID (`None` = no MSB assigned yet, which is ignored).
    ///
    /// # Errors
    /// [`BuendelError::DivergentMsb`] when the MeLos resolve to more than one MSB.
    pub fn validate_msb_consistency(
        &self,
        msb_by_melo: &std::collections::HashMap<String, Option<String>>,
    ) -> Result<(), BuendelError> {
        use std::collections::BTreeSet;
        let msbs: BTreeSet<String> = self
            .messlokationen
            .iter()
            .filter_map(|m| msb_by_melo.get(m).and_then(Clone::clone))
            .collect();
        if msbs.len() > 1 {
            return Err(BuendelError::DivergentMsb {
                malo_id: self.malo_id.clone(),
                msbs: msbs.into_iter().collect(),
            });
        }
        Ok(())
    }
}

// ── Device registry: Zaehler + Geraete (B3) ──────────────────────────────────

/// A stored `Zaehler` record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ZaehlerRecord {
    /// Manufacturer serial number or UUID.
    pub zaehler_id: String,
    /// Tenant GLN.
    pub tenant: String,
    /// Owning MeLo-ID.
    pub melo_id: String,
    /// Zähler type string (e.g. `"DREHSTROMZAEHLER"`).
    pub zaehler_typ: Option<String>,
    /// Eichgültigkeitsdatum — calibration valid until.
    pub eichung_bis: Option<time::Date>,
    /// Full BO4E `Zaehler` payload.
    pub data: serde_json::Value,
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
    pub version: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

// ── Geraet device-configuration types (MsbG §23) ─────────────────────────────

/// Configuration parameter keys for `GeraetKonfiguration`.
///
/// These cover the full spectrum of properties that an MSB must track per device
/// under **MsbG §23** (device records), **BSI TR-03109-1/3** (SMGW firmware and
/// TLS certificates), **§14a EnWG BK6-22-300** (CLS remote-control capability),
/// and **§ 13 StromNZV** (calibration and maintenance intervals).
///
/// Values are always strings; use ISO 8601 (`YYYY-MM-DD`) for dates and
/// `"true"` / `"false"` for booleans.  Custom keys use `Sonstiges` with the
/// actual key name in `notiz`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Konfigurationsparameter {
    /// Firmware version string (e.g. `"2.4.1"`) — BSI TR-03109-1 §4.3.
    FirmwareVersion,
    /// Hardware revision string (e.g. `"Rev. B"`).
    HardwareRevision,
    /// Communication technology used by this device's communication module.
    ///
    /// Valid values correspond to `rubo4e::current::Geraetetyp` variants:
    /// `"GPRS"` (ModemGprs), `"PLC"` (PlcKom), `"ETHERNET"` (EthernetKom),
    /// `"FUNK"` (ModemFunk), `"FESTNETZ"` (ModemFestnetz), `"GSM"` (ModemGsm).
    Kommunikation,
    /// Whether the device supports remote (over-the-air) firmware update.
    /// String value `"true"` or `"false"`.
    FernUpdateFaehig,
    /// Whether the device supports §14a EnWG remote control via a CLS channel.
    /// String value `"true"` or `"false"`.
    ClsFaehig,
    /// BSI TR-03109-3 SMGW TLS certificate SHA-256 fingerprint (64 lowercase hex chars).
    ///
    /// Used by the `edmd` certificate-expiry background worker to emit
    /// `de.messwert.cls.compliance-issue` before expiry.
    SmgwTlsCertFingerprint,
    /// SMGW TLS certificate expiry date (`YYYY-MM-DD`).
    ///
    /// The `edmd` worker alerts when `SmgwCertAblaufdatum ≤ today + 30 days`.
    SmgwCertAblaufdatum,
    /// CLS channel identifier for §14a Steuerungsauftrag routing (opaque string).
    ClsKanalId,
    /// GWA (Gateway-Administrator) BDEW-Codenummer — routes WAN traffic to the
    /// correct GWA for SMGW reconfiguration.
    GwaCodenummer,
    /// Manufacturer name (Hersteller).
    Hersteller,
    /// Commissioning date (`YYYY-MM-DD`).
    Inbetriebnahmedatum,
    /// Last maintenance visit date (`YYYY-MM-DD`) — § 13 StromNZV Kalibrierpflicht.
    LetzteWartung,
    /// Next scheduled maintenance date (`YYYY-MM-DD`).
    NaechsteWartung,
    /// Readout protocol for EDL21/EDL40 meters: `"SML"` | `"DLMS"` | `"IEC62056"`.
    AusleseProtokoll,
    /// MSB contract number (Vertragsnummer) for this device.
    MsbVertragsnummer,
    /// Custom / proprietary parameter.  Use the `notiz` field for the actual key name.
    Sonstiges,
}

/// A single device-configuration entry stored per `Geraet` under MsbG §23.
///
/// Configuration entries are stored in an ordered, deduplicated list keyed by
/// `parameter` (last-write-wins within the same `parameter` value).
/// The list is replaced atomically on `PUT .../konfigurationen`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GeraetKonfiguration {
    /// Configuration key.
    pub parameter: Konfigurationsparameter,
    /// Configuration value (string-typed; ISO 8601 for dates, `"true"`/`"false"` for booleans).
    pub wert: String,
    /// Server-side timestamp of the last write (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
    /// Free-text note or sub-key for `Konfigurationsparameter::Sonstiges` entries.
    pub notiz: Option<String>,
}

/// A stored `Geraet` record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GeraetRecord {
    /// Manufacturer serial number or UUID.
    pub geraet_id: String,
    /// Tenant GLN.
    pub tenant: String,
    /// Owning `zaehler_id`.
    pub zaehler_id: String,
    /// Gerätetyp string (e.g. `"WANDLER"`).
    pub geraet_typ: Option<String>,
    /// Full BO4E `Geraet` payload.
    pub data: serde_json::Value,
    /// Typed device configuration entries per MsbG §23.
    ///
    /// Stored in the `geraet_konfigurationen` JSONB column (separate from `data`)
    /// so they can be updated without rewriting the full BO4E payload.  GIN-indexed
    /// for fast queries such as "all devices with `SMGW_CERT_ABLAUFDATUM ≤ 30 days"`.
    pub konfigurationen: Vec<GeraetKonfiguration>,
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
    pub version: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Persistent store for Zähler (meters) and Geräte (devices).
///
/// Populated by WiM MSB/NB device handover processes (ORDERS PIDs 17001, 17002, 17009)
/// and operator REST uploads.
///
/// Source: WiM AHB BK6-24-174; BO4E Zaehler/Geraet schemas.
#[allow(async_fn_in_trait)]
pub trait DeviceRepository: Send + Sync {
    /// Upsert a `Zaehler` record.
    #[allow(clippy::too_many_arguments)]
    /// `zaehler_typ` and `eichung_bis` are **derived from `data`**
    /// ([`ZaehlerShadowColumns`](crate::bo4e::ZaehlerShadowColumns)), not passed
    /// alongside it: `Zaehler` declares both, and asking for them twice let the
    /// column contradict the document it shadows.
    async fn upsert_zaehler(
        &self,
        zaehler_id: &str,
        tenant: &str,
        melo_id: &str,
        data: &rubo4e::current::Zaehler,
        bo4e_version: &str,
    ) -> Result<(), MdmError>;

    /// Return all `Zaehler` for a given MeLo-ID.
    async fn list_zaehler_by_melo(
        &self,
        melo_id: &str,
        tenant: &str,
    ) -> Result<Vec<ZaehlerRecord>, MdmError>;

    /// Return the `Zaehler` for a given `zaehler_id`, or `None` if not found.
    async fn find_zaehler(
        &self,
        zaehler_id: &str,
        tenant: &str,
    ) -> Result<Option<ZaehlerRecord>, MdmError>;

    /// Upsert a `Geraet` record.
    /// `geraet_typ` is derived from `data.geraetetyp`, for the same reason
    /// [`upsert_zaehler`](Self::upsert_zaehler) derives its columns.
    async fn upsert_geraet(
        &self,
        geraet_id: &str,
        tenant: &str,
        zaehler_id: &str,
        data: &rubo4e::current::Geraet,
        bo4e_version: &str,
    ) -> Result<(), MdmError>;

    /// Return all `Geraete` for a given `zaehler_id`.
    async fn list_geraete_by_zaehler(
        &self,
        zaehler_id: &str,
        tenant: &str,
    ) -> Result<Vec<GeraetRecord>, MdmError>;

    /// Return a single `Geraet` by its `geraet_id`, or `None` if not found.
    async fn find_geraet(
        &self,
        geraet_id: &str,
        tenant: &str,
    ) -> Result<Option<GeraetRecord>, MdmError>;

    /// Atomically replace all `GeraetKonfiguration` entries for a `Geraet`.
    ///
    /// Returns `true` if the Geraet was found and updated, `false` if not found.
    ///
    /// The `updated_at` timestamp on each entry is set server-side; callers
    /// should not set it in the request (it is overwritten).
    ///
    /// Emits `de.markt.geraet.konfiguration.updated` via the durable fan-out.
    async fn upsert_geraet_konfigurationen(
        &self,
        geraet_id: &str,
        tenant: &str,
        konfigurationen: Vec<GeraetKonfiguration>,
    ) -> Result<bool, MdmError>;
}

// ── iMSys TOU registers: ZaehlzeitRegister + ZaehlzeitSaison ─────────────────

/// A `ZaehlzeitRegister` defines one metering register of an iMSys
/// (Intelligentes Messsystem) smart meter.
///
/// German smart meters record separate totals for each tariff zone:
/// - `HT` (Hochtarif) — peak-time consumption, higher grid tariff
/// - `NT` (Niedertarif) — off-peak consumption, lower tariff
/// - `EINZEL` — single-tariff (no zone discrimination)
///
/// The applicable zone at any given time is determined by the `ZaehlzeitSaison`
/// entries linked to this register.
///
/// Source: MsbG §19; BO4E Zaehlwerk; BDEW AHB WiM Teil 3.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ZaehlzeitRegisterRecord {
    /// Primary key (UUID).
    pub id: uuid::Uuid,
    /// Owning Zähler serial number.
    pub zaehler_id: String,
    /// Tenant GLN.
    pub tenant: String,
    /// Register human-readable label (e.g. `"HT"`, `"NT"`, `"Gesamt"`).
    pub bezeichnung: String,
    /// BO4E `Zaehlerauspraegung`: `"HT"` | `"NT"` | `"EINZEL"`.
    pub zaehlerauspraegung: String,
    /// OBIS kennzahl identifying this register in MSCONS (e.g. `"1-1:1.29.0"`).
    pub obis_kennzahl: Option<String>,
    /// Measurement unit (default `"KWH"`).
    #[serde(default = "default_kwh")]
    pub einheit: String,
    /// Start of validity.
    pub valid_from: time::Date,
    /// End of validity — `None` = currently valid.
    pub valid_to: Option<time::Date>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

fn default_kwh() -> String {
    "KWH".to_owned()
}

/// Seasonal / weekly time-of-use window within a `ZaehlzeitRegister`.
///
/// Defines the time windows during which the linked register's tariff zone is
/// active (e.g. "HT applies Monday–Friday from 07:00 to 22:00 in winter").
///
/// Multiple `ZaehlzeitSaison` entries cover the full 168-hour week.
///
/// Source: BO4E Zaehlzeitdefinition; MsbG Anlage 1; BDEW Rolloutprofil.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ZaehlzeitSaisonRecord {
    /// Primary key (UUID).
    pub id: uuid::Uuid,
    /// Owning `ZaehlzeitRegister` ID.
    pub register_id: uuid::Uuid,
    /// Season key: `"SOMMER"` | `"WINTER"` | `"GESAMT"` (year-round).
    pub saison: String,
    /// ISO weekday numbers this window applies to, 1 (Mon) through 7 (Sun).
    /// Example: `[1, 2, 3, 4, 5]` = Monday–Friday.
    ///
    /// A typed `Vec<i16>` rather than free JSON: the column it maps to is a
    /// constrained `SMALLINT[]`, so `["monday"]` and `[0]` are rejected at the
    /// boundary instead of being stored and silently matching nothing.
    pub wochentage: Vec<i16>,
    /// Window start in German local time, inclusive. Example: `07:00`.
    ///
    /// Serialised as `"HH:MM:SS"`. Without the explicit format `time::Time`
    /// derives to a component array (`[7,0,0,0]`), which is neither what a
    /// caller sends nor what any other timestamp in this API looks like.
    #[serde(with = "wall_clock")]
    pub zeit_von: time::Time,
    /// Window end in German local time, exclusive. Example: `22:00`.
    ///
    /// Typed `Time` rather than a `"HH:MM"` string: as text, `"7:00"` and
    /// `"07:00"` were distinct values that ordered differently, and the
    /// window comparison in `resolve_tariff_zone` was a lexicographic one that
    /// only worked while every writer happened to zero-pad.
    #[serde(with = "wall_clock")]
    pub zeit_bis: time::Time,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// `"HH:MM:SS"` on the wire for a [`time::Time`], accepting `"HH:MM"` too.
///
/// The default `time` serde impl emits a component array, which has bitten this
/// workspace before; a tariff-window boundary is a wall-clock time and reads as
/// one.
pub mod wall_clock {
    use serde::{Deserialize as _, Deserializer, Serializer, de::Error as _};

    /// Serialise as `"HH:MM:SS"`.
    ///
    /// # Errors
    /// Propagates the serializer's own error.
    pub fn serialize<S: Serializer>(t: &time::Time, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!(
            "{:02}:{:02}:{:02}",
            t.hour(),
            t.minute(),
            t.second()
        ))
    }

    /// Deserialise `"HH:MM"` or `"HH:MM:SS"`.
    ///
    /// # Errors
    /// Returns a serde error when the value is not a wall-clock time.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<time::Time, D::Error> {
        let raw = String::deserialize(d)?;
        let mut parts = raw.trim().split(':');
        let mut next = |what: &str| -> Result<u8, D::Error> {
            parts
                .next()
                .ok_or_else(|| D::Error::custom(format!("{raw:?}: missing {what}")))?
                .parse()
                .map_err(|e| D::Error::custom(format!("{raw:?}: {what}: {e}")))
        };
        let h = next("hour")?;
        let m = next("minute")?;
        let sec = match parts.next() {
            Some(s) => s
                .parse()
                .map_err(|e| D::Error::custom(format!("{raw:?}: second: {e}")))?,
            None => 0,
        };
        if parts.next().is_some() {
            return Err(D::Error::custom(format!(
                "{raw:?}: too many `:`-separated parts"
            )));
        }
        time::Time::from_hms(h, m, sec).map_err(|e| D::Error::custom(format!("{raw:?}: {e}")))
    }

    #[cfg(test)]
    mod tests {
        use super::super::ZaehlzeitSaisonRecord;

        #[test]
        fn a_window_round_trips_as_hh_mm_ss_not_a_component_array() {
            let json = serde_json::json!({
                "id": "00000000-0000-0000-0000-000000000001",
                "register_id": "00000000-0000-0000-0000-000000000002",
                "saison": "WINTER",
                "wochentage": [1, 2, 3, 4, 5],
                "zeit_von": "07:00",
                "zeit_bis": "22:00:00",
                "updated_at": "2026-01-01T00:00:00Z",
            });
            let rec: ZaehlzeitSaisonRecord =
                serde_json::from_value(json).expect("HH:MM and HH:MM:SS both parse");
            assert_eq!(rec.zeit_von, time::macros::time!(07:00));
            assert_eq!(rec.zeit_bis, time::macros::time!(22:00));

            let out = serde_json::to_value(&rec).expect("serialise");
            assert_eq!(out["zeit_von"], "07:00:00");
            assert!(
                out["zeit_bis"].is_string(),
                "a window boundary must stay a string, not become a component array: {out}"
            );
        }

        #[test]
        fn a_nonsense_time_is_refused_rather_than_defaulted() {
            for bad in ["25:00", "07", "07:00:00:00", "seven"] {
                let json = serde_json::json!({
                    "id": "00000000-0000-0000-0000-000000000001",
                    "register_id": "00000000-0000-0000-0000-000000000002",
                    "saison": "WINTER",
                    "wochentage": [1],
                    "zeit_von": bad,
                    "zeit_bis": "22:00",
                    "updated_at": "2026-01-01T00:00:00Z",
                });
                assert!(
                    serde_json::from_value::<ZaehlzeitSaisonRecord>(json).is_err(),
                    "{bad:?} must not parse as a window boundary"
                );
            }
        }
    }
}

/// Persistence store for iMSys TOU registers.
///
/// Allows `edmd` to correctly classify MSCONS reads by tariff zone
/// (HT vs NT) for iMSys smart meters without relying on the OBIS code alone.
#[allow(async_fn_in_trait)]
pub trait ZaehlzeitRepository: Send + Sync {
    /// Upsert a `ZaehlzeitRegister`.
    async fn upsert_register(&self, rec: &ZaehlzeitRegisterRecord) -> Result<(), MdmError>;

    /// Return all registers for a given `zaehler_id`.
    async fn list_registers_by_zaehler(
        &self,
        zaehler_id: &str,
        tenant: &str,
    ) -> Result<Vec<ZaehlzeitRegisterRecord>, MdmError>;

    /// Upsert a `ZaehlzeitSaison` for a given register.
    async fn upsert_saison(&self, rec: &ZaehlzeitSaisonRecord) -> Result<(), MdmError>;

    /// Return all `ZaehlzeitSaison` entries for a register.
    async fn list_saisons_by_register(
        &self,
        register_id: uuid::Uuid,
        tenant: &str,
    ) -> Result<Vec<ZaehlzeitSaisonRecord>, MdmError>;

    /// Resolve the applicable tariff zone (`HT`|`NT`|`EINZEL`) for a Zähler at
    /// a given local datetime.  Returns `None` if no matching window is found
    /// (treat as `EINZEL` in that case).
    async fn resolve_tariff_zone(
        &self,
        zaehler_id: &str,
        tenant: &str,
        local_datetime: time::PrimitiveDateTime,
    ) -> Result<Option<String>, MdmError>;
}

// ── MMMA Gas settlement prices (Trading Hub Europe / MGV) ────────────────────

/// A stored Gas MMM Abrechnungspreis record.
///
/// Published monthly by Trading Hub Europe (THE). Used by `netzbilanzd` when
/// generating INVOIC 31007/31008 and by `invoicd` for MMM position check 6.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MmmaPreisGasRecord {
    /// First day of the billing month (German local time).
    pub price_month: time::Date,
    /// Marktgebiet — always `"THE"` in Germany since 2021.
    pub marktgebiet: String,
    /// Ausgleichsenergiepreis Überschuss (Mehrmengen) in ct/kWh.
    pub mehr_ct_kwh: rust_decimal::Decimal,
    /// Ausgleichsenergiepreis Defizit (Mindermengen) in ct/kWh.
    pub minder_ct_kwh: rust_decimal::Decimal,
    /// How this record entered the system: `"manual"` | `"the-api"` | `"csv-import"`.
    pub source: String,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Read/write access to Gas MMM Abrechnungspreise.
///
/// `netzbilanzd` fetches these instead of requiring manual ERP input per billing run.
/// `invoicd` uses them for MMM position plausibility check.
#[allow(async_fn_in_trait)]
pub trait MmmaPreisGasRepository: Send + Sync {
    /// Upsert the Gas MMM price pair for a billing month + Marktgebiet.
    async fn upsert_gas(
        &self,
        price_month: time::Date,
        marktgebiet: &str,
        mehr_ct_kwh: rust_decimal::Decimal,
        minder_ct_kwh: rust_decimal::Decimal,
        source: &str,
    ) -> Result<(), MdmError>;

    /// Return the Gas MMM prices for a billing month. Returns `None` if not yet imported.
    async fn find_gas(
        &self,
        price_month: time::Date,
        marktgebiet: &str,
    ) -> Result<Option<MmmaPreisGasRecord>, MdmError>;

    /// List all Gas MMM price records, newest first.
    async fn list_gas(&self, limit: i64) -> Result<Vec<MmmaPreisGasRecord>, MdmError>;
}

// ── Strom Mehr-/Mindermengenpreise (§ 13 Abs. 3 StromNZV) ────────────────────

/// The nationwide Strom Mehr-/Mindermengenpreise for one application month.
///
/// § 13 Abs. 3 StromNZV requires *einheitliche* prices computed from monthly
/// market prices; the BDEW determines and publishes them centrally as one
/// series for the whole German market, with a Mehr and a Minder value per
/// month. There is deliberately no operator dimension here — every
/// Netzbetreiber settles against the same published values.
///
/// Read by `netzbilanzd` (INVOIC 31002/31005) and `invoicd` (MMM check 6).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MmmPreisStromRecord {
    /// First day of the application month.
    pub price_month: time::Date,
    /// Surplus price (Mehrmengen) in ct/kWh.
    pub mehr_ct_kwh: rust_decimal::Decimal,
    /// Deficit price (Mindermengen) in ct/kWh.
    pub minder_ct_kwh: rust_decimal::Decimal,
    pub source: String,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Read/write access to the Strom Mehr-/Mindermengenpreise.
#[allow(async_fn_in_trait)]
pub trait MmmPreisStromRepository: Send + Sync {
    async fn upsert_strom(
        &self,
        price_month: time::Date,
        mehr_ct_kwh: rust_decimal::Decimal,
        minder_ct_kwh: rust_decimal::Decimal,
        source: &str,
    ) -> Result<(), MdmError>;

    async fn find_strom(
        &self,
        price_month: time::Date,
    ) -> Result<Option<MmmPreisStromRecord>, MdmError>;
}

// ── NB Energiemix (§42 EnWG annual grid-area renewable mix) ─────────────────

/// A stored `NbEnergiemix` record.
///
/// The NB publishes the annual renewable energy mix of their grid area under
/// §42 Abs. 5 EnWG.  Lieferanten use this to compute the Reststrommix
/// for customer bills and to label Ökostrom tariffs in `productd`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NbEnergiemixRecord {
    /// 13-digit BDEW/DVGW/GS1 NB MP-ID.
    pub nb_mp_id: String,
    /// Calendar year this mix is valid for (e.g. `2025`).
    pub gueltig_fuer: i16,
    /// Full `rubo4e::current::Energiemix` COM payload (JSONB, camelCase).
    pub energiemix: serde_json::Value,
    /// Total EEG feed-in into this grid area in kWh (optional informational).
    pub eeg_einspeisung_kwh: Option<i64>,
    /// Total grid withdrawal (`Gesamtentnahme`) in kWh.
    pub gesamtentnahme_kwh: Option<i64>,
    /// Wall-clock time (UTC) when this record was last updated.
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub updated_at: Option<time::OffsetDateTime>,
}

/// Read/write access to NB annual grid-area Energiemix (§42 EnWG).
#[allow(async_fn_in_trait)]
pub trait NbEnergiemixRepository: Send + Sync {
    /// Upsert the annual Energiemix for an NB.
    ///
    /// Idempotent: re-publishing the same year with updated values replaces
    /// the existing row.
    async fn upsert_energiemix(
        &self,
        tenant: &str,
        nb_mp_id: &str,
        gueltig_fuer: i16,
        energiemix: serde_json::Value,
        eeg_einspeisung_kwh: Option<i64>,
        gesamtentnahme_kwh: Option<i64>,
    ) -> Result<(), MdmError>;

    /// Return the `NbEnergiemix` for the given NB and year.
    ///
    /// When `year` is `None`, returns the most recent available year.
    async fn find_energiemix(
        &self,
        tenant: &str,
        nb_mp_id: &str,
        year: Option<i16>,
    ) -> Result<Option<NbEnergiemixRecord>, MdmError>;

    /// Return all available years for a given NB (for history/audit).
    async fn list_energiemix_years(
        &self,
        tenant: &str,
        nb_mp_id: &str,
    ) -> Result<Vec<i16>, MdmError>;
}

// ── ESA consent registry (§49 Abs. 2 Nr. 9 MsbG) ──────────────────────────────

/// One ESA consent (Einwilligung) — the ESA's lawful basis for holding a
/// location's metering values (§49 Abs. 2 Nr. 9 MsbG, GDPR Art. 7).
///
/// Evidence-agnostic: `evidence_uri`/`evidence_hash` are stored verbatim and
/// never validated for form (BNetzA forbids rejecting consent for deviating
/// from the BDEW template).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EinwilligungRecord {
    #[serde(default)]
    pub id: Uuid,
    #[serde(default)]
    pub tenant: String,
    /// Opaque reference to the Anschlussnutzer (no PII stored here).
    pub anschlussnutzer_ref: String,
    /// MP-ID of the ESA the consent authorises.
    pub esa_mp_id: String,
    /// Locations (MaLo/MeLo/NeLo/ZPB) the consent covers.
    pub location_ids: Vec<String>,
    #[serde(default = "default_scope")]
    pub scope: String,
    #[serde(default = "unix_epoch", with = "time::serde::rfc3339")]
    pub granted_at: time::OffsetDateTime,
    #[serde(with = "date_iso")]
    pub valid_from: Date,
    #[serde(default, with = "date_iso::opt")]
    pub valid_to: Option<Date>,
    /// GDPR Art. 7(3): non-`None` once revoked.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<time::OffsetDateTime>,
    /// Opaque evidence pointer/hash — stored verbatim, never form-validated.
    #[serde(default)]
    pub evidence_uri: Option<String>,
    #[serde(default)]
    pub evidence_hash: Option<String>,
}

fn default_scope() -> String {
    "werte".to_owned()
}

/// Bilateral EDI@Energy framework agreement + AS4 cert state (MSB ↔ ESA).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EsaFrameworkAgreement {
    #[serde(default)]
    pub tenant: String,
    pub msb_mp_id: String,
    pub esa_mp_id: String,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub signed_at: Option<time::OffsetDateTime>,
    #[serde(default)]
    pub edi_agreement: bool,
    #[serde(default = "default_cert_state")]
    pub cert_state: String,
}

fn default_cert_state() -> String {
    "pending".to_owned()
}

/// Which side of the ESA relationship is gating a message.
///
/// The consent has **asymmetric** force. The MSB holds only the ESA's
/// self-assertion, so a missing record is not its problem to reject. The ESA is
/// the data controller that obtained the Einwilligung, so for the ESA a missing
/// record means no lawful basis at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsentPerspective {
    /// MSB *receiving* an inbound ESA order. Lenient: a missing consent record
    /// is self-assertion and never blocks (BNetzA forbids form-based rejection).
    #[default]
    MsbInbound,
    /// ESA *originating* an outbound request (Werteanfrage/Bestellung), or about
    /// to hold values. Strict: the ESA must hold a recorded, non-revoked consent
    /// — a missing record is no lawful basis (GDPR Art. 7), so it blocks.
    EsaOutbound,
}

/// Why an ESA-message consent check allowed or blocked delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsentCode {
    /// An active, non-revoked consent covers the location — deliver.
    Active,
    /// No consent record for the location, seen from the **MSB** side. The ESA's
    /// self-assertion stands and BNetzA Mitteilung Nr. 3 (07.02.2024) forbids
    /// rejecting on consent *form*, so absence alone never blocks — deliver.
    SelfAssertion,
    /// No consent record for the location, seen from the **ESA** side. The ESA
    /// holds no lawful basis (GDPR Art. 7) and must not originate the request —
    /// block.
    NoConsent,
    /// A recorded consent for the location has been revoked (GDPR Art. 7(3)) and
    /// no active consent superseded it — block (the Widerruf clearing case).
    Revoked,
    /// A framework agreement exists but is not established (no EDI agreement or a
    /// negative cert state) — the UC 4.1.1 Vorbedingung is unmet, so block.
    FrameworkRejected,
}

impl ConsentCode {
    /// Whether this outcome permits the message.
    #[must_use]
    pub const fn allowed(self) -> bool {
        matches!(self, Self::Active | Self::SelfAssertion)
    }
}

/// Outcome of gating an inbound ESA message against the consent registry.
///
/// Absence of a consent record is **not** a block: the MSB holds the ESA's
/// self-assertion and BNetzA forbids rejecting on form. Only an explicit
/// negative signal — a revoked consent, or a framework agreement that is on
/// record but not established — blocks delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentDecision {
    /// `true` when the inbound ESA message may be processed.
    pub allowed: bool,
    /// Machine-readable reason.
    pub code: ConsentCode,
    /// Human-readable reason (also used verbatim as the Ablehnung Begründung).
    pub reason: String,
}

impl ConsentDecision {
    /// Build a decision from a [`ConsentCode`], filling in the standard reason.
    #[must_use]
    pub fn from_code(code: ConsentCode) -> Self {
        let reason = match code {
            ConsentCode::Active => "aktive Einwilligung liegt vor",
            ConsentCode::SelfAssertion => {
                "keine Einwilligung erfasst — Zusicherung des ESA gilt (keine Formprüfung)"
            }
            ConsentCode::NoConsent => {
                "keine Einwilligung erfasst — der ESA hat keine Rechtsgrundlage (GDPR Art. 7)"
            }
            ConsentCode::Revoked => {
                "Einwilligung wurde widerrufen (GDPR Art. 7 Abs. 3) — keine Belieferung"
            }
            ConsentCode::FrameworkRejected => {
                "Rahmenvertrag/EDI-Vereinbarung nicht etabliert (Vorbedingung UC 4.1.1)"
            }
        };
        Self {
            allowed: code.allowed(),
            code,
            reason: reason.to_owned(),
        }
    }
}

/// Registry of ESA consents and framework agreements (`esa_einwilligungen`,
/// `esa_framework_agreements`).
#[allow(async_fn_in_trait)]
pub trait EinwilligungRepository: Send + Sync {
    /// Grant a consent, superseding any active consent for the same
    /// `(tenant, esa, Anschlussnutzer)`. Returns the new consent id.
    async fn grant(&self, rec: EinwilligungRecord) -> Result<Uuid, MdmError>;

    /// Fetch a consent by id (tenant-scoped).
    async fn get(&self, tenant: &str, id: Uuid) -> Result<Option<EinwilligungRecord>, MdmError>;

    /// List active (non-revoked) consents for an ESA.
    async fn list_for_esa(
        &self,
        tenant: &str,
        esa_mp_id: &str,
    ) -> Result<Vec<EinwilligungRecord>, MdmError>;

    /// Revoke a consent (Art. 7(3)). Returns the revoked record when it existed
    /// and was still active, so the caller can fire the 17008 Abbestellung.
    async fn revoke(&self, tenant: &str, id: Uuid) -> Result<Option<EinwilligungRecord>, MdmError>;

    /// Upsert a framework agreement.
    async fn upsert_framework(&self, rec: EsaFrameworkAgreement) -> Result<(), MdmError>;

    /// Fetch a framework agreement.
    async fn get_framework(
        &self,
        tenant: &str,
        msb_mp_id: &str,
        esa_mp_id: &str,
    ) -> Result<Option<EsaFrameworkAgreement>, MdmError>;

    /// Gate an ESA message for `location_id` against the registry.
    ///
    /// A revoked consent or an unestablished framework agreement always blocks.
    /// A *missing* consent record depends on `perspective`: lenient
    /// ([`ConsentPerspective::MsbInbound`]) treats it as self-assertion and
    /// allows; strict ([`ConsentPerspective::EsaOutbound`]) treats it as no
    /// lawful basis and blocks.
    async fn consent_check(
        &self,
        tenant: &str,
        esa_mp_id: &str,
        msb_mp_id: &str,
        location_id: &str,
        perspective: ConsentPerspective,
    ) -> Result<ConsentDecision, MdmError>;
}

// ── §20b EnWG Netzzugangsplattform ────────────────────────────────────────────

/// Use case of a §20b EnWG Netzzugangsplattform request (Abs. 2 Nr. 1–3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetzzugangAntragTyp {
    /// §20b Abs. 2 Nr. 1 — Zählpunktanordnung (umgangssprachlich Messkonzept)
    /// hinter einem Netzanschluss.
    Zaehlpunktanordnung,
    /// §20b Abs. 2 Nr. 2 — Verrechnungskonzept (Verrechnungsformel) hinter
    /// einem Netzanschluss.
    Verrechnungskonzept,
    /// §20b Abs. 2 Nr. 3 — Registrierung einer Energy-Sharing-Vereinbarung
    /// nach §42c EnWG.
    EnergySharingVereinbarung,
}

impl NetzzugangAntragTyp {
    /// Stable snake_case string used in SQL CHECK constraints and JSON.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Zaehlpunktanordnung => "zaehlpunktanordnung",
            Self::Verrechnungskonzept => "verrechnungskonzept",
            Self::EnergySharingVereinbarung => "energysharing_vereinbarung",
        }
    }
}

/// Action on a §20b request. `Registrierung` applies only to
/// [`NetzzugangAntragTyp::EnergySharingVereinbarung`]; the other two use cases
/// carry the statutory Bestellung/Änderung/Abbestellung triple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetzzugangAktion {
    /// Erstmalige Bestellung.
    Bestellung,
    /// Änderung.
    Aenderung,
    /// Abbestellung.
    Abbestellung,
    /// Registrierung (§42c-Vereinbarung only).
    Registrierung,
}

impl NetzzugangAktion {
    /// Stable snake_case string used in SQL CHECK constraints and JSON.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bestellung => "bestellung",
            Self::Aenderung => "aenderung",
            Self::Abbestellung => "abbestellung",
            Self::Registrierung => "registrierung",
        }
    }
}

/// Lifecycle state of a §20b request as tracked by the projection.
///
/// `Erfasst` on command acceptance, `Uebermittelt` once the makod outbox
/// sender delivered it (to the platform endpoint or, while none exists, to the
/// operator's ERP webhook for manual submission via the NB Webportal),
/// `Bestaetigt`/`Abgelehnt` when the answer arrives, `Fehlgeschlagen` when
/// delivery exhausted its retries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetzzugangStatus {
    /// Recorded; not yet delivered.
    Erfasst,
    /// Delivered to the platform endpoint or handed to the operator.
    Uebermittelt,
    /// Confirmed by the platform / Netzbetreiber.
    Bestaetigt,
    /// Rejected by the platform / Netzbetreiber.
    Abgelehnt,
    /// Delivery failed permanently.
    Fehlgeschlagen,
}

impl NetzzugangStatus {
    /// Stable snake_case string used in SQL CHECK constraints and JSON.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Erfasst => "erfasst",
            Self::Uebermittelt => "uebermittelt",
            Self::Bestaetigt => "bestaetigt",
            Self::Abgelehnt => "abgelehnt",
            Self::Fehlgeschlagen => "fehlgeschlagen",
        }
    }
}

/// A §20b EnWG Netzzugangsplattform request (Antrag) and its lifecycle state.
///
/// The platform itself does not exist yet (no BNetzA Festlegung under §20b
/// Abs. 3 as of 2026-07); the record is transport-agnostic: the payload is the
/// canonical JSON the adapter delivers, `platform_ref` is the platform's
/// reference once one is assigned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetzzugangAntrag {
    #[serde(default)]
    pub id: Uuid,
    #[serde(default)]
    pub tenant: String,
    /// §20b use case.
    pub antrag_typ: NetzzugangAntragTyp,
    /// Action within the use case.
    pub aktion: NetzzugangAktion,
    /// The Netzanschluss the request concerns (operator-scoped identifier).
    pub netzanschluss_id: String,
    /// MP-ID of the responsible Netzbetreiber.
    pub nb_mp_id: String,
    /// Requester on whose behalf the request is made (Anschlussnehmer /
    /// Anschlussnutzer / §20-Anspruchsberechtigter) — opaque reference, no PII.
    pub antragsteller_ref: String,
    /// Lifecycle state.
    #[serde(default = "default_netzzugang_status")]
    pub status: NetzzugangStatus,
    /// Canonical request payload delivered to the platform.
    #[serde(default)]
    pub payload: serde_json::Value,
    /// Reference assigned by the platform, once known.
    #[serde(default)]
    pub platform_ref: Option<String>,
    #[serde(default = "unix_epoch", with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub submitted_at: Option<time::OffsetDateTime>,
}

fn default_netzzugang_status() -> NetzzugangStatus {
    NetzzugangStatus::Erfasst
}

/// Registry of §20b EnWG Netzzugangsplattform requests.
#[allow(async_fn_in_trait)]
pub trait NetzzugangRepository: Send + Sync {
    /// Insert or update a request by id (tenant-scoped). Returns the id.
    async fn upsert(&self, rec: NetzzugangAntrag) -> Result<Uuid, MdmError>;

    /// Fetch a request by id (tenant-scoped).
    async fn get(&self, tenant: &str, id: Uuid) -> Result<Option<NetzzugangAntrag>, MdmError>;

    /// List requests, optionally filtered by status and/or Netzanschluss.
    async fn list(
        &self,
        tenant: &str,
        status: Option<NetzzugangStatus>,
        netzanschluss_id: Option<&str>,
    ) -> Result<Vec<NetzzugangAntrag>, MdmError>;

    /// Update lifecycle state (and optionally the platform reference).
    /// Returns the updated record when it existed.
    async fn set_status(
        &self,
        tenant: &str,
        id: Uuid,
        status: NetzzugangStatus,
        platform_ref: Option<String>,
    ) -> Result<Option<NetzzugangAntrag>, MdmError>;
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod partner_record_tests {
    use super::*;

    fn sample_partner() -> PartnerRecord {
        PartnerRecord {
            mp_id: "9900357000004".parse().expect("valid MP-ID"),
            display_name: Some("Stadtwerke Musterstadt Netz GmbH".to_owned()),
            marktrolle: Some(rubo4e::current::Marktrolle::Nb),
            sparte: Some(Sparte::Strom),
            rollencodetyp: Some(rubo4e::current::Rollencodetyp::Bdew),
            makoadresse: vec!["https://as4.musterstadt.example/msh".to_owned()],
            channels: serde_json::json!({}),
            version: 1,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
        }
    }

    /// The typed enums keep the wire format the TEXT columns and existing API
    /// clients rely on: bare BDEW codes.
    #[test]
    fn typed_enums_stay_string_compatible() {
        let p = sample_partner();
        let json = serde_json::to_value(&p).expect("serialise");
        assert_eq!(json["marktrolle"], "NB");
        assert_eq!(json["rollencodetyp"], "BDEW");

        let round: PartnerRecord = serde_json::from_value(json).expect("deserialise");
        assert_eq!(round.marktrolle, Some(rubo4e::current::Marktrolle::Nb));
        assert_eq!(
            round.rollencodetyp,
            Some(rubo4e::current::Rollencodetyp::Bdew)
        );

        // strum Display matches the serde repr — the PG TEXT binding uses it.
        assert_eq!(rubo4e::current::Marktrolle::Nb.to_string(), "NB");
        assert_eq!(rubo4e::current::Rollencodetyp::Gln.to_string(), "GLN");
        assert_eq!("LF".parse(), Ok(rubo4e::current::Marktrolle::Lf));
    }

    /// `to_marktteilnehmer` maps every stored field into the BO4E shape.
    #[test]
    fn to_marktteilnehmer_maps_all_fields() {
        let p = sample_partner();
        let mt = p.to_marktteilnehmer();

        assert_eq!(
            mt.rollencodenummer.as_ref().map(ToString::to_string),
            Some("9900357000004".to_owned())
        );
        assert_eq!(mt.marktrolle, Some(rubo4e::current::Marktrolle::Nb));
        assert_eq!(mt.rollencodetyp, Some(rubo4e::current::Rollencodetyp::Bdew));
        assert_eq!(mt.sparte, Some(rubo4e::current::Sparte::Strom));
        assert_eq!(
            mt.makoadresse,
            Some(vec!["https://as4.musterstadt.example/msh".to_owned()])
        );
        assert_eq!(
            mt.geschaeftspartner
                .as_ref()
                .and_then(|g| g.organisationsname.clone()),
            Some("Stadtwerke Musterstadt Netz GmbH".to_owned())
        );
        // The BO discriminator is set by the type's Default.
        assert_eq!(mt.typ, Some(rubo4e::current::BoTyp::Marktteilnehmer));

        // An empty makoadresse list is omitted, not serialised as [].
        let mut bare = sample_partner();
        bare.makoadresse.clear();
        bare.display_name = None;
        let mt = bare.to_marktteilnehmer();
        assert_eq!(mt.makoadresse, None);
        assert!(mt.geschaeftspartner.is_none());
    }
}

// ── MabisZpRecord ─────────────────────────────────────────────────────────────

/// The MaBiS-Zählpunkt a Bilanzierungsgebiet's Summenzeitreihen are filed under.
///
/// MSCONS Summenzeitreihen (PIDs 13003/13023) carry three distinct SG6 `LOC`
/// qualifiers: `172` the **Meldepunkt** (this MaBiS-Zählpunkt), `107` the
/// Bilanzierungsgebiet, and `237` the Bilanzkreis. They are different
/// identifiers with different meanings, and both are free text at the MIG level
/// — so filing a Summenzeitreihe under the wrong Meldepunkt produces a message
/// that parses, validates, and is indistinguishable to the BIKO from a correct
/// one.
///
/// Holding the mapping as master data rather than service configuration is what
/// lets a territory without an assignment fail loudly at submission time instead
/// of silently substituting the Bilanzierungsgebiet EIC.
///
/// There is deliberately no `sparte`: MaBiS is the *Marktregeln für die
/// Durchführung der Bilanzkreisabrechnung **Strom***. Gas balancing runs under
/// GaBi Gas, which has no MaBiS-Zählpunkt, so a Gas row described a thing that
/// does not exist and invited an operator to record one.
///
/// Regulatory basis: **BNetzA BK6-24-174 Anlage 3 (MaBiS)**; MSCONS AHB 3.2 SG6.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MabisZpRecord {
    /// Bilanzierungsgebiet-EIC this assignment is keyed on (16 characters).
    pub bilanzierungsgebiet: String,
    /// The MaBiS-Zählpunkt filed as `LOC+172` for this territory.
    pub mabis_zp_id: String,
    /// Where the assignment came from: `manual`, `erp`, or an import name.
    pub source: String,
    /// Deployment tenant.
    pub tenant: String,
    /// Last write time, set by the repository.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Read/write access to the Bilanzierungsgebiet → MaBiS-Zählpunkt assignments.
#[allow(async_fn_in_trait)]
pub trait MabisZpRepository: Send + Sync {
    /// Insert or replace the assignment for a Bilanzierungsgebiet.
    ///
    /// Idempotent; `updated_at` is set by the implementation.
    #[must_use]
    async fn upsert(&self, rec: MabisZpRecord) -> Result<(), MdmError>;

    /// Return the assignment for a Bilanzierungsgebiet, or `None`.
    ///
    /// `None` is the signal to refuse the submission — never to fall back to the
    /// Bilanzierungsgebiet EIC.
    #[must_use]
    async fn find(
        &self,
        bilanzierungsgebiet: &str,
        tenant: &str,
    ) -> Result<Option<MabisZpRecord>, MdmError>;

    /// Every assignment for a tenant, ascending by Bilanzierungsgebiet.
    #[must_use]
    async fn list(&self, tenant: &str) -> Result<Vec<MabisZpRecord>, MdmError>;
}

#[cfg(test)]
mod lokationsbuendel_tests {
    use std::collections::HashMap;

    use super::*;

    fn edge(
        von: &str,
        von_typ: Lokationstyp,
        nach: &str,
        nach_typ: Lokationstyp,
    ) -> LokationszuordnungEdge {
        LokationszuordnungEdge {
            id: uuid::Uuid::nil(),
            tenant: "t".to_owned(),
            von_id: von.to_owned(),
            von_typ,
            nach_id: nach.to_owned(),
            nach_typ,
            valid_from: None,
            valid_to: None,
            lokationsbuendelcode: Some("9992000000125".to_owned()),
            data: serde_json::json!({}),
            depth: 0,
        }
    }

    /// `von_typ`/`nach_typ` are the BO4E `Lokationstyp` and stay wire-compatible
    /// with the canonical uppercase codes the TEXT column and API rely on.
    #[test]
    fn edge_typ_is_bo4e_lokationstyp() {
        let e = edge("MALO1", Lokationstyp::Malo, "MELO1", Lokationstyp::Melo);
        let json = serde_json::to_value(&e).expect("serialise");
        assert_eq!(json["von_typ"], "MALO");
        assert_eq!(json["nach_typ"], "MELO");
        assert_eq!(<&'static str>::from(Lokationstyp::Nelo), "NELO");
        assert_eq!("SR".parse(), Ok(Lokationstyp::Sr));
    }

    /// A bundle projects every non-root node by type and de-duplicates.
    #[test]
    fn from_graph_projects_nodes_by_type() {
        let edges = vec![
            edge("MALO1", Lokationstyp::Malo, "MELO1", Lokationstyp::Melo),
            edge("MALO1", Lokationstyp::Malo, "MELO2", Lokationstyp::Melo),
            edge("MELO1", Lokationstyp::Melo, "NELO1", Lokationstyp::Nelo),
            edge("MELO1", Lokationstyp::Melo, "SR1", Lokationstyp::Sr),
            edge("SR1", Lokationstyp::Sr, "TR1", Lokationstyp::Tr),
            // duplicate edge must not double-count
            edge("MALO1", Lokationstyp::Malo, "MELO1", Lokationstyp::Melo),
        ];
        let b = Lokationsbuendel::from_graph("MALO1", &edges);
        assert_eq!(b.malo_id, "MALO1");
        assert_eq!(b.lokationsbuendelcode.as_deref(), Some("9992000000125"));
        assert_eq!(b.messlokationen, vec!["MELO1", "MELO2"]);
        assert_eq!(b.netzlokationen, vec!["NELO1"]);
        assert_eq!(b.steuerbare_ressourcen, vec!["SR1"]);
        assert_eq!(b.technische_ressourcen, vec!["TR1"]);
        b.validate().expect("bundle with a MeLo is valid");
    }

    /// A consuming MaLo with no MeLo violates the structural invariant.
    #[test]
    fn validate_requires_at_least_one_melo() {
        let edges = vec![edge(
            "MALO1",
            Lokationstyp::Malo,
            "NELO1",
            Lokationstyp::Nelo,
        )];
        let b = Lokationsbuendel::from_graph("MALO1", &edges);
        assert!(b.messlokationen.is_empty());
        assert!(matches!(
            b.validate(),
            Err(BuendelError::NoMesslokation { .. })
        ));
    }

    /// All MeLos of one MaLo must share a single MSB.
    #[test]
    fn validate_msb_consistency_flags_divergent_msb() {
        let edges = vec![
            edge("MALO1", Lokationstyp::Malo, "MELO1", Lokationstyp::Melo),
            edge("MALO1", Lokationstyp::Malo, "MELO2", Lokationstyp::Melo),
        ];
        let b = Lokationsbuendel::from_graph("MALO1", &edges);

        // Same MSB → ok (unassigned MeLos are ignored).
        let mut consistent = HashMap::new();
        consistent.insert("MELO1".to_owned(), Some("MSB_A".to_owned()));
        consistent.insert("MELO2".to_owned(), None);
        b.validate_msb_consistency(&consistent)
            .expect("single MSB is consistent");

        // Two distinct MSBs → error.
        let mut divergent = HashMap::new();
        divergent.insert("MELO1".to_owned(), Some("MSB_A".to_owned()));
        divergent.insert("MELO2".to_owned(), Some("MSB_B".to_owned()));
        assert!(matches!(
            b.validate_msb_consistency(&divergent),
            Err(BuendelError::DivergentMsb { .. })
        ));
    }
}

#[cfg(test)]
mod wire_format_guard {
    //! Timestamps on this API are RFC 3339, and the default is not.
    //!
    //! With the workspace's `time` features, a bare `time::OffsetDateTime`
    //! field serialises as `"2026-01-01 00:00:00.0 +00:00:00"` — a space
    //! instead of `T`, an explicit `+00:00:00` offset instead of `Z`, and a
    //! trailing `.0`. It is not RFC 3339 and most clients will not parse it,
    //! yet it looks close enough in a log to pass review. Every record here
    //! therefore carries `#[serde(with = "time::serde::rfc3339")]`, and this
    //! test fails if a new field forgets it.

    #[test]
    fn the_default_time_format_is_not_rfc_3339() {
        // Pins the premise: if `time` ever changes its default to RFC 3339,
        // this test is the place that says the annotations became redundant.
        let t = time::OffsetDateTime::from_unix_timestamp(1_767_225_600).expect("valid instant");
        let raw = serde_json::to_string(&t).expect("serialise");
        assert_eq!(raw, "\"2026-01-01 00:00:00.0 +00:00:00\"");
    }

    #[test]
    fn every_offsetdatetime_field_declares_the_rfc_3339_format() {
        let src = include_str!("repository.rs");
        let lines: Vec<&str> = src.lines().collect();
        let mut offenders = Vec::new();

        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if !trimmed.starts_with("pub ") || !trimmed.contains("time::OffsetDateTime") {
                continue;
            }
            // Walk back over doc comments and attributes to find a serde one.
            let annotated = lines[..i]
                .iter()
                .rev()
                .take_while(|l| {
                    let t = l.trim();
                    t.starts_with('#') || t.starts_with("///") || t.starts_with("//")
                })
                .any(|l| l.contains("time::serde::rfc3339"));
            if !annotated {
                offenders.push(format!("line {}: {trimmed}", i + 1));
            }
        }

        assert!(
            offenders.is_empty(),
            "these OffsetDateTime fields would serialise in `time`'s own non-RFC-3339 \
             format; add #[serde(with = \"time::serde::rfc3339\")] (or `::option`):\n  {}",
            offenders.join("\n  ")
        );
    }
}

#[cfg(test)]
mod netznutzer_typ_tests {
    use super::NetznutzerTyp;

    /// The DB tokens and the enum are one mapping, in both directions.
    #[test]
    fn the_db_token_round_trips() {
        for t in [NetznutzerTyp::Lieferant, NetznutzerTyp::Letztverbraucher] {
            assert_eq!(NetznutzerTyp::from_db_str(t.as_db_str()), Some(t));
        }
    }

    /// An unknown token is refused, not read as the ordinary case: a Selbstzahler
    /// silently downgraded to `Lieferant` goes back onto the automated
    /// Lieferantenwechsel path the flag exists to keep it off.
    #[test]
    fn an_unknown_token_is_refused() {
        assert_eq!(NetznutzerTyp::from_db_str("GROSSKUNDE"), None);
        assert_eq!(NetznutzerTyp::default(), NetznutzerTyp::Lieferant);
        assert!(!NetznutzerTyp::default().is_selbstzahler());
        assert!(NetznutzerTyp::Letztverbraucher.is_selbstzahler());
    }

    /// The wire form is the DB token, so a `marktd` response and a DB row read
    /// the same.
    #[test]
    fn the_json_form_is_the_db_token() {
        let json = serde_json::to_string(&NetznutzerTyp::Letztverbraucher).unwrap();
        assert_eq!(json, "\"LETZTVERBRAUCHER\"");
        let back: NetznutzerTyp = serde_json::from_str(&json).unwrap();
        assert_eq!(back, NetznutzerTyp::Letztverbraucher);
    }
}

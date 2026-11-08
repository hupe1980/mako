#![expect(
    clippy::doc_markdown,
    reason = "a pedantic style lint this crate does not adopt"
)]
//! Record, request and filter types for every `marktd` aggregate.
//!
//! The storage is marktd's: each `Pg*Repository` in `services/marktd/src/pg/`
//! carries its methods inherently.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use time::Date;
use uuid::Uuid;

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

    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "serde's `serialize_with` fixes the signature to a reference"
    )]
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

        #[expect(
            clippy::trivially_copy_pass_by_ref,
            clippy::ref_option,
            reason = "serde's `serialize_with` fixes the signature to `&Option<T>`"
        )]
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
    /// Role code the assignment is for (e.g. `NB`, `GNB`, `MSB`, `LF`).
    pub zuordnungstyp: String,
    /// 13-digit MP-ID of the Marktpartner holding the role.
    pub rollencodenummer: String,
    /// First day the assignment holds (inclusive).
    #[serde(with = "date_iso")]
    pub valid_from: Date,
    /// Day the assignment ends (exclusive); `None` = open-ended.
    #[serde(default, with = "date_iso::opt")]
    pub valid_to: Option<Date>,
}

/// Stored MaLo record as returned by repository reads.
///
/// `rollenzuordnung` contains only the role assignments valid at the
/// `at` date passed to `PgMaloRepository::find` / `PgMaloRepository::list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaloRecord {
    /// 11-digit MaLo-ID the record is keyed on.
    pub malo_id: MaloId,
    /// Whether the Marktlokation is Strom or Gas.
    pub sparte: Sparte,
    /// Voltage/pressure level — BO4E `Netzebene` wire value (`NSP`/`MSP`/`HSP`/`HSS`
    /// and their `*_UMSP` transformation levels for Strom; `HD`/`MD`/`ND` for Gas).
    /// `None` when the incoming BO4E payload did not carry the field.
    pub netzebene: Option<String>,
    /// Bilanzierungsgebiet EIC code extracted from the BO4E `Marktlokation`.
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
    /// NZR-EMob **Abwicklungsmodell** — BO4E `Abwicklungsmodell` wire value
    /// (`MODELL_1` | `MODELL_2`).
    ///
    /// Extracted from UTILMD SG10 `CCI+ZA2` DE 7037: `ZE9` („Bilanzierung an
    /// der Marktlokation") → `MODELL_1`, `ZF0` („Bilanzierung im
    /// Bilanzierungsgebiet (BG) des LPB") → `MODELL_2`. `None` when no
    /// counterparty has stated one, which is **not** the same as `MODELL_1`:
    /// the default is unknown, and a Modell-2 MaLo is no longer balanced by the
    /// VNB at all (BK6-20-160 Anlage 6 §II).
    ///
    /// The Klassentyp `ZA2` is part of the read — `ZE9` under another
    /// Klassentyp is „Quartalsweise" in the same AHB.
    #[serde(default)]
    pub abwicklungsmodell: Option<String>,
    /// Optimistic-concurrency version, incremented on every write. The ETag is derived from it.
    pub version: i64,
    /// Full BO4E `Marktlokation` payload.
    pub data: MaloPayload,
    /// Role assignments valid at the requested reference date.
    pub rollenzuordnung: Vec<Rollenzuordnung>,
    /// Time of the last write (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
    /// BO4E schema version of the `data` payload (e.g. `"202607.1.0"`).
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
}

/// Stored MeLo record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeloRecord {
    /// MeLo-ID (`DE` + 31 alphanumeric characters) the record is keyed on.
    pub melo_id: MeloId,
    /// MaLo this Messlokation belongs to; `None` when none is linked.
    pub malo_id: Option<MaloId>,
    /// Voltage/pressure level at the metering point, extracted from `Messlokation.netzebene_messung`.
    pub netzebene_messung: Option<String>,
    /// Regelzone EIC code extracted from
    /// `Messlokation.standorteigenschaften.eigenschaftenStrom[0].regelzoneEic`
    /// — the EIC, not the sibling `regelzone`, which is the Regelzone's *name*.
    ///
    /// Maps this MeLo to the ÜNB (Transmission System Operator) for:
    /// - Redispatch 2.0 `Stammdaten` forwarding (VNB → ÜNB)
    /// - MABIS IFTSTA 21000 routing (Bilanzkreisabrechnung Strom, BKV↔ÜNB)
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
    /// Optimistic-concurrency version, incremented on every write.
    pub version: i64,
    /// Full BO4E `Messlokation` payload.
    pub data: MeloPayload,
    /// Time of the last write (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
    /// BO4E schema version of the `data` payload.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
}

/// Stored webhook subscription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    /// Key the subscription is stored and addressed under.
    pub subscriber_id: String,
    /// Endpoint the fan-out worker POSTs each matching [`crate::cloudevents::MarktEvent`] to.
    pub webhook_url: String,
    /// The Standard Webhooks signing key, stored as plaintext — protect it with
    /// database least-privilege and storage encryption (marktd README).
    pub webhook_secret: Option<String>,
    /// Empty = all roles.
    pub roles: Vec<String>,
    /// Empty = all event types.
    pub event_types: Vec<String>,
    /// Empty = all Sparten.
    pub sparten: Vec<String>,
    /// Whether events are delivered to this subscriber.
    pub active: bool,
    /// Optimistic-concurrency version, incremented on every write.
    pub version: i64,
}

/// Stored trading-partner record, keyed by its 13-digit `MarktpartnerId`
/// (Rollencodenummer) — a BDEW-Codenummer, DVGW-Codenummer or GS1 GLN; use
/// [`crate::domain::nad_agency_code`] for the coding authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartnerRecord {
    /// 13-digit Marktpartner-ID.
    pub mp_id: MarktpartnerId,
    /// Human-readable partner name; served as `geschaeftspartner.organisationsname` by [`Self::to_marktteilnehmer`].
    pub display_name: Option<String>,
    /// BO4E market role (serialises as the BDEW code, e.g. `"LF"`, `"NB"`, `"MSB"`).
    pub marktrolle: Option<rubo4e::current::Marktrolle>,
    /// Sparte the MP-ID is registered for; `None` when not recorded.
    pub sparte: Option<Sparte>,
    /// Coding authority: `BDEW` | `DVGW` | `GLN` (BO4E `Rollencodetyp`).
    /// Derived from the MP-ID prefix; stored for fast AS4 routing lookups.
    pub rollencodetyp: Option<rubo4e::current::Rollencodetyp>,
    /// AS4 endpoint URL list from `Marktteilnehmer.makoadresse`.
    /// Used by `makod` for dynamic AS4 destination routing.
    pub makoadresse: Vec<String>,
    /// The partner's BO4E `Geschaeftspartner`, as stored.
    ///
    /// The AS4 endpoints are [`makoadresse`](Self::makoadresse); a
    /// communication *channel* list belongs to `mako_engine::PartnerRecord`,
    /// which is a different store for a different purpose.
    ///
    /// Opaque on read, like every stored BO4E document in mako — a row may
    /// predate the current schema series. What goes *in* is gated: see
    /// `marktd`'s `PartnerUpsertRequest`.
    #[serde(default)]
    pub geschaeftspartner: serde_json::Value,
    /// The partner's Zustimmung to receiving invoices as EDI (§ 27 Abs. 38
    /// Nr. 1 and 3 UStG); `None` when not recorded.
    #[serde(default)]
    pub edi_zustimmung: Option<bool>,
    /// Whether the EDI format agreed with this partner extracts into an
    /// EN 16931 format, which makes it an E-Rechnung (§ 14 Abs. 1 Satz 6 Nr. 2
    /// UStG); `None` when not recorded.
    #[serde(default)]
    pub edi_extrahierbar: Option<bool>,
    /// Optimistic-concurrency version.
    #[serde(default)]
    pub version: i64,
    /// Time of the last write (UTC).
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
    /// `makod` process ID (WorkflowId) this entry correlates.
    pub process_id: Uuid,
    /// Workflow family name (e.g. `"gpke-supplier-change"`).
    pub workflow_name: Option<String>,
    /// Prüfidentifikator of the process.
    pub pid: Option<i32>,
    /// MaLo the process concerns, when known.
    pub malo_id: Option<MaloId>,
    /// MeLo the process concerns, when known.
    pub melo_id: Option<MeloId>,
    /// Contract the process concerns, when known.
    pub contract_id: Option<String>,
    /// The ERP's own reference for that contract, when supplied.
    pub erp_contract_id: Option<String>,
    /// The ERP's order reference for the command that started the process, when supplied.
    pub erp_order_id: Option<String>,
    /// EDIFACT conversation ID, from the `makoconvid` CloudEvents extension.
    pub edifact_conv_id: Option<Uuid>,
    /// Role the operator holds in the process (`NB`, `LF`, `MSB`, `UNB`, …).
    pub marktrolle: Option<String>,
    /// Format version the process runs under (e.g. `"FV2026-10-01"`).
    pub format_version: Option<String>,
    /// Lifecycle state of the process.
    pub status: ProcessStatus,
    /// When the process started (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub initiated_at: time::OffsetDateTime,
    /// When the process completed or failed (UTC); `None` while it runs.
    #[serde(with = "time::serde::rfc3339::option")]
    pub completed_at: Option<time::OffsetDateTime>,
}

// ── Pagination ────────────────────────────────────────────────────────────────

/// A paged collection returned by list operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageResult<T> {
    /// Rows of the requested page.
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
    /// Only MaLos of this Sparte.
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
    /// Zero-based page index.
    pub page: u32,
    /// Page size.
    pub size: u32,
}

/// Filters for `GET /api/v1/correlations`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CorrelationFilter {
    /// Only entries with this ERP order reference.
    pub erp_order_id: Option<String>,
    /// Only entries concerning this MaLo.
    pub malo_id: Option<MaloId>,
    /// Only entries in this lifecycle state.
    pub status: Option<ProcessStatus>,
}

// ── Traits ────────────────────────────────────────────────────────────────────

/// Read/write access to `MARKTLOKATION` records.
/// Lightweight read model returned by `MarktdClient::get_malo`.
///
/// Contains only the typed fields extracted from the `Marktlokation` JSONB — not
/// the full payload. Used by `processd` NB check 4 (Bilanzierungsgebiet) as the
/// primary source before falling back to the `malo_grid` side table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct MaloTypedFields {
    /// 11-digit MaLo-ID.
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
/// that `marktd` applies via `PgMaloRepository::patch_stammdaten`.
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
    /// NZR-EMob **Abwicklungsmodell** — BO4E `Abwicklungsmodell` wire value
    /// (`MODELL_1` / `MODELL_2`), from `CCI+ZA2++ZE9`/`ZF0`.
    ///
    /// Says whether this Marktlokation is balanced at the MaLo or inside a
    /// Ladepunktbetreiber's Bilanzierungsgebiet (BK6-20-160 Anlage 6). It
    /// arrives on the Stammdatenänderung band and the WiM Anmeldungen, **not**
    /// on the Modellwechsel PIDs 55238–55243 — those move the model, this
    /// records the state a counterparty reports.
    pub abwicklungsmodell: Option<String>,
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
            && self.abwicklungsmodell.is_none()
    }
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
/// patch, and its GPKE-vs-WiM-MSB-Wechsel semantics keep it from being
/// auto-applied here. These fields therefore rarely fire in practice.
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
    /// When the record was first stored (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    /// Time of the last write (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
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
    /// When the record was first stored (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    /// Time of the last write (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
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
    /// BO4E schema version of `data`.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
    /// How this record entered the system.
    pub source: PreisblattSource,
    /// When the record was first stored (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    /// Time of the last write (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

// ── PreisblattDienstleistung (MSB service price sheets) ──────────────────────

/// A stored `PreisblattDienstleistung` record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PreisblattDienstleistungRecord {
    /// MP-ID of the Messstellenbetreiber that published this sheet.
    pub msb_mp_id: String,
    /// Full BO4E `PreisblattDienstleistung` payload (stored as JSONB).
    pub data: serde_json::Value,
    /// BO4E schema version of `data`.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
    /// How this record entered the system: `api` (operator upload) or `mako` (engine ingest).
    pub source: PreisblattSource,
    /// When the record was first stored (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    /// Time of the last write (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

// ── PreisblattHardware (MSB hardware rental price sheets) ────────────────────

/// A stored `PreisblattHardware` record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PreisblattHardwareRecord {
    /// MP-ID of the Messstellenbetreiber that published this sheet.
    pub msb_mp_id: String,
    /// Full BO4E `PreisblattHardware` payload (stored as JSONB).
    pub data: serde_json::Value,
    /// BO4E schema version of `data`.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
    /// How this record entered the system: `api` (operator upload) or `mako` (engine ingest).
    pub source: PreisblattSource,
    /// When the record was first stored (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    /// Time of the last write (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
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
    /// When the record was first stored (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    /// Time of the last write (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// One row in the PRICAT dispatch audit log.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PriCatDispatchEntry {
    /// Surrogate primary key (UUID v4).
    pub id: uuid::Uuid,
    /// The [`PriCatVersion`] that was sent.
    pub pricat_version_id: uuid::Uuid,
    /// MP-ID of the NB whose price sheet was sent.
    pub nb_mp_id: String,
    /// MP-ID of the Lieferant it was sent to.
    pub lf_mp_id: String,
    /// Tenant identifier — data-isolation key written to every database row. Typically the operator's BDEW- or DVGW-Codenummer, but any stable unique string is valid.
    pub tenant: String,
    /// `makod` process ID returned by `MakodClient`, or `None` if dispatch failed.
    pub process_id: Option<uuid::Uuid>,
    /// When the dispatch was attempted (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub dispatched_at: time::OffsetDateTime,
    /// `ok` or `error`.
    pub outcome: String,
    /// Error message when `outcome` is `error`.
    pub error_detail: Option<String>,
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

/// Whether an LF-Zuordnung is announced or running.
///
/// The two states an assignment can be in between an Anmeldung and its
/// Zuordnungsende. There is deliberately no `Beendet`: an assignment that has
/// ended is removed from the live projection and survives only in the
/// `versorgungsstatus_history` snapshot, which is what a point-in-time read
/// resolves against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ZuordnungsStatus {
    /// The Anmeldung is in flight — a UTILMD 55001 / 55077 / 44001 has arrived
    /// and the NB has neither confirmed nor refused it.
    Angekuendigt,
    /// The Zuordnung runs: the NB confirmed it and the Zuordnungsbeginn is
    /// reached or passed.
    Aktiv,
}

impl ZuordnungsStatus {
    /// Wire form, matching the `lf_zuordnung.status` `CHECK`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Angekuendigt => "Angekuendigt",
            Self::Aktiv => "Aktiv",
        }
    }
}

impl std::str::FromStr for ZuordnungsStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Angekuendigt" => Ok(Self::Angekuendigt),
            "Aktiv" => Ok(Self::Aktiv),
            other => Err(format!("unknown ZuordnungsStatus '{other}'")),
        }
    }
}

/// One supplier's hold on a Marktlokation.
///
/// A Marktlokation is **not** held by one Lieferant. A verbrauchende one
/// normally is, at 100 %, but an erzeugende Marktlokation can be *tranchiert* —
/// split into Tranchen that several LFA hold at once, each with its own
/// Aufteilungsfaktor (GPKE Teil 1 Geschäftsvorfall 3). `E_0623` Prüfschritte
/// 500–540 decide such an Anmeldung on the arithmetic over those shares rather
/// than on one supplier's answer, so the assignment has to be a list before
/// four of that tree's six outcomes are reachable at all.
///
/// The same list carries the second thing one slot could not express: an
/// **LFZ** — a supplier whose *future* Zuordnung an incoming Anmeldung
/// displaces — is simply a second [`ZuordnungsStatus::Angekuendigt`] row, which
/// is what 55038 / 44038 „Aufhebung einer zukünftigen Zuordnung" addresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LfZuordnung {
    /// MP-ID of the Lieferant holding this share.
    pub lf_mp_id: String,
    /// The share of the Marktlokation, in percent. `100` for an untranchierte
    /// Marktlokation; the Aufteilungsfaktor of the Tranchengröße product
    /// (`9991000002090`) for a Tranche.
    pub prozent: Decimal,
    /// Tranchen-ID when the Marktlokation is tranchiert (`SG5 LOC+Z21`),
    /// `None` for the single 100 % assignment of an untranchierte one.
    pub tranche_id: Option<String>,
    /// Announced or running.
    pub status: ZuordnungsStatus,
    /// Zuordnungsbeginn — the Lieferbeginn of this assignment.
    #[serde(default, with = "date_iso::opt")]
    pub zuordnungsbeginn: Option<Date>,
    /// Zuordnungsende, once one is agreed.
    #[serde(default, with = "date_iso::opt")]
    pub zuordnungsende: Option<Date>,
    /// `process_id` of the process that wrote this assignment.
    pub process_id: Option<Uuid>,
}

impl LfZuordnung {
    /// The whole Marktlokation, held by one supplier — the untranchierte case.
    #[must_use]
    pub fn ganz(lf_mp_id: impl Into<String>, status: ZuordnungsStatus) -> Self {
        Self {
            lf_mp_id: lf_mp_id.into(),
            prozent: Decimal::ONE_HUNDRED,
            tranche_id: None,
            status,
            zuordnungsbeginn: None,
            zuordnungsende: None,
            process_id: None,
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
    /// Every Lieferant holding a share of this Marktlokation, announced or
    /// running — the single source of truth for who supplies it.
    ///
    /// A list because two regulated cases carry more than one of each: a
    /// **tranchierte** Marktlokation is held by several LFA at once (`E_0623`
    /// Prüfschritte 500–540 decide on the arithmetic over their shares), and an
    /// incoming Anmeldung can displace an **LFZ** whose future Zuordnung is a
    /// second announced assignment (55038 / 44038).
    ///
    /// [`Self::lf_mp_id`] and [`Self::lf_mp_id_next`] read the ordinary
    /// one-supplier case back out of it.
    #[serde(default)]
    pub zuordnungen: Vec<LfZuordnung>,
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
    /// 3-month maximum (§38 Abs. 4 EnWG) enforced by the `processd`
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
    /// SQL query is already scoped by `AppState::tenant`, so the client only
    /// ever sees their own data and the value is implicit from the server config.
    #[serde(skip_serializing, default)]
    pub tenant: String,
    /// Optimistic concurrency version; incremented on each update.
    pub version: i64,
}

impl VersorgungsStatusRecord {
    /// Every running assignment.
    pub fn aktive(&self) -> impl Iterator<Item = &LfZuordnung> {
        self.zuordnungen
            .iter()
            .filter(|z| z.status == ZuordnungsStatus::Aktiv)
    }

    /// Every announced-but-unconfirmed assignment.
    pub fn angekuendigte(&self) -> impl Iterator<Item = &LfZuordnung> {
        self.zuordnungen
            .iter()
            .filter(|z| z.status == ZuordnungsStatus::Angekuendigt)
    }

    /// The active Lieferant of an **untranchierte** Marktlokation.
    ///
    /// `None` when nobody supplies it *and* when several do: a tranchierte
    /// Marktlokation has no single supplier, and answering with an arbitrary
    /// one of them would be worse than answering with nothing. Callers that
    /// have to handle Tranchen read [`Self::aktive`].
    #[must_use]
    pub fn lf_mp_id(&self) -> Option<&str> {
        let mut aktive = self.aktive();
        let first = aktive.next()?;
        aktive.next().is_none().then_some(first.lf_mp_id.as_str())
    }

    /// The announced future Lieferant, when exactly one Anmeldung is pending.
    ///
    /// `None` when several are — see [`Self::lf_mp_id`] for why that is not the
    /// same as „the first one".
    #[must_use]
    pub fn lf_mp_id_next(&self) -> Option<&str> {
        let mut pending = self.angekuendigte();
        let first = pending.next()?;
        pending.next().is_none().then_some(first.lf_mp_id.as_str())
    }

    /// Zuordnungsbeginn of the single running assignment.
    #[must_use]
    pub fn lieferbeginn(&self) -> Option<Date> {
        let mut aktive = self.aktive();
        let first = aktive.next()?;
        aktive.next().is_none().then_some(first.zuordnungsbeginn)?
    }

    /// Announced Lieferbeginn of the single pending assignment.
    #[must_use]
    pub fn lf_next_lieferbeginn(&self) -> Option<Date> {
        let mut pending = self.angekuendigte();
        let first = pending.next()?;
        pending.next().is_none().then_some(first.zuordnungsbeginn)?
    }

    /// Is this Marktlokation held in Tranchen?
    ///
    /// True as soon as any assignment names a Tranche or carries less than the
    /// whole Marktlokation — the condition `E_0623` Prüfschritt 500 reads.
    #[must_use]
    pub fn ist_tranchiert(&self) -> bool {
        self.zuordnungen
            .iter()
            .any(|z| z.tranche_id.is_some() || z.prozent < Decimal::ONE_HUNDRED)
    }

    /// Is an Anmeldung from `lf_mp_id` already pending on this Marktlokation?
    ///
    /// `E_0622` Prüfschritt 70 („Andere Anmeldung in Bearbeitung") asks about
    /// an assignment announced by **someone else**: `marktd` writes the
    /// Anmeldung under evaluation before the decision runs, so the requesting
    /// supplier's own announcement must not refuse it.
    #[must_use]
    pub fn andere_anmeldung_in_bearbeitung(&self, lf_mp_id: &str) -> Option<&LfZuordnung> {
        self.angekuendigte().find(|z| z.lf_mp_id != lf_mp_id)
    }
}

/// Single entry in the supply-state change history of a MaLo.
///
/// Populated by `PgVersorgungsStatusRepository::upsert` — each successful write
/// appends one row to `versorgungsstatus_history`.  Used by
/// `GET /api/v1/versorgung/{malo_id}/history` and the `?at=YYYY-MM-DD`
/// point-in-time query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersorgungsStatusHistoryRecord {
    /// Auto-incremented surrogate key (`BIGSERIAL`).
    pub id: i64,
    /// 11-digit MaLo-ID.
    pub malo_id: MaloId,
    /// Tenant identifier — data-isolation key written to every database row. Typically the operator's BDEW- or DVGW-Codenummer, but any stable unique string is valid.
    pub tenant: String,
    /// Supply state at this snapshot.
    pub lieferstatus: LieferStatus,
    /// The assignment list as it stood, snapshotted whole.
    ///
    /// Denormalised into the history row rather than versioned inside
    /// `lf_zuordnung`, because a point-in-time read wants the state of the
    /// Marktlokation at an instant, and reassembling that from per-assignment
    /// validity would be a second temporal model for the same fact.
    #[serde(default)]
    pub zuordnungen: Vec<LfZuordnung>,
    /// Agreed Lieferende date at this snapshot, if one was set.
    #[serde(default, with = "date_iso::opt")]
    pub lieferende: Option<Date>,
    /// MP-ID of the active Messstellenbetreiber at this snapshot.
    pub msb_mp_id: Option<String>,
    /// MP-ID of the Netzbetreiber responsible for the MaLo.
    pub nb_mp_id: String,
    /// `makod` process whose outcome produced this state.
    pub last_process_id: Option<Uuid>,
    /// Version of the `versorgungsstatus` row that this snapshot captures.
    pub version: i64,
    /// UTC instant when this state became active (set when the upsert commits).
    #[serde(with = "time::serde::rfc3339")]
    pub valid_from: time::OffsetDateTime,
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
    /// Aggregationsverantwortung — BO4E wire values `UENB` / `VNB`.
    ///
    /// **Absent is not "nobody"**, which is why
    /// [`aggregationszustaendigkeit`](Self::aggregationszustaendigkeit) exists
    /// beside it: in Modell 2 the Aggregationsverantwortung *ruht* (AWH to
    /// BK6-20-160 § 1.6.2) and the wire encoding of that is an **absent**
    /// field, indistinguishable here from a payload that simply does not say.
    #[serde(default)]
    pub aggregationsverantwortung: Option<String>,
    /// Which Abwicklungsmodell — BO4E wire values `MODELL_1` / `MODELL_2`.
    ///
    /// Shadowed because it is half of the pair that decides
    /// [`aggregationszustaendigkeit`](Self::aggregationszustaendigkeit), and
    /// because "is this MaLo balanced in the LPB's Bilanzierungsgebiet" is a
    /// query, not a field to dig out of JSONB.
    #[serde(default)]
    pub abwicklungsmodell: Option<String>,
    /// Who aggregates, in the four states the market rules need:
    /// `UEBERTRAGUNGSNETZBETREIBER`, `VERTEILNETZBETREIBER`, `RUHEND`,
    /// `UNBEKANNT`.
    ///
    /// Derived by `rubo4e`'s `Bilanzierung::aggregationszustaendigkeit()` from
    /// the **pair** — `Aggregationsverantwortung` has two members and cannot
    /// say "ruht" on its own, and an absent field alone is genuinely ambiguous.
    #[serde(default)]
    pub aggregationszustaendigkeit: Option<String>,
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

/// Why a [`BilanzierungRecord`] could not be built from a BO4E document.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BilanzierungRecordError {
    /// No `bilanzierungsbeginn`, which is half the temporal primary key.
    #[error(
        "bilanzierungsbeginn is required — it is the temporal key \
         (tenant, malo_id, bilanzierungsbeginn)"
    )]
    NoBeginn,
    /// `bilanzierungsende` is at or before `bilanzierungsbeginn`.
    #[error("bilanzierungsende {ende} is not after bilanzierungsbeginn {beginn}")]
    EndeBeforeBeginn {
        /// The stated start.
        beginn: time::OffsetDateTime,
        /// The stated end.
        ende: time::OffsetDateTime,
    },
    /// The validated document could not be serialised back to JSON.
    #[error(transparent)]
    Serialise(#[from] crate::bo4e::Bo4eSerialiseError),
}

impl BilanzierungRecord {
    /// Build a record from a **gated** BO4E `Bilanzierung`.
    ///
    /// Every column is read off the typed object, never `data.get("…")`: a
    /// string lookup yields `None` when the field is spelled differently than
    /// the reader guesses. What is stored is the gate's canonical round-trip,
    /// so a column cannot disagree with the document it shadows.
    ///
    /// `bo4e_version` is the **server's** fact, from the linked `rubo4e` — only
    /// the server knows which schema series it parsed the payload under.
    ///
    /// # Errors
    ///
    /// [`BilanzierungRecordError`].
    pub fn from_bo4e(
        tenant: &str,
        malo_id: &str,
        bo: &crate::bo4e::Bo4e<rubo4e::current::Bilanzierung>,
    ) -> Result<Self, BilanzierungRecordError> {
        use rubo4e::convenience::Aggregationszustaendigkeit;

        let beginn = bo
            .bilanzierungsbeginn
            .ok_or(BilanzierungRecordError::NoBeginn)?;
        let ende = bo.bilanzierungsende;
        // Half-open `[beginn, ende)`, like every other temporal range in mako:
        // an end at or before the start describes no interval at all, and
        // storing one makes the row invisible to every point-in-time read.
        if let Some(ende) = ende
            && ende <= beginn
        {
            return Err(BilanzierungRecordError::EndeBeforeBeginn { beginn, ende });
        }
        // Four states, because the pair says more than either field: in
        // Modell 2 the Aggregationsverantwortung *ruht* and its wire encoding
        // is an absent field, which `aggregationsverantwortung` alone cannot
        // distinguish from "not stated".
        let zustaendigkeit = match bo.aggregationszustaendigkeit() {
            Aggregationszustaendigkeit::Uebertragungsnetzbetreiber => "UEBERTRAGUNGSNETZBETREIBER",
            Aggregationszustaendigkeit::Verteilnetzbetreiber => "VERTEILNETZBETREIBER",
            Aggregationszustaendigkeit::Ruhend => "RUHEND",
            _ => "UNBEKANNT",
        };
        Ok(Self {
            malo_id: malo_id.to_owned(),
            bilanzierungsbeginn: beginn,
            bilanzierungsende: ende,
            bilanzkreis: bo.bilanzkreis.as_ref().map(ToString::to_string),
            aggregationsverantwortung: bo.aggregationsverantwortung.map(|v| v.as_wire().to_owned()),
            abwicklungsmodell: bo.abwicklungsmodell.map(|v| v.as_wire().to_owned()),
            aggregationszustaendigkeit: Some(zustaendigkeit.to_owned()),
            prognosegrundlage: bo.prognosegrundlage.map(|v| v.as_wire().to_owned()),
            fallgruppenzuordnung: bo.fallgruppenzuordnung.map(|v| v.as_wire().to_owned()),
            data: bo.canonical_json()?,
            bo4e_version: crate::bo4e::schema_version(),
            tenant: tenant.to_owned(),
            updated_at: time::OffsetDateTime::now_utc(),
        })
    }
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
    /// Tenant identifier — data-isolation key written to every database row. Typically the operator's BDEW- or DVGW-Codenummer, but any stable unique string is valid.
    pub tenant: String,
    /// Human-readable Bezeichnung.
    pub name: Option<String>,
    /// Whether the NeLo is in a Strom or a Gas grid.
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
    /// Optimistic-concurrency version, incremented on every write.
    pub version: i64,
    /// Time of the last write (UTC).
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

// ── Tranche ──────────────────────────────────────────────────────────────────

/// Stored Tranche record.
///
/// A **Tranche** is a share of a Marktlokation's energy assigned to a distinct
/// balancing responsibility (GPKE Teil 4 „Daten der Tranche",
/// PIDs 55619/55642/55652/55662/55686). One row per `(tranche_id, tenant)`; the
/// parent MaLo is recorded for `list_by_malo` grouping.
///
/// # There is no BO4E `Tranche`
///
/// BO4E models **no** Tranche Geschäftsobjekt: `BoTyp` has 39 members
/// and none is `TRANCHE`. The word does appear in the schema — as
/// `Preismodell::Tranche`, the B2B *pricing* model where volume is bought in
/// instalments — which is a different concept that happens to share a German
/// noun. So `data` is mako's own open-ended payload, not a Business Object, and
/// it crosses no BO4E gate because there is no BO4E type to gate it against.
///
/// Source: GPKE Teil 4 (BK6-22-024 Anlage 1d) §1.4.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrancheRecord {
    /// Tranche identifier (e.g. `<MaLo>-T01`).
    pub tranche_id: String,
    /// Tenant identifier — data-isolation key written to every database row. Typically the operator's BDEW- or DVGW-Codenummer, but any stable unique string is valid.
    pub tenant: String,
    /// Parent Marktlokation this Tranche belongs to.
    pub malo_id: Option<String>,
    /// Bilanzierungsgebiet EIC.
    pub bilanzierungsgebiet: Option<String>,
    /// Netzebene (`netzebene`).
    pub netzebene: Option<String>,
    /// Energierichtung (`EINSPEISUNG` / `ENTNAHME`).
    pub energierichtung: Option<String>,
    /// mako's own open-ended Tranche payload. **Not** BO4E — see the type docs.
    pub data: serde_json::Value,
    /// Optimistic-concurrency version, incremented on every write.
    pub version: i64,
    /// Time of the last write (UTC).
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
    /// Bilanzierungsgebiet-EIC, if known.
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
    /// Time of the last write (UTC).
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
    /// Tenant identifier — data-isolation key written to every database row. Typically the operator's BDEW- or DVGW-Codenummer, but any stable unique string is valid.
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
    /// BO4E schema version of `data`.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
    /// Optimistic-concurrency version, incremented on every write.
    pub version: i64,
    /// Time of the last write (UTC).
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

// ── Lokationszuordnung graph (B5) ────────────────────────────────────────────

/// One directed edge of the MaKo location graph.
///
/// The graph models: `MaLo ↔ MeLo ↔ NeLo ↔ SteuerbareRessource ↔ TechnischeRessource`
///
/// Temporal validity: `valid_from IS NULL` means "from the beginning of time";
/// `valid_to IS NULL` means "open-ended (currently active)".
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LokationszuordnungEdge {
    /// Edge ID (UUID).
    pub id: uuid::Uuid,
    /// Tenant identifier — data-isolation key written to every database row. Typically the operator's BDEW- or DVGW-Codenummer, but any stable unique string is valid.
    pub tenant: String,
    /// Source node ID (e.g. MaLo-ID, MeLo-ID).
    pub von_id: String,
    /// Source node type ([`Lokationstyp`]: `MALO`/`MELO`/`NELO`/`SR`/`TR`).
    pub von_typ: Lokationstyp,
    /// Target node ID.
    pub nach_id: String,
    /// Target node type ([`Lokationstyp`]).
    pub nach_typ: Lokationstyp,
    /// First day the edge holds; `None` = from the beginning of time.
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

/// Error raised when a [`Lokationsbuendel`] violates a structural integrity rule.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuendelError {
    /// A consuming Marktlokation bundles no Messlokation.
    #[error(
        "Lokationsbündel for MaLo {malo_id} has no Messlokation \
         (a consuming MaLo must bundle at least one MeLo)"
    )]
    NoMesslokation {
        /// MaLo-ID the bundle is rooted at.
        malo_id: String,
    },
    /// The bundle's Messlokationen are operated by more than one MSB.
    #[error(
        "Lokationsbündel for MaLo {malo_id} spans divergent MSB assignments {msbs:?} \
         (all MeLos of a MaLo must share one Messstellenbetreiber)"
    )]
    DivergentMsb {
        /// MaLo-ID the bundle is rooted at.
        malo_id: String,
        /// MP-IDs of the distinct MSBs found across the bundle's Messlokationen.
        msbs: Vec<String>,
    },
}

/// One way a bundle departs from the BDEW structure it declares.
///
/// Deliberately mako's own and not `rubo4e::lokationsbuendel::Befund`: that
/// enum reports per-**object-code** findings a BO4E `Lokationszuordnung`
/// carries, and this projection has ids grouped by type instead. Reusing the
/// name would promise findings this audit cannot produce.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Buendelbefund {
    /// No `lokationsbuendelcode`, so there is no structure to check against.
    #[error("the bundle declares no lokationsbuendelcode")]
    StrukturcodeFehlt,
    /// The code is not a 13-digit BDEW code with a valid check digit.
    #[error("lokationsbuendelcode {code} is not a valid BDEW code: {grund}")]
    StrukturcodeUngueltig {
        /// The value as stored.
        code: String,
        /// Why it failed.
        grund: String,
    },
    /// Well-formed, but not one of the fifteen published structures.
    #[error("lokationsbuendelcode {code} is not a published Lokationsbündelstruktur")]
    StrukturUnbekannt {
        /// The declared code.
        code: String,
    },
    /// The structure has no row for this object type at all.
    #[error("the structure describes no {objekttyp}, but the bundle holds {ist}")]
    ObjekttypNichtVorgesehen {
        /// The object type, in the codelist's spelling.
        objekttyp: String,
        /// How many the bundle holds.
        ist: usize,
    },
    /// The structure needs more than one Marktlokation and this projection
    /// keeps only the root, so the count cannot be checked from the graph.
    ///
    /// Not a defect in the bundle — a limit of what a `Lokationstyp` edge graph
    /// can answer. `rubo4e`'s `Lokationszuordnung::audit_buendel()` decides it
    /// from the BO4E document, where the objects and their
    /// `lokationsbuendelObjektcode`s are inline.
    #[error(
        "the structure needs at least {min} Marktlokationen; the location graph keeps only \
         the root, so this cannot be checked here — audit the BO4E Lokationszuordnung instead"
    )]
    MarktlokationenNichtPruefbar {
        /// Fewest Marktlokationen the structure permits.
        min: u32,
    },
    /// The count of one object type is outside what the structure permits.
    #[error(
        "the structure permits {} {objekttyp}, the bundle holds {ist}",
        match max { Some(m) if *m == *min => min.to_string(),
                    Some(m) => format!("{min}-{m}"),
                    None => format!("≥{min}") }
    )]
    Kardinalitaet {
        /// The object type, in the codelist's spelling.
        objekttyp: String,
        /// How many the bundle holds.
        ist: usize,
        /// Fewest the structure permits.
        min: u32,
        /// Most it permits, or `None` for the codelist's `N`.
        max: Option<u32>,
    },
}

/// What [`Lokationsbuendel::audit_struktur`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Buendelstrukturaudit {
    /// The published structure the declared code names, where it named one.
    pub struktur: Option<&'static rubo4e::lokationsbuendel::Lokationsbuendelstruktur>,
    /// Every departure found. Empty is conformant.
    pub befunde: Vec<Buendelbefund>,
}

impl Buendelstrukturaudit {
    /// `true` when the bundle matches the structure it declares.
    #[must_use]
    pub fn is_conformant(&self) -> bool {
        self.befunde.is_empty()
    }
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
    /// (typically the output of `PgLokationszuordnungRepository::find_graph`).
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

    /// Check the bundle against the BDEW *Lokationsbündelstruktur* it declares.
    ///
    /// Checks the `lokationsbuendelcode` check digit and resolves it to one of
    /// the fifteen published structures through `rubo4e::lokationsbuendel` — EDI@Energy's *Codeliste der
    /// Lokationsbündelstrukturen* (BDEW v1.0, 31.03.2023, applicable from
    /// 01.10.2024) — and reports every disagreement.
    ///
    /// # What it can and cannot see
    ///
    /// This projection holds **ids grouped by [`Lokationstyp`]**, not the
    /// per-object `lokationsbuendelObjektcode`s. So the cardinalities are
    /// checked per *object type* — the structure's rows for that type summed —
    /// and not per code. A structure wanting one consuming and one generating
    /// Marktlokation is satisfied here by any two Marktlokationen.
    ///
    /// `rubo4e`'s `Lokationszuordnung::audit_buendel()` is the per-code check;
    /// it needs the BO4E document with its objects inline, which only the write
    /// path holds.
    ///
    /// **Steuerbare Ressourcen** are not counted at all: chapter 2.1 of the
    /// codelist has no object code for one, so there is no cardinality to hold
    /// them to.
    ///
    /// **Marktlokationen cannot be counted here**, and that is *reported*, not
    /// assumed away. [`from_graph`](Self::from_graph) keeps only the root MaLo
    /// — a sibling Marktlokation on an edge is discarded — so the projection
    /// holds exactly one by construction — right for twelve structures, wrong
    /// for the three **Summenmessung** structures (`9992000000125` requires a
    /// consumption MaLo *and* at least one generating MaLo). For those,
    /// [`Buendelbefund::MarktlokationenNichtPruefbar`] says the graph cannot
    /// decide and names `audit_buendel` as what can.
    ///
    /// A report, never a refusal: BDEW requires none of this of a stored
    /// record, and a bundle is legitimately incomplete mid-Einzug.
    #[must_use]
    pub fn audit_struktur(&self) -> Buendelstrukturaudit {
        use rubo4e::lokationsbuendel::{Lokationsbuendelstruktur, Objekttyp};

        let mut befunde = Vec::new();
        let Some(raw) = self.lokationsbuendelcode.as_deref() else {
            return Buendelstrukturaudit {
                struktur: None,
                befunde: vec![Buendelbefund::StrukturcodeFehlt],
            };
        };
        // The check digit first: `Lokationsbuendelcode` enforces BDEW § 8.1, and
        // all 42 published codes verify under it. A code that fails it cannot
        // name a structure, so there is nothing further to check.
        let code = match rubo4e::identifiers::Lokationsbuendelcode::new(raw) {
            Ok(c) => c,
            Err(e) => {
                return Buendelstrukturaudit {
                    struktur: None,
                    befunde: vec![Buendelbefund::StrukturcodeUngueltig {
                        code: raw.to_owned(),
                        grund: e.to_string(),
                    }],
                };
            }
        };
        let Some(struktur) = Lokationsbuendelstruktur::from_code(&code) else {
            return Buendelstrukturaudit {
                struktur: None,
                befunde: vec![Buendelbefund::StrukturUnbekannt {
                    code: raw.to_owned(),
                }],
            };
        };

        // The Marktlokation the bundle is rooted at is the only one the
        // projection keeps, so a structure needing two or more cannot be
        // decided here. Say so rather than pass.
        let malo_min: u32 = struktur
            .objekte_of(Objekttyp::Marktlokation)
            .map(|o| o.min)
            .sum();
        if malo_min > 1 {
            befunde.push(Buendelbefund::MarktlokationenNichtPruefbar { min: malo_min });
        }

        // Per object type: the structure's rows summed. `max: None` on any row
        // makes the type unbounded, which is the codelist's `N`.
        for (typ, ist) in [
            (Objekttyp::Messlokation, self.messlokationen.len()),
            (Objekttyp::Netzlokation, self.netzlokationen.len()),
            (
                Objekttyp::TechnischeRessource,
                self.technische_ressourcen.len(),
            ),
        ] {
            let rows: Vec<_> = struktur.objekte_of(typ).collect();
            if rows.is_empty() {
                // The structure has no row for this type at all, so any object
                // of it is one the structure does not describe.
                if ist > 0 {
                    befunde.push(Buendelbefund::ObjekttypNichtVorgesehen {
                        objekttyp: typ.to_string(),
                        ist,
                    });
                }
                continue;
            }
            let min: u32 = rows.iter().map(|o| o.min).sum();
            let max: Option<u32> = rows.iter().try_fold(0_u32, |acc, o| o.max.map(|m| acc + m));
            let ist_u32 = u32::try_from(ist).unwrap_or(u32::MAX);
            if ist_u32 < min || max.is_some_and(|m| ist_u32 > m) {
                befunde.push(Buendelbefund::Kardinalitaet {
                    objekttyp: typ.to_string(),
                    ist,
                    min,
                    max,
                });
            }
        }

        Buendelstrukturaudit {
            struktur: Some(struktur),
            befunde,
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
    /// BO4E schema version of `data`.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
    /// Optimistic-concurrency version, incremented on every write.
    pub version: i64,
    /// Time of the last write (UTC).
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
    /// Master data only: `edmd`'s certificate-expiry worker reads the
    /// `SmgwSession` it stores itself, not this Konfiguration.
    SmgwTlsCertFingerprint,
    /// SMGW TLS certificate expiry date (`YYYY-MM-DD`).
    ///
    /// Master data only, like [`Self::SmgwTlsCertFingerprint`]: the expiry
    /// alert `edmd` raises reads its own `SmgwSession`.
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
    /// BO4E schema version of `data`.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
    /// Optimistic-concurrency version, incremented on every write.
    pub version: i64,
    /// Time of the last write (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
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
#[serde(deny_unknown_fields)]
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
    /// Time of the last write (UTC).
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
#[serde(deny_unknown_fields)]
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
    /// Window start in German local time, inclusive. Example: `07:00`. A
    /// window whose start is after its end crosses midnight (`22:00`–`06:00`).
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
    /// Time of the last write (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// The tariff zone a register counts: `HT`, `NT` or `EINZEL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Tarifzone {
    /// Hochtarif.
    Ht,
    /// Niedertarif.
    Nt,
    /// Single-rate: the register counts every hour.
    Einzel,
}

impl Tarifzone {
    /// The stored value, as `zaehler_register`'s CHECK lists it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ht => "HT",
            Self::Nt => "NT",
            Self::Einzel => "EINZEL",
        }
    }
}

/// The season a window applies in: `SOMMER`, `WINTER` or `GESAMT` (all year).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Saison {
    /// Summer.
    Sommer,
    /// Winter.
    Winter,
    /// All year.
    Gesamt,
}

impl Saison {
    /// The stored value, as `zaehler_saisons`' CHECK lists it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sommer => "SOMMER",
            Self::Winter => "WINTER",
            Self::Gesamt => "GESAMT",
        }
    }
}

/// The body of `PUT /api/v1/zaehler/{zaehler_id}/register`.
///
/// What a caller states about a register; the Zähler comes from the path, the
/// tenant from the session, `id` and `updated_at` from the server. A register
/// is identified by Zähler, `bezeichnung` and `valid_from`: a second `PUT` with
/// those three updates it and keeps its id.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZaehlzeitRegisterUpsert {
    /// Register label (e.g. `"HT"`, `"NT"`, `"Gesamt"`).
    pub bezeichnung: String,
    /// The tariff zone the register counts.
    pub zaehlerauspraegung: Tarifzone,
    /// OBIS kennzahl identifying this register in MSCONS (e.g. `"1-1:1.8.1"`).
    #[serde(default)]
    pub obis_kennzahl: Option<String>,
    /// Measurement unit, `"KWH"` when absent.
    #[serde(default = "default_kwh")]
    pub einheit: String,
    /// First day of validity.
    pub valid_from: time::Date,
    /// First day no longer valid — `None` while the register is current.
    #[serde(default)]
    pub valid_to: Option<time::Date>,
}

impl ZaehlzeitRegisterUpsert {
    /// Refuse what the schema would refuse, with the field named.
    ///
    /// # Errors
    /// [`MdmError::Unprocessable`] for an empty `bezeichnung` or a `valid_to`
    /// not after `valid_from`.
    pub fn validate(&self) -> Result<(), MdmError> {
        if self.bezeichnung.trim().is_empty() {
            return Err(MdmError::Unprocessable {
                reason: "bezeichnung must not be empty".to_owned(),
            });
        }
        if self.valid_to.is_some_and(|to| to <= self.valid_from) {
            return Err(MdmError::Unprocessable {
                reason: "valid_to must be after valid_from".to_owned(),
            });
        }
        Ok(())
    }
}

/// The body of `PUT /api/v1/zaehler-register/{register_id}/saisons`.
///
/// One window of a register; the register comes from the path, `id` and
/// `updated_at` from the server. A window is identified by register, `saison`
/// and `zeit_von`: a second `PUT` with those three updates it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZaehlzeitSaisonUpsert {
    /// The season the window applies in.
    pub saison: Saison,
    /// ISO weekdays, 1 (Mon) through 7 (Sun), each at most once.
    pub wochentage: Vec<i16>,
    /// Window start in German local time, inclusive (`"07:00"`). A start
    /// after the end crosses midnight: `"22:00"`–`"06:00"` is one window.
    #[serde(with = "wall_clock")]
    pub zeit_von: time::Time,
    /// Window end in German local time, exclusive (`"22:00"`).
    #[serde(with = "wall_clock")]
    pub zeit_bis: time::Time,
}

impl ZaehlzeitSaisonUpsert {
    /// Refuse what the schema would refuse, with the field named.
    ///
    /// # Errors
    /// [`MdmError::Unprocessable`] for no weekday, a weekday outside 1–7 or
    /// named twice, or a window that ends where it starts — which would
    /// classify no reading and show up as missing HT/NT energy.
    pub fn validate(&self) -> Result<(), MdmError> {
        let refuse = |reason: &str| {
            Err(MdmError::Unprocessable {
                reason: reason.to_owned(),
            })
        };
        if self.wochentage.is_empty() {
            return refuse("wochentage must name at least one weekday");
        }
        if self.wochentage.iter().any(|d| !(1..=7).contains(d)) {
            return refuse("wochentage are ISO weekdays, 1 (Mon) through 7 (Sun)");
        }
        let mut days = self.wochentage.clone();
        days.sort_unstable();
        days.dedup();
        if days.len() != self.wochentage.len() {
            return refuse("wochentage names a weekday twice");
        }
        if self.zeit_von == self.zeit_bis {
            return refuse("zeit_bis must differ from zeit_von");
        }
        Ok(())
    }
}

#[cfg(test)]
mod zaehlzeit_body_tests {
    use super::{Saison, Tarifzone, ZaehlzeitRegisterUpsert, ZaehlzeitSaisonUpsert};
    use time::macros::{date, time};

    fn window(wochentage: Vec<i16>, von: time::Time, bis: time::Time) -> ZaehlzeitSaisonUpsert {
        ZaehlzeitSaisonUpsert {
            saison: Saison::Winter,
            wochentage,
            zeit_von: von,
            zeit_bis: bis,
        }
    }

    #[test]
    fn a_window_that_classifies_no_reading_is_refused() {
        assert!(
            window(vec![1, 2, 3, 4, 5], time!(07:00), time!(22:00))
                .validate()
                .is_ok()
        );
        assert!(
            window(vec![1, 2, 3, 4, 5], time!(22:00), time!(06:00))
                .validate()
                .is_ok(),
            "a midnight-crossing NT window is one row"
        );
        for bad in [
            window(vec![], time!(07:00), time!(22:00)),
            window(vec![0], time!(07:00), time!(22:00)),
            window(vec![8], time!(07:00), time!(22:00)),
            window(vec![1, 1], time!(07:00), time!(22:00)),
            window(vec![1], time!(07:00), time!(07:00)),
        ] {
            assert!(bad.validate().is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn a_register_body_carries_no_server_owned_field() {
        let body: ZaehlzeitRegisterUpsert = serde_json::from_value(serde_json::json!({
            "bezeichnung": "HT", "zaehlerauspraegung": "HT", "valid_from": "2026-01-01",
        }))
        .expect("the three stated fields are a register");
        assert_eq!(body.zaehlerauspraegung, Tarifzone::Ht);
        assert_eq!(body.einheit, "KWH");
        for key in ["id", "zaehler_id", "tenant", "updated_at"] {
            let mut json = serde_json::json!({
                "bezeichnung": "HT", "zaehlerauspraegung": "HT", "valid_from": "2026-01-01",
            });
            json[key] = serde_json::json!("x");
            assert!(
                serde_json::from_value::<ZaehlzeitRegisterUpsert>(json).is_err(),
                "`{key}` is the server's, so a body naming it is refused"
            );
        }
        let ended = ZaehlzeitRegisterUpsert {
            valid_to: Some(date!(2026 - 01 - 01)),
            ..body
        };
        assert!(
            ended.validate().is_err(),
            "valid_to on valid_from is an empty validity"
        );
    }
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
        use super::super::ZaehlzeitSaisonUpsert;

        #[test]
        fn a_window_round_trips_as_hh_mm_ss_not_a_component_array() {
            let json = serde_json::json!({
                "saison": "WINTER",
                "wochentage": [1, 2, 3, 4, 5],
                "zeit_von": "07:00",
                "zeit_bis": "22:00:00",
            });
            let rec: ZaehlzeitSaisonUpsert =
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
                    "saison": "WINTER",
                    "wochentage": [1],
                    "zeit_von": bad,
                    "zeit_bis": "22:00",
                });
                assert!(
                    serde_json::from_value::<ZaehlzeitSaisonUpsert>(json).is_err(),
                    "{bad:?} must not parse as a window boundary"
                );
            }
        }
    }
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
    /// Time of the last write (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

// ── Strom Mehr-/Mindermengenpreise ───────────────────────────────────────────

/// The nationwide Strom Mehr-/Mindermengenpreise for one application month.
///
/// The prices are *einheitlich* and computed from monthly market prices. The
/// authority is keyed on the delivery period, not stated flat: § 13 Abs. 3
/// StromNZV to 31.12.2025, and § 20 Abs. 3 EnWG through the GPKE Festlegung
/// (BK6-24-174) from 01.01.2026, since the StromNZV ceased to have effect with
/// the end of 31.12.2025 (Art. 15 Abs. 4 G. v. 22.12.2023, BGBl. 2023 I
/// Nr. 405). The BDEW determines and publishes them centrally as one
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
    /// How the prices entered the system: `manual`, `bdew-csv` or `csv-import`.
    pub source: String,
    /// Time of the last write (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
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

// ── ESA consent registry (§49 Abs. 2 Nr. 9 MsbG) ──────────────────────────────

/// One ESA consent (Einwilligung) — the ESA's lawful basis for holding a
/// location's metering values (§49 Abs. 2 Nr. 9 MsbG, GDPR Art. 7).
///
/// Evidence-agnostic: `evidence_uri`/`evidence_hash` are stored verbatim and
/// never validated for form (BNetzA forbids rejecting consent for deviating
/// from the BDEW template).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EinwilligungRecord {
    /// Consent ID (UUID), assigned by the database on insert.
    #[serde(default)]
    pub id: Uuid,
    /// Tenant identifier — data-isolation key written to every database row. Typically the operator's BDEW- or DVGW-Codenummer, but any stable unique string is valid.
    #[serde(default)]
    pub tenant: String,
    /// Opaque reference to the Anschlussnutzer (no PII stored here).
    pub anschlussnutzer_ref: String,
    /// MP-ID of the ESA the consent authorises.
    pub esa_mp_id: String,
    /// Locations (MaLo/MeLo/NeLo/ZPB) the consent covers.
    pub location_ids: Vec<String>,
    /// Free-form scope of the consent (e.g. `lastgang`, `zaehlerstaende`); `werte` when not given.
    #[serde(default = "default_scope")]
    pub scope: String,
    /// When the consent was recorded (UTC).
    #[serde(default = "unix_epoch", with = "time::serde::rfc3339")]
    pub granted_at: time::OffsetDateTime,
    /// First day the consent is effective.
    #[serde(with = "date_iso")]
    pub valid_from: Date,
    /// Last day of the consent window; `None` = open-ended until revoked.
    #[serde(default, with = "date_iso::opt")]
    pub valid_to: Option<Date>,
    /// GDPR Art. 7(3): non-`None` once revoked.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<time::OffsetDateTime>,
    /// Opaque evidence pointer/hash — stored verbatim, never form-validated.
    #[serde(default)]
    pub evidence_uri: Option<String>,
    /// Hash of the Einwilligungserklärung, stored verbatim.
    #[serde(default)]
    pub evidence_hash: Option<String>,
}

fn default_scope() -> String {
    "werte".to_owned()
}

/// Bilateral EDI@Energy framework agreement + AS4 cert state (MSB ↔ ESA).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EsaFrameworkAgreement {
    /// Tenant identifier — data-isolation key written to every database row. Typically the operator's BDEW- or DVGW-Codenummer, but any stable unique string is valid.
    #[serde(default)]
    pub tenant: String,
    /// MP-ID of the Messstellenbetreiber.
    pub msb_mp_id: String,
    /// MP-ID of the ESA.
    pub esa_mp_id: String,
    /// When the agreement was signed; `None` when not recorded.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub signed_at: Option<time::OffsetDateTime>,
    /// Whether the EDI@Energy framework agreement is in place.
    #[serde(default)]
    pub edi_agreement: bool,
    /// AS4 certificate exchange state (e.g. `pending`, `active`, `expired`); `rejected`, `revoked` and `suspended` count as not established.
    #[serde(default = "default_cert_state")]
    pub cert_state: String,
}

fn default_cert_state() -> String {
    "pending".to_owned()
}

/// One row of an MSB's **ESA Messprodukt-Katalog**: a Kapitel-4.6 product it
/// serves, in which Abo mode, over which window.
///
/// Answers the one commercial question `E_0252` Prüfschritt 2 and `E_0256`
/// Prüfschritte 4/5 ask and the Codeliste cannot: which of the *optional*
/// products this MSB carries. The seven Pflichtprodukte are served regardless
/// (BNetzA *Mitteilung Nr. 3*, §34 Abs. 2 S. 2 Nr. 10 MsbG), so an empty
/// catalogue never refuses a mandated Zusatzleistung.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EsaMessproduktAngebot {
    /// Tenant identifier — data-isolation key written to every database row. Typically the operator's BDEW- or DVGW-Codenummer, but any stable unique string is valid.
    #[serde(default)]
    pub tenant: String,
    /// The MSB whose catalogue this belongs to.
    pub msb_mp_id: String,
    /// 13-digit Messprodukt-Code from Codeliste der Konfigurationen 1.4
    /// Kap. 4.6, digits only.
    pub messprodukt: String,
    /// `E_0256` Prüfschritt 4 — served as a turnusmäßige Übermittlung
    /// (`IMD++Z01`).
    #[serde(default = "crate::repository::default_true")]
    pub als_abo: bool,
    /// `E_0256` Prüfschritt 5 — served as a single transmission (`IMD++Z03`).
    ///
    /// Separate from [`Self::als_abo`] because the tree refuses the two with
    /// **different codes** — `A04` for the Abo, `A05` for the einmalige
    /// Übermittlung — so collapsing them loses which one the MSB declined.
    #[serde(default = "crate::repository::default_true")]
    pub als_einmalig: bool,
    /// Half-open `[valid_from, valid_to)`. A catalogue changes, and a
    /// Vergangenheitswerte-Bestellung is judged against the period the values
    /// are wanted for.
    #[serde(default)]
    pub valid_from: Option<time::Date>,
    /// Exclusive end of the window; `None` = open-ended.
    #[serde(default)]
    pub valid_to: Option<time::Date>,
}

/// Default for the two catalogue flags: an entry that exists serves the product
/// unless it says otherwise.
#[must_use]
pub const fn default_true() -> bool {
    true
}

/// One priced Artikel-ID of an accepted QUOTES 15003 Angebot.
///
/// **The ESA price basis**, and the only one it has. `PreisblattMessung` is what
/// an MSB *publishes* toward the NB and the LF; there is none for the
/// Kapitel-4.6 Messprodukte, because §35 MsbG leaves the Entgelt for a
/// Zusatzleistung to be agreed per request — which is what the
/// Universalbestellprozess exists for. UC 4.1.1 has the ESA asking for „die
/// Übermittlung von Werten **und die damit verbundenen Kosten**", the offer
/// carries a Bindungsfrist because it binds, and the MSB's later INVOIC 31009
/// names the same Artikel-IDs back (`SG26 LIN` DE 7143 `Z09`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EsaMessproduktPreis {
    /// Tenant identifier — data-isolation key written to every database row. Typically the operator's BDEW- or DVGW-Codenummer, but any stable unique string is valid.
    #[serde(default)]
    pub tenant: String,
    /// The two parties to the offer. An ESA holds a separate agreement with
    /// every MSB it reaches, and prices do not carry across them.
    pub esa_mp_id: String,
    /// MP-ID of the Messstellenbetreiber that made the offer.
    pub msb_mp_id: String,
    /// Which subscription was ordered at this price — a subscription is the
    /// (Meldepunkt, Messprodukt) pair, so both are named.
    pub lokations_id: String,
    /// Messprodukt-Code of the subscription (13 digits, as in [`EsaMessproduktAngebot::messprodukt`]).
    pub messprodukt: String,
    /// `SG27 PIA+Z02` DE 7140.
    pub artikel_id: String,
    /// `SG31 PRI` DE 5387 — `Z01` Einrichtungs-, `Z02` Transaktions-, `Z03`
    /// Betriebspreis.
    pub preistyp: String,
    /// `SG31 PRI` DE 5118. A decimal, not a float: the AHB admits six decimal
    /// places and a Betriebspreis per Tag lands in the last of them.
    pub betrag: rust_decimal::Decimal,
    /// `SG31 PRI` DE 6411 — `H87` Stück or `DAY` Tag.
    pub einheit: String,
    /// `SG4 CUX` DE 6345.
    #[serde(default = "default_waehrung")]
    pub waehrung: String,
    /// Belegnummer of the ORDERS 17007 the offer was accepted with.
    #[serde(default)]
    pub bestellung_ref: Option<String>,
    /// Half-open validity; `valid_to` closes when the subscription ends.
    #[serde(default)]
    pub valid_from: Option<time::Date>,
    /// Exclusive end of the validity; `None` while the subscription runs.
    #[serde(default)]
    pub valid_to: Option<time::Date>,
}

fn default_waehrung() -> String {
    "EUR".to_owned()
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
#[serde(deny_unknown_fields)]
pub struct NetzzugangAntrag {
    /// Request ID (UUID).
    #[serde(default)]
    pub id: Uuid,
    /// Tenant identifier — data-isolation key written to every database row. Typically the operator's BDEW- or DVGW-Codenummer, but any stable unique string is valid.
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
    /// When the request was recorded (UTC).
    #[serde(default = "unix_epoch", with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    /// When the request was first marked `uebermittelt`; `None` before that.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub submitted_at: Option<time::OffsetDateTime>,
}

fn default_netzzugang_status() -> NetzzugangStatus {
    NetzzugangStatus::Erfasst
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
            geschaeftspartner: serde_json::json!({}),
            edi_zustimmung: None,
            edi_extrahierbar: None,
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

        // `Display` matches the serde repr — the PG TEXT binding uses it.
        assert_eq!(rubo4e::current::Marktrolle::Nb.to_string(), "NB");
        assert_eq!(rubo4e::current::Rollencodetyp::Gln.to_string(), "GLN");
        // `from_wire` refuses `"UNKNOWN"`, the catch-all's own spelling.
        assert_eq!(
            rubo4e::current::Marktrolle::from_wire("LF"),
            Ok(rubo4e::current::Marktrolle::Lf)
        );
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

    /// A 13-digit code with a valid BDEW § 8.1 check digit that names none of
    /// the fifteen published structures.
    const UNPUBLISHED_BUT_WELL_FORMED: &str = "9992000009002";

    /// Build a bundle declaring `code`, with `melos` Messlokationen.
    fn buendel_with(code: Option<&str>, melos: usize) -> Lokationsbuendel {
        Lokationsbuendel {
            malo_id: "MALO1".to_owned(),
            lokationsbuendelcode: code.map(ToOwned::to_owned),
            messlokationen: (1..=melos).map(|i| format!("MELO{i}")).collect(),
            netzlokationen: vec![],
            steuerbare_ressourcen: vec![],
            technische_ressourcen: vec![],
        }
    }

    /// The BDEW codelist resolves the declared code to a named structure, and a
    /// bundle that matches its cardinalities reports nothing.
    ///
    /// `9992000000026` is *Verbrauch mit einer Messlokation (Standard)*: exactly
    /// one Marktlokation, exactly one Messlokation, any number of technische
    /// Ressourcen, at most one Netzlokation.
    #[test]
    fn a_conformant_bundle_names_its_published_structure() {
        let audit = buendel_with(Some("9992000000026"), 1).audit_struktur();
        assert!(audit.is_conformant(), "{:?}", audit.befunde);
        let s = audit.struktur.expect("a published structure");
        assert_eq!(s.bezeichnung, "Verbrauch mit einer Messlokation (Standard)");
        assert_eq!(s.max_ebene(), 1);
    }

    /// The same structure permits exactly one Messlokation, so two is a finding
    /// that names the count and the bound — not a silently accepted bundle.
    #[test]
    fn a_second_messlokation_breaks_the_standard_structure() {
        let audit = buendel_with(Some("9992000000026"), 2).audit_struktur();
        assert_eq!(audit.befunde.len(), 1, "{:?}", audit.befunde);
        let msg = audit.befunde[0].to_string();
        assert!(
            msg.contains("permits 1") && msg.contains("holds 2"),
            "the finding must state both numbers: {msg}"
        );
    }

    /// `9992000000018` is *Verbrauch ohne Messlokation (Pauschal)* — it has no
    /// Messlokation row at all, so any MeLo is an object the structure does not
    /// describe. That is a different finding from a broken cardinality.
    #[test]
    fn an_object_the_structure_does_not_describe_is_its_own_finding() {
        let audit = buendel_with(Some("9992000000018"), 1).audit_struktur();
        assert!(matches!(
            audit.befunde.as_slice(),
            [Buendelbefund::ObjekttypNichtVorgesehen { ist: 1, .. }]
        ));
    }

    /// **The Summenmessung structures need more than one Marktlokation**, and
    /// the graph projection keeps only the root — so the audit says it cannot
    /// decide rather than passing the bundle.
    ///
    /// `9992000000125` is *Summenmessung mit mindestens einer separat gemessenen
    /// Erzeugung*: `…1016` (exactly one consumption MaLo) plus `…1115` (at least
    /// one generating MaLo) sum to a minimum of two. It is also the code the
    /// `marktd` edge fixtures carry, so this is not a hypothetical shape.
    #[test]
    fn a_structure_needing_two_malos_is_reported_as_undecidable() {
        // Two Messlokationen, which this structure's summed minimum also wants,
        // so the only finding left is the one about Marktlokationen.
        let audit = buendel_with(Some("9992000000125"), 2).audit_struktur();
        assert!(
            matches!(
                audit.befunde.as_slice(),
                [Buendelbefund::MarktlokationenNichtPruefbar { min: 2 }]
            ),
            "{:?}",
            audit.befunde
        );
        assert!(
            audit.befunde[0].to_string().contains("audit the BO4E"),
            "the finding must name what *can* decide it: {}",
            audit.befunde[0]
        );
    }

    /// The twelve structures that want exactly one Marktlokation stay silent
    /// about it — the projection holds exactly one by construction.
    #[test]
    fn a_single_malo_structure_says_nothing_about_marktlokationen() {
        let audit = buendel_with(Some("9992000000026"), 1).audit_struktur();
        assert!(audit.is_conformant(), "{:?}", audit.befunde);
    }

    /// A code with a wrong BDEW check digit cannot name a structure, so the
    /// audit stops there rather than reporting cardinalities against nothing.
    #[test]
    fn a_bad_check_digit_is_refused_before_the_lookup() {
        let audit = buendel_with(Some("9992000000019"), 1).audit_struktur();
        assert!(audit.struktur.is_none());
        assert!(matches!(
            audit.befunde.as_slice(),
            [Buendelbefund::StrukturcodeUngueltig { .. }]
        ));
    }

    /// A well-formed code outside the fifteen published structures is reported
    /// as unpublished — the codelist's introduction says complex structures are
    /// agreed bilaterally, so this is a fact about the bundle, not a defect.
    ///
    /// `9992000009996` is a fabricated code with a correct § 8.1 check digit.
    /// `9992000000125` looked like one and is not: it is *Summenmessung mit
    /// mindestens einer separat gemessenen Erzeugung*, which is why the fixture
    /// above uses it.
    #[test]
    fn a_well_formed_unpublished_code_says_so() {
        let audit = buendel_with(Some(UNPUBLISHED_BUT_WELL_FORMED), 1).audit_struktur();
        assert!(audit.struktur.is_none());
        assert!(
            matches!(
                audit.befunde.as_slice(),
                [Buendelbefund::StrukturUnbekannt { .. }]
            ),
            "{:?}",
            audit.befunde
        );
    }

    /// No code at all is the commonest case and says exactly that.
    #[test]
    fn a_bundle_with_no_code_reports_the_absence() {
        let audit = buendel_with(None, 1).audit_struktur();
        assert_eq!(audit.befunde, vec![Buendelbefund::StrukturcodeFehlt]);
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

#[cfg(test)]
mod bilanzierung_record_tests {
    use super::{BilanzierungRecord, BilanzierungRecordError};
    use crate::bo4e::Bo4e;
    use rubo4e::current::{Abwicklungsmodell, Aggregationsverantwortung, Bilanzierung};
    use time::macros::datetime;

    fn bo(b: Bilanzierung) -> Bo4e<Bilanzierung> {
        Bo4e::from_built(b)
    }

    fn beginn() -> Bilanzierung {
        Bilanzierung {
            bilanzierungsbeginn: Some(datetime!(2026-01-01 00:00 UTC)),
            ..Default::default()
        }
    }

    /// **The Modell-2 state has a spelling now.** In e-mobility Modell 2 the
    /// Aggregationsverantwortung *ruht*, and its wire encoding is an absent
    /// field — so the raw column is `NULL`, exactly as it is for a payload that
    /// says nothing, and only the derived column tells the two apart.
    #[test]
    fn modell_2_with_no_holder_is_ruhend_not_unknown() {
        let rec = BilanzierungRecord::from_bo4e(
            "t",
            "51238696012",
            &bo(Bilanzierung {
                abwicklungsmodell: Some(Abwicklungsmodell::Modell2),
                ..beginn()
            }),
        )
        .expect("a record");
        assert_eq!(rec.aggregationsverantwortung, None);
        assert_eq!(rec.abwicklungsmodell.as_deref(), Some("MODELL_2"));
        assert_eq!(rec.aggregationszustaendigkeit.as_deref(), Some("RUHEND"));
    }

    /// The same absent field with no Modell 2 beside it says nothing at all,
    /// and must not be reported as "nobody holds it".
    #[test]
    fn an_absent_holder_alone_is_unbekannt() {
        let rec = BilanzierungRecord::from_bo4e("t", "51238696012", &bo(beginn())).expect("record");
        assert_eq!(rec.aggregationszustaendigkeit.as_deref(), Some("UNBEKANNT"));
    }

    /// A named holder is a named holder whatever the model, and the column
    /// carries BO4E's own wire spelling — `VNB`, not the German `NB`.
    #[test]
    fn a_named_holder_wins_over_the_model() {
        let rec = BilanzierungRecord::from_bo4e(
            "t",
            "51238696012",
            &bo(Bilanzierung {
                abwicklungsmodell: Some(Abwicklungsmodell::Modell2),
                aggregationsverantwortung: Some(Aggregationsverantwortung::Vnb),
                ..beginn()
            }),
        )
        .expect("a record");
        assert_eq!(rec.aggregationsverantwortung.as_deref(), Some("VNB"));
        assert_eq!(
            rec.aggregationszustaendigkeit.as_deref(),
            Some("VERTEILNETZBETREIBER")
        );
    }

    /// `bilanzierungsbeginn` is half the primary key, so its absence is a named
    /// refusal rather than a row that cannot be addressed.
    #[test]
    fn a_missing_beginn_is_refused_by_name() {
        let err = BilanzierungRecord::from_bo4e("t", "51238696012", &bo(Bilanzierung::default()))
            .expect_err("no temporal key");
        assert_eq!(err, BilanzierungRecordError::NoBeginn);
    }

    /// The range is half-open `[beginn, ende)`. An end at or before the start
    /// describes no interval, and a row carrying one is invisible to every
    /// point-in-time read.
    #[test]
    fn an_end_before_the_start_is_refused() {
        let err = BilanzierungRecord::from_bo4e(
            "t",
            "51238696012",
            &bo(Bilanzierung {
                bilanzierungsende: Some(datetime!(2025-01-01 00:00 UTC)),
                ..beginn()
            }),
        )
        .expect_err("an empty interval");
        assert!(matches!(
            err,
            BilanzierungRecordError::EndeBeforeBeginn { .. }
        ));
    }

    /// The stored document is the gate's round-trip and the version stamp is
    /// the server's, not the payload's.
    #[test]
    fn the_stored_document_is_canonical_and_the_stamp_is_ours() {
        let rec = BilanzierungRecord::from_bo4e("t", "51238696012", &bo(beginn())).expect("record");
        assert_eq!(rec.data["_typ"], "BILANZIERUNG");
        assert_eq!(rec.bo4e_version, crate::bo4e::schema_version());
    }
}

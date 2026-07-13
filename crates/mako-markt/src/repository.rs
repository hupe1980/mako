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

use crate::{
    domain::{MaloId, MarktpartnerId, MeloId, ProcessStatus, Sparte},
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
/// Full BO4E `VERTRAG` payload with `_mdm_billing` extension.
pub type ContractPayload = serde_json::Value;

/// Default BO4E schema version used by `#[serde(default = ...)]` on record
/// structs. Returns `"v202607.0.0"` so that records written before M5
/// (the `bo4e_version` migration) are read as the baseline version.
fn default_bo4e_version() -> String {
    "v202607.0.0".to_owned()
}

// ── MaLo ─────────────────────────────────────────────────────────────────────

/// Point-in-time `lokationszuordnung` record extracted from a `MARKTLOKATION`.
///
/// The `malo_id` is implicit (always the parent `MaloRecord.malo_id`) and
/// is therefore not repeated here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lokationszuordnung {
    pub zuordnungstyp: String,
    pub rollencodenummer: String,
    #[serde(with = "date_iso")]
    pub valid_from: Date,
    #[serde(default, with = "date_iso::opt")]
    pub valid_to: Option<Date>,
}

/// Stored MaLo record as returned by repository reads.
///
/// `lokationszuordnung` contains only the role assignments valid at the
/// `at` date passed to `MaloRepository::find` / `MaloRepository::list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaloRecord {
    pub malo_id: MaloId,
    pub sparte: Sparte,
    /// Voltage/pressure level extracted from `Marktlokation.netzebene` (e.g. `"NS"`, `"MS"`).
    /// `None` when the incoming BO4E payload did not carry the field.
    pub netzebene: Option<String>,
    /// Bilanzierungsgebiet EIC code (`LOC+237` in UTILMD) extracted from `Marktlokation`.
    /// Used by `processd` NB check 4 as fallback when `malo_grid` is not populated.
    pub bilanzierungsgebiet: Option<String>,
    /// Gas quality (`HGas` | `LGas`) extracted from `Marktlokation`.
    /// Used for Gas tariff routing and GeLi Gas process validation.
    pub gasqualitaet: Option<String>,
    /// Energy direction (`Aussp` = generation, `Einsp` = consumption).
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
    /// Gas GaBi RLM Fallgruppe, extracted from `data["fallgruppenzuordnung"]`.
    ///
    /// Values: `"GABI_RLM_MIT_TAGESBAND"` | `"GABI_RLM_OHNE_TAGESBAND"` |
    /// `"GABI_RLM_IM_NOMINIERUNGSERSATZVERFAHREN"`.
    ///
    /// Determines the GaBi billing category for Gas RLM MaLos.
    /// Required for `netzbilanzd` Gas MMM settlement routing.
    pub fallgruppe: Option<String>,
    pub version: i64,
    pub data: MaloPayload,
    /// Role assignments valid at the requested reference date.
    pub lokationszuordnung: Vec<Lokationszuordnung>,
    pub updated_at: time::OffsetDateTime,
    /// BO4E schema version of the `data` payload (e.g. `"v202607.0.0"`).
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
    /// - netz-checker check 5 (Bilanzierungszone at MeLo level)
    pub standorteigenschaften: Option<serde_json::Value>,
    pub version: i64,
    pub data: MeloPayload,
    pub updated_at: time::OffsetDateTime,
    /// BO4E schema version of the `data` payload.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
}

/// Stored contract record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractRecord {
    pub contract_id: String,
    pub malo_id: Option<MaloId>,
    pub sparte: Sparte,
    pub vertragsart: String,
    pub version: i64,
    pub data: ContractPayload,
    /// Start date of the contract validity period.
    ///
    /// `None` for records created before this field was added (pre-migration).
    /// Used by [`ContractRepository::find_active_by_malo`] to detect overlapping
    /// active contracts when validating Wechselprozess requests.
    #[serde(default, with = "date_iso::opt")]
    pub valid_from: Option<Date>,
    /// End date of the contract validity period.
    ///
    /// `None` means the contract is open-ended (currently active with no known end).
    #[serde(default, with = "date_iso::opt")]
    pub valid_to: Option<Date>,
    pub created_at: time::OffsetDateTime,
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
    pub marktrolle: Option<String>,
    pub sparte: Option<Sparte>,
    /// Coding authority: `"BDEW"` | `"DVGW"` | `"GS1"`.
    /// Derived from the MP-ID prefix; stored for fast AS4 routing lookups.
    pub rollencodetyp: Option<String>,
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
    pub initiated_at: time::OffsetDateTime,
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

/// Filters for `GET /api/v1/malo` listing.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MaloFilter {
    pub sparte: Option<Sparte>,
    /// Filter by `zuordnungstyp` in active `lokationszuordnung` (e.g. `"NB"`, `"LF"`).
    pub zuordnungstyp: Option<String>,
    /// Filter by `rollencodenummer` (GLN) in active `lokationszuordnung`.
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
    /// Gas quality (`"HGas"` | `"LGas"`).
    pub gasqualitaet: Option<String>,
    /// Energy direction (`"Aussp"` = generation, `"Einsp"` = consumption).
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

/// Read/write access to `MARKTLOKATION` records.
#[allow(async_fn_in_trait)]
pub trait MaloRepository: Send + Sync {
    /// Insert or update a `MARKTLOKATION`.
    ///
    /// Validates optimistic concurrency via `if_match` (the caller's ETag).
    /// Pass `None` for unconditional upsert (first write).
    ///
    /// Returns the new version number.
    async fn upsert(
        &self,
        malo_id: &MaloId,
        sparte: Sparte,
        data: MaloPayload,
        lokationszuordnung: Vec<Lokationszuordnung>,
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
    /// be set on the first `PUT /api/v1/malo` call instead.
    async fn patch_typenmerkmal(
        &self,
        malo_id: &MaloId,
        bilanzierungsmethode: Option<&str>,
        fallgruppe: Option<&str>,
    ) -> Result<(), MdmError>;

    /// Return the `MARKTLOKATION` with `lokationszuordnung` valid at `at`.
    ///
    /// `at` defaults to today (German local date).
    async fn find(&self, malo_id: &MaloId, at: Date) -> Result<Option<MaloRecord>, MdmError>;

    /// Return a paged list filtered by the given predicates.
    ///
    /// `at` is the reference date for `lokationszuordnung` validity.
    async fn list(&self, filter: MaloFilter, at: Date) -> Result<PageResult<MaloRecord>, MdmError>;
}

/// Read/write access to `MESSLOKATION` records.
#[allow(async_fn_in_trait)]
pub trait MeloRepository: Send + Sync {
    /// Insert or update a `MESSLOKATION`.
    ///
    /// Returns the new version number.
    async fn upsert(
        &self,
        melo_id: &MeloId,
        malo_id: Option<&MaloId>,
        data: MeloPayload,
        if_match: Option<i64>,
        bo4e_version: &str,
    ) -> Result<i64, MdmError>;

    /// Return the `MESSLOKATION` record.
    async fn find(&self, melo_id: &MeloId) -> Result<Option<MeloRecord>, MdmError>;
}

/// Read/write access to contract (`VERTRAG`) records.
#[allow(async_fn_in_trait)]
pub trait ContractRepository: Send + Sync {
    /// Insert or update a contract.
    ///
    /// `valid_from` / `valid_to` define the contract validity period used by
    /// [`find_active_by_malo`](ContractRepository::find_active_by_malo) to
    /// detect overlapping active contracts during Wechselprozess validation.
    ///
    /// Returns the new version number.
    #[allow(clippy::too_many_arguments)]
    async fn upsert(
        &self,
        contract_id: &str,
        malo_id: Option<&MaloId>,
        sparte: Sparte,
        vertragsart: &str,
        data: ContractPayload,
        valid_from: Option<Date>,
        valid_to: Option<Date>,
        if_match: Option<i64>,
        bo4e_version: &str,
    ) -> Result<i64, MdmError>;

    /// Return a contract by its MDM ID.
    async fn find(&self, contract_id: &str) -> Result<Option<ContractRecord>, MdmError>;

    /// Return all contracts for `malo_id` that are active at date `at`.
    ///
    /// A contract is considered active when:
    /// - `valid_from IS NULL OR valid_from <= at`, AND
    /// - `valid_to IS NULL OR valid_to >= at`
    ///
    /// Contracts without `valid_from` / `valid_to` (pre-migration) are always
    /// returned so callers can apply their own filtering logic.
    ///
    /// Results are ordered by `valid_from DESC NULLS LAST`.
    async fn find_active_by_malo(
        &self,
        malo_id: &MaloId,
        at: Date,
    ) -> Result<Vec<ContractRecord>, MdmError>;
}

/// Read/write access to ERP webhook subscriptions.
#[allow(async_fn_in_trait)]
pub trait SubscriptionRepository: Send + Sync {
    /// Insert or update a subscription.
    ///
    /// `webhook_secret` is stored encrypted at rest by the implementation.
    ///
    /// Returns the new version number.
    async fn upsert(&self, sub: Subscription) -> Result<i64, MdmError>;

    /// Return a subscription by subscriber ID.
    async fn find(&self, subscriber_id: &str) -> Result<Option<Subscription>, MdmError>;

    /// List all active subscriptions.
    async fn list_active(&self) -> Result<Vec<Subscription>, MdmError>;

    /// Return all active subscriptions that match a given event type and role.
    ///
    /// Used by the fan-out worker to select delivery targets.
    async fn list_matching(
        &self,
        event_type: &str,
        role: &str,
        sparte: Option<&str>,
    ) -> Result<Vec<Subscription>, MdmError>;
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
    pub created_at: time::OffsetDateTime,
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
    pub created_at: time::OffsetDateTime,
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
/// §17 StromNZV requires the NB to include Konzessionsabgabe (KA) as a separate
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
    pub created_at: time::OffsetDateTime,
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
    pub created_at: time::OffsetDateTime,
    pub updated_at: time::OffsetDateTime,
}

/// Read/write access to MSB service price sheets.
///
/// Used by `invoic-checker` for INVOIC 31009 service position validation
/// and by `mako-wim` REQOTE/QUOTES (PIDs 35001–35005).
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
    pub created_at: time::OffsetDateTime,
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
    pub created_at: time::OffsetDateTime,
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
/// Used by `processd` (M17) to drive fully-automated LFA E_0624 responses
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
/// EventBus delivery guarantees convergence).
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
    /// `process_id` of the last process that triggered a state change.
    pub last_process_id: Option<Uuid>,
    /// Last time this record was updated.
    pub updated_at: time::OffsetDateTime,
    /// Tenant GLN (operator primary GLN).
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
    /// Voltage / pressure level (`NS`, `MS`, …, `HöS/HS`).
    pub netzebene: Option<String>,
    /// Owning Netzbetreiber GLN.
    pub nb_mp_id: String,
    /// Whether this NeLo can be remote-controlled (Redispatch 2.0 `steuerkanal`).
    ///
    /// Required by DELORD/DELRES topology queries.
    pub steuerkanal: Option<bool>,
    /// `eigenschaftMsbLokation` — which Marktrolle is responsible for MSB at this NeLo.
    ///
    /// E.g. `"NB"` (grundzuständiger MSB = NB) or `"MSB"` (wechselbar).
    /// Used for WiM Gas gMSB routing.
    pub eigenschaft_msb_lokation: Option<String>,
    /// `grundzustaendigerMsbCodenr` — gMSB MP-ID (13-digit BDEW/DVGW Codenummer).
    pub grundzustaendiger_msb_codenr: Option<String>,
    /// Additional Redispatch 2.0 attributes (open-ended JSONB).
    pub data: serde_json::Value,
    pub version: i64,
    pub updated_at: time::OffsetDateTime,
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

/// Convenience bundle of all repositories, passed to handlers via `Arc<AppState<...>>`.
///
/// Uses concrete generic parameters (same pattern as `mako-engine`'s `EngineContext`)
/// so all trait methods are statically dispatched — AFIT is **not** dyn-compatible.
///
/// `services/marktd` instantiates this with the Postgres implementations:
/// ```text
/// AppState<PgMaloRepo, PgMeloRepo, PgContractRepo, PgSubscriptionRepo, PgCorrelationIndex, PgPartnerRepo>
/// ```
///
/// `testing` feature instantiates it with InMemory implementations.
#[derive(Clone)]
pub struct AppState<Ma, Me, Co, Su, Ci, Pa>
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    pub malo_repo: Ma,
    pub melo_repo: Me,
    pub contract_repo: Co,
    pub subscription_repo: Su,
    pub correlation_index: Ci,
    pub partner_repo: Pa,
    #[cfg(feature = "makod-client")]
    pub makod_client: std::sync::Arc<crate::makod_client::MakodClient>,
    /// Channel for internal domain events routed to the fan-out worker.
    ///
    /// Uses an unbounded MPSC sender (single consumer: the fan-out worker).
    /// Unlike `broadcast`, this never silently drops events on lag.
    ///
    /// Payload is a serialised CloudEvent envelope (`serde_json::Value`).
    /// Callers serialise typed `MarktEvent` structs before sending so the
    /// fan-out worker and the `EventBus` abstraction share the same channel.
    pub event_tx: tokio::sync::mpsc::UnboundedSender<serde_json::Value>,
    /// Operator primary GLN (matches `makod.toml` `[[party]] primary = true`).
    pub tenant_gln: String,
}

// ── MaloGridRecord ────────────────────────────────────────────────────────────

/// NB grid topology record for a single Marktlokation.
///
/// Written by the NB's **NIS/GIS adapter** (network information system) or
/// provisioned manually via `PUT /api/v1/malo/{id}/grid` on `marktd`.
/// Read by `processd` NB module for Anmeldung STP decisions (checks 1, 4).
///
/// NOTE: This is NOT MaStR data. MaStR (BNetzA) covers generation/consumption
/// units, not NB grid topology or Bilanzierungsgebiet assignments.
///
/// Without a grid record, `netz-checker` returns `NetzCheckResult::Escalate`
/// — the NB cannot auto-decide.
///
/// # STP impact
///
/// With `nis-syncd` active (N7), STP ≥ 95 % is achievable.
/// Without it, ~40 % of Anmeldungen will escalate (missing grid records → cold cache).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaloGridRecord {
    /// 11-digit Marktlokations-ID (Strom) or Gas-MaLo-ID.
    pub malo_id: MaloId,
    /// GLN of the Netzbetreiber that owns this MaLo in their grid.
    pub nb_mp_id: String,
    /// Bilanzierungsgebiet-EIC (`LOC+237` in UTILMD), if known.
    ///
    /// `None` when the NIS has not yet provided this value.  Check 4 in
    /// `netz-checker` is skipped (not failed) when both this and the
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
    pub updated_at: time::OffsetDateTime,
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
    /// Classification: `"EMobilitaet"` | `"Erzeugung"` | `"Speicher"`.
    pub tr_typ: Option<String>,
    /// Whether the resource can be remote-controlled (Redispatch 2.0 `ist_fernschaltbar`).
    pub ist_fernschaltbar: Option<bool>,
    /// Full BO4E `TechnischeRessource` payload.
    pub data: serde_json::Value,
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
    pub version: i64,
    pub updated_at: time::OffsetDateTime,
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
        tr_typ: Option<&str>,
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
    /// Source node type: `"malo"` | `"melo"` | `"nelo"` | `"sr"` | `"tr"`.
    pub von_typ: String,
    /// Target node ID.
    pub nach_id: String,
    /// Target node type.
    pub nach_typ: String,
    pub valid_from: Option<time::Date>,
    /// `None` = open-ended (currently active).
    pub valid_to: Option<time::Date>,
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
        von_typ: &str,
        nach_id: &str,
        nach_typ: &str,
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
    pub updated_at: time::OffsetDateTime,
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
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
    pub version: i64,
    pub updated_at: time::OffsetDateTime,
}

/// Persistent store for Zähler (meters) and Geräte (devices).
///
/// Populated by WiM MSB/NB device handover processes (ORDERS PIDs 17001–17011)
/// and operator REST uploads.
///
/// Source: WiM AHB BK6-24-174; BO4E Zaehler/Geraet schemas.
#[allow(async_fn_in_trait)]
pub trait DeviceRepository: Send + Sync {
    /// Upsert a `Zaehler` record.
    #[allow(clippy::too_many_arguments)]
    async fn upsert_zaehler(
        &self,
        zaehler_id: &str,
        tenant: &str,
        melo_id: &str,
        zaehler_typ: Option<&str>,
        eichung_bis: Option<time::Date>,
        data: serde_json::Value,
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
    async fn upsert_geraet(
        &self,
        geraet_id: &str,
        tenant: &str,
        zaehler_id: &str,
        geraet_typ: Option<&str>,
        data: serde_json::Value,
        bo4e_version: &str,
    ) -> Result<(), MdmError>;

    /// Return all `Geraete` for a given `zaehler_id`.
    async fn list_geraete_by_zaehler(
        &self,
        zaehler_id: &str,
        tenant: &str,
    ) -> Result<Vec<GeraetRecord>, MdmError>;
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
    /// Days of week this window applies: bitmask or JSON array of ISO weekday
    /// numbers 1 (Mon) through 7 (Sun).  Stored as a JSON array for clarity.
    /// Example: `[1,2,3,4,5]` = Monday–Friday.
    pub wochentage: serde_json::Value,
    /// Window start time (local German time, HH:MM).  Example: `"07:00"`.
    pub zeit_von: String,
    /// Window end time (local German time, HH:MM, exclusive).  Example: `"22:00"`.
    pub zeit_bis: String,
    pub updated_at: time::OffsetDateTime,
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

// ── MMM Strom settlement prices (ÜNB per §22 StromNZV) ───────────────────────

/// A stored Strom MMM Ausgleichsenergie price record.
///
/// Published monthly by each ÜNB (50Hertz, TenneT, Amprion, TransnetBW).
/// Used by `netzbilanzd` (INVOIC 31002/31005) and `invoicd` (MMM check 6).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MmmPreisStromRecord {
    /// First day of the billing month.
    pub price_month: time::Date,
    /// ÜNB MP-ID (BDEW-Codenummer, `99…`).
    pub unb_mp_id: String,
    /// Surplus energy price (Mehrmengen) in ct/kWh.
    pub mehr_ct_kwh: rust_decimal::Decimal,
    /// Deficit energy price (Mindermengen) in ct/kWh.
    pub minder_ct_kwh: rust_decimal::Decimal,
    pub source: String,
    pub updated_at: time::OffsetDateTime,
}

/// Read/write access to Strom MMM Ausgleichsenergie prices.
#[allow(async_fn_in_trait)]
pub trait MmmPreisStromRepository: Send + Sync {
    async fn upsert_strom(
        &self,
        price_month: time::Date,
        unb_mp_id: &str,
        mehr_ct_kwh: rust_decimal::Decimal,
        minder_ct_kwh: rust_decimal::Decimal,
        source: &str,
    ) -> Result<(), MdmError>;

    async fn find_strom(
        &self,
        price_month: time::Date,
        unb_mp_id: &str,
    ) -> Result<Option<MmmPreisStromRecord>, MdmError>;
}

// ── NB Energiemix (§42 EnWG annual grid-area renewable mix) ─────────────────

/// A stored `NbEnergiemix` record.
///
/// The NB publishes the annual renewable energy mix of their grid area under
/// §42 Abs. 5 EnWG.  Lieferanten use this to compute the Reststrommix
/// for customer bills and to label Ökostrom tariffs in `tarifbd`.
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

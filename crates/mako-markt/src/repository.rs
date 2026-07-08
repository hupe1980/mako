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

    #[allow(clippy::trivially_copy_pass_by_ref)]
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

        #[allow(clippy::trivially_copy_pass_by_ref, clippy::ref_option)]
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
/// structs. Returns `"v202501.0.0"` so that records written before M5
/// (the `bo4e_version` migration) are read as the baseline version.
fn default_bo4e_version() -> String {
    "v202501.0.0".to_owned()
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
    pub version: i64,
    pub data: MaloPayload,
    /// Role assignments valid at the requested reference date.
    pub lokationszuordnung: Vec<Lokationszuordnung>,
    pub updated_at: time::OffsetDateTime,
    /// BO4E schema version of the `data` payload (e.g. `"v202501.0.0"`).
    /// Populated from `rubo4e::Bo4eObject::schema_version()` at write time.
    /// Used by the read-path dispatcher for zero-downtime schema migration.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
}

/// Stored MeLo record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeloRecord {
    pub melo_id: MeloId,
    pub malo_id: Option<MaloId>,
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
    /// 13-digit Marktpartner-ID (field name kept as `gln` for DB compat).
    pub mp_id: MarktpartnerId,
    pub display_name: Option<String>,
    pub marktrolle: Option<String>,
    pub sparte: Option<Sparte>,
    /// Raw JSON for channel details (AS4 endpoint, certificate, etc.)
    pub channels: serde_json::Value,
    /// Optimistic-concurrency version. Defaults to 0 when deserializing from
    /// PUT request bodies (the repository sets the final value on upsert).
    #[serde(default)]
    pub version: i64,
    /// Last-updated timestamp. Defaults to UNIX epoch when deserializing from
    /// PUT request bodies (the repository sets `now()` on upsert).
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
    pub netzebene: String,
    /// Metering / balancing method: `RLM` | `SLP`
    pub bilanzierungsmethode: String,
    /// How often the NB bills for network usage.
    pub billing_schedule: BillingSchedule,
    /// Start of contract validity (local date in MEZ/MESZ).
    #[serde(with = "date_iso")]
    pub valid_from: time::Date,
    /// End of contract validity (`None` = currently active).
    #[serde(with = "date_iso::opt")]
    pub valid_to: Option<time::Date>,
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
    /// GLN of the announced future Lieferant (post UTILMD 55001, pre NB confirm 55003).
    pub lf_gln_next: Option<String>,
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
    pub lf_gln_next: Option<String>,
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

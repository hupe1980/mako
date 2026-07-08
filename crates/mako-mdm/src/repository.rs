#![allow(clippy::doc_markdown)]
//! Repository traits for all MDM aggregate types.
//!
//! Every trait has exactly two implementations:
//! - Production: `Pg*Repository` in `services/mdmd/src/pg/`
//! - Testing: `InMemory*` in `crates/mako-mdm/src/testing.rs` (feature = "testing")
//!
//! All methods are `async` (AFIT, stable since Rust 1.75).
//! All methods return `Result<_, MdmError>` annotated `#[must_use]`.

use serde::{Deserialize, Serialize};
use time::Date;
use uuid::Uuid;

use crate::{
    domain::{Gln, MaloId, MeloId, ProcessStatus, Sparte},
    error::MdmError,
};

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartnerRecord {
    pub mp_id: Gln,
    pub display_name: Option<String>,
    pub marktrolle: Option<String>,
    pub sparte: Option<Sparte>,
    /// Raw JSON for channel details (AS4 endpoint, certificate, etc.)
    pub channels: serde_json::Value,
    pub version: i64,
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

    /// Return a partner by GLN.
    async fn find(&self, mp_id: &Gln) -> Result<Option<PartnerRecord>, MdmError>;

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

/// Convenience bundle of all repositories, passed to handlers via `Arc<AppState<...>>`.
///
/// Uses concrete generic parameters (same pattern as `mako-engine`'s `EngineContext`)
/// so all trait methods are statically dispatched — AFIT is **not** dyn-compatible.
///
/// `services/mdmd` instantiates this with the Postgres implementations:
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
    /// Channel for internal MDM domain events routed to the fan-out worker.
    ///
    /// Uses an unbounded MPSC sender (single consumer: the fan-out worker).
    /// Unlike `broadcast`, this never silently drops events on lag.
    ///
    /// Payload is a serialised CloudEvent envelope (`serde_json::Value`).
    /// Callers serialise typed `MdmEvent` structs before sending so the
    /// fan-out worker and the `EventBus` abstraction share the same channel.
    pub event_tx: tokio::sync::mpsc::UnboundedSender<serde_json::Value>,
    /// Operator primary GLN (matches `makod.toml` `[[party]] primary = true`).
    pub tenant_gln: String,
}

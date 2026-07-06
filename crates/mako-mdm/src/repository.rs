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

// ── Type aliases ──────────────────────────────────────────────────────────────

/// Full BO4E `MARKTLOKATION` payload (stored as JSONB; returned as-is to callers).
pub type MaloPayload = serde_json::Value;
/// Full BO4E `MESSLOKATION` payload.
pub type MeloPayload = serde_json::Value;
/// Full BO4E `VERTRAG` payload with `_mdm_billing` extension.
pub type ContractPayload = serde_json::Value;

// ── MaLo ─────────────────────────────────────────────────────────────────────

/// Point-in-time `lokationszuordnung` record extracted from a `MARKTLOKATION`.
///
/// The `malo_id` is implicit (always the parent `MaloRecord.malo_id`) and
/// is therefore not repeated here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lokationszuordnung {
    pub zuordnungstyp: String,
    pub rollencodenummer: String,
    pub valid_from: Date,
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
}

/// Stored MeLo record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeloRecord {
    pub melo_id: MeloId,
    pub malo_id: Option<MaloId>,
    pub version: i64,
    pub data: MeloPayload,
    pub updated_at: time::OffsetDateTime,
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
    pub created_at: time::OffsetDateTime,
    pub updated_at: time::OffsetDateTime,
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
    pub gln: Gln,
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
    ) -> Result<i64, MdmError>;

    /// Return the `MESSLOKATION` record.
    async fn find(&self, melo_id: &MeloId) -> Result<Option<MeloRecord>, MdmError>;
}

/// Read/write access to contract (`VERTRAG`) records.
#[allow(async_fn_in_trait)]
pub trait ContractRepository: Send + Sync {
    /// Insert or update a contract.
    ///
    /// Returns the new version number.
    async fn upsert(
        &self,
        contract_id: &str,
        malo_id: Option<&MaloId>,
        sparte: Sparte,
        vertragsart: &str,
        data: ContractPayload,
        if_match: Option<i64>,
    ) -> Result<i64, MdmError>;

    /// Return a contract by its MDM ID.
    async fn find(&self, contract_id: &str) -> Result<Option<ContractRecord>, MdmError>;
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
    async fn find(&self, gln: &Gln) -> Result<Option<PartnerRecord>, MdmError>;

    /// List all partners.
    async fn list(&self) -> Result<Vec<PartnerRecord>, MdmError>;
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
    pub event_tx: tokio::sync::mpsc::UnboundedSender<crate::cloudevents::MdmEvent>,
    /// Operator primary GLN (matches `makod.toml` `[[party]] primary = true`).
    pub tenant_gln: String,
}

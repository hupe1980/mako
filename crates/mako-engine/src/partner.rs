//! Trading-partner master data — the [`PartnerStore`] trait and supporting
//! types.
//!
//! # Why not just a `HashMap<GLN, URL>` in config?
//!
//! The `partners = ["GLN=URL", …]` field in `makod.toml` works for
//! development but falls short in production:
//!
//! | Requirement | Config-only | `PartnerStore` |
//! |---|---|---|
//! | Survives restarts without re-deployment | ❌ | ✅ |
//! | Carries PARTIN-derived metadata (validity, contacts, bank) | ❌ | ✅ |
//! | Updatable from inbound PARTIN messages at runtime | ❌ | ✅ |
//! | Tenant-scoped isolation | ❌ | ✅ |
//! | Multiple communication channels per partner | ❌ | ✅ |
//! | Validity windows (Gültig Ab) for future-dated updates | ❌ | ✅ |
//!
//! # PARTIN data model
//!
//! The German energy market uses EDIFACT **PARTIN** messages (PIDs 37000–37014)
//! to distribute market-participant master data. Each PARTIN carries:
//!
//! - `NAD` → GLN, company name, country code
//! - `COM` → communication channels: AS4 endpoint URL, email, fax (up to 5)
//! - `CCI/CAV` → availability windows (*Erreichbarkeit*)
//! - `FII` → bank account (IBAN, BIC)
//! - `RFF` → tax number, VAT ID
//! - `CTA/NAD` → contact persons (*Ansprechpartner*)
//! - `DTM` → valid-from date (*Gültig Ab*)
//! - `CCI` → associated Bilanzkreis
//!
//! [`PartnerRecord`] captures all of these fields in a form that is both
//! serializable to SlateDB and constructible from static config.
//!
//! # Bootstrap pattern
//!
//! ```rust,ignore
//! // At startup — seed from makod.toml `[as4] partners` list:
//! for record in PartnerRecord::from_cli_pairs(&config.as4.partners)? {
//!     store.upsert(tenant_id, &record).await?;
//! }
//!
//! // Later — update from inbound PARTIN message:
//! let record = parse_partin_37001(&edifact_interchange)?;
//! store.upsert(tenant_id, &record).await?;
//!
//! // Outbound AS4 dispatch:
//! let partner = store.get(tenant_id, &gln).await?
//!     .ok_or(EngineError::partner(format!("no endpoint for {mp_id}")))?;
//! let endpoint = partner.as4_endpoint
//!     .ok_or(EngineError::partner(format!("{mp_id} has no AS4 endpoint")))?;
//! ```
//!
//! # Key schema (SlateDB)
//!
//! `pt/{tenant_id}/{mp_id}` → `JSON(PartnerRecord)`
//!
//! Both `TenantId` and GLN are fixed-width strings, giving a
//! `pt/{36-chars}/{13-chars}` prefix that bounds efficient per-tenant scans.

use std::sync::Arc;

#[cfg(any(test, feature = "testing"))]
use std::collections::HashMap;
#[cfg(any(test, feature = "testing"))]
use tokio::sync::RwLock;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::{error::EngineError, ids::TenantId, types::MarktpartnerCode};

// ── CommunicationChannel ──────────────────────────────────────────────────────

/// A single communication channel extracted from a PARTIN `COM` segment.
///
/// PARTIN allows up to 5 `COM` segments per party. The `qualifier` uses the
/// UN/EDIFACT DE 3155 code list:
///
/// | Qualifier | Meaning |
/// |---|---|
/// | `EM` | Electronic mail (primary) |
/// | `AK` | Electronic mail (alternative) |
/// | `TE` | Telephone |
/// | `FX` | Fax |
/// | `AS4` | BDEW AS4 endpoint URL (non-standard extension) |
/// | `AW` | BDEW API-Webdienste Strom endpoint URL (Verzeichnisdienst-discovered) |
///
/// > **Note**: BDEW uses qualifier `AK` for the AS4 endpoint URL in PARTIN
/// > AHB 1.0f. The `AS4` literal is used here as an explicit semantic label
/// > for channels that have already been identified as AS4 endpoints.
/// >
/// > `AW` is a project-internal qualifier used to store the API-Webdienste
/// > Strom base URL discovered from the BDEW Verzeichnisdienst.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommunicationChannel {
    /// DE 3155 communication qualifier (`EM`, `TE`, `FX`, `AK`, …).
    pub qualifier: Box<str>,
    /// The communication address (URL, email address, phone number).
    pub address: Box<str>,
}

impl CommunicationChannel {
    /// Construct a new channel.
    #[must_use]
    pub fn new(qualifier: impl Into<Box<str>>, address: impl Into<Box<str>>) -> Self {
        Self {
            qualifier: qualifier.into(),
            address: address.into(),
        }
    }

    /// Convenience: construct an AS4 endpoint channel.
    ///
    /// Uses qualifier `"AK"` per PARTIN AHB 1.0f DE 3155 convention.
    #[must_use]
    pub fn as4(endpoint_url: impl Into<Box<str>>) -> Self {
        Self::new("AK", endpoint_url)
    }

    /// Convenience: construct an email channel.
    #[must_use]
    pub fn email(address: impl Into<Box<str>>) -> Self {
        Self::new("EM", address)
    }

    /// Convenience: construct an API-Webdienste Strom endpoint channel.
    ///
    /// Uses qualifier `"AW"` (project-internal) to store the base URL
    /// discovered from the BDEW Verzeichnisdienst for a given partner.
    #[must_use]
    pub fn api_webdienste(base_url: impl Into<Box<str>>) -> Self {
        Self::new("AW", base_url)
    }
}

// ── ContactPerson ─────────────────────────────────────────────────────────────

/// A contact person extracted from a PARTIN `CTA`/`NAD`/`COM` group.
///
/// Corresponds to the *Ansprechpartner* group in PARTIN AHB 1.0f.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactPerson {
    /// Full name or department name.
    pub name: Box<str>,
    /// Contact channels (phone, email, …).
    pub channels: Vec<CommunicationChannel>,
}

// ── MarketRole ────────────────────────────────────────────────────────────────

/// The market role of a trading partner as declared in their PARTIN message.
///
/// Matches BDEW PARTIN Prüfidentifikator prefixes:
///
/// | PID | Role |
/// |---|---|
/// | 37000 | Lieferant Strom (`LfStrom`) |
/// | 37001 | Netzbetreiber Strom (`NbStrom`) |
/// | 37002 | Messstellenbetreiber Strom (`MsbStrom`) |
/// | 37003 | Bilanzkreisverantwortlicher Strom (`Bkv`) |
/// | 37004 | Bilanzkoordinator Strom (`Biko`) |
/// | 37005 | Übertragungsnetzbetreiber Strom (`Uenb`) |
/// | 37006 | Energiedienstleister/Serviceanbieter Strom (`Esa`) |
/// | 37008 | Lieferant Gas (`LfGas`) |
/// | 37009 | Netzbetreiber Gas (`NbGas`) |
/// | 37010 | Messstellenbetreiber Gas (`MsbGas`) |
/// | 37011 | Marktgebietsverantwortlicher Gas (`Mgv`) |
/// | 37012–37014 | Cross-commodity roles |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum MarketRole {
    /// Lieferant Strom (PID 37000)
    LfStrom,
    /// Netzbetreiber Strom (PID 37001)
    NbStrom,
    /// Messstellenbetreiber Strom (PID 37002)
    MsbStrom,
    /// Bilanzkreisverantwortlicher Strom (PID 37003)
    Bkv,
    /// Bilanzkoordinator Strom (PID 37004)
    Biko,
    /// Übertragungsnetzbetreiber Strom (PID 37005)
    Uenb,
    /// Energiedienstleister / Serviceanbieter Strom (PID 37006)
    Esa,
    /// Lieferant Gas (PID 37008)
    LfGas,
    /// Netzbetreiber Gas (PID 37009)
    NbGas,
    /// Messstellenbetreiber Gas (PID 37010)
    MsbGas,
    /// Marktgebietsverantwortlicher Gas (PID 37011)
    Mgv,
    /// Cross-commodity (PIDs 37012–37014)
    CrossCommodity,
}

impl MarketRole {
    /// Map a PARTIN Prüfidentifikator code to the corresponding `MarketRole`.
    ///
    /// Returns `None` for unrecognised codes.
    #[must_use]
    pub fn from_pid(pid: u32) -> Option<Self> {
        match pid {
            37000 => Some(Self::LfStrom),
            37001 => Some(Self::NbStrom),
            37002 => Some(Self::MsbStrom),
            37003 => Some(Self::Bkv),
            37004 => Some(Self::Biko),
            37005 => Some(Self::Uenb),
            37006 => Some(Self::Esa),
            37008 => Some(Self::LfGas),
            37009 => Some(Self::NbGas),
            37010 => Some(Self::MsbGas),
            37011 => Some(Self::Mgv),
            37012..=37014 => Some(Self::CrossCommodity),
            _ => None,
        }
    }
}

// ── PartnerRecord ─────────────────────────────────────────────────────────────

/// Full trading-partner master record as stored in the [`PartnerStore`].
///
/// Populated either from static `makod.toml` config (minimal — GLN + AS4 URL
/// only) or from an inbound PARTIN EDIFACT message (complete). Records from
/// different sources coexist: a bootstrapped config record is upgraded in-place
/// when the same partner later sends a PARTIN.
///
/// ## Constructors
///
/// - [`PartnerRecord::minimal`] — for bootstrapping from `GLN=URL` config pairs
/// - [`PartnerRecord::from_cli_pairs`] — parse `[as4] partners` list from config
///
/// ## Merging
///
/// Use [`PartnerRecord::merge_from_partin`] to update an existing record with
/// fields from a newer inbound PARTIN (respects validity dates).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartnerRecord {
    /// The partner's 13-digit Global Location Number.
    pub mp_id: MarktpartnerCode,

    /// Company name from the PARTIN `NAD` segment.
    pub display_name: Option<Box<str>>,

    /// All communication channels from PARTIN `COM` segments.
    ///
    /// The AS4 endpoint is the entry with qualifier `"AK"` (PARTIN AHB 1.0f
    /// DE 3155 convention).  Use [`as4_endpoint`] for direct access.
    ///
    /// [`as4_endpoint`]: PartnerRecord::as4_endpoint
    pub channels: Vec<CommunicationChannel>,

    /// Market roles this partner has declared via PARTIN.
    pub roles: Vec<MarketRole>,

    /// Date from which this record version is valid (`DTM/137`).
    ///
    /// `None` when bootstrapped from static config (no validity date known).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub valid_from: Option<OffsetDateTime>,

    /// Contact persons from the PARTIN *Ansprechpartner* group.
    pub contacts: Vec<ContactPerson>,

    /// ISO 3166-1 alpha-2 country code from `NAD+MS+++...+DE` (usually `DE`).
    pub country_code: Option<Box<str>>,

    /// Wall-clock time when this record was last written to the store.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl PartnerRecord {
    /// Create a minimal record from a GLN and an AS4 endpoint URL.
    ///
    /// Used when bootstrapping from `[as4] partners = ["GLN=URL", …]` in
    /// `makod.toml`. The record has no PARTIN-derived metadata — only the
    /// GLN and a single AS4 channel.
    #[must_use]
    pub fn minimal(mp_id: impl Into<MarktpartnerCode>, as4_url: impl Into<Box<str>>) -> Self {
        Self {
            mp_id: mp_id.into(),
            display_name: None,
            channels: vec![CommunicationChannel::as4(as4_url)],
            roles: Vec::new(),
            valid_from: None,
            contacts: Vec::new(),
            country_code: None,
            updated_at: OffsetDateTime::now_utc(),
        }
    }

    /// Parse `["GLN=HTTPS-URL", …]` configuration entries into minimal records.
    ///
    /// Returns an error on the first malformed or non-HTTPS entry.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Partner`] when an entry lacks `=`, has an empty
    /// GLN, or uses a non-HTTPS URL.
    pub fn from_cli_pairs(pairs: &[impl AsRef<str>]) -> Result<Vec<Self>, EngineError> {
        pairs
            .iter()
            .map(|entry| {
                let pair = entry.as_ref();
                let (mp_id, url) = pair.split_once('=').ok_or_else(|| {
                    EngineError::partner(format!(
                        "invalid partner entry {pair:?} — expected <GLN>=<HTTPS-URL>"
                    ))
                })?;
                let mp_id = mp_id.trim();
                let url = url.trim();
                if mp_id.is_empty() {
                    return Err(EngineError::partner(format!(
                        "invalid partner entry {pair:?} — GLN must not be empty"
                    )));
                }
                if !url.starts_with("https://") {
                    return Err(EngineError::partner(format!(
                        "invalid partner entry {pair:?} — endpoint URL must use HTTPS (got {url:?})"
                    )));
                }
                Ok(Self::minimal(mp_id, url))
            })
            .collect()
    }

    /// Return the AS4 endpoint URL if one has been registered.
    ///
    /// Looks for a channel with qualifier `"AK"` (PARTIN AHB 1.0f
    /// convention for the AS4 endpoint). Falls back to `"AS4"` for records
    /// that were imported with a non-standard qualifier.
    #[must_use]
    pub fn as4_endpoint(&self) -> Option<&str> {
        self.channels
            .iter()
            .find(|c| c.qualifier.as_ref() == "AK" || c.qualifier.as_ref() == "AS4")
            .map(|c| c.address.as_ref())
    }

    /// Return the primary email address if one has been registered.
    ///
    /// Looks for a channel with qualifier `"EM"`.
    #[must_use]
    pub fn email(&self) -> Option<&str> {
        self.channels
            .iter()
            .find(|c| c.qualifier.as_ref() == "EM")
            .map(|c| c.address.as_ref())
    }

    /// Return the API-Webdienste Strom base URL if one has been registered.
    ///
    /// Looks for a channel with qualifier `"AW"`.  This URL is typically
    /// populated by the Verzeichnisdienst discovery worker and is
    /// used by `MaloIdentSender` to reach the LF's callback endpoint.
    #[must_use]
    pub fn api_webdienste_endpoint(&self) -> Option<&str> {
        self.channels
            .iter()
            .find(|c| c.qualifier.as_ref() == "AW")
            .map(|c| c.address.as_ref())
    }

    /// Merge fields from a newer PARTIN-derived record into `self`.
    ///
    /// Only updates `self` when `incoming.valid_from` is newer than
    /// `self.valid_from` (or when `self.valid_from` is `None`). Config-
    /// bootstrapped records (no `valid_from`) are always overwritten.
    ///
    /// The GLN must match — mismatches are silently ignored (the caller is
    /// responsible for routing PARTIN messages to the correct record).
    pub fn merge_from_partin(&mut self, incoming: PartnerRecord) {
        if incoming.mp_id != self.mp_id {
            return;
        }
        let should_update = match (self.valid_from, incoming.valid_from) {
            (None, _) => true,
            (Some(_), None) => false, // keep the dated record
            (Some(a), Some(b)) => b >= a,
        };
        if !should_update {
            return;
        }
        self.display_name = incoming.display_name.or(self.display_name.take());
        self.channels = incoming.channels;
        self.roles = incoming.roles;
        self.valid_from = incoming.valid_from;
        self.contacts = incoming.contacts;
        self.country_code = incoming.country_code.or(self.country_code.take());
        self.updated_at = incoming.updated_at;
    }
}

// ── PartnerStore ──────────────────────────────────────────────────────────────

/// Durable store for trading-partner master records.
///
/// Provides tenant-scoped access to [`PartnerRecord`]s. Records are upserted
/// when a new PARTIN message arrives or when `makod` bootstraps from static
/// config.
///
/// All three operations are idempotent — reinserting the same record is safe.
///
/// ## Blanket `Arc` implementation
///
/// `Arc<S>` implements `PartnerStore` whenever `S: PartnerStore`.
#[allow(async_fn_in_trait)]
pub trait PartnerStore: Send + Sync {
    /// Insert or update the record for `(tenant_id, record.mp_id)`.
    ///
    /// If a record already exists for this GLN, it is **merged** via
    /// [`PartnerRecord::merge_from_partin`] — i.e. the newer PARTIN-derived
    /// record wins, but a config-only bootstrap is always overwritten.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Partner`] on storage failure.
    async fn upsert(&self, tenant_id: TenantId, record: &PartnerRecord) -> Result<(), EngineError>;

    /// Return the record for `(tenant_id, gln)`, or `None` if not registered.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Partner`] on storage failure.
    async fn get(
        &self,
        tenant_id: TenantId,
        mp_id: &MarktpartnerCode,
    ) -> Result<Option<PartnerRecord>, EngineError>;

    /// Remove the record for `(tenant_id, gln)`.
    ///
    /// No-op when the record does not exist.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Partner`] on storage failure.
    async fn remove(
        &self,
        tenant_id: TenantId,
        mp_id: &MarktpartnerCode,
    ) -> Result<(), EngineError>;

    /// Return all records registered for `tenant_id`.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Partner`] on storage failure.
    async fn list(&self, tenant_id: TenantId) -> Result<Vec<PartnerRecord>, EngineError>;

    /// Return the AS4 endpoint URL for `gln`, if known.
    ///
    /// Convenience wrapper over `get` + `as4_endpoint`.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Partner`] on storage failure.
    async fn as4_endpoint(
        &self,
        tenant_id: TenantId,
        mp_id: &MarktpartnerCode,
    ) -> Result<Option<Box<str>>, EngineError> {
        Ok(self
            .get(tenant_id, mp_id)
            .await?
            .and_then(|r| r.as4_endpoint().map(std::convert::Into::into)))
    }

    /// Return the API-Webdienste Strom base URL for `gln`, if known.
    ///
    /// Looks for a channel with qualifier `"AW"` (populated by the
    /// Verzeichnisdienst discovery path.
    ///
    /// Convenience wrapper over `get` + `api_webdienste_endpoint`.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Partner`] on storage failure.
    async fn api_webdienste_endpoint(
        &self,
        tenant_id: TenantId,
        mp_id: &MarktpartnerCode,
    ) -> Result<Option<Box<str>>, EngineError> {
        Ok(self
            .get(tenant_id, mp_id)
            .await?
            .and_then(|r| r.api_webdienste_endpoint().map(std::convert::Into::into)))
    }
}

// ── Arc<S> blanket impl ───────────────────────────────────────────────────────

impl<S: PartnerStore> PartnerStore for Arc<S> {
    async fn upsert(&self, tenant_id: TenantId, record: &PartnerRecord) -> Result<(), EngineError> {
        self.as_ref().upsert(tenant_id, record).await
    }

    async fn get(
        &self,
        tenant_id: TenantId,
        mp_id: &MarktpartnerCode,
    ) -> Result<Option<PartnerRecord>, EngineError> {
        self.as_ref().get(tenant_id, mp_id).await
    }

    async fn remove(
        &self,
        tenant_id: TenantId,
        mp_id: &MarktpartnerCode,
    ) -> Result<(), EngineError> {
        self.as_ref().remove(tenant_id, mp_id).await
    }

    async fn list(&self, tenant_id: TenantId) -> Result<Vec<PartnerRecord>, EngineError> {
        self.as_ref().list(tenant_id).await
    }
}

// ── NoopPartnerStore ──────────────────────────────────────────────────────────

/// A [`PartnerStore`] that never persists anything.
///
/// Every `get` returns `None`. Use as the default in deployments that rely
/// exclusively on static config-based partner lookup (i.e. when
/// `PartnerDirectory::from_cli_pairs` is sufficient).
///
/// ⚠️ **Data loss**: All upserts are silently discarded. PARTIN-derived
/// updates received at runtime will not be retained across restarts.
#[cfg_attr(
    not(any(test, feature = "testing")),
    deprecated = "NoopPartnerStore must not be instantiated in production builds; \
                  PARTIN-derived partner updates will be silently discarded. \
                  Use SlateDbPartnerStore or another durable PartnerStore instead."
)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopPartnerStore;

// The `#[allow(deprecated)]` is required because the `deprecated` attribute on
// `NoopPartnerStore` fires on the impl block inside the same file. This is a
// known Rust quirk (implementing a deprecated type fires the lint even in the
// defining module). The guard is still effective: *callers* that instantiate
// `NoopPartnerStore` outside of test/feature-gated code will see the warning.
#[allow(deprecated)]
impl PartnerStore for NoopPartnerStore {
    async fn upsert(
        &self,
        _tenant_id: TenantId,
        _record: &PartnerRecord,
    ) -> Result<(), EngineError> {
        Ok(())
    }

    async fn get(
        &self,
        _tenant_id: TenantId,
        _mp_id: &MarktpartnerCode,
    ) -> Result<Option<PartnerRecord>, EngineError> {
        Ok(None)
    }

    async fn remove(
        &self,
        _tenant_id: TenantId,
        _mp_id: &MarktpartnerCode,
    ) -> Result<(), EngineError> {
        Ok(())
    }

    async fn list(&self, _tenant_id: TenantId) -> Result<Vec<PartnerRecord>, EngineError> {
        Ok(vec![])
    }
}

// ── InMemoryPartnerStore ──────────────────────────────────────────────────────

/// An in-memory [`PartnerStore`] for tests and development.
///
/// Backed by a `HashMap<(TenantId, MarktpartnerCode), PartnerRecord>` protected by an
/// `Arc<RwLock<…>>`. Clones share the underlying data — all clones see the
/// same records. Upsert calls `merge_from_partin` for existing records.
///
/// Only available in `#[cfg(test)]` or with the `testing` feature enabled.
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Clone, Default)]
pub struct InMemoryPartnerStore {
    inner: Arc<RwLock<HashMap<(TenantId, MarktpartnerCode), PartnerRecord>>>,
}

#[cfg(any(test, feature = "testing"))]
impl InMemoryPartnerStore {
    /// Create a new empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(any(test, feature = "testing"))]
impl PartnerStore for InMemoryPartnerStore {
    async fn upsert(&self, tenant_id: TenantId, record: &PartnerRecord) -> Result<(), EngineError> {
        let mut guard = self.inner.write().await;
        let key = (tenant_id, record.mp_id.clone());
        match guard.get_mut(&key) {
            Some(existing) => existing.merge_from_partin(record.clone()),
            None => {
                guard.insert(key, record.clone());
            }
        }
        Ok(())
    }

    async fn get(
        &self,
        tenant_id: TenantId,
        mp_id: &MarktpartnerCode,
    ) -> Result<Option<PartnerRecord>, EngineError> {
        Ok(self
            .inner
            .read()
            .await
            .get(&(tenant_id, mp_id.clone()))
            .cloned())
    }

    async fn remove(
        &self,
        tenant_id: TenantId,
        mp_id: &MarktpartnerCode,
    ) -> Result<(), EngineError> {
        self.inner.write().await.remove(&(tenant_id, mp_id.clone()));
        Ok(())
    }

    async fn list(&self, tenant_id: TenantId) -> Result<Vec<PartnerRecord>, EngineError> {
        Ok(self
            .inner
            .read()
            .await
            .iter()
            .filter(|((tid, _), _)| *tid == tenant_id)
            .map(|(_, record)| record.clone())
            .collect())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mp_id(s: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(s)
    }
    fn tid() -> TenantId {
        TenantId::new()
    }

    fn minimal_record(gln_str: &str, url: &str) -> PartnerRecord {
        PartnerRecord::minimal(mp_id(gln_str), url)
    }

    // ── from_cli_pairs ────────────────────────────────────────────────────────

    #[test]
    fn from_cli_pairs_parses_valid_entries() {
        let pairs = vec![
            "9900000000002=https://partner-a.example/as4/inbox",
            "9900000000003=https://partner-b.example/as4/inbox",
        ];
        let records = PartnerRecord::from_cli_pairs(&pairs).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].mp_id.as_str(), "9900000000002");
        assert_eq!(
            records[0].as4_endpoint(),
            Some("https://partner-a.example/as4/inbox")
        );
        assert_eq!(records[1].mp_id.as_str(), "9900000000003");
    }

    #[test]
    fn from_cli_pairs_rejects_missing_equals() {
        let pairs = vec!["9900000000002https://no-equals.example"];
        assert!(PartnerRecord::from_cli_pairs(&pairs).is_err());
    }

    #[test]
    fn from_cli_pairs_rejects_http_url() {
        let pairs = vec!["9900000000002=http://insecure.example/as4"];
        assert!(PartnerRecord::from_cli_pairs(&pairs).is_err());
    }

    #[test]
    fn from_cli_pairs_rejects_empty_gln() {
        let pairs = vec!["=https://no-mp_id.example/as4"];
        assert!(PartnerRecord::from_cli_pairs(&pairs).is_err());
    }

    // ── as4_endpoint ──────────────────────────────────────────────────────────

    #[test]
    fn as4_endpoint_returns_ak_channel() {
        let r = minimal_record("9900000000002", "https://a.example/as4");
        assert_eq!(r.as4_endpoint(), Some("https://a.example/as4"));
    }

    #[test]
    fn as4_endpoint_returns_none_when_absent() {
        let r = PartnerRecord {
            mp_id: mp_id("9900000000002"),
            display_name: None,
            channels: vec![CommunicationChannel::email("info@example.de")],
            roles: vec![],
            valid_from: None,
            contacts: vec![],
            country_code: None,
            updated_at: OffsetDateTime::now_utc(),
        };
        assert!(r.as4_endpoint().is_none());
    }

    // ── merge_from_partin ─────────────────────────────────────────────────────

    #[test]
    fn merge_overwrites_config_record_with_partin_data() {
        let mut base = minimal_record("9900000000002", "https://old.example/as4");
        let newer = PartnerRecord {
            mp_id: mp_id("9900000000002"),
            display_name: Some("Stadtwerke AG".into()),
            channels: vec![
                CommunicationChannel::as4("https://new.example/as4"),
                CommunicationChannel::email("edifact@sw.example"),
            ],
            roles: vec![MarketRole::NbStrom],
            valid_from: Some(OffsetDateTime::now_utc()),
            contacts: vec![],
            country_code: Some("DE".into()),
            updated_at: OffsetDateTime::now_utc(),
        };
        base.merge_from_partin(newer.clone());
        assert_eq!(base.as4_endpoint(), Some("https://new.example/as4"));
        assert_eq!(base.display_name.as_deref(), Some("Stadtwerke AG"));
        assert_eq!(base.roles, vec![MarketRole::NbStrom]);
    }

    #[test]
    fn merge_ignores_older_partin() {
        use time::Duration;
        let old_ts = OffsetDateTime::now_utc() - Duration::days(30);
        let new_ts = OffsetDateTime::now_utc();

        let mut current = PartnerRecord {
            mp_id: mp_id("9900000000002"),
            display_name: Some("Current Name".into()),
            channels: vec![CommunicationChannel::as4("https://current.example/as4")],
            roles: vec![MarketRole::NbStrom],
            valid_from: Some(new_ts),
            contacts: vec![],
            country_code: Some("DE".into()),
            updated_at: OffsetDateTime::now_utc(),
        };

        let stale = PartnerRecord {
            mp_id: mp_id("9900000000002"),
            display_name: Some("Stale Name".into()),
            channels: vec![CommunicationChannel::as4("https://stale.example/as4")],
            roles: vec![],
            valid_from: Some(old_ts),
            contacts: vec![],
            country_code: None,
            updated_at: OffsetDateTime::now_utc(),
        };

        current.merge_from_partin(stale);
        // Should not be overwritten
        assert_eq!(current.display_name.as_deref(), Some("Current Name"));
        assert_eq!(current.as4_endpoint(), Some("https://current.example/as4"));
    }

    #[test]
    fn merge_ignores_wrong_gln() {
        let mut r = minimal_record("9900000000002", "https://a.example/as4");
        let other = minimal_record("9900000000003", "https://b.example/as4");
        r.merge_from_partin(other);
        assert_eq!(r.as4_endpoint(), Some("https://a.example/as4"));
    }

    // ── MarketRole::from_pid ──────────────────────────────────────────────────

    #[test]
    fn market_role_from_pid_covers_all_partin_pids() {
        for pid in [
            37000u32, 37001, 37002, 37003, 37004, 37005, 37006, 37008, 37009, 37010, 37011, 37012,
            37013, 37014,
        ] {
            assert!(
                MarketRole::from_pid(pid).is_some(),
                "MarketRole::from_pid({pid}) should return Some"
            );
        }
        // PID 37007 is not in the AHB (gap)
        assert!(MarketRole::from_pid(37007).is_none());
        assert!(MarketRole::from_pid(0).is_none());
    }

    // ── InMemoryPartnerStore ──────────────────────────────────────────────────

    #[tokio::test]
    async fn in_memory_upsert_and_get() {
        let store = InMemoryPartnerStore::new();
        let tenant = tid();
        let record = minimal_record("9900000000001", "https://a.example/as4");

        store.upsert(tenant, &record).await.unwrap();
        let found = store
            .get(tenant, &mp_id("9900000000001"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.as4_endpoint(), Some("https://a.example/as4"));
    }

    #[tokio::test]
    async fn in_memory_get_returns_none_for_unknown() {
        let store = InMemoryPartnerStore::new();
        assert!(
            store
                .get(tid(), &mp_id("9900000000099"))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn in_memory_upsert_merges_into_existing() {
        let store = InMemoryPartnerStore::new();
        let tenant = tid();
        let base = minimal_record("9900000000001", "https://old.example/as4");
        store.upsert(tenant, &base).await.unwrap();

        let newer = PartnerRecord {
            mp_id: mp_id("9900000000001"),
            display_name: Some("Partner AG".into()),
            channels: vec![CommunicationChannel::as4("https://new.example/as4")],
            roles: vec![MarketRole::LfStrom],
            valid_from: Some(OffsetDateTime::now_utc()),
            contacts: vec![],
            country_code: Some("DE".into()),
            updated_at: OffsetDateTime::now_utc(),
        };
        store.upsert(tenant, &newer).await.unwrap();

        let found = store
            .get(tenant, &mp_id("9900000000001"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.as4_endpoint(), Some("https://new.example/as4"));
        assert_eq!(found.display_name.as_deref(), Some("Partner AG"));
    }

    #[tokio::test]
    async fn in_memory_remove_clears_record() {
        let store = InMemoryPartnerStore::new();
        let tenant = tid();
        let record = minimal_record("9900000000001", "https://a.example/as4");

        store.upsert(tenant, &record).await.unwrap();
        store.remove(tenant, &mp_id("9900000000001")).await.unwrap();
        assert!(
            store
                .get(tenant, &mp_id("9900000000001"))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn in_memory_list_is_tenant_scoped() {
        let store = InMemoryPartnerStore::new();
        let t1 = tid();
        let t2 = tid();

        store
            .upsert(
                t1,
                &minimal_record("9900000000001", "https://a.example/as4"),
            )
            .await
            .unwrap();
        store
            .upsert(
                t2,
                &minimal_record("9900000000002", "https://b.example/as4"),
            )
            .await
            .unwrap();

        let t1_list = store.list(t1).await.unwrap();
        assert_eq!(t1_list.len(), 1);
        assert_eq!(t1_list[0].mp_id.as_str(), "9900000000001");

        let t2_list = store.list(t2).await.unwrap();
        assert_eq!(t2_list.len(), 1);
        assert_eq!(t2_list[0].mp_id.as_str(), "9900000000002");
    }

    #[tokio::test]
    async fn as4_endpoint_convenience_method() {
        let store = InMemoryPartnerStore::new();
        let tenant = tid();
        let record = minimal_record("9900000000001", "https://a.example/as4");

        store.upsert(tenant, &record).await.unwrap();
        let url = store
            .as4_endpoint(tenant, &mp_id("9900000000001"))
            .await
            .unwrap();
        assert_eq!(url.as_deref(), Some("https://a.example/as4"));

        let none = store
            .as4_endpoint(tenant, &mp_id("9900000000099"))
            .await
            .unwrap();
        assert!(none.is_none());
    }
}

//! Process routing registry.
//!
//! Maps string routing keys to [`ProcessIdentity`] values so inbound
//! EDIFACT messages can be dispatched to the correct running process without
//! the caller managing a bespoke routing table.
//!
//! # Routing key conventions
//!
//! Any stable string that uniquely identifies a process for a given message
//! type works as a routing key. Common patterns:
//!
//! | Message type | Recommended key |
//! |---|---|
//! | UTILMD waiting for APERAK | `RegistryKey::from_conversation_and_sender(conversation_id, sender_gln)` |
//! | Route follow-up by correlation | `RegistryKey::from_correlation(correlation_id)` |
//! | Direct lookup by process | `RegistryKey::from_process(process_id)` |
//!
//! One process may be registered under multiple keys when it handles several
//! different message types simultaneously.
//!
//! # Tenant scoping
//!
//! All registry operations are scoped to a `TenantId`. This prevents routing
//! keys from leaking across tenant boundaries when the engine handles multiple
//! market participants in a single deployment.
//!
//! # Usage
//!
//! ```rust,ignore
//! // After spawning a process, register it under the UTILMD conversation ID + sender GLN:
//! ctx.registry
//!     .register(tenant_id, &RegistryKey::from_conversation_and_sender(utilmd_conv_id, sender_gln), process.identity())
//!     .await?;
//!
//! // When the APERAK arrives, look up by conversation ID + APERAK sender GLN:
//! let identity = ctx.registry
//!     .lookup(tenant_id, &RegistryKey::from_conversation_and_sender(aperak_conv_id, aperak_sender_gln))
//!     .await?
//!     .ok_or(EngineError::registry("unknown conversation"))?;
//!
//! let process = ctx.resume::<SupplierChangeWorkflow>(identity);
//! process.execute(HandleAperak { .. }).await?;
//!
//! // Clean up after process completion:
//! ctx.registry.remove(tenant_id, &RegistryKey::from_conversation_and_sender(utilmd_conv_id, sender_gln)).await?;
//! ```

use std::{fmt, sync::Arc};

#[cfg(any(test, feature = "testing"))]
use std::collections::HashMap;
#[cfg(any(test, feature = "testing"))]
use tokio::sync::RwLock;

use crate::{
    error::EngineError,
    ids::{ConversationId, CorrelationId, ProcessId, ProcessIdentity, TenantId},
};

/// Maximum byte length for a [`RegistryKey`] routing key.
///
/// Keys beyond this limit are rejected by [`RegistryKey::parse`] to
/// prevent oversized LSM keys from bloating the `pr/` key namespace in
/// SlateDB. AS4 `MessageId` values and EDIFACT correlation identifiers are
/// typically ≤ 36 bytes (UUID); 256 bytes provides ample headroom.
pub const MAX_REGISTRY_KEY_LEN: usize = 256;

// ── RegistryKey ───────────────────────────────────────────────────────────────

/// A typed routing key for the [`ProcessRegistry`].
///
/// Using a newtype instead of a bare `&str` prevents accidental key-format
/// mismatches (e.g. mixing `conversation_id` and `correlation_id` keys at
/// different call sites) and makes the key derivation convention explicit at
/// the type level.
///
/// # Named constructors
///
/// | Constructor | Use case |
/// |---|---|
/// | Constructor | Use case |
/// |---|---|
/// | [`from_conversation_and_sender`] | Route inbound APERAK by UTILMD conversation ID + sender GLN |
/// | [`from_correlation`] | Route follow-up messages by root correlation |
/// | [`from_process`] | Direct lookup by process instance |
/// | [`parse`] | Primary validated constructor for runtime-derived keys |
/// | [`from_static`] | Infallible constructor for compile-time-known string literals |
///
/// [`from_conversation_and_sender`]: RegistryKey::from_conversation_and_sender
/// [`from_correlation`]: RegistryKey::from_correlation
/// [`from_process`]: RegistryKey::from_process
/// [`parse`]: RegistryKey::parse
/// [`from_static`]: RegistryKey::from_static
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct RegistryKey(Box<str>);

impl RegistryKey {
    /// Key derived from a conversation ID **and sender GLN**.
    ///
    /// This is the correct constructor for UTILMD ↔ APERAK routing in
    /// multi-market-participant deployments.
    ///
    /// # Why sender is required
    ///
    /// EDIFACT conversation IDs are assigned by the sender within their own
    /// numbering space.  Two senders may independently assign the same
    /// conversation-ID string, which would collide if keyed on conversation
    /// alone.  Including the sender GLN as a discriminator makes the key
    /// globally unique within the tenant's namespace.
    ///
    /// # Key format
    ///
    /// `"{sender_gln}:{conversation_id}"` — stable, URL-safe, human-readable.
    #[must_use]
    pub fn from_conversation_and_sender(id: ConversationId, sender_gln: &str) -> Self {
        let key = format!("{sender_gln}:{id}");
        Self(key.into_boxed_str())
    }

    /// Key derived from a correlation ID (route all messages in the same root trace).
    #[must_use]
    pub fn from_correlation(id: CorrelationId) -> Self {
        Self(id.to_string().into_boxed_str())
    }

    /// Key derived from a process ID (direct process lookup).
    #[must_use]
    pub fn from_process(id: ProcessId) -> Self {
        Self(id.to_string().into_boxed_str())
    }

    /// Primary validated constructor for runtime-derived keys.
    ///
    /// Returns [`EngineError::Registry`] when `s` contains a NUL byte or
    /// exceeds [`MAX_REGISTRY_KEY_LEN`] bytes. Use this for all keys derived
    /// from untrusted input (e.g. EDIFACT `MessageId`, AS4 `conversation_id`,
    /// or user-supplied strings).
    ///
    /// # Errors
    ///
    /// - [`EngineError::Registry`] when `s` contains `\0`.
    /// - [`EngineError::Registry`] when `s.len() > MAX_REGISTRY_KEY_LEN`.
    pub fn parse(s: &str) -> Result<Self, EngineError> {
        if s.contains('\0') {
            return Err(EngineError::registry(
                "registry key must not contain NUL bytes",
            ));
        }
        if s.len() > MAX_REGISTRY_KEY_LEN {
            return Err(EngineError::registry(format!(
                "registry key is {} bytes, exceeds maximum of {MAX_REGISTRY_KEY_LEN}",
                s.len()
            )));
        }
        Ok(Self(s.into()))
    }

    /// Infallible constructor for **compile-time-known** string literals.
    ///
    /// Panics at runtime if `s` contains a NUL byte or exceeds
    /// [`MAX_REGISTRY_KEY_LEN`] — but since this is only correct to call with
    /// string literals, any violation would be caught immediately in tests.
    ///
    /// # Panics
    ///
    /// Panics when `s` contains a NUL byte or exceeds [`MAX_REGISTRY_KEY_LEN`]
    /// bytes. **Only use this with string literals.** Use [`parse`] for any
    /// value that may be runtime-derived.
    ///
    /// [`parse`]: RegistryKey::parse
    #[must_use]
    pub fn from_static(s: &'static str) -> Self {
        assert!(
            !s.contains('\0'),
            "RegistryKey::from_static: key must not contain NUL bytes"
        );
        assert!(
            s.len() <= MAX_REGISTRY_KEY_LEN,
            "RegistryKey::from_static: key exceeds MAX_REGISTRY_KEY_LEN ({MAX_REGISTRY_KEY_LEN} bytes)"
        );
        Self(s.into())
    }

    /// The raw key string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RegistryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for RegistryKey {
    type Err = EngineError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        RegistryKey::parse(s)
    }
}

// ── ProcessRegistry ───────────────────────────────────────────────────────────

/// Routes inbound messages to their target processes by string key.
///
/// A `ProcessRegistry` decouples message routing from process creation.
/// Register a [`ProcessIdentity`] under a stable key at process creation
/// time, then look it up by that key when routing subsequent inbound messages.
///
/// All operations are scoped to a [`TenantId`] so routing entries from
/// different market participants cannot collide.
///
/// ## Correlated-process lookup (1:many)
///
/// The standard `register`/`lookup` API maps a key 1:1 to a single process.
/// For cases where multiple processes share a common business identifier —
/// for example, all MSCONS measurement-data processes for a single MaLo ID
/// in MABIS billing aggregation — use the correlated index:
///
/// - [`register_correlated`]: associate a `(tenant, tag, process_id)` triple.
/// - [`lookup_correlated`]: retrieve **all** `ProcessIdentity` values for a tag.
/// - [`remove_correlated`]: remove a single process from the tag's fan-out set.
///
/// The tag is an arbitrary opaque string (e.g. a MaLo ID such as
/// `"DE0001234567890"`). Key validation rules match [`RegistryKey`].
///
/// ```rust,ignore
/// // Register all MSCONS processes for a Bilanzkreis MaLo:
/// for process in &mscons_processes {
///     ctx.registry
///         .register_correlated(tenant_id, malo_id, process.process_id(), process.identity())
///         .await?;
/// }
///
/// // Retrieve all processes for billing aggregation:
/// let identities = ctx.registry
///     .lookup_correlated(tenant_id, malo_id)
///     .await?;
/// ```
///
/// ## Blanket `Arc` implementation
///
/// `Arc<S>` implements `ProcessRegistry` whenever `S: ProcessRegistry`,
/// enabling shared access from multiple concurrent message handlers.
///
/// [`register_correlated`]: ProcessRegistry::register_correlated
/// [`lookup_correlated`]:   ProcessRegistry::lookup_correlated
/// [`remove_correlated`]:   ProcessRegistry::remove_correlated
#[allow(async_fn_in_trait)]
pub trait ProcessRegistry: Send + Sync {
    /// Associate `key` with `identity` for the given `tenant_id`.
    ///
    /// Overwrites any existing mapping for the `(tenant_id, key)` pair
    /// (upsert semantics).
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Registry`] on storage failure.
    async fn register(
        &self,
        tenant_id: TenantId,
        key: &RegistryKey,
        identity: ProcessIdentity,
    ) -> Result<(), EngineError>;

    /// Return the identity associated with `(tenant_id, key)`, or `None`
    /// if not registered.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Registry`] on storage failure.
    async fn lookup(
        &self,
        tenant_id: TenantId,
        key: &RegistryKey,
    ) -> Result<Option<ProcessIdentity>, EngineError>;

    /// Remove the mapping for `(tenant_id, key)`. No-op if not found.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Registry`] on storage failure.
    async fn remove(&self, tenant_id: TenantId, key: &RegistryKey) -> Result<(), EngineError>;

    /// Return `true` when `(tenant_id, key)` has a registered mapping.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Registry`] on storage failure.
    async fn contains(&self, tenant_id: TenantId, key: &RegistryKey) -> Result<bool, EngineError> {
        Ok(self.lookup(tenant_id, key).await?.is_some())
    }

    /// Total number of registered routing keys across all tenants.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Registry`] on storage failure.
    async fn len(&self) -> Result<usize, EngineError>;

    /// Return `true` when no routing keys are registered.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Registry`] on storage failure.
    async fn is_empty(&self) -> Result<bool, EngineError> {
        Ok(self.len().await? == 0)
    }

    // ── Correlated (1:many) index ────────────────────────────────────────────

    /// Associate `process_id`/`identity` with the correlation `tag` for the
    /// given `tenant_id`.
    ///
    /// Multiple processes can be registered under the same `(tenant_id, tag)`,
    /// making `lookup_correlated` return all of them. This is the fan-out
    /// counterpart to the 1:1 `register`/`lookup` API.
    ///
    /// # Tag constraints
    ///
    /// Same validation as [`RegistryKey`]: must not contain `\0`, must be
    /// ≤ [`MAX_REGISTRY_KEY_LEN`] bytes.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Registry`] on storage failure or invalid tag.
    async fn register_correlated(
        &self,
        tenant_id: TenantId,
        tag: &str,
        process_id: crate::ids::ProcessId,
        identity: ProcessIdentity,
    ) -> Result<(), EngineError>;

    /// Return all `ProcessIdentity` values registered under `(tenant_id, tag)`.
    ///
    /// Returns an empty `Vec` when no entries exist for the tag.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Registry`] on storage failure.
    async fn lookup_correlated(
        &self,
        tenant_id: TenantId,
        tag: &str,
    ) -> Result<Vec<ProcessIdentity>, EngineError>;

    /// Remove the `process_id` entry from the `(tenant_id, tag)` fan-out set.
    ///
    /// No-op when the entry does not exist.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Registry`] on storage failure.
    async fn remove_correlated(
        &self,
        tenant_id: TenantId,
        tag: &str,
        process_id: crate::ids::ProcessId,
    ) -> Result<(), EngineError>;
}

// ── Arc<S> blanket impl ───────────────────────────────────────────────────────

impl<S: ProcessRegistry> ProcessRegistry for Arc<S> {
    async fn register(
        &self,
        tenant_id: TenantId,
        key: &RegistryKey,
        identity: ProcessIdentity,
    ) -> Result<(), EngineError> {
        self.as_ref().register(tenant_id, key, identity).await
    }

    async fn lookup(
        &self,
        tenant_id: TenantId,
        key: &RegistryKey,
    ) -> Result<Option<ProcessIdentity>, EngineError> {
        self.as_ref().lookup(tenant_id, key).await
    }

    async fn remove(&self, tenant_id: TenantId, key: &RegistryKey) -> Result<(), EngineError> {
        self.as_ref().remove(tenant_id, key).await
    }

    async fn len(&self) -> Result<usize, EngineError> {
        self.as_ref().len().await
    }

    async fn register_correlated(
        &self,
        tenant_id: TenantId,
        tag: &str,
        process_id: crate::ids::ProcessId,
        identity: ProcessIdentity,
    ) -> Result<(), EngineError> {
        self.as_ref()
            .register_correlated(tenant_id, tag, process_id, identity)
            .await
    }

    async fn lookup_correlated(
        &self,
        tenant_id: TenantId,
        tag: &str,
    ) -> Result<Vec<ProcessIdentity>, EngineError> {
        self.as_ref().lookup_correlated(tenant_id, tag).await
    }

    async fn remove_correlated(
        &self,
        tenant_id: TenantId,
        tag: &str,
        process_id: crate::ids::ProcessId,
    ) -> Result<(), EngineError> {
        self.as_ref()
            .remove_correlated(tenant_id, tag, process_id)
            .await
    }
}

// ── NoopProcessRegistry ───────────────────────────────────────────────────────

/// A [`ProcessRegistry`] that never stores any mappings.
///
/// Every `lookup` returns `None`. Use this as the default when routing is
/// managed externally or not required.
///
/// # ⚠️ Routing loss warning
///
/// `NoopProcessRegistry` **discards every routing registration silently**.
/// All `lookup` calls return `None`. Inbound messages for existing processes
/// will not be routed. Do not use in production when message routing is needed.
///
/// This type is available in all build configurations so it can serve as a
/// default type parameter in [`EngineBuilder`]. However, [`EngineBuilder::new`]
/// (which wires this as the default) is only available with the `testing`
/// feature or in `cfg(test)`. Production binaries must call
/// [`EngineBuilder::with_stores`] instead.
///
/// [`EngineBuilder`]: crate::builder::EngineBuilder
/// [`EngineBuilder::new`]: crate::builder::EngineBuilder::new
/// [`EngineBuilder::with_stores`]: crate::builder::EngineBuilder::with_stores
#[derive(Debug, Clone, Copy, Default)]
#[must_use = "NoopProcessRegistry discards all routing registrations silently — use a persistent ProcessRegistry in production"]
pub struct NoopProcessRegistry;

#[cfg(any(test, feature = "testing"))]
impl ProcessRegistry for NoopProcessRegistry {
    async fn register(
        &self,
        _tenant_id: TenantId,
        _key: &RegistryKey,
        _identity: ProcessIdentity,
    ) -> Result<(), EngineError> {
        Ok(())
    }

    async fn lookup(
        &self,
        _tenant_id: TenantId,
        _key: &RegistryKey,
    ) -> Result<Option<ProcessIdentity>, EngineError> {
        Ok(None)
    }

    async fn remove(&self, _tenant_id: TenantId, _key: &RegistryKey) -> Result<(), EngineError> {
        Ok(())
    }

    async fn len(&self) -> Result<usize, EngineError> {
        Ok(0)
    }

    async fn register_correlated(
        &self,
        _tenant_id: TenantId,
        _tag: &str,
        _process_id: crate::ids::ProcessId,
        _identity: ProcessIdentity,
    ) -> Result<(), EngineError> {
        Ok(())
    }

    async fn lookup_correlated(
        &self,
        _tenant_id: TenantId,
        _tag: &str,
    ) -> Result<Vec<ProcessIdentity>, EngineError> {
        Ok(vec![])
    }

    async fn remove_correlated(
        &self,
        _tenant_id: TenantId,
        _tag: &str,
        _process_id: crate::ids::ProcessId,
    ) -> Result<(), EngineError> {
        Ok(())
    }
}

// ── InMemoryProcessRegistry ───────────────────────────────────────────────────

/// An in-memory [`ProcessRegistry`] for tests and development.
///
/// Backed by a `HashMap<(TenantId, String), ProcessIdentity>` protected by a
/// `RwLock`. Cloning shares the underlying data via `Arc` — all clones see the
/// same mappings.
///
/// Use this with [`EngineContext`] to verify message routing without
/// depending on an external registry service.
///
/// Only available in `#[cfg(test)]` or with the `testing` feature enabled.
///
/// [`EngineContext`]: crate::builder::EngineContext
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Default, Clone)]
pub struct InMemoryProcessRegistry {
    #[allow(clippy::type_complexity)]
    inner: Arc<RwLock<HashMap<(TenantId, Box<str>), ProcessIdentity>>>,
    /// Correlated 1:many index: `(tenant_id, tag, process_id)` → `ProcessIdentity`.
    #[allow(clippy::type_complexity)]
    correlated: Arc<RwLock<HashMap<(TenantId, Box<str>, crate::ids::ProcessId), ProcessIdentity>>>,
}

#[cfg(any(test, feature = "testing"))]
impl InMemoryProcessRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return `true` when no routing keys are registered.
    pub async fn is_empty_async(&self) -> bool {
        self.inner.read().await.is_empty()
    }
}

#[cfg(any(test, feature = "testing"))]
impl ProcessRegistry for InMemoryProcessRegistry {
    async fn register(
        &self,
        tenant_id: TenantId,
        key: &RegistryKey,
        identity: ProcessIdentity,
    ) -> Result<(), EngineError> {
        self.inner
            .write()
            .await
            .insert((tenant_id, key.0.clone()), identity);
        Ok(())
    }

    async fn lookup(
        &self,
        tenant_id: TenantId,
        key: &RegistryKey,
    ) -> Result<Option<ProcessIdentity>, EngineError> {
        Ok(self
            .inner
            .read()
            .await
            .get(&(tenant_id, key.0.clone()))
            .cloned())
    }

    async fn remove(&self, tenant_id: TenantId, key: &RegistryKey) -> Result<(), EngineError> {
        self.inner.write().await.remove(&(tenant_id, key.0.clone()));
        Ok(())
    }

    async fn len(&self) -> Result<usize, EngineError> {
        Ok(self.inner.read().await.len())
    }

    async fn register_correlated(
        &self,
        tenant_id: TenantId,
        tag: &str,
        process_id: crate::ids::ProcessId,
        identity: ProcessIdentity,
    ) -> Result<(), EngineError> {
        self.correlated
            .write()
            .await
            .insert((tenant_id, tag.into(), process_id), identity);
        Ok(())
    }

    async fn lookup_correlated(
        &self,
        tenant_id: TenantId,
        tag: &str,
    ) -> Result<Vec<ProcessIdentity>, EngineError> {
        let guard = self.correlated.read().await;
        let result = guard
            .iter()
            .filter(|((tid, t, _), _)| *tid == tenant_id && t.as_ref() == tag)
            .map(|(_, identity)| identity.clone())
            .collect();
        Ok(result)
    }

    async fn remove_correlated(
        &self,
        tenant_id: TenantId,
        tag: &str,
        process_id: crate::ids::ProcessId,
    ) -> Result<(), EngineError> {
        self.correlated
            .write()
            .await
            .remove(&(tenant_id, tag.into(), process_id));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ids::{ProcessId, TenantId},
        version::WorkflowId,
    };

    fn make_identity() -> ProcessIdentity {
        let pid = ProcessId::new();
        ProcessIdentity::new(
            pid,
            TenantId::new(),
            WorkflowId::new("test", "FV2024-10-01"),
        )
    }

    fn tid() -> TenantId {
        TenantId::new()
    }

    fn key(s: &str) -> RegistryKey {
        RegistryKey::parse(s).expect("valid test key")
    }

    #[tokio::test]
    async fn register_and_lookup() {
        let reg = InMemoryProcessRegistry::new();
        let tenant = tid();
        let id = make_identity();
        reg.register(tenant, &key("conv:abc"), id.clone())
            .await
            .unwrap();
        let found = reg
            .lookup(tenant, &key("conv:abc"))
            .await
            .unwrap()
            .expect("must be found");
        assert_eq!(found.process_id, id.process_id);
    }

    #[tokio::test]
    async fn lookup_returns_none_for_unknown_key() {
        let reg = InMemoryProcessRegistry::new();
        assert!(reg.lookup(tid(), &key("unknown")).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn remove_clears_mapping() {
        let reg = InMemoryProcessRegistry::new();
        let tenant = tid();
        let id = make_identity();
        reg.register(tenant, &key("k1"), id).await.unwrap();
        reg.remove(tenant, &key("k1")).await.unwrap();
        assert!(reg.lookup(tenant, &key("k1")).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn upsert_overwrites_existing() {
        let reg = InMemoryProcessRegistry::new();
        let tenant = tid();
        let id1 = make_identity();
        let id2 = make_identity();
        reg.register(tenant, &key("k1"), id1).await.unwrap();
        reg.register(tenant, &key("k1"), id2.clone()).await.unwrap();
        let found = reg.lookup(tenant, &key("k1")).await.unwrap().unwrap();
        assert_eq!(found.process_id, id2.process_id);
        assert_eq!(
            reg.len().await.unwrap(),
            1,
            "upsert must not duplicate the key"
        );
    }

    #[tokio::test]
    async fn contains_matches_register() {
        let reg = InMemoryProcessRegistry::new();
        let tenant = tid();
        assert!(!reg.contains(tenant, &key("k1")).await.unwrap());
        reg.register(tenant, &key("k1"), make_identity())
            .await
            .unwrap();
        assert!(reg.contains(tenant, &key("k1")).await.unwrap());
    }

    #[tokio::test]
    async fn clone_shares_state() {
        let reg1 = InMemoryProcessRegistry::new();
        let reg2 = reg1.clone();
        let tenant = tid();
        reg1.register(tenant, &key("k1"), make_identity())
            .await
            .unwrap();
        assert!(reg2.contains(tenant, &key("k1")).await.unwrap());
    }

    /// `from_conversation_and_sender` key contains sender prefix.
    #[test]
    fn from_conversation_and_sender_key_contains_sender() {
        use crate::ids::ConversationId;
        let conv = ConversationId::new();
        let k = RegistryKey::from_conversation_and_sender(conv, "4012345000023");
        assert!(k.as_str().starts_with("4012345000023:"));
        assert!(k.as_str().ends_with(&conv.to_string()));
    }

    #[tokio::test]
    async fn tenant_keys_are_isolated() {
        let reg = InMemoryProcessRegistry::new();
        let t1 = tid();
        let t2 = tid();
        reg.register(t1, &key("k1"), make_identity()).await.unwrap();
        assert!(
            reg.contains(t1, &key("k1")).await.unwrap(),
            "tenant1 must see key"
        );
        assert!(
            !reg.contains(t2, &key("k1")).await.unwrap(),
            "tenant2 must not see key"
        );
    }
}

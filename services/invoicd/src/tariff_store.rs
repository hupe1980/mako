//! Shared `Arc`-wrapped tariff store for use across request handlers.

use std::sync::Arc;

use invoic_checker::InMemoryTariffStore;
use tokio::sync::RwLock;

/// A cloneable, shared-ownership handle to the in-memory tariff store.
///
/// Multiple Axum handler clones share the same store via `Arc<RwLock<_>>`.
/// The lock is `tokio::sync::RwLock` so concurrent readers don't block.
#[derive(Debug, Clone)]
pub struct TariffStoreHandle(pub Arc<RwLock<InMemoryTariffStore>>);

impl TariffStoreHandle {
    /// Create a new handle wrapping an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(InMemoryTariffStore::new())))
    }
}

impl Default for TariffStoreHandle {
    fn default() -> Self {
        Self::new()
    }
}

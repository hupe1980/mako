#![allow(clippy::doc_markdown)]
#![allow(clippy::collapsible_if)]
//! In-memory test doubles for all `mako-mdm` repository traits.
//!
//! Enabled only with `features = ["testing"]` — never in production.
//!
//! Each `InMemory*` implementation is `Clone + Send + Sync` and uses a
//! `tokio::sync::RwLock<HashMap<...>>` as backing store.

use std::collections::HashMap;
use std::sync::Arc;

use time::Date;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::{
    domain::{Gln, MaloId, MeloId, ProcessStatus, Sparte},
    error::MdmError,
    repository::{
        ContractRecord, ContractRepository, CorrelationEntry, CorrelationFilter, CorrelationIndex,
        Lokationszuordnung, MaloFilter, MaloRecord, MaloRepository, MeloRecord, MeloRepository,
        PageResult, PartnerRecord, PartnerRepository, Subscription, SubscriptionRepository,
    },
};

// ── InMemoryMaloRepository ────────────────────────────────────────────────────

/// In-memory `MaloRepository` for unit tests.
#[derive(Clone, Default)]
pub struct InMemoryMaloRepository {
    store: Arc<RwLock<HashMap<String, MaloRecord>>>,
}

impl MaloRepository for InMemoryMaloRepository {
    async fn upsert(
        &self,
        malo_id: &MaloId,
        sparte: Sparte,
        data: serde_json::Value,
        _lokationszuordnung: Vec<Lokationszuordnung>,
        if_match: Option<i64>,
    ) -> Result<i64, MdmError> {
        let mut store = self.store.write().await;
        let key = malo_id.as_ref().to_owned();
        let version = if let Some(existing) = store.get(&key) {
            if let Some(expected) = if_match {
                if existing.version != expected {
                    return Err(MdmError::VersionConflict {
                        expected: expected.to_string(),
                        actual: existing.version.to_string(),
                    });
                }
            }
            existing.version + 1
        } else {
            1
        };
        store.insert(
            key.clone(),
            MaloRecord {
                malo_id: malo_id.clone(),
                sparte,
                version,
                data,
                lokationszuordnung: vec![],
                updated_at: time::OffsetDateTime::now_utc(),
            },
        );
        Ok(version)
    }

    async fn find(&self, malo_id: &MaloId, _at: Date) -> Result<Option<MaloRecord>, MdmError> {
        let store = self.store.read().await;
        Ok(store.get(malo_id.as_ref()).cloned())
    }

    async fn list(
        &self,
        filter: MaloFilter,
        _at: Date,
    ) -> Result<PageResult<MaloRecord>, MdmError> {
        let store = self.store.read().await;
        let items: Vec<MaloRecord> = store.values().cloned().collect();
        let total = items.len() as u64;
        let size = if filter.size == 0 { 50 } else { filter.size };
        let start = (filter.page as usize) * (size as usize);
        let page_items = items.into_iter().skip(start).take(size as usize).collect();
        Ok(PageResult {
            items: page_items,
            total,
            page: filter.page,
            size,
        })
    }
}

// ── InMemoryMeloRepository ────────────────────────────────────────────────────

/// In-memory `MeloRepository` for unit tests.
#[derive(Clone, Default)]
pub struct InMemoryMeloRepository {
    store: Arc<RwLock<HashMap<String, MeloRecord>>>,
}

impl MeloRepository for InMemoryMeloRepository {
    #[allow(clippy::similar_names)]
    async fn upsert(
        &self,
        melo_id: &MeloId,
        malo_id: Option<&MaloId>,
        data: serde_json::Value,
        if_match: Option<i64>,
    ) -> Result<i64, MdmError> {
        let mut store = self.store.write().await;
        let key = melo_id.as_ref().to_owned();
        let version = if let Some(existing) = store.get(&key) {
            if let Some(expected) = if_match {
                if existing.version != expected {
                    return Err(MdmError::VersionConflict {
                        expected: expected.to_string(),
                        actual: existing.version.to_string(),
                    });
                }
            }
            existing.version + 1
        } else {
            1
        };
        store.insert(
            key.clone(),
            MeloRecord {
                melo_id: melo_id.clone(),
                malo_id: malo_id.cloned(),
                version,
                data,
                updated_at: time::OffsetDateTime::now_utc(),
            },
        );
        Ok(version)
    }

    async fn find(&self, melo_id: &MeloId) -> Result<Option<MeloRecord>, MdmError> {
        let store = self.store.read().await;
        Ok(store.get(melo_id.as_ref()).cloned())
    }
}

// ── InMemoryContractRepository ────────────────────────────────────────────────

/// In-memory `ContractRepository` for unit tests.
#[derive(Clone, Default)]
pub struct InMemoryContractRepository {
    store: Arc<RwLock<HashMap<String, ContractRecord>>>,
}

impl ContractRepository for InMemoryContractRepository {
    async fn upsert(
        &self,
        contract_id: &str,
        malo_id: Option<&MaloId>,
        sparte: Sparte,
        vertragsart: &str,
        data: serde_json::Value,
        if_match: Option<i64>,
    ) -> Result<i64, MdmError> {
        let mut store = self.store.write().await;
        let version = if let Some(existing) = store.get(contract_id) {
            if let Some(expected) = if_match {
                if existing.version != expected {
                    return Err(MdmError::VersionConflict {
                        expected: expected.to_string(),
                        actual: existing.version.to_string(),
                    });
                }
            }
            existing.version + 1
        } else {
            1
        };
        store.insert(
            contract_id.to_owned(),
            ContractRecord {
                contract_id: contract_id.to_owned(),
                malo_id: malo_id.cloned(),
                sparte,
                vertragsart: vertragsart.to_owned(),
                version,
                data,
                created_at: time::OffsetDateTime::now_utc(),
                updated_at: time::OffsetDateTime::now_utc(),
            },
        );
        Ok(version)
    }

    async fn find(&self, contract_id: &str) -> Result<Option<ContractRecord>, MdmError> {
        let store = self.store.read().await;
        Ok(store.get(contract_id).cloned())
    }
}

// ── InMemorySubscriptionRepository ───────────────────────────────────────────

/// In-memory `SubscriptionRepository` for unit tests.
#[derive(Clone, Default)]
pub struct InMemorySubscriptionRepository {
    store: Arc<RwLock<HashMap<String, Subscription>>>,
}

impl SubscriptionRepository for InMemorySubscriptionRepository {
    async fn upsert(&self, sub: Subscription) -> Result<i64, MdmError> {
        let mut store = self.store.write().await;
        let version = store.get(&sub.subscriber_id).map_or(1, |e| e.version + 1);
        let id = sub.subscriber_id.clone();
        let mut sub = sub;
        sub.version = version;
        store.insert(id, sub);
        Ok(version)
    }

    async fn find(&self, subscriber_id: &str) -> Result<Option<Subscription>, MdmError> {
        let store = self.store.read().await;
        Ok(store.get(subscriber_id).cloned())
    }

    async fn list_active(&self) -> Result<Vec<Subscription>, MdmError> {
        let store = self.store.read().await;
        Ok(store.values().filter(|s| s.active).cloned().collect())
    }

    async fn list_matching(
        &self,
        event_type: &str,
        role: &str,
        sparte: Option<&str>,
    ) -> Result<Vec<Subscription>, MdmError> {
        let store = self.store.read().await;
        let matches = store
            .values()
            .filter(|s| {
                if !s.active {
                    return false;
                }
                let role_ok =
                    s.roles.is_empty() || s.roles.iter().any(|r| r.eq_ignore_ascii_case(role));
                let type_ok = s.event_types.is_empty()
                    || s.event_types.iter().any(|t| {
                        t == event_type
                            || (t.ends_with('*') && event_type.starts_with(t.trim_end_matches('*')))
                    });
                let sparte_ok = sparte.is_none_or(|sp| {
                    s.sparten.is_empty() || s.sparten.iter().any(|s| s.eq_ignore_ascii_case(sp))
                });
                role_ok && type_ok && sparte_ok
            })
            .cloned()
            .collect();
        Ok(matches)
    }
}

// ── InMemoryCorrelationIndex ──────────────────────────────────────────────────

/// In-memory `CorrelationIndex` for unit tests.
#[derive(Clone, Default)]
pub struct InMemoryCorrelationIndex {
    store: Arc<RwLock<HashMap<Uuid, CorrelationEntry>>>,
}

impl CorrelationIndex for InMemoryCorrelationIndex {
    async fn insert(&self, entry: CorrelationEntry) -> Result<(), MdmError> {
        let mut store = self.store.write().await;
        store.entry(entry.process_id).or_insert(entry);
        Ok(())
    }

    async fn update_status(
        &self,
        process_id: Uuid,
        status: ProcessStatus,
        completed_at: Option<time::OffsetDateTime>,
    ) -> Result<(), MdmError> {
        let mut store = self.store.write().await;
        if let Some(entry) = store.get_mut(&process_id) {
            entry.status = status;
            entry.completed_at = completed_at;
        }
        Ok(())
    }

    async fn update_edifact_conv_id(
        &self,
        process_id: Uuid,
        conv_id: Uuid,
    ) -> Result<(), MdmError> {
        let mut store = self.store.write().await;
        if let Some(entry) = store.get_mut(&process_id) {
            entry.edifact_conv_id = Some(conv_id);
        }
        Ok(())
    }

    async fn find_by_erp_order_id(
        &self,
        erp_order_id: &str,
    ) -> Result<Option<CorrelationEntry>, MdmError> {
        let store = self.store.read().await;
        Ok(store
            .values()
            .find(|e| e.erp_order_id.as_deref() == Some(erp_order_id))
            .cloned())
    }

    async fn find_by_process_id(
        &self,
        process_id: Uuid,
    ) -> Result<Option<CorrelationEntry>, MdmError> {
        let store = self.store.read().await;
        Ok(store.get(&process_id).cloned())
    }

    async fn list(&self, filter: CorrelationFilter) -> Result<Vec<CorrelationEntry>, MdmError> {
        let store = self.store.read().await;
        let matches = store
            .values()
            .filter(|e| {
                filter
                    .erp_order_id
                    .as_deref()
                    .is_none_or(|id| e.erp_order_id.as_deref() == Some(id))
                    && filter
                        .malo_id
                        .as_ref()
                        .is_none_or(|id| e.malo_id.as_ref() == Some(id))
                    && filter.status.is_none_or(|s| e.status == s)
            })
            .cloned()
            .collect();
        Ok(matches)
    }
}

// ── InMemoryPartnerRepository ─────────────────────────────────────────────────

/// In-memory `PartnerRepository` for unit tests.
#[derive(Clone, Default)]
pub struct InMemoryPartnerRepository {
    store: Arc<RwLock<HashMap<String, PartnerRecord>>>,
}

impl PartnerRepository for InMemoryPartnerRepository {
    async fn upsert(&self, partner: PartnerRecord) -> Result<i64, MdmError> {
        let mut store = self.store.write().await;
        let key: String = partner.gln.as_ref().to_owned();
        let version = store.get(&key).map_or(1, |e| e.version + 1);
        let mut partner = partner;
        partner.version = version;
        store.insert(key, partner);
        Ok(version)
    }

    async fn find(&self, gln: &Gln) -> Result<Option<PartnerRecord>, MdmError> {
        let store = self.store.read().await;
        Ok(store.get(gln.as_ref()).cloned())
    }

    async fn list(&self) -> Result<Vec<PartnerRecord>, MdmError> {
        let store = self.store.read().await;
        Ok(store.values().cloned().collect())
    }
}

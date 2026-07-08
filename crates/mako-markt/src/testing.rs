#![allow(clippy::doc_markdown)]
#![allow(clippy::collapsible_if)]
//! In-memory test doubles for all `mako-markt` repository traits.
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
    domain::{MaloId, MarktpartnerId, MeloId, ProcessStatus, Sparte},
    error::MdmError,
    repository::{
        ContractRecord, ContractRepository, CorrelationEntry, CorrelationFilter, CorrelationIndex,
        LieferStatus, Lokationszuordnung, MaloFilter, MaloGridRecord, MaloGridRepository,
        MaloRecord, MaloRepository, MeloRecord, MeloRepository, NeLoRecord, NeLoRepository,
        PageResult, PartnerRecord, PartnerRepository, PreisblattSource, PriCatDispatchEntry,
        PriCatDispatchState, PriCatRepository, PriCatVersion, Subscription, SubscriptionRepository,
        VersorgungsStatusHistoryRecord, VersorgungsStatusRecord, VersorgungsStatusRepository,
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
        bo4e_version: &str,
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
                bo4e_version: bo4e_version.to_owned(),
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
        bo4e_version: &str,
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
                bo4e_version: bo4e_version.to_owned(),
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
    #[allow(clippy::too_many_arguments)]
    async fn upsert(
        &self,
        contract_id: &str,
        malo_id: Option<&MaloId>,
        sparte: Sparte,
        vertragsart: &str,
        data: serde_json::Value,
        valid_from: Option<Date>,
        valid_to: Option<Date>,
        if_match: Option<i64>,
        bo4e_version: &str,
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
                valid_from,
                valid_to,
                created_at: time::OffsetDateTime::now_utc(),
                updated_at: time::OffsetDateTime::now_utc(),
                bo4e_version: bo4e_version.to_owned(),
            },
        );
        Ok(version)
    }

    async fn find(&self, contract_id: &str) -> Result<Option<ContractRecord>, MdmError> {
        let store = self.store.read().await;
        Ok(store.get(contract_id).cloned())
    }

    async fn find_active_by_malo(
        &self,
        malo_id: &MaloId,
        at: Date,
    ) -> Result<Vec<ContractRecord>, MdmError> {
        let store = self.store.read().await;
        let mut results: Vec<ContractRecord> = store
            .values()
            .filter(|r| {
                r.malo_id.as_ref() == Some(malo_id)
                    && r.valid_from.is_none_or(|f| f <= at)
                    && r.valid_to.is_none_or(|t| t >= at)
            })
            .cloned()
            .collect();
        results.sort_by(|a, b| b.valid_from.cmp(&a.valid_from));
        Ok(results)
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
        let key: String = partner.mp_id.as_ref().to_owned();
        let version = store.get(&key).map_or(1, |e| e.version + 1);
        let mut partner = partner;
        partner.version = version;
        store.insert(key, partner);
        Ok(version)
    }

    async fn find(&self, id: &MarktpartnerId) -> Result<Option<PartnerRecord>, MdmError> {
        let store = self.store.read().await;
        Ok(store.get(id.as_ref()).cloned())
    }

    async fn list(&self) -> Result<Vec<PartnerRecord>, MdmError> {
        let store = self.store.read().await;
        Ok(store.values().cloned().collect())
    }
}

// ── InMemoryVersorgungsStatusRepository ──────────────────────────────────────

/// In-memory `VersorgungsStatusRepository` for unit tests.
///
/// Thread-safe (`Arc<RwLock>`); implements optimistic concurrency via version
/// comparison.  History appended on every successful `upsert`.
#[derive(Clone, Default)]
pub struct InMemoryVersorgungsStatusRepository {
    store: Arc<RwLock<HashMap<(String, String), VersorgungsStatusRecord>>>,
    history: Arc<RwLock<Vec<VersorgungsStatusHistoryRecord>>>,
}

impl VersorgungsStatusRepository for InMemoryVersorgungsStatusRepository {
    async fn upsert(
        &self,
        rec: VersorgungsStatusRecord,
        if_version: Option<i64>,
    ) -> Result<i64, MdmError> {
        let key = (rec.malo_id.as_ref().to_owned(), rec.tenant.clone());
        let mut store = self.store.write().await;
        let existing = store.get(&key);
        if let Some(expected) = if_version {
            let actual = existing.map_or(0, |e| e.version);
            if actual != expected {
                return Err(MdmError::VersionConflict {
                    expected: expected.to_string(),
                    actual: actual.to_string(),
                });
            }
        }
        let new_version = existing.map_or(1, |e| e.version + 1);
        let now = time::OffsetDateTime::now_utc();
        let mut rec = rec;
        rec.version = new_version;
        rec.updated_at = now;
        let hist = VersorgungsStatusHistoryRecord {
            id: new_version, // use version as surrogate in tests
            malo_id: rec.malo_id.clone(),
            tenant: rec.tenant.clone(),
            lieferstatus: rec.lieferstatus,
            lf_mp_id: rec.lf_mp_id.clone(),
            lf_gln_next: rec.lf_gln_next.clone(),
            lieferbeginn: rec.lieferbeginn,
            lieferende: rec.lieferende,
            msb_mp_id: rec.msb_mp_id.clone(),
            nb_mp_id: rec.nb_mp_id.clone(),
            last_process_id: rec.last_process_id,
            version: new_version,
            valid_from: now,
        };
        store.insert(key, rec);
        drop(store);
        self.history.write().await.push(hist);
        Ok(new_version)
    }

    async fn find(
        &self,
        malo_id: &MaloId,
        tenant: &str,
    ) -> Result<Option<VersorgungsStatusRecord>, MdmError> {
        let key = (malo_id.as_ref().to_owned(), tenant.to_owned());
        let store = self.store.read().await;
        Ok(store.get(&key).cloned())
    }

    async fn find_at(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        at: Date,
    ) -> Result<Option<VersorgungsStatusRecord>, MdmError> {
        let history = self.history.read().await;
        // In-memory: treat valid_from as UTC date (no timezone conversion needed in tests)
        let rec = history
            .iter()
            .filter(|h| {
                h.malo_id.as_ref() == malo_id.as_ref()
                    && h.tenant == tenant
                    && h.valid_from.date() <= at
            })
            .max_by_key(|h| h.valid_from)
            .map(|h| VersorgungsStatusRecord {
                malo_id: h.malo_id.clone(),
                tenant: h.tenant.clone(),
                lieferstatus: h.lieferstatus,
                lf_mp_id: h.lf_mp_id.clone(),
                lf_gln_next: h.lf_gln_next.clone(),
                lieferbeginn: h.lieferbeginn,
                lieferende: h.lieferende,
                msb_mp_id: h.msb_mp_id.clone(),
                nb_mp_id: h.nb_mp_id.clone(),
                last_process_id: h.last_process_id,
                updated_at: h.valid_from,
                version: h.version,
            });
        Ok(rec)
    }

    async fn find_history(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        page: u32,
        size: u32,
    ) -> Result<PageResult<VersorgungsStatusHistoryRecord>, MdmError> {
        let history = self.history.read().await;
        let mut all: Vec<_> = history
            .iter()
            .filter(|h| h.malo_id.as_ref() == malo_id.as_ref() && h.tenant == tenant)
            .cloned()
            .collect();
        all.sort_by(|a, b| b.valid_from.cmp(&a.valid_from));
        let total = all.len() as u64;
        let start = (page * size) as usize;
        let items = all.into_iter().skip(start).take(size as usize).collect();
        Ok(PageResult {
            items,
            total,
            page,
            size,
        })
    }

    async fn list_by_tenant(
        &self,
        tenant: &str,
        page: u32,
        size: u32,
    ) -> Result<PageResult<VersorgungsStatusRecord>, MdmError> {
        let store = self.store.read().await;
        let all: Vec<_> = store
            .values()
            .filter(|r| r.tenant == tenant)
            .cloned()
            .collect();
        let total = all.len() as u64;
        let start = (page * size) as usize;
        let items = all.into_iter().skip(start).take(size as usize).collect();
        Ok(PageResult {
            items,
            total,
            page,
            size,
        })
    }
}

#[allow(dead_code)]
fn _assert_lieferstatus_variants_used() {
    // Keep LieferStatus variants from triggering dead_code warnings in testing.
    let _ = LieferStatus::Grundversorgung;
    let _ = LieferStatus::Ersatzversorgung;
    let _ = LieferStatus::Ruhend;
    let _ = LieferStatus::Stillgelegt;
}

// ── InMemoryNeLoRepository ────────────────────────────────────────────────────

/// In-memory `NeLoRepository` for unit tests.
#[derive(Clone, Default)]
pub struct InMemoryNeLoRepository {
    store: Arc<RwLock<HashMap<(String, String), NeLoRecord>>>,
}

impl NeLoRepository for InMemoryNeLoRepository {
    async fn upsert(&self, rec: NeLoRecord, if_match: Option<i64>) -> Result<i64, MdmError> {
        let key = (rec.nelo_id.clone(), rec.tenant.clone());
        let mut store = self.store.write().await;
        let existing = store.get(&key);
        if let Some(expected) = if_match {
            let actual = existing.map_or(0, |e| e.version);
            if actual != expected {
                return Err(MdmError::VersionConflict {
                    expected: expected.to_string(),
                    actual: actual.to_string(),
                });
            }
        }
        let new_version = existing.map_or(1, |e| e.version + 1);
        let mut rec = rec;
        rec.version = new_version;
        rec.updated_at = time::OffsetDateTime::now_utc();
        store.insert(key, rec);
        Ok(new_version)
    }

    async fn find(&self, nelo_id: &str, tenant: &str) -> Result<Option<NeLoRecord>, MdmError> {
        let store = self.store.read().await;
        Ok(store.get(&(nelo_id.to_owned(), tenant.to_owned())).cloned())
    }

    async fn list_by_nb(
        &self,
        nb_mp_id: &str,
        tenant: &str,
        page: u32,
        size: u32,
    ) -> Result<PageResult<NeLoRecord>, MdmError> {
        let store = self.store.read().await;
        let all: Vec<_> = store
            .values()
            .filter(|r| r.tenant == tenant && r.nb_mp_id == nb_mp_id)
            .cloned()
            .collect();
        let total = all.len() as u64;
        let start = (page * size) as usize;
        let items = all.into_iter().skip(start).take(size as usize).collect();
        Ok(PageResult {
            items,
            total,
            page,
            size,
        })
    }

    async fn list_by_tenant(
        &self,
        tenant: &str,
        page: u32,
        size: u32,
    ) -> Result<PageResult<NeLoRecord>, MdmError> {
        let store = self.store.read().await;
        let all: Vec<_> = store
            .values()
            .filter(|r| r.tenant == tenant)
            .cloned()
            .collect();
        let total = all.len() as u64;
        let start = (page * size) as usize;
        let items = all.into_iter().skip(start).take(size as usize).collect();
        Ok(PageResult {
            items,
            total,
            page,
            size,
        })
    }
}

// ── InMemoryPriCatRepository ──────────────────────────────────────────────────

/// In-memory `PriCatRepository` for unit tests.
///
/// Thread-safe (`Arc<RwLock>`); keyed on `(nb_mp_id, tenant, valid_from)`.
#[allow(clippy::type_complexity)]
#[derive(Clone, Default)]
pub struct InMemoryPriCatRepository {
    versions: Arc<RwLock<HashMap<(String, String, time::Date), PriCatVersion>>>,
    log: Arc<RwLock<Vec<PriCatDispatchEntry>>>,
}

impl PriCatRepository for InMemoryPriCatRepository {
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
    ) -> Result<uuid::Uuid, MdmError> {
        let mut store = self.versions.write().await;
        let key = (nb_mp_id.to_owned(), tenant.to_owned(), valid_from);
        let id = store.get(&key).map_or_else(uuid::Uuid::new_v4, |v| v.id);
        let now = time::OffsetDateTime::now_utc();
        store.insert(
            key,
            PriCatVersion {
                id,
                nb_mp_id: nb_mp_id.to_owned(),
                tenant: tenant.to_owned(),
                valid_from,
                valid_to,
                data,
                bo4e_version: bo4e_version.to_owned(),
                source,
                dispatch_state: PriCatDispatchState::Pending,
                dispatch_error: None,
                created_at: now,
                updated_at: now,
            },
        );
        Ok(id)
    }

    async fn list_versions(
        &self,
        nb_mp_id: &str,
        tenant: &str,
    ) -> Result<Vec<PriCatVersion>, MdmError> {
        let store = self.versions.read().await;
        let mut items: Vec<PriCatVersion> = store
            .values()
            .filter(|v| v.nb_mp_id == nb_mp_id && v.tenant == tenant)
            .cloned()
            .collect();
        items.sort_by(|a, b| b.valid_from.cmp(&a.valid_from));
        Ok(items)
    }

    async fn find_latest(
        &self,
        nb_mp_id: &str,
        tenant: &str,
    ) -> Result<Option<PriCatVersion>, MdmError> {
        let versions = self.list_versions(nb_mp_id, tenant).await?;
        Ok(versions.into_iter().next())
    }

    async fn list_pending(&self, tenant: &str) -> Result<Vec<PriCatVersion>, MdmError> {
        let store = self.versions.read().await;
        let mut items: Vec<PriCatVersion> = store
            .values()
            .filter(|v| {
                v.tenant == tenant && !matches!(v.dispatch_state, PriCatDispatchState::Done)
            })
            .cloned()
            .collect();
        items.sort_by(|a, b| b.valid_from.cmp(&a.valid_from));
        Ok(items)
    }

    async fn mark_queued(&self, id: uuid::Uuid) -> Result<(), MdmError> {
        let mut store = self.versions.write().await;
        for v in store.values_mut() {
            if v.id == id {
                v.dispatch_state = PriCatDispatchState::Queued;
                v.updated_at = time::OffsetDateTime::now_utc();
                return Ok(());
            }
        }
        Err(MdmError::NotFound {
            resource_type: "PriCatVersion",
            id: id.to_string(),
        })
    }

    async fn mark_done(&self, id: uuid::Uuid) -> Result<(), MdmError> {
        let mut store = self.versions.write().await;
        for v in store.values_mut() {
            if v.id == id {
                v.dispatch_state = PriCatDispatchState::Done;
                v.dispatch_error = None;
                v.updated_at = time::OffsetDateTime::now_utc();
                return Ok(());
            }
        }
        Err(MdmError::NotFound {
            resource_type: "PriCatVersion",
            id: id.to_string(),
        })
    }

    async fn mark_error(&self, id: uuid::Uuid, error: &str) -> Result<(), MdmError> {
        let mut store = self.versions.write().await;
        for v in store.values_mut() {
            if v.id == id {
                v.dispatch_state = PriCatDispatchState::Error;
                v.dispatch_error = Some(error.to_owned());
                v.updated_at = time::OffsetDateTime::now_utc();
                return Ok(());
            }
        }
        Err(MdmError::NotFound {
            resource_type: "PriCatVersion",
            id: id.to_string(),
        })
    }

    async fn log_dispatch(&self, entry: PriCatDispatchEntry) -> Result<(), MdmError> {
        self.log.write().await.push(entry);
        Ok(())
    }

    async fn dispatch_log(
        &self,
        pricat_version_id: uuid::Uuid,
    ) -> Result<Vec<PriCatDispatchEntry>, MdmError> {
        let log = self.log.read().await;
        Ok(log
            .iter()
            .filter(|e| e.pricat_version_id == pricat_version_id)
            .cloned()
            .collect())
    }
}

// ── InMemoryMaloGridRepository ────────────────────────────────────────────────

/// In-memory `MaloGridRepository` for unit tests.
///
/// Keyed by `(malo_id_string, tenant)`.
#[derive(Clone, Default)]
pub struct InMemoryMaloGridRepository {
    store: Arc<RwLock<HashMap<(String, String), MaloGridRecord>>>,
}

impl MaloGridRepository for InMemoryMaloGridRepository {
    async fn upsert(&self, rec: MaloGridRecord) -> Result<(), MdmError> {
        let key = (rec.malo_id.as_ref().to_owned(), rec.tenant.clone());
        self.store.write().await.insert(key, rec);
        Ok(())
    }

    async fn find(
        &self,
        malo_id: &MaloId,
        tenant: &str,
    ) -> Result<Option<MaloGridRecord>, MdmError> {
        let key = (malo_id.as_ref().to_owned(), tenant.to_owned());
        Ok(self.store.read().await.get(&key).cloned())
    }

    async fn list_by_nb(
        &self,
        nb_mp_id: &str,
        tenant: &str,
    ) -> Result<Vec<MaloGridRecord>, MdmError> {
        Ok(self
            .store
            .read()
            .await
            .values()
            .filter(|r| r.nb_mp_id == nb_mp_id && r.tenant == tenant)
            .cloned()
            .collect())
    }

    async fn delete(&self, malo_id: &MaloId, tenant: &str) -> Result<(), MdmError> {
        let key = (malo_id.as_ref().to_owned(), tenant.to_owned());
        self.store.write().await.remove(&key);
        Ok(())
    }
}

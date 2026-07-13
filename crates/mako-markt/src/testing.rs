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
                netzebene: data
                    .get("netzebene")
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned),
                bilanzierungsgebiet: data
                    .get("bilanzierungsgebiet")
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned),
                gasqualitaet: data
                    .get("gasqualitaet")
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned),
                energierichtung: data
                    .get("energierichtung")
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned),
                bilanzierungsmethode: data
                    .get("bilanzierungsmethode")
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned),
                regelzone: data
                    .get("regelzone")
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned),
                fallgruppe: data
                    .get("fallgruppenzuordnung")
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned),
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

    async fn patch_typenmerkmal(
        &self,
        malo_id: &MaloId,
        bilanzierungsmethode: Option<&str>,
        fallgruppe: Option<&str>,
    ) -> Result<(), MdmError> {
        let mut store = self.store.write().await;
        if let Some(rec) = store.get_mut(malo_id.as_ref()) {
            if let Some(b) = bilanzierungsmethode {
                rec.bilanzierungsmethode = Some(b.to_owned());
            }
            if let Some(f) = fallgruppe {
                rec.fallgruppe = Some(f.to_owned());
            }
        }
        Ok(())
    }
}

// ── InMemoryMeloRepository ────────────────────────────────────────────────────

/// In-memory `MeloRepository` for unit tests.
#[derive(Clone, Default)]
pub struct InMemoryMeloRepository {
    store: Arc<RwLock<HashMap<String, MeloRecord>>>,
}

impl MeloRepository for InMemoryMeloRepository {
    #[expect(clippy::similar_names)]
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
                netzebene_messung: data
                    .get("netzebeneMessung")
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned),
                regelzone: data
                    .get("standorteigenschaften")
                    .and_then(|s| s.get("eigenschaftenStrom"))
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|first| first.get("regelzone"))
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned),
                standorteigenschaften: data.get("standorteigenschaften").cloned(),
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
            lf_mp_id_next: rec.lf_mp_id_next.clone(),
            lf_next_lieferbeginn: rec.lf_next_lieferbeginn,
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
                lf_mp_id_next: h.lf_mp_id_next.clone(),
                lf_next_lieferbeginn: h.lf_next_lieferbeginn,
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

    async fn announce_lf_next(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        lf_mp_id_next: &str,
        lf_next_lieferbeginn: Option<time::Date>,
        nb_mp_id: &str,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        let key = (malo_id.as_ref().to_owned(), tenant.to_owned());
        let mut store = self.store.write().await;
        let now = time::OffsetDateTime::now_utc();
        let entry = store.entry(key).or_insert_with(|| VersorgungsStatusRecord {
            malo_id: malo_id.clone(),
            tenant: tenant.to_owned(),
            lieferstatus: LieferStatus::Unbeliefert,
            lf_mp_id: None,
            lf_mp_id_next: None,
            lf_next_lieferbeginn: None,
            lieferbeginn: None,
            lieferende: None,
            msb_mp_id: None,
            nb_mp_id: nb_mp_id.to_owned(),
            last_process_id: process_id,
            updated_at: now,
            version: 0,
        });
        entry.lf_mp_id_next = Some(lf_mp_id_next.to_owned());
        entry.lf_next_lieferbeginn = lf_next_lieferbeginn;
        entry.last_process_id = process_id;
        entry.updated_at = now;
        entry.version += 1;
        let rec = entry.clone();
        drop(store);
        let hist = VersorgungsStatusHistoryRecord {
            id: rec.version,
            malo_id: rec.malo_id.clone(),
            tenant: rec.tenant.clone(),
            lieferstatus: rec.lieferstatus,
            lf_mp_id: rec.lf_mp_id.clone(),
            lf_mp_id_next: rec.lf_mp_id_next.clone(),
            lf_next_lieferbeginn: rec.lf_next_lieferbeginn,
            lieferbeginn: rec.lieferbeginn,
            lieferende: rec.lieferende,
            msb_mp_id: rec.msb_mp_id.clone(),
            nb_mp_id: rec.nb_mp_id.clone(),
            last_process_id: rec.last_process_id,
            version: rec.version,
            valid_from: now,
        };
        self.history.write().await.push(hist);
        Ok(())
    }

    async fn confirm_supply(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        let key = (malo_id.as_ref().to_owned(), tenant.to_owned());
        let mut store = self.store.write().await;
        let now = time::OffsetDateTime::now_utc();
        if let Some(entry) = store.get_mut(&key) {
            if entry.lf_mp_id_next.is_some() {
                entry.lf_mp_id = entry.lf_mp_id_next.take();
                entry.lieferbeginn = entry.lf_next_lieferbeginn.take();
                entry.lf_next_lieferbeginn = None;
                entry.lieferstatus = LieferStatus::Beliefert;
                entry.last_process_id = process_id;
                entry.updated_at = now;
                entry.version += 1;
                let rec = entry.clone();
                drop(store);
                let hist = VersorgungsStatusHistoryRecord {
                    id: rec.version,
                    malo_id: rec.malo_id.clone(),
                    tenant: rec.tenant.clone(),
                    lieferstatus: rec.lieferstatus,
                    lf_mp_id: rec.lf_mp_id.clone(),
                    lf_mp_id_next: rec.lf_mp_id_next.clone(),
                    lf_next_lieferbeginn: rec.lf_next_lieferbeginn,
                    lieferbeginn: rec.lieferbeginn,
                    lieferende: rec.lieferende,
                    msb_mp_id: rec.msb_mp_id.clone(),
                    nb_mp_id: rec.nb_mp_id.clone(),
                    last_process_id: rec.last_process_id,
                    version: rec.version,
                    valid_from: now,
                };
                self.history.write().await.push(hist);
            }
        }
        Ok(())
    }

    async fn end_supply(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        nb_mp_id: &str,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        let key = (malo_id.as_ref().to_owned(), tenant.to_owned());
        let mut store = self.store.write().await;
        let now = time::OffsetDateTime::now_utc();
        let entry = store.entry(key).or_insert_with(|| VersorgungsStatusRecord {
            malo_id: malo_id.clone(),
            tenant: tenant.to_owned(),
            lieferstatus: LieferStatus::Unbeliefert,
            lf_mp_id: None,
            lf_mp_id_next: None,
            lf_next_lieferbeginn: None,
            lieferbeginn: None,
            lieferende: None,
            msb_mp_id: None,
            nb_mp_id: nb_mp_id.to_owned(),
            last_process_id: process_id,
            updated_at: now,
            version: 0,
        });
        entry.lieferstatus = LieferStatus::Unbeliefert;
        entry.lf_mp_id = None;
        entry.lieferbeginn = None;
        entry.nb_mp_id.clone_from(&nb_mp_id.to_owned());
        entry.last_process_id = process_id;
        entry.updated_at = now;
        entry.version += 1;
        let rec = entry.clone();
        drop(store);
        let hist = VersorgungsStatusHistoryRecord {
            id: rec.version,
            malo_id: rec.malo_id.clone(),
            tenant: rec.tenant.clone(),
            lieferstatus: rec.lieferstatus,
            lf_mp_id: rec.lf_mp_id.clone(),
            lf_mp_id_next: rec.lf_mp_id_next.clone(),
            lf_next_lieferbeginn: rec.lf_next_lieferbeginn,
            lieferbeginn: rec.lieferbeginn,
            lieferende: rec.lieferende,
            msb_mp_id: rec.msb_mp_id.clone(),
            nb_mp_id: rec.nb_mp_id.clone(),
            last_process_id: rec.last_process_id,
            version: rec.version,
            valid_from: now,
        };
        self.history.write().await.push(hist);
        Ok(())
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
#[expect(clippy::type_complexity)]
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

// ── InMemoryPreisblattMessungRepository (B5) ──────────────────────────────────

use crate::repository::{
    DeviceRepository, GeraetRecord, PreisblattMessungRecord, PreisblattMessungRepository,
    SteuerbareRessourceRecord, SteuerbareRessourceRepository, TechnischeRessourceRecord,
    TechnischeRessourceRepository, ZaehlerRecord,
};

/// In-memory test double for [`PreisblattMessungRepository`].
#[derive(Default, Clone)]
pub struct InMemoryPreisblattMessungRepository {
    store: Arc<RwLock<HashMap<String, PreisblattMessungRecord>>>,
}

impl PreisblattMessungRepository for InMemoryPreisblattMessungRepository {
    async fn upsert_messung(
        &self,
        msb_mp_id: &str,
        data: serde_json::Value,
        bo4e_version: &str,
        source: PreisblattSource,
    ) -> Result<(), crate::error::MdmError> {
        let mut store = self.store.write().await;
        store.insert(
            msb_mp_id.to_owned(),
            PreisblattMessungRecord {
                msb_mp_id: msb_mp_id.to_owned(),
                data,
                bo4e_version: bo4e_version.to_owned(),
                source,
                auf_abschlaege: vec![],
                created_at: time::OffsetDateTime::now_utc(),
                updated_at: time::OffsetDateTime::now_utc(),
            },
        );
        Ok(())
    }

    async fn find_messung_for_date(
        &self,
        msb_mp_id: &str,
        _billing_date: &str,
    ) -> Result<Option<PreisblattMessungRecord>, crate::error::MdmError> {
        Ok(self.store.read().await.get(msb_mp_id).cloned())
    }
}

// ── InMemorySteuerbareRessourceRepository (B4b) ───────────────────────────────

/// In-memory test double for [`SteuerbareRessourceRepository`].
#[derive(Default, Clone)]
pub struct InMemorySteuerbareRessourceRepository {
    store: Arc<RwLock<HashMap<(String, String), SteuerbareRessourceRecord>>>,
}

impl SteuerbareRessourceRepository for InMemorySteuerbareRessourceRepository {
    #[expect(clippy::similar_names)]
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
    ) -> Result<(), crate::error::MdmError> {
        let key = (sr_id.to_owned(), tenant.to_owned());
        let version = {
            let store = self.store.read().await;
            store.get(&key).map_or(1, |r| r.version + 1)
        };
        self.store.write().await.insert(
            key,
            SteuerbareRessourceRecord {
                sr_id: sr_id.to_owned(),
                tenant: tenant.to_owned(),
                malo_id: malo_id.map(std::borrow::ToOwned::to_owned),
                melo_id: melo_id.map(std::borrow::ToOwned::to_owned),
                data,
                konfigurationsprodukte,
                bo4e_version: bo4e_version.to_owned(),
                version,
                updated_at: time::OffsetDateTime::now_utc(),
            },
        );
        Ok(())
    }

    async fn find_sr(
        &self,
        sr_id: &str,
        tenant: &str,
    ) -> Result<Option<SteuerbareRessourceRecord>, crate::error::MdmError> {
        Ok(self
            .store
            .read()
            .await
            .get(&(sr_id.to_owned(), tenant.to_owned()))
            .cloned())
    }

    async fn list_sr_by_malo(
        &self,
        malo_id: &str,
        tenant: &str,
    ) -> Result<Vec<SteuerbareRessourceRecord>, crate::error::MdmError> {
        let store = self.store.read().await;
        Ok(store
            .values()
            .filter(|r| r.tenant == tenant && r.malo_id.as_deref() == Some(malo_id))
            .cloned()
            .collect())
    }

    async fn replace_sr_konfigurationsprodukte(
        &self,
        sr_id: &str,
        tenant: &str,
        konfigurationsprodukte: serde_json::Value,
    ) -> Result<bool, crate::error::MdmError> {
        let mut store = self.store.write().await;
        let key = (sr_id.to_owned(), tenant.to_owned());
        if let Some(rec) = store.get_mut(&key) {
            rec.konfigurationsprodukte = Some(konfigurationsprodukte);
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

// ── InMemoryTechnischeRessourceRepository (B9) ───────────────────────────────

/// In-memory test double for [`TechnischeRessourceRepository`].
#[derive(Default, Clone)]
pub struct InMemoryTechnischeRessourceRepository {
    store: Arc<RwLock<HashMap<(String, String), TechnischeRessourceRecord>>>,
}

impl TechnischeRessourceRepository for InMemoryTechnischeRessourceRepository {
    #[allow(clippy::too_many_arguments)]
    #[expect(clippy::similar_names)]
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
    ) -> Result<(), crate::error::MdmError> {
        let key = (tr_id.to_owned(), tenant.to_owned());
        let version = {
            let store = self.store.read().await;
            store.get(&key).map_or(1, |r| r.version + 1)
        };
        self.store.write().await.insert(
            key,
            TechnischeRessourceRecord {
                tr_id: tr_id.to_owned(),
                tenant: tenant.to_owned(),
                malo_id: malo_id.map(std::borrow::ToOwned::to_owned),
                melo_id: melo_id.map(std::borrow::ToOwned::to_owned),
                tr_typ: tr_typ.map(std::borrow::ToOwned::to_owned),
                ist_fernschaltbar,
                data,
                bo4e_version: bo4e_version.to_owned(),
                version,
                updated_at: time::OffsetDateTime::now_utc(),
            },
        );
        Ok(())
    }

    async fn find_tr(
        &self,
        tr_id: &str,
        tenant: &str,
    ) -> Result<Option<TechnischeRessourceRecord>, crate::error::MdmError> {
        Ok(self
            .store
            .read()
            .await
            .get(&(tr_id.to_owned(), tenant.to_owned()))
            .cloned())
    }

    async fn list_tr_by_malo(
        &self,
        malo_id: &str,
        tenant: &str,
    ) -> Result<Vec<TechnischeRessourceRecord>, crate::error::MdmError> {
        let store = self.store.read().await;
        Ok(store
            .values()
            .filter(|r| r.tenant == tenant && r.malo_id.as_deref() == Some(malo_id))
            .cloned()
            .collect())
    }

    async fn list_tr_by_melo(
        &self,
        melo_id: &str,
        tenant: &str,
    ) -> Result<Vec<TechnischeRessourceRecord>, crate::error::MdmError> {
        let store = self.store.read().await;
        Ok(store
            .values()
            .filter(|r| r.tenant == tenant && r.melo_id.as_deref() == Some(melo_id))
            .cloned()
            .collect())
    }
}

// ── InMemoryLokationszuordnungRepository (B5) ────────────────────────────────

/// In-memory test double for [`crate::repository::LokationszuordnungRepository`].
#[derive(Default, Clone)]
pub struct InMemoryLokationszuordnungRepository {
    edges: Arc<RwLock<Vec<crate::repository::LokationszuordnungEdge>>>,
}

impl crate::repository::LokationszuordnungRepository for InMemoryLokationszuordnungRepository {
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
    ) -> Result<uuid::Uuid, crate::error::MdmError> {
        let id = uuid::Uuid::new_v4();
        let mut edges = self.edges.write().await;
        // Replace existing edge with same (tenant, von_id, nach_id, valid_from)
        edges.retain(|e| {
            !(e.tenant == tenant
                && e.von_id == von_id
                && e.nach_id == nach_id
                && e.valid_from == valid_from)
        });
        edges.push(crate::repository::LokationszuordnungEdge {
            id,
            tenant: tenant.to_owned(),
            von_id: von_id.to_owned(),
            von_typ: von_typ.to_owned(),
            nach_id: nach_id.to_owned(),
            nach_typ: nach_typ.to_owned(),
            valid_from,
            valid_to,
            data,
            depth: 0,
        });
        Ok(id)
    }

    async fn find_graph(
        &self,
        tenant: &str,
        root_id: &str,
        at_date: Option<time::Date>,
    ) -> Result<Vec<crate::repository::LokationszuordnungEdge>, crate::error::MdmError> {
        // Drop the lock before the BFS loop to avoid holding it across computation.
        let filtered: Vec<_> = {
            let edges = self.edges.read().await;
            edges
                .iter()
                .filter(|e| e.tenant == tenant && is_valid_at(e, at_date))
                .cloned()
                .collect()
        };
        // BFS from root_id (max depth 8)
        let mut result = Vec::new();
        let mut frontier = vec![root_id.to_owned()];
        let mut visited = std::collections::HashSet::new();
        let mut depth = 0i32;
        while !frontier.is_empty() && depth <= 8 {
            let mut next = Vec::new();
            for node in &frontier {
                for edge in filtered.iter().filter(|e| &e.von_id == node) {
                    if !visited.contains(&edge.nach_id) {
                        let mut e = edge.clone();
                        e.depth = depth;
                        result.push(e);
                        next.push(edge.nach_id.clone());
                        visited.insert(edge.nach_id.clone());
                    }
                }
            }
            frontier = next;
            depth += 1;
        }
        Ok(result)
    }

    async fn list_edges_from(
        &self,
        tenant: &str,
        von_id: &str,
        at_date: Option<time::Date>,
    ) -> Result<Vec<crate::repository::LokationszuordnungEdge>, crate::error::MdmError> {
        let edges = self.edges.read().await;
        Ok(edges
            .iter()
            .filter(|e| e.tenant == tenant && e.von_id == von_id && is_valid_at(e, at_date))
            .cloned()
            .collect())
    }

    async fn delete_edge(
        &self,
        tenant: &str,
        von_id: &str,
        nach_id: &str,
    ) -> Result<bool, crate::error::MdmError> {
        let mut edges = self.edges.write().await;
        let before = edges.len();
        edges.retain(|e| !(e.tenant == tenant && e.von_id == von_id && e.nach_id == nach_id));
        Ok(edges.len() < before)
    }
}

fn is_valid_at(e: &crate::repository::LokationszuordnungEdge, at: Option<time::Date>) -> bool {
    let Some(d) = at else { return true };
    let from_ok = e.valid_from.is_none_or(|f| f <= d);
    let to_ok = e.valid_to.is_none_or(|t| t >= d);
    from_ok && to_ok
}

// ── InMemoryDeviceRepository (B3) ────────────────────────────────────────────

/// In-memory test double for [`DeviceRepository`].
#[derive(Default, Clone)]
pub struct InMemoryDeviceRepository {
    zaehler: Arc<RwLock<HashMap<(String, String), ZaehlerRecord>>>,
    geraete: Arc<RwLock<HashMap<(String, String), GeraetRecord>>>,
}

impl DeviceRepository for InMemoryDeviceRepository {
    async fn upsert_zaehler(
        &self,
        zaehler_id: &str,
        tenant: &str,
        melo_id: &str,
        zaehler_typ: Option<&str>,
        eichung_bis: Option<time::Date>,
        data: serde_json::Value,
        bo4e_version: &str,
    ) -> Result<(), crate::error::MdmError> {
        let key = (zaehler_id.to_owned(), tenant.to_owned());
        let version = {
            let z = self.zaehler.read().await;
            z.get(&key).map_or(1, |r| r.version + 1)
        };
        self.zaehler.write().await.insert(
            key,
            ZaehlerRecord {
                zaehler_id: zaehler_id.to_owned(),
                tenant: tenant.to_owned(),
                melo_id: melo_id.to_owned(),
                zaehler_typ: zaehler_typ.map(std::borrow::ToOwned::to_owned),
                eichung_bis,
                data,
                bo4e_version: bo4e_version.to_owned(),
                version,
                updated_at: time::OffsetDateTime::now_utc(),
            },
        );
        Ok(())
    }

    async fn list_zaehler_by_melo(
        &self,
        melo_id: &str,
        tenant: &str,
    ) -> Result<Vec<ZaehlerRecord>, crate::error::MdmError> {
        Ok(self
            .zaehler
            .read()
            .await
            .values()
            .filter(|r| r.tenant == tenant && r.melo_id == melo_id)
            .cloned()
            .collect())
    }

    async fn find_zaehler(
        &self,
        zaehler_id: &str,
        tenant: &str,
    ) -> Result<Option<ZaehlerRecord>, crate::error::MdmError> {
        Ok(self
            .zaehler
            .read()
            .await
            .get(&(zaehler_id.to_owned(), tenant.to_owned()))
            .cloned())
    }

    async fn upsert_geraet(
        &self,
        geraet_id: &str,
        tenant: &str,
        zaehler_id: &str,
        geraet_typ: Option<&str>,
        data: serde_json::Value,
        bo4e_version: &str,
    ) -> Result<(), crate::error::MdmError> {
        let key = (geraet_id.to_owned(), tenant.to_owned());
        let version = {
            let g = self.geraete.read().await;
            g.get(&key).map_or(1, |r| r.version + 1)
        };
        self.geraete.write().await.insert(
            key,
            GeraetRecord {
                geraet_id: geraet_id.to_owned(),
                tenant: tenant.to_owned(),
                zaehler_id: zaehler_id.to_owned(),
                geraet_typ: geraet_typ.map(std::borrow::ToOwned::to_owned),
                data,
                bo4e_version: bo4e_version.to_owned(),
                version,
                updated_at: time::OffsetDateTime::now_utc(),
            },
        );
        Ok(())
    }

    async fn list_geraete_by_zaehler(
        &self,
        zaehler_id: &str,
        tenant: &str,
    ) -> Result<Vec<GeraetRecord>, crate::error::MdmError> {
        Ok(self
            .geraete
            .read()
            .await
            .values()
            .filter(|r| r.tenant == tenant && r.zaehler_id == zaehler_id)
            .cloned()
            .collect())
    }
}

// ── InMemoryPreisblattKaRepository (B3) ──────────────────────────────────────

use crate::repository::{PreisblattKaRecord, PreisblattKaRepository};

type KaKey = (String, String, Option<String>);

/// In-memory test double for [`PreisblattKaRepository`].
#[derive(Default, Clone)]
pub struct InMemoryPreisblattKaRepository {
    store: Arc<RwLock<HashMap<KaKey, PreisblattKaRecord>>>,
}

impl PreisblattKaRepository for InMemoryPreisblattKaRepository {
    async fn upsert_ka(
        &self,
        nb_mp_id: &str,
        sparte: &str,
        kundengruppe_ka: Option<&str>,
        data: serde_json::Value,
        bo4e_version: &str,
        source: PreisblattSource,
    ) -> Result<(), crate::error::MdmError> {
        let key = (
            nb_mp_id.to_owned(),
            sparte.to_owned(),
            kundengruppe_ka.map(ToOwned::to_owned),
        );
        self.store.write().await.insert(
            key,
            PreisblattKaRecord {
                nb_mp_id: nb_mp_id.to_owned(),
                sparte: sparte.to_owned(),
                kundengruppe_ka: kundengruppe_ka.map(ToOwned::to_owned),
                data,
                bo4e_version: bo4e_version.to_owned(),
                source,
                created_at: time::OffsetDateTime::now_utc(),
                updated_at: time::OffsetDateTime::now_utc(),
            },
        );
        Ok(())
    }

    async fn find_ka_for_date(
        &self,
        nb_mp_id: &str,
        sparte: &str,
        kundengruppe_ka: Option<&str>,
        _billing_date: &str,
    ) -> Result<Option<PreisblattKaRecord>, crate::error::MdmError> {
        let store = self.store.read().await;
        // First try exact match, then fallback to generic (None kundengruppe)
        let exact = store.get(&(
            nb_mp_id.to_owned(),
            sparte.to_owned(),
            kundengruppe_ka.map(ToOwned::to_owned),
        ));
        if exact.is_some() {
            return Ok(exact.cloned());
        }
        Ok(store
            .get(&(nb_mp_id.to_owned(), sparte.to_owned(), None))
            .cloned())
    }
}

// ── InMemoryPreisblattDienstleistungRepository ───────────────────────────────

use crate::repository::{
    PreisblattDienstleistungRecord, PreisblattDienstleistungRepository, PreisblattHardwareRecord,
    PreisblattHardwareRepository,
};

/// In-memory test double for [`PreisblattDienstleistungRepository`].
#[derive(Default, Clone)]
pub struct InMemoryPreisblattDienstleistungRepository {
    store: Arc<RwLock<HashMap<String, PreisblattDienstleistungRecord>>>,
}

impl PreisblattDienstleistungRepository for InMemoryPreisblattDienstleistungRepository {
    async fn upsert_dienstleistung(
        &self,
        msb_mp_id: &str,
        data: serde_json::Value,
        bo4e_version: &str,
        source: PreisblattSource,
    ) -> Result<(), crate::error::MdmError> {
        self.store.write().await.insert(
            msb_mp_id.to_owned(),
            PreisblattDienstleistungRecord {
                msb_mp_id: msb_mp_id.to_owned(),
                data,
                bo4e_version: bo4e_version.to_owned(),
                source,
                created_at: time::OffsetDateTime::now_utc(),
                updated_at: time::OffsetDateTime::now_utc(),
            },
        );
        Ok(())
    }
    async fn find_dienstleistung_for_date(
        &self,
        msb_mp_id: &str,
        _billing_date: &str,
    ) -> Result<Option<PreisblattDienstleistungRecord>, crate::error::MdmError> {
        Ok(self.store.read().await.get(msb_mp_id).cloned())
    }
}

/// In-memory test double for [`PreisblattHardwareRepository`].
#[derive(Default, Clone)]
pub struct InMemoryPreisblattHardwareRepository {
    store: Arc<RwLock<HashMap<String, PreisblattHardwareRecord>>>,
}

impl PreisblattHardwareRepository for InMemoryPreisblattHardwareRepository {
    async fn upsert_hardware(
        &self,
        msb_mp_id: &str,
        data: serde_json::Value,
        bo4e_version: &str,
        source: PreisblattSource,
    ) -> Result<(), crate::error::MdmError> {
        self.store.write().await.insert(
            msb_mp_id.to_owned(),
            PreisblattHardwareRecord {
                msb_mp_id: msb_mp_id.to_owned(),
                data,
                bo4e_version: bo4e_version.to_owned(),
                source,
                created_at: time::OffsetDateTime::now_utc(),
                updated_at: time::OffsetDateTime::now_utc(),
            },
        );
        Ok(())
    }
    async fn find_hardware_for_date(
        &self,
        msb_mp_id: &str,
        _billing_date: &str,
    ) -> Result<Option<PreisblattHardwareRecord>, crate::error::MdmError> {
        Ok(self.store.read().await.get(msb_mp_id).cloned())
    }
}

// ── InMemoryZaehlzeitRepository ───────────────────────────────────────────────

use crate::repository::{ZaehlzeitRegisterRecord, ZaehlzeitRepository, ZaehlzeitSaisonRecord};

/// In-memory test double for [`ZaehlzeitRepository`].
#[derive(Default, Clone)]
pub struct InMemoryZaehlzeitRepository {
    registers: Arc<RwLock<HashMap<uuid::Uuid, ZaehlzeitRegisterRecord>>>,
    saisons: Arc<RwLock<HashMap<uuid::Uuid, ZaehlzeitSaisonRecord>>>,
}

impl ZaehlzeitRepository for InMemoryZaehlzeitRepository {
    async fn upsert_register(
        &self,
        rec: &ZaehlzeitRegisterRecord,
    ) -> Result<(), crate::error::MdmError> {
        self.registers.write().await.insert(rec.id, rec.clone());
        Ok(())
    }

    async fn list_registers_by_zaehler(
        &self,
        zaehler_id: &str,
        tenant: &str,
    ) -> Result<Vec<ZaehlzeitRegisterRecord>, crate::error::MdmError> {
        Ok(self
            .registers
            .read()
            .await
            .values()
            .filter(|r| r.zaehler_id == zaehler_id && r.tenant == tenant)
            .cloned()
            .collect())
    }

    async fn upsert_saison(
        &self,
        rec: &ZaehlzeitSaisonRecord,
    ) -> Result<(), crate::error::MdmError> {
        self.saisons.write().await.insert(rec.id, rec.clone());
        Ok(())
    }

    async fn list_saisons_by_register(
        &self,
        register_id: uuid::Uuid,
        _tenant: &str,
    ) -> Result<Vec<ZaehlzeitSaisonRecord>, crate::error::MdmError> {
        Ok(self
            .saisons
            .read()
            .await
            .values()
            .filter(|s| s.register_id == register_id)
            .cloned()
            .collect())
    }

    async fn resolve_tariff_zone(
        &self,
        zaehler_id: &str,
        tenant: &str,
        local_datetime: time::PrimitiveDateTime,
    ) -> Result<Option<String>, crate::error::MdmError> {
        use time::Weekday;
        let registers = self.list_registers_by_zaehler(zaehler_id, tenant).await?;
        let time_str = format!(
            "{:02}:{:02}",
            local_datetime.hour(),
            local_datetime.minute()
        );
        let weekday_iso = match local_datetime.weekday() {
            Weekday::Monday => 1u8,
            Weekday::Tuesday => 2,
            Weekday::Wednesday => 3,
            Weekday::Thursday => 4,
            Weekday::Friday => 5,
            Weekday::Saturday => 6,
            Weekday::Sunday => 7,
        };
        for reg in &registers {
            let saisons = self.list_saisons_by_register(reg.id, tenant).await?;
            for s in &saisons {
                // Check weekday mask.
                let days: Vec<u8> =
                    serde_json::from_value(s.wochentage.clone()).unwrap_or_default();
                if !days.contains(&weekday_iso) {
                    continue;
                }
                // Check time window (lexicographic comparison works for HH:MM).
                if time_str >= s.zeit_von && time_str < s.zeit_bis {
                    return Ok(Some(reg.zaehlerauspraegung.clone()));
                }
            }
        }
        Ok(None)
    }
}

// ── InMemoryNbEnergiemixRepository ───────────────────────────────────────────

use crate::repository::{NbEnergiemixRecord, NbEnergiemixRepository};

/// In-memory `NbEnergiemixRepository` for unit tests.
///
/// Key: `(tenant, nb_mp_id, gueltig_fuer)`.
#[derive(Clone, Default)]
pub struct InMemoryNbEnergiemixRepository {
    #[allow(clippy::type_complexity)]
    store: Arc<RwLock<HashMap<(String, String, i16), NbEnergiemixRecord>>>,
}

impl NbEnergiemixRepository for InMemoryNbEnergiemixRepository {
    async fn upsert_energiemix(
        &self,
        tenant: &str,
        nb_mp_id: &str,
        gueltig_fuer: i16,
        energiemix: serde_json::Value,
        eeg_einspeisung_kwh: Option<i64>,
        gesamtentnahme_kwh: Option<i64>,
    ) -> Result<(), crate::error::MdmError> {
        let mut store = self.store.write().await;
        store.insert(
            (tenant.to_owned(), nb_mp_id.to_owned(), gueltig_fuer),
            NbEnergiemixRecord {
                nb_mp_id: nb_mp_id.to_owned(),
                gueltig_fuer,
                energiemix,
                eeg_einspeisung_kwh,
                gesamtentnahme_kwh,
                updated_at: Some(time::OffsetDateTime::now_utc()),
            },
        );
        Ok(())
    }

    async fn find_energiemix(
        &self,
        tenant: &str,
        nb_mp_id: &str,
        year: Option<i16>,
    ) -> Result<Option<NbEnergiemixRecord>, crate::error::MdmError> {
        let store = self.store.read().await;
        if let Some(y) = year {
            return Ok(store
                .get(&(tenant.to_owned(), nb_mp_id.to_owned(), y))
                .cloned());
        }
        // Most recent year
        let record = store
            .iter()
            .filter(|((t, n, _), _)| t == tenant && n == nb_mp_id)
            .max_by_key(|((_, _, y), _)| *y)
            .map(|(_, v)| v.clone());
        Ok(record)
    }

    async fn list_energiemix_years(
        &self,
        tenant: &str,
        nb_mp_id: &str,
    ) -> Result<Vec<i16>, crate::error::MdmError> {
        let store = self.store.read().await;
        let mut years: Vec<i16> = store
            .keys()
            .filter(|(t, n, _)| t == tenant && n == nb_mp_id)
            .map(|(_, _, y)| *y)
            .collect();
        years.sort_unstable_by(|a, b| b.cmp(a)); // desc
        Ok(years)
    }
}

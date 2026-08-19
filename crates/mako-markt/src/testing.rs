#![allow(clippy::doc_markdown)]
#![allow(clippy::collapsible_if)]
//! In-memory test doubles for the two `mako-markt` repository traits whose
//! *regulatory* semantics are worth asserting without a database.
//!
//! Enabled only with `features = ["testing"]` — never in production.
//!
//! ## Why only two
//!
//! A hand-written double is a second implementation of the same contract, and
//! the two drift: the subscription double once matched `event_types` with its
//! own trailing-`*` rule while production used `mako_events::matches`, so the
//! tests agreed with the double and not with the service. Every other
//! repository is therefore exercised against a real PostgreSQL in
//! `services/marktd/tests/*_integration.rs` (testcontainers), which is the only
//! place a SQL contract can actually be checked.
//!
//! The two kept here back `tests/regulatory_scenarios.rs`, which asserts
//! statutory state machines — the §36/§38 EnWG supply-status transitions and the
//! §42 EnWG Energiemix disclosure — where the interesting logic is in the
//! transition rules, not in the SQL. Their SQL counterparts are covered by
//! `versorgung_integration.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use time::Date;
use tokio::sync::RwLock;

use crate::{
    domain::MaloId,
    error::MdmError,
    repository::{
        LieferStatus, PageResult, VersorgungsStatusHistoryRecord, VersorgungsStatusRecord,
        VersorgungsStatusRepository,
    },
};

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
                eog_seit: None,
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
            eog_seit: None,
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
            eog_seit: None,
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

    async fn clear_lf_next(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        let key = (malo_id.as_ref().to_owned(), tenant.to_owned());
        let mut store = self.store.write().await;
        let Some(entry) = store.get_mut(&key) else {
            return Ok(());
        };
        if entry.lf_mp_id_next.is_none() {
            return Ok(()); // no pending announcement — no-op
        }
        entry.lf_mp_id_next = None;
        entry.lf_next_lieferbeginn = None;
        entry.last_process_id = process_id;
        entry.updated_at = time::OffsetDateTime::now_utc();
        entry.version += 1;
        let rec = entry.clone();
        drop(store);
        self.history
            .write()
            .await
            .push(VersorgungsStatusHistoryRecord {
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
                valid_from: rec.updated_at,
            });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn begin_eog_supply(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        gv_mp_id: &str,
        nb_mp_id: &str,
        eog_status: LieferStatus,
        eog_seit: Option<time::Date>,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        if !matches!(
            eog_status,
            LieferStatus::Ersatzversorgung | LieferStatus::Grundversorgung
        ) {
            return Err(MdmError::Unprocessable {
                reason: "begin_eog_supply requires Ersatzversorgung or Grundversorgung".into(),
            });
        }
        let key = (malo_id.as_ref().to_owned(), tenant.to_owned());
        let mut store = self.store.write().await;
        let now = time::OffsetDateTime::now_utc();
        let entry = store.entry(key).or_insert_with(|| VersorgungsStatusRecord {
            malo_id: malo_id.clone(),
            tenant: tenant.to_owned(),
            lieferstatus: eog_status,
            lf_mp_id: None,
            lf_mp_id_next: None,
            lf_next_lieferbeginn: None,
            lieferbeginn: None,
            lieferende: None,
            msb_mp_id: None,
            nb_mp_id: nb_mp_id.to_owned(),
            eog_seit: None,
            last_process_id: process_id,
            updated_at: now,
            version: 0,
        });
        entry.lieferstatus = eog_status;
        entry.lf_mp_id = Some(gv_mp_id.to_owned());
        entry.lieferbeginn = eog_seit;
        entry.eog_seit = eog_seit;
        entry.nb_mp_id = nb_mp_id.to_owned();
        entry.last_process_id = process_id;
        entry.updated_at = now;
        entry.version += 1;
        let rec = entry.clone();
        drop(store);
        self.history
            .write()
            .await
            .push(VersorgungsStatusHistoryRecord {
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
            });
        Ok(())
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

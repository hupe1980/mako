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

use rust_decimal::Decimal;

use crate::{
    domain::MaloId,
    error::MdmError,
    repository::{
        LfZuordnung, LieferStatus, PageResult, VersorgungsStatusHistoryRecord,
        VersorgungsStatusRecord, VersorgungsStatusRepository, ZuordnungsStatus,
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

impl InMemoryVersorgungsStatusRepository {
    /// A fresh, unsupplied record — what every mutation starts from when the
    /// MaLo is not in the projection yet.
    fn blank(malo_id: &MaloId, tenant: &str, nb_mp_id: &str) -> VersorgungsStatusRecord {
        VersorgungsStatusRecord {
            malo_id: malo_id.clone(),
            tenant: tenant.to_owned(),
            lieferstatus: LieferStatus::Unbeliefert,
            zuordnungen: Vec::new(),
            lieferende: None,
            msb_mp_id: None,
            nb_mp_id: nb_mp_id.to_owned(),
            eog_seit: None,
            last_process_id: None,
            updated_at: time::OffsetDateTime::now_utc(),
            version: 0,
        }
    }

    /// Snapshot a record into the history log — the one place the two shapes
    /// are mapped onto each other.
    async fn snapshot(&self, rec: &VersorgungsStatusRecord, at: time::OffsetDateTime) {
        self.history
            .write()
            .await
            .push(VersorgungsStatusHistoryRecord {
                id: rec.version,
                malo_id: rec.malo_id.clone(),
                tenant: rec.tenant.clone(),
                lieferstatus: rec.lieferstatus,
                zuordnungen: rec.zuordnungen.clone(),
                lieferende: rec.lieferende,
                msb_mp_id: rec.msb_mp_id.clone(),
                nb_mp_id: rec.nb_mp_id.clone(),
                last_process_id: rec.last_process_id,
                version: rec.version,
                valid_from: at,
            });
    }

    /// Apply `edit` to the record for this MaLo, bump its version and snapshot
    /// it. `edit` returning `false` means „nothing changed" — no version bump,
    /// no history row, which is what makes a redelivered event idempotent.
    async fn mutate(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        nb_mp_id: &str,
        process_id: Option<uuid::Uuid>,
        edit: impl FnOnce(&mut VersorgungsStatusRecord) -> bool,
    ) {
        let key = (malo_id.as_ref().to_owned(), tenant.to_owned());
        let now = time::OffsetDateTime::now_utc();
        let mut store = self.store.write().await;
        let entry = store
            .entry(key)
            .or_insert_with(|| Self::blank(malo_id, tenant, nb_mp_id));
        if !edit(entry) {
            return;
        }
        entry.last_process_id = process_id;
        entry.updated_at = now;
        entry.version += 1;
        let rec = entry.clone();
        drop(store);
        self.snapshot(&rec, now).await;
    }
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
        let snap = rec.clone();
        store.insert(key, rec);
        drop(store);
        self.snapshot(&snap, now).await;
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
                zuordnungen: h.zuordnungen.clone(),
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
        prozent: Decimal,
        tranche_id: Option<&str>,
        nb_mp_id: &str,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        self.mutate(malo_id, tenant, nb_mp_id, process_id, |rec| {
            // Re-announcing the same (LF, Tranche) updates in place, so an
            // at-least-once redelivery does not accumulate assignments.
            let slot = rec.zuordnungen.iter_mut().find(|z| {
                z.status == ZuordnungsStatus::Angekuendigt
                    && z.lf_mp_id == lf_mp_id_next
                    && z.tranche_id.as_deref() == tranche_id
            });
            let Some(z) = slot else {
                rec.zuordnungen.push(LfZuordnung {
                    lf_mp_id: lf_mp_id_next.to_owned(),
                    prozent,
                    tranche_id: tranche_id.map(ToOwned::to_owned),
                    status: ZuordnungsStatus::Angekuendigt,
                    zuordnungsbeginn: lf_next_lieferbeginn,
                    zuordnungsende: None,
                    process_id,
                });
                return true;
            };
            z.prozent = prozent;
            z.zuordnungsbeginn = lf_next_lieferbeginn;
            z.process_id = process_id;
            true
        })
        .await;
        Ok(())
    }

    async fn confirm_supply(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        lf_mp_id: Option<&str>,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        self.mutate(malo_id, tenant, "", process_id, |rec| {
            // `None` is „the one that is pending", which is well defined
            // exactly while there is one; with several it resolves to none.
            let lf_mp_id = match lf_mp_id {
                Some(lf) => lf.to_owned(),
                None => match rec.lf_mp_id_next() {
                    Some(lf) => lf.to_owned(),
                    None => return false,
                },
            };
            let lf_mp_id = lf_mp_id.as_str();
            let Some(idx) = rec
                .zuordnungen
                .iter()
                .position(|z| z.status == ZuordnungsStatus::Angekuendigt && z.lf_mp_id == lf_mp_id)
            else {
                return false; // nothing announced by this LF — idempotent no-op
            };
            let tranche = rec.zuordnungen[idx].tranche_id.clone();
            // An Anmeldung for a Tranche displaces only that Tranche's holder;
            // an untranchierte one displaces the single 100 % assignment.
            rec.zuordnungen
                .retain(|z| z.status != ZuordnungsStatus::Aktiv || z.tranche_id != tranche);
            if let Some(z) = rec
                .zuordnungen
                .iter_mut()
                .find(|z| z.status == ZuordnungsStatus::Angekuendigt && z.lf_mp_id == lf_mp_id)
            {
                z.status = ZuordnungsStatus::Aktiv;
                z.process_id = process_id;
            }
            rec.lieferstatus = LieferStatus::Beliefert;
            rec.eog_seit = None;
            true
        })
        .await;
        Ok(())
    }

    async fn end_supply(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        lf_mp_id: Option<&str>,
        nb_mp_id: &str,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        self.mutate(malo_id, tenant, nb_mp_id, process_id, |rec| {
            rec.zuordnungen.retain(|z| {
                z.status != ZuordnungsStatus::Aktiv || lf_mp_id.is_some_and(|lf| z.lf_mp_id != lf)
            });
            // One LFA leaving a tranchierte Marktlokation does not make it
            // unsupplied — only the last one does.
            if rec.aktive().next().is_none() {
                rec.lieferstatus = LieferStatus::Unbeliefert;
                rec.eog_seit = None;
            }
            nb_mp_id.clone_into(&mut rec.nb_mp_id);
            true
        })
        .await;
        Ok(())
    }

    async fn clear_lf_next(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        lf_mp_id: Option<&str>,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        let key = (malo_id.as_ref().to_owned(), tenant.to_owned());
        if !self.store.read().await.contains_key(&key) {
            return Ok(());
        }
        self.mutate(malo_id, tenant, "", process_id, |rec| {
            let before = rec.zuordnungen.len();
            rec.zuordnungen.retain(|z| {
                z.status != ZuordnungsStatus::Angekuendigt
                    || lf_mp_id.is_some_and(|lf| z.lf_mp_id != lf)
            });
            rec.zuordnungen.len() != before
        })
        .await;
        Ok(())
    }

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
        self.mutate(malo_id, tenant, nb_mp_id, process_id, |rec| {
            // The E/G becomes the sole supplier of record; every announced
            // assignment stays, because a pending switch ends the fallback.
            rec.zuordnungen
                .retain(|z| z.status != ZuordnungsStatus::Aktiv);
            rec.zuordnungen.push(LfZuordnung {
                zuordnungsbeginn: eog_seit,
                process_id,
                ..LfZuordnung::ganz(gv_mp_id, ZuordnungsStatus::Aktiv)
            });
            rec.lieferstatus = eog_status;
            rec.eog_seit = eog_seit;
            nb_mp_id.clone_into(&mut rec.nb_mp_id);
            true
        })
        .await;
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

//! In-memory [`ProcessProjectionRepository`] for tests.

use std::{collections::HashMap, sync::Mutex};

use time::Date;
use uuid::Uuid;

use crate::{
    domain::{DeadlineRisk, KpiReport, ObsQuery, ProcessProjection, ProcessState},
    error::ObsError,
    repository::ProcessProjectionRepository,
};

/// Thread-safe in-memory projection store.
#[derive(Debug, Default)]
pub struct InMemoryProcessProjectionRepository {
    projections: Mutex<HashMap<Uuid, ProcessProjection>>,
}

impl InMemoryProcessProjectionRepository {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl ProcessProjectionRepository for InMemoryProcessProjectionRepository {
    async fn upsert(&self, p: &ProcessProjection) -> Result<(), ObsError> {
        let mut guard = self.projections.lock().unwrap();
        guard.insert(p.process_id, p.clone());
        Ok(())
    }

    async fn query(&self, q: &ObsQuery) -> Result<Vec<ProcessProjection>, ObsError> {
        let guard = self.projections.lock().unwrap();
        let mut results: Vec<ProcessProjection> = guard
            .values()
            .filter(|p| {
                q.state.is_none_or(|s| p.state == s)
                    && q.pid.is_none_or(|pid| p.pid == pid)
                    && q.partner_mp_id
                        .as_deref()
                        .is_none_or(|g| p.partner_mp_id.as_deref() == Some(g))
                    && q.mdm_role
                        .as_deref()
                        .is_none_or(|r| p.mdm_role.as_deref() == Some(r))
                    && q.since.is_none_or(|s| p.started_at >= s)
                    && q.tenant.as_deref().is_none_or(|t| p.tenant == t)
                    && q.family.as_deref().is_none_or(|f| p.family == f)
            })
            .cloned()
            .collect();

        results.sort_by(|a, b| b.last_event_at.cmp(&a.last_event_at));
        results.truncate(q.limit as usize);
        Ok(results)
    }

    async fn get(&self, process_id: Uuid) -> Result<Option<ProcessProjection>, ObsError> {
        let guard = self.projections.lock().unwrap();
        Ok(guard.get(&process_id).cloned())
    }

    async fn kpi_report(
        &self,
        pid: u32,
        from: Date,
        to: Date,
        _tenant: &str,
    ) -> Result<KpiReport, ObsError> {
        let guard = self.projections.lock().unwrap();
        let relevant: Vec<_> = guard
            .values()
            .filter(|p| p.pid == pid && p.started_at.date() >= from && p.started_at.date() <= to)
            .collect();

        if relevant.is_empty() {
            return Err(ObsError::NoKpiData {
                pid,
                from: from.to_string(),
                to: to.to_string(),
            });
        }

        let count = |state: ProcessState| -> u64 {
            relevant.iter().filter(|p| p.state == state).count() as u64
        };
        let total = relevant.len() as u64;
        let with_frist = relevant.iter().filter(|p| p.deadline_at.is_some()).count() as u64;
        let now = time::OffsetDateTime::now_utc();
        let breached = relevant
            .iter()
            .filter(|p| {
                // The business window, never the APERAK clock: measured against
                // when the process closed, or against now while it is open.
                let measured_at = if p.state.is_terminal() {
                    p.last_event_at
                } else {
                    now
                };
                p.deadline_at.is_some_and(|d| d < measured_at)
            })
            .count() as u64;

        Ok(KpiReport {
            pid,
            period_from: from,
            period_to: to,
            total_initiated: total,
            total_completed: count(ProcessState::Completed),
            total_rejected: count(ProcessState::Rejected),
            total_failed: count(ProcessState::Failed),
            total_aperak_timeout: count(ProcessState::AperakTimeout),
            total_frist_breached: breached,
            total_with_frist: with_frist,
            frist_compliance_rate: (with_frist > 0)
                .then(|| 1.0 - (breached as f64 / with_frist as f64)),
            avg_cycle_time_hours: None,
            p95_cycle_time_hours: None,
        })
    }

    async fn overdue_processes(
        &self,
        now: time::OffsetDateTime,
        _tenant: &str,
    ) -> Result<Vec<ProcessProjection>, ObsError> {
        let guard = self.projections.lock().unwrap();
        Ok(guard
            .values()
            .filter(|p| {
                !p.state.is_terminal()
                    && p.deadline_risk == DeadlineRisk::Red
                    && p.deadline_at.is_some_and(|d| d < now)
            })
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{DeadlineRisk, ProcessState};
    use time::OffsetDateTime;

    #[tokio::test]
    async fn upsert_and_get() {
        let repo = InMemoryProcessProjectionRepository::new();
        let process_id = Uuid::new_v4();
        let proj = ProcessProjection {
            process_id,
            pid: 55001,
            family: "gpke".into(),
            workflow_name: "gpke-lf-anmeldung".into(),
            state: ProcessState::Initiated,
            malo_id: Some("DE00001".into()),
            partner_mp_id: Some("9900000000001".into()),
            mdm_role: Some("LF".into()),
            deadline_at: None,
            deadline_source: None,
            deadline_risk: DeadlineRisk::Unknown,
            started_at: OffsetDateTime::now_utc(),
            last_event_at: OffsetDateTime::now_utc(),
            erc_code: None,
            initiator_is_affiliate: false,
            tenant: "9900357000004".into(),
        };
        repo.upsert(&proj).await.unwrap();
        let found = repo.get(process_id).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().pid, 55001);
    }

    #[tokio::test]
    async fn query_by_state() {
        let repo = InMemoryProcessProjectionRepository::new();
        for _ in 0..3 {
            let proj = ProcessProjection {
                process_id: Uuid::new_v4(),
                pid: 55001,
                family: "gpke".into(),
                workflow_name: "gpke-lf-anmeldung".into(),
                state: ProcessState::Completed,
                malo_id: None,
                partner_mp_id: None,
                mdm_role: None,
                deadline_at: None,
                deadline_source: None,
                deadline_risk: DeadlineRisk::Unknown,
                started_at: OffsetDateTime::now_utc(),
                last_event_at: OffsetDateTime::now_utc(),
                erc_code: None,
                initiator_is_affiliate: false,
                tenant: "9900357000004".into(),
            };
            repo.upsert(&proj).await.unwrap();
        }
        let results = repo
            .query(&ObsQuery {
                state: Some(ProcessState::Completed),
                limit: 100,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(results.len(), 3);
    }
}

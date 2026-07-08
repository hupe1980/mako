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
                    && q.tenant_id.is_none_or(|t| p.tenant_id == Some(t))
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
        _tenant_id: Option<Uuid>,
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

        let total = relevant.len() as u64;
        let completed = relevant
            .iter()
            .filter(|p| p.state == ProcessState::Completed)
            .count() as u64;
        let rejected = relevant
            .iter()
            .filter(|p| p.state == ProcessState::Rejected)
            .count() as u64;
        let timeout = relevant
            .iter()
            .filter(|p| p.state == ProcessState::AperakTimeout)
            .count() as u64;
        let cancelled = relevant
            .iter()
            .filter(|p| p.state == ProcessState::Cancelled)
            .count() as u64;

        let compliance = if total > 0 {
            (total - timeout) as f64 / total as f64
        } else {
            1.0
        };

        Ok(KpiReport {
            pid,
            period_from: from,
            period_to: to,
            total_initiated: total,
            total_completed: completed,
            total_rejected: rejected,
            total_aperak_timeout: timeout,
            total_cancelled: cancelled,
            aperak_compliance_rate: compliance,
            avg_cycle_time_hours: 0.0,
            p95_cycle_time_hours: 0.0,
        })
    }

    async fn overdue_processes(
        &self,
        now: time::OffsetDateTime,
        _tenant_id: Option<Uuid>,
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
            deadline_risk: DeadlineRisk::Green,
            started_at: OffsetDateTime::now_utc(),
            last_event_at: OffsetDateTime::now_utc(),
            erc_code: None,
            tenant_id: None,
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
                deadline_risk: DeadlineRisk::Green,
                started_at: OffsetDateTime::now_utc(),
                last_event_at: OffsetDateTime::now_utc(),
                erc_code: None,
                tenant_id: None,
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

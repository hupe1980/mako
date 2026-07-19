//! Dead-letter queue for failed CloudEvent sessions.
//!
//! When an agent session fails (outcome `"error"` or `"timeout"`), the original
//! CloudEvent is placed here.  A background worker retries it with exponential
//! backoff.  After `max_retries` attempts the entry is marked `EXHAUSTED` and
//! a `de.agent.session.dlq.exhausted` CloudEvent is emitted to the audit webhook.
//!
//! ## Queue lifecycle
//!
//! ```text
//! webhook handler
//!   → session fails (outcome=error/timeout)
//!   → DlqStore::push(entry)         ← bounded to `capacity` (oldest entry evicted)
//!
//! DLQ background worker (every 10 s)
//!   → DlqStore::due_entries()       ← entries past next_retry_at
//!   → dispatch via Orchestrator
//!   → on success: DlqStore::ack(id)
//!   → on failure: DlqStore::record_failure(id, error)
//!                 if attempts >= max_retries → EXHAUSTED → emit alert CE
//! ```
//!
//! ## Endpoints
//!
//! `GET /api/v1/dlq` — live queue depth + up to 20 recent exhausted entries.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use time::OffsetDateTime;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DlqStatus {
    /// Pending retry — `next_retry_at` tells when to fire.
    Pending,
    /// All retry attempts exhausted — alert was emitted.
    Exhausted,
}

/// A single dead-letter entry.
#[derive(Debug, Clone)]
pub struct DlqEntry {
    pub id: String,
    pub event_type: String,
    pub event_id: String,
    pub payload: serde_json::Value,
    pub attempts: u32,
    pub max_retries: u32,
    pub first_failure_at: OffsetDateTime,
    pub next_retry_at: OffsetDateTime,
    pub last_error: String,
    pub status: DlqStatus,
    /// Base backoff in seconds (configured via DlqConfig).
    base_backoff_secs: u64,
}

impl DlqEntry {
    pub fn new(
        event_type: impl Into<String>,
        event_id: impl Into<String>,
        payload: serde_json::Value,
        error: impl Into<String>,
        max_retries: u32,
        base_backoff_secs: u64,
    ) -> Self {
        let now = OffsetDateTime::now_utc();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            event_type: event_type.into(),
            event_id: event_id.into(),
            payload,
            attempts: 0,
            max_retries,
            first_failure_at: now,
            // First retry immediately (no delay on first attempt)
            next_retry_at: now,
            last_error: error.into(),
            status: DlqStatus::Pending,
            base_backoff_secs,
        }
    }

    /// Whether this entry is due for another retry attempt.
    pub fn is_due(&self) -> bool {
        self.status == DlqStatus::Pending && OffsetDateTime::now_utc() >= self.next_retry_at
    }

    /// Record a retry failure and schedule the next attempt.
    pub fn record_failure(&mut self, error: impl Into<String>) {
        self.attempts += 1;
        self.last_error = error.into();

        if self.attempts >= self.max_retries {
            self.status = DlqStatus::Exhausted;
        } else {
            // Exponential backoff: base * 3^attempt (capped at 1 hour)
            let delay_secs = self
                .base_backoff_secs
                .saturating_mul(3u64.saturating_pow(self.attempts))
                .min(3600);
            self.next_retry_at =
                OffsetDateTime::now_utc() + time::Duration::seconds(delay_secs as i64);
        }
    }

    /// Mark as successfully retried.
    pub fn ack(&mut self) {
        // Entries that are acked are removed from the queue by `DlqStore::ack`.
    }
}

// ── DlqStore ─────────────────────────────────────────────────────────────────

/// Thread-safe dead-letter store.  Cheap to clone (Arc-backed).
#[derive(Clone)]
pub struct DlqStore {
    inner: Arc<Mutex<Inner>>,
    capacity: usize,
    pub max_retries: u32,
    pub base_backoff_secs: u64,
}

struct Inner {
    /// Active pending / retrying entries.
    pending: VecDeque<DlqEntry>,
    /// Recent exhausted entries (for `GET /api/v1/dlq`).
    exhausted: VecDeque<DlqEntry>,
}

impl DlqStore {
    pub fn new(capacity: usize, max_retries: u32, base_backoff_secs: u64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                pending: VecDeque::new(),
                exhausted: VecDeque::with_capacity(20),
            })),
            capacity,
            max_retries,
            base_backoff_secs,
        }
    }

    /// Push a new dead-letter entry.  Returns `false` if the queue is at capacity
    /// (entry is silently dropped with a WARN log to prevent memory exhaustion).
    pub fn push(&self, entry: DlqEntry) -> bool {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.pending.len() >= self.capacity {
            tracing::warn!(
                capacity = self.capacity,
                event_type = %entry.event_type,
                "DLQ at capacity — dropping entry; increase [dlq] capacity"
            );
            return false;
        }
        g.pending.push_back(entry);
        true
    }

    /// Return clones of all entries that are due for retry.
    pub fn due_entries(&self) -> Vec<DlqEntry> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.pending.iter().filter(|e| e.is_due()).cloned().collect()
    }

    /// Record a retry failure for the entry with the given `id`.
    /// If the entry is now exhausted, moves it to the exhausted archive and
    /// returns a clone for alert emission.
    pub fn record_failure(&self, id: &str, error: &str) -> Option<DlqEntry> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = g.pending.iter_mut().find(|e| e.id == id) {
            entry.record_failure(error);
            if entry.status == DlqStatus::Exhausted {
                let exhausted = entry.clone();
                g.pending.retain(|e| e.id != id);
                if g.exhausted.len() >= 20 {
                    g.exhausted.pop_front();
                }
                g.exhausted.push_back(exhausted.clone());
                return Some(exhausted);
            }
        }
        None
    }

    /// Remove a successfully retried entry from the queue.
    pub fn ack(&self, id: &str) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.pending.retain(|e| e.id != id);
    }

    /// Snapshot for `GET /api/v1/dlq`.
    pub fn snapshot(&self) -> DlqSnapshot {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        DlqSnapshot {
            pending_count: g.pending.len(),
            exhausted_count: g.exhausted.len(),
            entries: g.pending.iter().map(DlqEntryView::from).collect(),
            recent_exhausted: g
                .exhausted
                .iter()
                .rev()
                .take(20)
                .map(DlqEntryView::from)
                .collect(),
        }
    }
}

// ── REST response types ───────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct DlqSnapshot {
    pub pending_count: usize,
    pub exhausted_count: usize,
    pub entries: Vec<DlqEntryView>,
    pub recent_exhausted: Vec<DlqEntryView>,
}

#[derive(Debug, Serialize)]
pub struct DlqEntryView {
    pub id: String,
    pub event_type: String,
    pub event_id: String,
    pub attempts: u32,
    pub max_retries: u32,
    pub status: &'static str,
    pub last_error: String,
    pub first_failure_at: String,
    pub next_retry_at: String,
}

impl From<&DlqEntry> for DlqEntryView {
    fn from(e: &DlqEntry) -> Self {
        Self {
            id: e.id.clone(),
            event_type: e.event_type.clone(),
            event_id: e.event_id.clone(),
            attempts: e.attempts,
            max_retries: e.max_retries,
            status: if e.status == DlqStatus::Pending {
                "pending"
            } else {
                "exhausted"
            },
            last_error: e.last_error.clone(),
            first_failure_at: e
                .first_failure_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
            next_retry_at: e
                .next_retry_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
        }
    }
}

// ── Retry pass (called by background worker in main.rs) ──────────────────────

/// Run one retry pass: dispatch all due DLQ entries via the orchestrator.
///
/// Called every 10 s by the background worker spawned in `main.rs`.
/// Uses `AppState` to access the orchestrator, registry, MCP pool, and RAG engine
/// — the same path used by the live webhook handler.
pub async fn run_retry_pass(state: &std::sync::Arc<crate::handlers::AppState>) {
    let due = state.dlq.due_entries();
    if due.is_empty() {
        return;
    }
    tracing::info!(count = due.len(), "DLQ: retrying due entries");

    for entry in due {
        let id = entry.id.clone();
        let decision = state
            .orchestrator
            .dispatch(
                entry.event_id.clone(),
                entry.event_type.clone(),
                entry.payload.clone(),
                &state.registry,
                &state.mcp,
                state.rag.as_ref(),
                &state.cfg.tenant,
            )
            .await;

        if matches!(decision.outcome.as_str(), "error" | "timeout") {
            tracing::warn!(
                dlq_id = %id,
                attempt = entry.attempts + 1,
                max = entry.max_retries,
                outcome = %decision.outcome,
                "DLQ retry failed"
            );
            if let Some(exhausted) = state.dlq.record_failure(&id, &decision.summary) {
                tracing::error!(
                    dlq_id = %id,
                    event_type = %exhausted.event_type,
                    attempts = exhausted.attempts,
                    "DLQ entry exhausted — emitting alert"
                );
                // Emit exhaustion alert CloudEvent to audit webhook
                let alert_ce = serde_json::json!({
                    "specversion": "1.0",
                    "type": "de.agent.session.dlq.exhausted",
                    "source": format!("agentd/{}", state.cfg.tenant),
                    "id": uuid::Uuid::new_v4().to_string(),
                    "time": time::OffsetDateTime::now_utc()
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default(),
                    "data": {
                        "dlq_id": id,
                        "event_type": exhausted.event_type,
                        "event_id": exhausted.event_id,
                        "attempts": exhausted.attempts,
                        "last_error": exhausted.last_error,
                    }
                });
                if let Some(ref url) = state.cfg.audit_webhook_url {
                    let _ = mako_service::http::default_client()
                        .post(url)
                        .header("Content-Type", "application/cloudevents+json")
                        .json(&alert_ce)
                        .send()
                        .await;
                }
            }
        } else {
            tracing::info!(dlq_id = %id, "DLQ retry succeeded — acking entry");
            state.dlq.ack(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_due_entries() {
        let store = DlqStore::new(10, 3, 1);
        let entry = DlqEntry::new("de.test.event", "evt-1", serde_json::json!({}), "err", 3, 1);
        store.push(entry);
        let due = store.due_entries();
        assert_eq!(due.len(), 1);
    }

    #[test]
    fn capacity_limit_drops_excess() {
        let store = DlqStore::new(2, 3, 1);
        for i in 0..5u32 {
            store.push(DlqEntry::new(
                "de.test",
                format!("evt-{i}"),
                serde_json::json!({}),
                "err",
                3,
                1,
            ));
        }
        let snap = store.snapshot();
        assert_eq!(snap.pending_count, 2, "should be capped at capacity=2");
    }

    #[test]
    fn ack_removes_entry() {
        let store = DlqStore::new(10, 3, 1);
        let entry = DlqEntry::new("de.test.event", "evt-2", serde_json::json!({}), "err", 3, 1);
        let id = entry.id.clone();
        store.push(entry);
        store.ack(&id);
        assert_eq!(store.snapshot().pending_count, 0);
    }

    #[test]
    fn exhaustion_after_max_retries() {
        let store = DlqStore::new(10, 2, 1);
        let entry = DlqEntry::new("de.test.event", "evt-3", serde_json::json!({}), "err", 2, 1);
        let id = entry.id.clone();
        store.push(entry);
        store.record_failure(&id, "fail-1");
        let exhausted = store.record_failure(&id, "fail-2");
        assert!(exhausted.is_some(), "should be exhausted after 2 failures");
        assert_eq!(store.snapshot().pending_count, 0);
        assert_eq!(store.snapshot().exhausted_count, 1);
    }
}

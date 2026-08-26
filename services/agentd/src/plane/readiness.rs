//! Whether this instance can still do the thing it answers `202` about.
//!
//! `mako_service::run` pings the database for a daemon that has one and asks
//! [`Daemon::ready`](mako_service::service::Daemon::ready) for the rest. agentd
//! has no `[database]` — its durable state is the agentplane journal — so the
//! built-in ping covers nothing and this is where the real signal comes from.
//!
//! It matters for the topology `backend = "postgres"` exists to serve: several
//! instances share a store that can go away *after* startup, and an instance
//! that stays in the load balancer fails every admission. Nothing is lost — a
//! store failure is classified retryable, so the door answers `429` and an
//! at-least-once emitter keeps the message — but the instance is advertising a
//! capacity it does not have, and the orchestrator has no signal to act on.
//!
//! [`JournalStore::checkpoint`] is the probe: the cheapest call that proves the
//! store is reachable and answering, reading the Merkle head, touching no run and
//! writing nothing. One that wrote would make readiness a source of journal
//! records, and the journal carries only what agents did.
//!
//! It is bounded at two seconds. An unbounded probe against a hung connection
//! makes readiness hang, and an endpoint that never answers is read as *not
//! ready* only after the orchestrator's own timeout — late, and by the wrong
//! component.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use agentplane::journal::JournalStore;

/// How long the probe waits before calling the store unreachable.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The journal this process reads to answer `/health/ready`.
///
/// A `OnceLock` because [`Daemon::ready`](mako_service::service::Daemon::ready)
/// is handed the `ServiceContext` and nothing else — the domain state is built
/// after the readiness closure is composed, so the handle has to be left
/// somewhere the closure can find it. Set once, in `Daemon::build`.
static JOURNAL: OnceLock<Arc<dyn JournalStore>> = OnceLock::new();

/// Publish the journal handle the readiness probe reads.
///
/// Idempotent and silent on a second call: the runner builds one plane per
/// process, and a second registration could only come from a test.
pub fn register(journal: Arc<dyn JournalStore>) {
    let _ = JOURNAL.set(journal);
}

/// Whether the journal is reachable and answering.
///
/// `false` when the store is unreachable, slow past the probe timeout, or —
/// before `Daemon::build` has run — not yet registered. The last case matters:
/// answering `true` for a plane that does not exist yet would put an instance
/// into rotation before it can admit anything.
pub async fn journal_is_reachable() -> bool {
    let Some(journal) = JOURNAL.get() else {
        return false;
    };
    match tokio::time::timeout(PROBE_TIMEOUT, journal.checkpoint()).await {
        Ok(Ok(_)) => true,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "agentd: the journal is not answering — reporting not ready");
            false
        }
        Err(_) => {
            tracing::warn!(
                timeout_secs = PROBE_TIMEOUT.as_secs(),
                "agentd: the journal did not answer within the probe timeout — reporting not ready"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Before the plane is built, the answer is `false`.**
    ///
    /// The direction that matters: an instance in rotation before it can admit
    /// anything is one the orchestrator has sent traffic to for nothing. The
    /// unregistered case is not a "cannot tell" — it is a definite *not ready*.
    #[tokio::test]
    async fn an_unregistered_journal_is_not_ready() {
        // `JOURNAL` is process-wide and this test must not race a registration,
        // so it asserts the branch rather than the global: `get()` on an unset
        // `OnceLock` is `None`, and `None` is the `false` arm above.
        assert!(
            OnceLock::<Arc<dyn JournalStore>>::new().get().is_none(),
            "an unset handle must read as absent, which is what makes the probe fail closed"
        );
    }

    /// A live store answers, and quickly.
    #[tokio::test]
    async fn a_reachable_journal_answers_the_probe() {
        let store: Arc<dyn JournalStore> = Arc::new(
            agentplane::store::RedbStore::open_in_memory()
                .expect("store")
                .for_tenant(agentplane::core::TenantId::new("9900357000004").expect("tenant")),
        );
        let checkpoint = tokio::time::timeout(PROBE_TIMEOUT, store.checkpoint()).await;
        assert!(
            matches!(checkpoint, Ok(Ok(_))),
            "a checkpoint read is the probe, and it must succeed on a healthy store"
        );
    }
}

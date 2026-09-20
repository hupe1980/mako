//! Inbox deduplication for inbound messages.
//!
//! In AS4-based market communication, messages may be retransmitted by the
//! sender (AS4 retry logic). Without deduplication, a retry would create a
//! duplicate process or trigger a duplicate state transition.
//!
//! The inbox assigns a stable **idempotency key** to each inbound message
//! (typically the UNH message reference + sender GLN) and refuses to process
//! the same key twice.
//!
//! # Usage
//!
//! ```rust,ignore
//! use mako_engine::inbox::{InboxStore, InMemoryInboxStore, inbox_key};
//!
//! let inbox = InMemoryInboxStore::new();
//! let key = inbox_key(&sender_party_id, &message_ref)?;;
//! if !inbox.accept(&key).await? {
//!     return Ok(()); // duplicate — drop silently
//! }
//! // process the message ...
//! ```

#[cfg(any(test, feature = "testing"))]
use std::sync::Arc;
#[cfg(any(test, feature = "testing"))]
use tokio::sync::RwLock;

use crate::error::EngineError;

/// Maximum byte length of the **caller-supplied** inbox deduplication key.
///
/// AS4 `MessageId` values are bounded to 255 bytes by the AS4 specification.
/// Adding the sender GLN (13 digits) and a separator gives ≤ 270 bytes in
/// practice. 509 bytes provides a generous margin while ensuring the stored
/// SlateDB key (`ib/{key}` — 3 additional bytes for the `ib/` prefix) never
/// exceeds 512 bytes.
pub const MAX_INBOX_KEY_LEN: usize = 509;

// ── InboxStore trait ──────────────────────────────────────────────────────────

/// Async idempotency store for inbound messages.
///
/// Three phases, not two. A two-state store — seen or not — has to record the
/// key *before* the caller knows whether the message was handled, so a message
/// that fails after the record is written can never be retried: its
/// retransmission carries the same key and is answered "duplicate". The
/// message is lost while the sender holds an acknowledgement saying it arrived.
///
/// So a key is **claimed** first, and settled afterwards:
///
/// - [`claim`](Self::claim) takes the key, atomically, and says which of three
///   things happened.
/// - [`accept`](Self::accept) settles it once the message is *durably* handled
///   — dispatched, persisted, or dead-lettered. From then on every
///   retransmission is a duplicate.
/// - [`abandon`](Self::abandon) releases it when nothing durable recorded the
///   message, so the next delivery is treated as first-seen. This is what makes
///   a retransmission a recovery path instead of a silent drop.
///
/// A claim that is never settled — the process died mid-message — must not
/// block the key forever, so an unsettled claim carries a lease
/// ([`CLAIM_LEASE`]) after which another delivery may take it over.
#[allow(async_fn_in_trait)]
pub trait InboxStore: Send + Sync {
    /// Phase 1: atomically claim `key`.
    ///
    /// The whole decision — inspect, expire, write — must be **one atomic
    /// step**. A read followed by a separate write lets two concurrent
    /// deliveries of the same message both see "absent" and both claim it.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] on storage failure. Callers fail closed:
    /// a storage error is not "first seen".
    async fn claim(&self, key: &str) -> Result<InboxClaim, EngineError>;

    /// Phase 2a: settle a claim as handled.
    ///
    /// Idempotent — settling a key that is not currently claimed is a no-op,
    /// because settlement itself can be retried.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] on storage failure.
    async fn accept(&self, key: &str) -> Result<(), EngineError>;

    /// Phase 2b: settle a claim as *not* handled, releasing the key.
    ///
    /// Never releases a key that was already accepted: an accepted key is a
    /// statement that the message was handled, and a later abandon would
    /// re-open it for reprocessing.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] on storage failure.
    async fn abandon(&self, key: &str) -> Result<(), EngineError>;
}

/// How long an unsettled claim holds a key before another delivery may take it.
///
/// Long enough that a slow dispatch is not overtaken by a retransmission, short
/// enough that a process that died mid-message does not strand the key until
/// the retention purge. The BDEW `ReceptionAwareness` retry interval is minutes,
/// so five of them is the wrong side of one retry rather than of ten.
pub const CLAIM_LEASE: std::time::Duration = std::time::Duration::from_secs(300);

/// What [`InboxStore::claim`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxClaim {
    /// First occurrence, or a lease that had expired. The caller now owns an
    /// unsettled claim and **must** settle it.
    Claimed,
    /// Another delivery of the same message holds an unsettled claim.
    ///
    /// Not a duplicate: nothing has been handled yet, so acknowledging it would
    /// tell the sender a message was processed that may still fail. Nor is it
    /// first-seen. Without this third answer a concurrent delivery has to be
    /// miscalled one of the two.
    InFlight,
    /// The message was already accepted; this is a replay.
    Duplicate,
}

// ── InMemoryInboxStore ────────────────────────────────────────────────────────

/// An in-memory [`InboxStore`] for tests and development.
///
/// Backed by a `HashSet` protected by a `RwLock`. Cloning shares the underlying
/// data via `Arc` — all clones see the same deduplication state.
///
/// For production use, replace with a persistent backend (e.g. a PostgreSQL
/// table or a redb database) to survive process restarts.
///
/// Only available in `#[cfg(test)]` or with the `testing` feature enabled.
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Default, Clone)]
pub struct InMemoryInboxStore {
    seen: Arc<RwLock<std::collections::HashMap<String, InboxState>>>,
}

/// What the in-memory store holds for one key.
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Clone, Copy)]
enum InboxState {
    /// Claimed but not settled; the lease runs out at `until`.
    Claimed { until: std::time::Instant },
    /// Settled as handled.
    Accepted,
}

#[cfg(any(test, feature = "testing"))]
impl InMemoryInboxStore {
    /// Create an empty inbox store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the number of registered keys.
    pub async fn len(&self) -> usize {
        self.seen.read().await.len()
    }

    /// Return `true` when no keys have been registered yet.
    pub async fn is_empty(&self) -> bool {
        self.seen.read().await.is_empty()
    }
}

#[cfg(any(test, feature = "testing"))]
impl InboxStore for InMemoryInboxStore {
    async fn claim(&self, key: &str) -> Result<InboxClaim, EngineError> {
        check_key_len(key)?;
        let mut seen = self.seen.write().await;
        match seen.get(key) {
            Some(InboxState::Accepted) => Ok(InboxClaim::Duplicate),
            Some(InboxState::Claimed { until }) if *until > std::time::Instant::now() => {
                Ok(InboxClaim::InFlight)
            }
            // Absent, or a lease that has run out and may be taken over.
            _ => {
                seen.insert(
                    key.to_owned(),
                    InboxState::Claimed {
                        until: std::time::Instant::now() + CLAIM_LEASE,
                    },
                );
                Ok(InboxClaim::Claimed)
            }
        }
    }

    async fn accept(&self, key: &str) -> Result<(), EngineError> {
        check_key_len(key)?;
        self.seen
            .write()
            .await
            .insert(key.to_owned(), InboxState::Accepted);
        Ok(())
    }

    async fn abandon(&self, key: &str) -> Result<(), EngineError> {
        check_key_len(key)?;
        let mut seen = self.seen.write().await;
        // Only drop our own unsettled claim — never an accepted key.
        if matches!(seen.get(key), Some(InboxState::Claimed { .. })) {
            seen.remove(key);
        }
        Ok(())
    }
}

/// Refuse a key too long to store, before it reaches a backend.
///
/// Gated with its only caller: the durable backend in `store_slatedb` carries
/// its own copy, because the two crates' limits have to be checkable
/// independently of which feature set a build selected.
#[cfg(any(test, feature = "testing"))]
fn check_key_len(key: &str) -> Result<(), EngineError> {
    if key.len() > MAX_INBOX_KEY_LEN {
        return Err(EngineError::inbox(format!(
            "inbox key is {} bytes, exceeds maximum of {MAX_INBOX_KEY_LEN}",
            key.len()
        )));
    }
    Ok(())
}

// ── InboxKey helpers ──────────────────────────────────────────────────────────

/// Build a canonical inbox key from an EDIFACT message reference and the
/// sender GLN, returning an error if either component is empty.
///
/// The combination `<sender>:<message_ref>` is unique per market participant
/// per message. Using only the message reference is insufficient because
/// different senders may use the same reference numbering.
///
/// # Errors
///
/// Returns an error string when either `sender_party_id` or `message_ref` is empty.
/// BDEW codes and GLNs are always 13 digits; EIC codes are 16 chars;
/// UNH message references are always non-empty. An empty component indicates
/// a parsing error upstream and must not silently pass deduplication checks.
pub fn inbox_key(sender_party_id: &str, message_ref: &str) -> Result<String, &'static str> {
    if sender_party_id.is_empty() {
        return Err("inbox_key: sender_party_id must not be empty");
    }
    if message_ref.is_empty() {
        return Err("inbox_key: message_ref must not be empty");
    }
    Ok(format!("{sender_party_id}:{message_ref}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_new_message_is_claimed() {
        let inbox = InMemoryInboxStore::new();
        assert_eq!(
            inbox.claim("sender:ref-001").await.unwrap(),
            InboxClaim::Claimed
        );
    }

    /// A claim that is not settled blocks a concurrent delivery — and says so,
    /// rather than calling it a duplicate.
    ///
    /// Reporting `Duplicate` here would acknowledge a message nothing has
    /// handled yet; reporting `Claimed` would let two deliveries process it.
    #[tokio::test]
    async fn an_unsettled_claim_is_in_flight_not_a_duplicate() {
        let inbox = InMemoryInboxStore::new();
        assert_eq!(
            inbox.claim("sender:ref-001").await.unwrap(),
            InboxClaim::Claimed
        );
        assert_eq!(
            inbox.claim("sender:ref-001").await.unwrap(),
            InboxClaim::InFlight
        );
    }

    /// Only an accepted key is a duplicate.
    #[tokio::test]
    async fn an_accepted_message_is_a_duplicate() {
        let inbox = InMemoryInboxStore::new();
        inbox.claim("sender:ref-001").await.unwrap();
        inbox.accept("sender:ref-001").await.unwrap();
        assert_eq!(
            inbox.claim("sender:ref-001").await.unwrap(),
            InboxClaim::Duplicate
        );
    }

    /// Abandoning releases the key, which is what makes a retransmission a
    /// recovery path instead of a silent drop.
    ///
    /// This is the whole reason the store has three phases. With a two-state
    /// store the key is recorded before the message is handled, so a message
    /// that fails afterwards can never be retried: its retransmission carries
    /// the same key and is answered "duplicate" while nothing was done.
    #[tokio::test]
    async fn abandoning_lets_the_retransmission_through() {
        let inbox = InMemoryInboxStore::new();
        inbox.claim("sender:ref-001").await.unwrap();
        inbox.abandon("sender:ref-001").await.unwrap();
        assert_eq!(
            inbox.claim("sender:ref-001").await.unwrap(),
            InboxClaim::Claimed,
            "a released key must be claimable again"
        );
    }

    /// Abandon never re-opens an accepted key.
    ///
    /// An accepted key states that the message was handled. Releasing it would
    /// invite the same message to be processed twice — a second Lieferantenwechsel
    /// from one Anmeldung.
    #[tokio::test]
    async fn abandoning_an_accepted_key_does_nothing() {
        let inbox = InMemoryInboxStore::new();
        inbox.claim("sender:ref-001").await.unwrap();
        inbox.accept("sender:ref-001").await.unwrap();
        inbox.abandon("sender:ref-001").await.unwrap();
        assert_eq!(
            inbox.claim("sender:ref-001").await.unwrap(),
            InboxClaim::Duplicate
        );
    }

    /// Settling is idempotent: the recovery path may retry it.
    #[tokio::test]
    async fn accepting_twice_is_not_an_error() {
        let inbox = InMemoryInboxStore::new();
        inbox.claim("sender:ref-001").await.unwrap();
        inbox.accept("sender:ref-001").await.unwrap();
        inbox.accept("sender:ref-001").await.unwrap();
        assert_eq!(
            inbox.claim("sender:ref-001").await.unwrap(),
            InboxClaim::Duplicate
        );
    }

    /// Accepting a key nobody claimed is a no-op, not an error.
    #[tokio::test]
    async fn accepting_an_unclaimed_key_is_a_no_op() {
        let inbox = InMemoryInboxStore::new();
        inbox.accept("sender:never-claimed").await.unwrap();
    }

    #[tokio::test]
    async fn different_senders_same_ref_are_independent() {
        let inbox = InMemoryInboxStore::new();
        assert_eq!(
            inbox
                .claim(&inbox_key("sender-A", "ref-001").unwrap())
                .await
                .unwrap(),
            InboxClaim::Claimed
        );
        assert_eq!(
            inbox
                .claim(&inbox_key("sender-B", "ref-001").unwrap())
                .await
                .unwrap(),
            InboxClaim::Claimed,
            "the sender is part of the key"
        );
    }

    #[test]
    fn inbox_key_rejects_empty_party_id() {
        assert!(inbox_key("", "ref-001").is_err());
    }

    #[test]
    fn inbox_key_rejects_empty_ref() {
        assert!(inbox_key("4012345000023", "").is_err());
    }

    #[test]
    fn inbox_key_formats_correctly() {
        assert_eq!(
            inbox_key("4012345000023", "MSG-001").unwrap(),
            "4012345000023:MSG-001",
        );
    }
}

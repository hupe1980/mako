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
use std::{collections::HashSet, sync::Arc};
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
/// Implement this trait to plug in persistent deduplication storage (e.g.
/// PostgreSQL, redb). Use [`InMemoryInboxStore`] for tests and development.
#[allow(async_fn_in_trait)]
pub trait InboxStore: Send + Sync {
    /// Check whether `key` has been seen before and, if not, register it.
    ///
    /// Returns `Ok(true)` when `key` is **new** (the message should be
    /// processed). Returns `Ok(false)` when `key` was already seen (duplicate
    /// — the caller should drop the message).
    ///
    /// Implementations must guarantee atomic check-and-set semantics under
    /// concurrent callers.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Store`] on storage failure.
    async fn accept(&self, key: &str) -> Result<bool, EngineError>;
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
    seen: Arc<RwLock<HashSet<String>>>,
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
    async fn accept(&self, key: &str) -> Result<bool, EngineError> {
        if key.len() > MAX_INBOX_KEY_LEN {
            return Err(EngineError::inbox(format!(
                "inbox key is {} bytes, exceeds maximum of {MAX_INBOX_KEY_LEN}",
                key.len()
            )));
        }
        Ok(self.seen.write().await.insert(key.to_owned()))
    }
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
    async fn new_message_is_accepted() {
        let inbox = InMemoryInboxStore::new();
        assert!(inbox.accept("sender:ref-001").await.unwrap());
    }

    #[tokio::test]
    async fn duplicate_message_is_rejected() {
        let inbox = InMemoryInboxStore::new();
        assert!(inbox.accept("sender:ref-001").await.unwrap());
        assert!(
            !inbox.accept("sender:ref-001").await.unwrap(),
            "second accept should return false"
        );
    }

    #[tokio::test]
    async fn different_senders_same_ref_are_independent() {
        let inbox = InMemoryInboxStore::new();
        assert!(
            inbox
                .accept(&inbox_key("sender-A", "ref-001").unwrap())
                .await
                .unwrap()
        );
        assert!(
            inbox
                .accept(&inbox_key("sender-B", "ref-001").unwrap())
                .await
                .unwrap()
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

//! Durable `Idempotency-Key` records for `POST /api/v1/commands`.
//!
//! The accepted response is stored under the key and replayed verbatim for 24
//! hours: callers get the same `202` and the same `process_id` however many
//! times they retry. A key reused for a *different* command or payload is
//! refused rather than answered with the first one's result — which is what
//! catches an ERP generating one key per session instead of one per order.
//!
//! The per-family business guard is not a substitute. It answers a retry with
//! `409 duplicate_process` rather than the original `202`, and only while the
//! process is still active; after a terminal state the same retry would start a
//! second process.
//!
//! # Why the key is hashed
//!
//! The value is caller-supplied text and becomes part of a storage key. A key
//! containing `/`, a NUL byte or a few kilobytes of padding would otherwise
//! reach the prefix scans the engine keys on. The stored suffix is a SHA-256
//! digest in hex, so it is fixed-length, prefix-free and contains nothing the
//! caller chose.
//!
//! # Retention
//!
//! Records are swept by the same daily worker that purges the AS4 inbox. A
//! record past the window is treated as absent on read as well, so a stalled
//! sweep degrades to no replay rather than to a stale one.

use mako_engine::error::EngineError;
use mako_engine::store_slatedb::{KvNamespace, SlateDbStore};
use sha2::{Digest as _, Sha256};
use time::OffsetDateTime;

/// Namespace holding one record per `(tenant, Idempotency-Key)`.
pub(crate) const IDEMPOTENCY: KvNamespace = KvNamespace::new("idem/");

/// How long an accepted response is replayable.
///
/// Twenty-four hours is the industry-standard window and comfortably outlives
/// any ERP retry schedule. It is deliberately shorter than the process
/// lifetimes involved: the record answers "did this exact request already
/// land", not "what is this process doing now", which `GET /api/v1/processes`
/// answers without a time bound.
pub(crate) const RETENTION: time::Duration = time::Duration::hours(24);

/// A stored response for one idempotency key.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Record {
    /// Unix seconds at which the response was recorded.
    pub recorded_at: i64,
    /// SHA-256 of the request the key was first used for; a later request with
    /// the same key and a different fingerprint is refused.
    pub fingerprint: String,
    /// The `202 Accepted` body, replayed verbatim.
    pub body: serde_json::Value,
}

/// Outcome of consulting the store for an incoming request.
pub(crate) enum Lookup {
    /// No live record — dispatch the command.
    Fresh,
    /// This exact request already succeeded; replay `body` as `202 Accepted`.
    Replay(serde_json::Value),
    /// The key was used for a different request. Refusing is the only safe
    /// answer: replaying the first response would report a `process_id` that
    /// belongs to another command, and dispatching would make the key
    /// meaningless.
    Conflict,
}

/// Storage suffix for `(tenant, key)`.
fn suffix(tenant: &str, key: &str) -> String {
    let mut h = Sha256::new();
    h.update(tenant.as_bytes());
    h.update([0u8]);
    h.update(key.as_bytes());
    format!("{:x}", h.finalize())
}

/// Fingerprint of the request a key is bound to.
#[must_use]
pub(crate) fn fingerprint(command: &str, payload: &serde_json::Value) -> String {
    let mut h = Sha256::new();
    h.update(command.as_bytes());
    h.update([0u8]);
    // `to_string` on a `serde_json::Value` orders object keys deterministically
    // (the default feature set keeps a `BTreeMap`), so a re-serialised
    // equivalent payload fingerprints identically.
    h.update(payload.to_string().as_bytes());
    format!("{:x}", h.finalize())
}

/// Consult the store for `key`.
///
/// # Errors
///
/// Returns [`EngineError`] on storage failure. A read failure is **not**
/// treated as `Fresh` by the caller: answering a retry by re-dispatching is the
/// behaviour this module exists to remove.
pub(crate) async fn lookup(
    store: &SlateDbStore,
    tenant: &str,
    key: &str,
    fingerprint: &str,
) -> Result<Lookup, EngineError> {
    let Some(raw) = store.kv_get(IDEMPOTENCY, &suffix(tenant, key)).await? else {
        return Ok(Lookup::Fresh);
    };
    let Ok(record) = serde_json::from_slice::<Record>(&raw) else {
        // An unreadable record is as good as none: the schema changed or the
        // blob is corrupt, and refusing the request would strand the caller.
        tracing::warn!("idempotency record could not be decoded — treating the key as fresh");
        return Ok(Lookup::Fresh);
    };
    let age = OffsetDateTime::now_utc().unix_timestamp() - record.recorded_at;
    if age > RETENTION.whole_seconds() {
        return Ok(Lookup::Fresh);
    }
    if record.fingerprint != fingerprint {
        return Ok(Lookup::Conflict);
    }
    Ok(Lookup::Replay(record.body))
}

/// Record the accepted response for `key`.
///
/// # Errors
///
/// Returns [`EngineError`] on storage failure.
pub(crate) async fn record(
    store: &SlateDbStore,
    tenant: &str,
    key: &str,
    fingerprint: &str,
    body: &serde_json::Value,
) -> Result<(), EngineError> {
    let record = Record {
        recorded_at: OffsetDateTime::now_utc().unix_timestamp(),
        fingerprint: fingerprint.to_owned(),
        body: body.clone(),
    };
    let bytes = serde_json::to_vec(&record).map_err(|e| EngineError::store(e.to_string()))?;
    store
        .kv_put(IDEMPOTENCY, &suffix(tenant, key), &bytes)
        .await
}

/// Delete every record older than the 24-hour retention window; returns how
/// many were removed.
///
/// Run by the daily inbox-purge worker. The namespace is keyed by digest rather
/// than by time, so this is a full scan — acceptable at one pass a day over a
/// namespace bounded by a day's command volume.
///
/// # Errors
///
/// Returns [`EngineError`] on storage failure.
pub async fn purge_expired(
    store: &SlateDbStore,
    now: OffsetDateTime,
) -> Result<usize, EngineError> {
    let cutoff = now.unix_timestamp() - RETENTION.whole_seconds();
    let mut removed = 0usize;
    for (suffix, raw) in store.kv_scan_prefix(IDEMPOTENCY).await? {
        let expired =
            serde_json::from_slice::<Record>(&raw).map_or(true, |r| r.recorded_at < cutoff);
        if expired {
            store.kv_delete(IDEMPOTENCY, &suffix).await?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::{Lookup, fingerprint, lookup, purge_expired, record};
    use serde_json::json;

    async fn store() -> mako_engine::store_slatedb::SlateDbStore {
        mako_engine::store_slatedb::SlateDbStore::open_in_memory()
            .await
            .expect("open in-memory SlateDB")
    }

    #[tokio::test]
    async fn an_unused_key_is_fresh() {
        let s = store().await;
        let fp = fingerprint("gpke.lieferbeginn.anmelden", &json!({"malo_id": "1"}));
        assert!(matches!(
            lookup(&s, "9900357000004", "k1", &fp).await.unwrap(),
            Lookup::Fresh
        ));
    }

    #[tokio::test]
    async fn the_same_request_replays_the_recorded_response() {
        let s = store().await;
        let fp = fingerprint("gpke.lieferbeginn.anmelden", &json!({"malo_id": "1"}));
        let body = json!({"status": "accepted", "process_id": "p-1"});
        record(&s, "9900357000004", "k1", &fp, &body).await.unwrap();
        match lookup(&s, "9900357000004", "k1", &fp).await.unwrap() {
            Lookup::Replay(v) => assert_eq!(v, body),
            _ => panic!("a recorded key must replay"),
        }
    }

    /// A key reused for a different command must not answer with the first
    /// command's `process_id`.
    #[tokio::test]
    async fn reusing_a_key_for_a_different_request_conflicts() {
        let s = store().await;
        let first = fingerprint("gpke.lieferbeginn.anmelden", &json!({"malo_id": "1"}));
        let second = fingerprint("gpke.lieferende.abmelden", &json!({"malo_id": "1"}));
        record(
            &s,
            "9900357000004",
            "k1",
            &first,
            &json!({"process_id": "p-1"}),
        )
        .await
        .unwrap();
        assert!(matches!(
            lookup(&s, "9900357000004", "k1", &second).await.unwrap(),
            Lookup::Conflict
        ));
    }

    /// Two operators' keys never collide, and one cannot read the other's
    /// recorded response.
    #[tokio::test]
    async fn records_are_scoped_to_the_tenant() {
        let s = store().await;
        let fp = fingerprint("gpke.lieferbeginn.anmelden", &json!({"malo_id": "1"}));
        record(
            &s,
            "9900357000004",
            "k1",
            &fp,
            &json!({"process_id": "p-1"}),
        )
        .await
        .unwrap();
        assert!(matches!(
            lookup(&s, "9900111000001", "k1", &fp).await.unwrap(),
            Lookup::Fresh
        ));
    }

    /// A caller-supplied key containing path separators and control bytes must
    /// not reach the storage key.
    #[tokio::test]
    async fn a_hostile_key_is_stored_under_a_digest() {
        let s = store().await;
        let fp = fingerprint("gpke.lieferbeginn.anmelden", &json!({}));
        let hostile = "../../e/tenant/stream\u{0}\u{1}";
        record(&s, "9900357000004", hostile, &fp, &json!({"ok": true}))
            .await
            .unwrap();
        let keys = s
            .kv_scan_prefix(super::IDEMPOTENCY)
            .await
            .expect("scan")
            .into_iter()
            .map(|(k, _)| k)
            .collect::<Vec<_>>();
        assert_eq!(keys.len(), 1);
        assert!(
            keys[0].len() == 64 && keys[0].chars().all(|c| c.is_ascii_hexdigit()),
            "the storage suffix must be a hex digest, got {:?}",
            keys[0]
        );
    }

    #[tokio::test]
    async fn expired_records_are_swept() {
        let s = store().await;
        let fp = fingerprint("gpke.lieferbeginn.anmelden", &json!({}));
        record(&s, "9900357000004", "k1", &fp, &json!({"ok": true}))
            .await
            .unwrap();
        // A sweep 25 hours from now sees the record as expired.
        let later = time::OffsetDateTime::now_utc() + time::Duration::hours(25);
        assert_eq!(purge_expired(&s, later).await.unwrap(), 1);
        assert!(matches!(
            lookup(&s, "9900357000004", "k1", &fp).await.unwrap(),
            Lookup::Fresh
        ));
    }
}

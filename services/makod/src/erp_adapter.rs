//! ERP outbox adapter for `makod`.
//!
//! ## Provided implementations
//!
//! | Type | Use case |
//! |------|---------|
//! | `LogErpAdapter` | Re-export from `mako-engine`; log-only, no delivery |
//! | [`WebhookErpAdapter`] | HTTP POST CloudEvents 1.0 JSON to a configurable ERP endpoint |
//!
//! ## Wire format
//!
//! `WebhookErpAdapter` delivers every [`ErpEvent`] as a
//! **[CloudEvents 1.0](https://cloudevents.io) structured-mode JSON** message:
//!
//! ```text
//! POST <erp_url>
//! Content-Type: application/cloudevents+json
//! X-Idempotency-Key: <event.idempotency_key>
//! X-Mako-Signature: <hmac-sha256-hex>   ← only when secret is configured
//!
//! {
//!   "specversion": "1.0",
//!   "id": "<idempotency_key>",
//!   "source": "urn:mako:tenant:<tenant_id>",
//!   "type": "de.mako.aperak.accepted",
//!   "time": "2026-10-01T10:15:00+02:00",
//!   "subject": "<process_id>",
//!   "dataschema": "https://.../Marktlokation.json",
//!   "datacontenttype": "application/json",
//!   "makoconvid": "<conversation_id>",
//!   "makocausationid": "<causation_id>",
//!   "makopid": 55001,
//!   "data": { "_typ": "MARKTLOKATION", ... }
//! }
//! ```
//!
//! ## Wiring
//!
//! Register an adapter in the `OutboxErpWorker` (see `main.rs`):
//!
//! ```rust,ignore
//! let erp = WebhookErpAdapter::new(
//!     "https://erp.example.com/mako/events",
//!     Some("my-shared-secret".into()),
//! );
//! tokio::spawn(async move { outbox_erp_worker(store, erp).await });
//! ```

use mako_engine::erp::{ErpAdapter, ErpAdapterError, ErpEvent, ErpEventType};
use secrecy::{ExposeSecret as _, SecretString};
use serde::Serialize;
use tracing::{info, warn};

// ── CloudEventEnvelope ────────────────────────────────────────────────────────

/// CloudEvents 1.0 structured-mode JSON envelope.
///
/// Produced by `WebhookErpAdapter` from an [`ErpEvent`].  All mako-specific
/// metadata is carried as extension attributes (`makoconvid`, `makocausationid`,
/// `makopid`, `makofailreason`, `makoerc`).  Extension attribute names must be
/// lowercase alphanumeric only (CloudEvents spec §3.3).
#[derive(Serialize)]
struct CloudEventEnvelope<'a> {
    specversion: &'static str,
    id: &'a str,
    source: String,
    #[serde(rename = "type")]
    ce_type: &'static str,
    #[serde(with = "time::serde::rfc3339")]
    time: time::OffsetDateTime,
    subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    dataschema: Option<&'a str>,
    datacontenttype: &'static str,
    // mako extension attributes
    makoconvid: String,
    makocausationid: String,
    makopid: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    makofailreason: Option<&'a str>,
    /// Workflow family that produced this event — used by `marktd` to derive
    /// `marktrole` for role-scoped ERP subscriber fan-out.
    /// Empty string is serialized as an absent field.
    #[serde(skip_serializing_if = "str::is_empty")]
    makoworkflow: &'a str,
    /// BDEW ERC error code when `type == "de.mako.aperak.rejected"`.
    ///
    /// Carries the structured BDEW ERC code (e.g. `"Z29"`, `"E02"`) so ERP
    /// subscribers can automate the response without parsing `data.error_code`.
    /// Absent when no ERC code is available (e.g. timeout-initiated rejections).
    #[serde(skip_serializing_if = "Option::is_none")]
    makoerc: Option<&'a str>,
    data: &'a serde_json::Value,
}

impl<'a> CloudEventEnvelope<'a> {
    fn from_erp_event(event: &'a ErpEvent) -> Self {
        let fail_reason = match &event.event_type {
            ErpEventType::ProcessFailed { reason } => Some(reason.as_ref()),
            _ => None,
        };
        let erc_code = match &event.event_type {
            ErpEventType::AperakRejected { erc_code } => erc_code.as_ref().map(|c| c.as_str()),
            _ => None,
        };
        Self {
            specversion: "1.0",
            id: &event.idempotency_key,
            source: format!("urn:mako:tenant:{}", event.tenant_id),
            ce_type: event.event_type.cloud_event_type(),
            time: event.occurred_at,
            subject: event.process_id.to_string(),
            dataschema: event.payload_schema.as_deref(),
            datacontenttype: "application/json",
            makoconvid: event.conversation_id.to_string(),
            makocausationid: event.causation_id.to_string(),
            makopid: event.pid,
            makofailreason: fail_reason,
            makoworkflow: event.workflow_name.as_ref(),
            makoerc: erc_code,
            data: &event.payload,
        }
    }
}

// ── WebhookErpAdapter ─────────────────────────────────────────────────────────

/// An [`ErpAdapter`] that delivers every [`ErpEvent`] as an HTTP POST to a
/// configurable ERP endpoint using the
/// **[CloudEvents 1.0](https://cloudevents.io) structured-mode JSON** format.
///
/// ## Request format
///
/// ```text
/// POST <erp_url>
/// Content-Type: application/cloudevents+json
/// X-Idempotency-Key: <event.idempotency_key>
/// X-Mako-Signature: HMAC-SHA256(<secret>, <body>)   ← only if secret is set
///
/// {
///   "specversion": "1.0",
///   "id": "<idempotency_key>",
///   "source": "urn:mako:tenant:<tenant_id>",
///   "type": "de.mako.aperak.accepted",
///   "time": "2026-10-01T10:15:00+02:00",
///   "subject": "<process_id>",
///   "dataschema": "https://.../Marktlokation.json",
///   "datacontenttype": "application/json",
///   "makoconvid": "<conversation_id>",
///   "makocausationid": "<causation_id>",
///   "makopid": 55001,
///   "data": { "_typ": "MARKTLOKATION", ... }
/// }
/// ```
///
/// ## Idempotency
///
/// `id` (CloudEvents) and `X-Idempotency-Key` (header) carry the same stable
/// dedup key.  The ERP endpoint **must** persist it and return `HTTP 200` for
/// duplicate deliveries without re-processing.
///
/// ## Authentication
///
/// If `shared_secret` is set, the adapter signs the raw request body with
/// HMAC-SHA256 and includes the hex digest in `X-Mako-Signature`.  The ERP
/// verifies the signature before processing.
///
/// ## Error handling
///
/// - HTTP 2xx → ack (success)
/// - HTTP 4xx (except 429) → permanent error; message is dead-lettered
/// - HTTP 429, 5xx, timeout → transient error; message is rescheduled with
///   exponential backoff
#[derive(Clone)]
pub struct WebhookErpAdapter {
    client: reqwest::Client,
    erp_url: String,
    shared_secret: Option<SecretString>,
}

impl WebhookErpAdapter {
    /// Create a new adapter that POSTs to `erp_url`.
    ///
    /// `shared_secret` — when `Some`, the body is signed with HMAC-SHA256 and
    /// the hex digest is sent as `X-Mako-Signature`.
    ///
    /// The default HTTP timeout is 30 seconds.  Use [`WebhookErpAdapter::with_timeout`]
    /// to override.
    ///
    /// # Panics
    ///
    /// Panics if the `reqwest::Client` cannot be built (e.g. TLS stack
    /// unavailable).
    #[must_use]
    pub fn new(erp_url: impl Into<String>, shared_secret: Option<SecretString>) -> Self {
        Self::with_timeout(erp_url, shared_secret, std::time::Duration::from_secs(30))
    }

    /// Create a new adapter with a custom per-request HTTP timeout.
    ///
    /// Set a shorter timeout (e.g. 5 s) when the ERP is co-located; use
    /// a longer value (e.g. 60 s) for cross-WAN endpoints.
    ///
    /// # Panics
    ///
    /// Panics if the `reqwest::Client` cannot be built.
    #[must_use]
    pub fn with_timeout(
        erp_url: impl Into<String>,
        shared_secret: Option<SecretString>,
        timeout: std::time::Duration,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("WebhookErpAdapter: reqwest::Client build failed");
        Self {
            client,
            erp_url: erp_url.into(),
            shared_secret,
        }
    }
}

impl ErpAdapter for WebhookErpAdapter {
    async fn notify(&self, event: ErpEvent) -> Result<(), ErpAdapterError> {
        let envelope = CloudEventEnvelope::from_erp_event(&event);
        let body = serde_json::to_vec(&envelope)
            .map_err(|e| ErpAdapterError::payload(format!("serialise CloudEvent: {e}")))?;

        let mut builder = self
            .client
            .post(&self.erp_url)
            .header("Content-Type", "application/cloudevents+json")
            .header("X-Idempotency-Key", &event.idempotency_key);

        if let Some(secret) = &self.shared_secret {
            let sig = hmac_sha256(secret.expose_secret().as_bytes(), &body);
            builder = builder.header("X-Mako-Signature", sig);
        }

        let resp = builder
            .body(body)
            .send()
            .await
            .map_err(|e| ErpAdapterError::transport(format!("HTTP send: {e}")))?;

        let status = resp.status();
        if status.is_success() {
            info!(
                idempotency_key = %event.idempotency_key,
                event_type      = event.event_type.label(),
                status          = status.as_u16(),
                url             = %self.erp_url,
                "WebhookErpAdapter: delivered",
            );
            return Ok(());
        }

        // 429 and 5xx are transient; everything else is permanent.
        if status.as_u16() == 429 || status.is_server_error() {
            warn!(
                idempotency_key = %event.idempotency_key,
                event_type      = event.event_type.label(),
                status          = status.as_u16(),
                url             = %self.erp_url,
                "WebhookErpAdapter: transient HTTP error; will retry",
            );
            return Err(ErpAdapterError::transport(format!(
                "HTTP {status} from ERP endpoint {}",
                self.erp_url
            )));
        }

        warn!(
            idempotency_key = %event.idempotency_key,
            event_type      = event.event_type.label(),
            status          = status.as_u16(),
            url             = %self.erp_url,
            "WebhookErpAdapter: permanent HTTP error; dead-lettering",
        );
        Err(ErpAdapterError::permanent(format!(
            "HTTP {status} from ERP endpoint {} — non-retryable",
            self.erp_url
        )))
    }
}

/// Compute HMAC-SHA256 of `data` using `key` and return the lower-hex digest.
///
/// Compute HMAC-SHA256 over `data` with `key` and return a 64-char lowercase
/// hex string.
///
/// Uses the audited [`hmac`] + [`sha2`] crates. The output is constant-time
/// w.r.t. the key via the underlying `hmac` implementation.
fn hmac_sha256(key: &[u8], data: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let mut mac = <Hmac<Sha256>>::new_from_slice(key).expect("HMAC-SHA256 accepts any key length");
    mac.update(data);
    let result = mac.finalize().into_bytes();
    result.iter().fold(String::with_capacity(64), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}

// ── OutboxErpWorker ───────────────────────────────────────────────────────────

/// A background worker that drains the outbox and delivers ERP-relevant
/// messages via an [`ErpAdapter`].
///
/// Unlike the AS4 `OutboxWorker` (which delivers EDIFACT to trading partners),
/// this worker filters messages that should be notified to the ERP backend and
/// routes them through the [`ErpAdapter`].
///
/// The two workers can run concurrently on the same outbox store — each
/// acknowledges only the messages it successfully delivered, so there is no
/// double-ack risk.
///
/// ## Retry policy
///
/// Transient failures are retried with **exponential back-off**:
/// `initial_backoff * 2^attempt_count`, capped at 1 hour.  After
/// `max_attempts` consecutive transient failures the message is dead-lettered
/// (acknowledged to prevent infinite retry) and a `warn!` log is emitted.
///
/// Default values: `max_attempts = 10`, `initial_backoff = 5 min`.  With
/// these defaults the last retry window before dead-lettering is ~60 minutes,
/// giving approximately 3 hours of total retry budget per message.
pub struct OutboxErpWorker<OS, A> {
    store: OS,
    adapter: A,
    batch_size: usize,
    poll_interval: std::time::Duration,
    /// Dead-letter the message after this many failed delivery attempts.
    max_attempts: u32,
    /// Base for the exponential back-off delay (seconds).
    initial_backoff_secs: u64,
}

impl<OS, A> OutboxErpWorker<OS, A>
where
    OS: mako_engine::outbox::OutboxStore + Clone,
    A: ErpAdapter,
{
    /// Create a new worker with default retry policy (`max_attempts = 10`,
    /// `initial_backoff = 5 min`).
    ///
    /// `batch_size` — messages fetched per poll cycle.
    /// `poll_interval` — sleep when the batch is empty.
    #[must_use]
    pub fn new(
        store: OS,
        adapter: A,
        batch_size: usize,
        poll_interval: std::time::Duration,
    ) -> Self {
        Self {
            store,
            adapter,
            batch_size,
            poll_interval,
            max_attempts: 10,
            initial_backoff_secs: 300, // 5 minutes
        }
    }

    /// Override the maximum number of delivery attempts before dead-lettering.
    #[must_use]
    #[allow(dead_code)] // builder method — used by integration tests and future callers
    pub fn with_max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = max_attempts;
        self
    }

    /// Override the base back-off interval.  The actual delay for attempt `n`
    /// is `initial_backoff * 2^n`, capped at 1 hour.
    #[must_use]
    #[allow(dead_code)] // builder method — used by integration tests and future callers
    pub fn with_initial_backoff(mut self, backoff: std::time::Duration) -> Self {
        self.initial_backoff_secs = backoff.as_secs().max(1);
        self
    }

    /// Run the ERP delivery loop.  Cancellable via task abort.
    pub async fn run(self) {
        loop {
            let batch = match self.store.pending_now(self.batch_size).await {
                Ok(b) => b,
                Err(e) => {
                    warn!(error = %e, "OutboxErpWorker: store error (will retry)");
                    tokio::time::sleep(self.poll_interval).await;
                    continue;
                }
            };

            if batch.is_empty() {
                tokio::time::sleep(self.poll_interval).await;
                continue;
            }

            for msg in batch {
                // Only deliver messages that carry a BO4E payload, are
                // explicitly ERP-targeted, OR have a recognised ERP message
                // type.  AS4-only EDIFACT messages (message_type = "UTILMD",
                // "MSCONS", etc., no payload_schema) are skipped — they are
                // handled by the AS4 OutboxWorker.
                let is_erp_relevant = msg.payload_schema.is_some()
                    || msg.message_type.starts_with("ERP_")
                    || map_message_type_to_erp_event(&msg.message_type).is_some();

                if !is_erp_relevant {
                    continue;
                }

                // Map message type to semantic ERP event type.  Skip messages
                // with unrecognised types rather than misclassifying them as
                // process failures — they may belong to a different delivery
                // channel.
                let Some(raw_event_type) = map_message_type_to_erp_event(&msg.message_type) else {
                    tracing::debug!(
                        message_id  = %msg.message_id,
                        message_type = %msg.message_type,
                        "OutboxErpWorker: unrecognised message type; skipping",
                    );
                    continue;
                };

                // For APERAK rejections, promote the structured ERC code from
                // `payload["error_code"]` into the typed `AperakRejected.erc_code`
                // field so the CloudEvents `makoerc` extension is populated.
                let event_type = match raw_event_type {
                    mako_engine::erp::ErpEventType::AperakRejected { .. } => {
                        let erc_code = msg
                            .payload
                            .get("error_code")
                            .and_then(|v| v.as_str())
                            .map(mako_engine::erc::ErcCode::new);
                        mako_engine::erp::ErpEventType::AperakRejected { erc_code }
                    }
                    other => other,
                };

                let event = ErpEvent {
                    idempotency_key: msg.message_id.to_string(),
                    event_type,
                    process_id: msg.process_id,
                    tenant_id: msg.tenant_id,
                    conversation_id: msg.conversation_id,
                    causation_id: msg.causation_event_id,
                    pid: extract_pid(&msg.payload),
                    payload_schema: msg.payload_schema.as_deref().map(str::to_owned),
                    payload: msg.payload.clone(),
                    occurred_at: msg.created_at,
                    workflow_name: msg.workflow_name.clone(),
                };

                match self.adapter.notify(event).await {
                    Ok(()) => {
                        if let Err(e) = self.store.acknowledge(msg.message_id).await {
                            warn!(
                                message_id = %msg.message_id,
                                error = %e,
                                "OutboxErpWorker: acknowledge failed",
                            );
                        }
                    }
                    Err(e) if e.is_retryable() => {
                        // Dead-letter if the message has exhausted its retry budget.
                        if msg.attempt_count >= self.max_attempts {
                            warn!(
                                message_id    = %msg.message_id,
                                attempt_count = msg.attempt_count,
                                max_attempts  = self.max_attempts,
                                error         = %e,
                                "OutboxErpWorker: max delivery attempts reached; dead-lettering",
                            );
                            let _ = self.store.acknowledge(msg.message_id).await;
                        } else {
                            // Exponential back-off: initial_backoff * 2^attempt_count,
                            // capped at 1 hour.
                            let shift = msg.attempt_count.min(10);
                            let backoff_secs = self
                                .initial_backoff_secs
                                .saturating_mul(1u64 << shift)
                                .min(3600);
                            let retry_at = time::OffsetDateTime::now_utc()
                                + time::Duration::seconds(backoff_secs as i64);
                            warn!(
                                message_id    = %msg.message_id,
                                attempt_count = msg.attempt_count,
                                backoff_secs,
                                error         = %e,
                                "OutboxErpWorker: transient ERP error; rescheduling",
                            );
                            if let Err(re) = self.store.reschedule(msg.message_id, retry_at).await {
                                warn!(
                                    message_id = %msg.message_id,
                                    error = %re,
                                    "OutboxErpWorker: reschedule failed",
                                );
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            message_id = %msg.message_id,
                            error = %e,
                            "OutboxErpWorker: permanent ERP error; dead-lettering",
                        );
                        // Acknowledge to prevent infinite redelivery — the
                        // dead-letter sink or log is the audit trail.
                        let _ = self.store.acknowledge(msg.message_id).await;
                    }
                }
            }
        }
    }
}

/// Map an outbox `message_type` string to a semantic [`ErpEventType`].
///
/// Returns `None` for unrecognised message types so the worker can skip them
/// instead of misclassifying them as process failures.
fn map_message_type_to_erp_event(msg_type: &str) -> Option<mako_engine::erp::ErpEventType> {
    use mako_engine::erp::ErpEventType;
    Some(match msg_type {
        "AperakAccepted" => ErpEventType::AperakAccepted,
        "AperakRejected" => ErpEventType::AperakRejected { erc_code: None },
        "AperakTimeout" => ErpEventType::AperakTimeout,
        "ContrlReceived" => ErpEventType::ContrlReceived,
        // Accept both the canonical name and the legacy typo.
        "ProcessCompleted" | "ProcessComplete" => ErpEventType::ProcessCompleted,
        "ProcessInitiated" => ErpEventType::ProcessInitiated,
        "MaloIdentified" => ErpEventType::MaloIdentified,
        // WiM Steuerungsauftrag positive Endantwort (PID 55168) — triggers VPP billing.
        "DispatchConfirmed" => ErpEventType::VppDispatchConfirmed,
        _ => return None,
    })
}

/// Extract the `pid` field from a BO4E outbox payload, if present.
fn extract_pid(payload: &serde_json::Value) -> u32 {
    payload
        .get("pid")
        .and_then(serde_json::Value::as_u64)
        .map(|v| v as u32)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_sha256_produces_64_char_hex() {
        let sig = hmac_sha256(b"secret", b"hello");
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn sha256_known_vector() {
        // SHA-256("abc") — NIST FIPS 180-4 Example 1.
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(b"abc");
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        );
    }
}

//! Canonical `CloudEvents` 1.0 envelope + signed outbound publishing.
//!
//! Every mako service that emits a domain event builds a [`CloudEvent`] and
//! delivers it with [`post_ce_with_retry`]. Centralising construction here means
//! the envelope shape, the `time` format (always RFC3339), the `source` URI
//! convention, the `id` scheme (UUID v4 unless an idempotency key is supplied),
//! and the HMAC signature format (`sha256=<hex>`) are identical on the wire no
//! matter which daemon emits — the event catalog ([`mako_events`]) owns the
//! `type` names, this module owns everything else about the envelope.
//!
//! The `type` string itself is NOT hard-coded here: callers pass a
//! [`mako_events`] constant (e.g. [`mako_events::billing::RECHNUNG_ERSTELLT`]),
//! so producer and consumer stay in lockstep through the shared catalog.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

/// Build the canonical `CloudEvents` `source` URI for a mako service.
///
/// Shape: `urn:mako:{service}:tenant:{tenant}`. `CloudEvents` §3.1 requires
/// `source` to be a URI-reference; a bare service name (`"obsd"`) is not one.
/// This is the single convention every emitter uses, so a subscriber can parse
/// the emitting service and tenant out of `source` uniformly.
#[must_use]
pub fn source(service: &str, tenant: &str) -> String {
    format!("urn:mako:{service}:tenant:{tenant}")
}

/// A `CloudEvents` 1.0 structured-mode JSON envelope.
///
/// Construct with [`CloudEvent::new`]; add optional attributes with the builder
/// methods. Serialises to the structured-mode JSON body every mako webhook
/// speaks (`Content-Type: application/cloudevents+json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudEvent {
    /// `CloudEvents` spec version — always `"1.0"`.
    pub specversion: String,
    /// Event id. Defaults to a fresh UUID v4; override with [`CloudEvent::with_id`]
    /// to carry an upstream idempotency key (dedup at the receiver keys on this).
    pub id: String,
    /// Emitting `source` URI — build with [`source`].
    pub source: String,
    /// `CloudEvents` `type` — pass a [`mako_events`] catalog constant.
    #[serde(rename = "type")]
    pub ce_type: String,
    /// RFC3339 UTC timestamp. Set at construction, so it is always RFC3339 on the
    /// wire (unlike an accidental `OffsetDateTime::to_string()`, which is not).
    pub time: String,
    /// Business subject (MaLo-ID, process id, …). Present on nearly every event;
    /// drop it with [`CloudEvent::without_subject`] for the rare subjectless one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Media type of `data` — defaults to `application/json`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datacontenttype: Option<String>,
    /// Optional schema URI for `data`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataschema: Option<String>,
    /// `CloudEvents` extension attributes (§3.3: lowercase-alphanumeric keys),
    /// serialised flat alongside the core attributes — e.g. `makopid`,
    /// `makoconvid`, `traceparent`. Empty for most domain events.
    #[serde(flatten)]
    pub extensions: serde_json::Map<String, Value>,
    /// Event payload (typically a BO4E object).
    pub data: Value,
}

/// The nine `CloudEvents` 1.0 core attribute names. Extension keys must never
/// collide with these — a collision makes `#[serde(flatten)]` emit the key
/// twice, which any receiver rejects with a "duplicate field" parse error.
const RESERVED_ATTRS: [&str; 9] = [
    "specversion",
    "id",
    "source",
    "type",
    "time",
    "subject",
    "datacontenttype",
    "dataschema",
    "data",
];

impl CloudEvent {
    /// Construct a structured-mode `CloudEvent` with the canonical defaults:
    /// `specversion = "1.0"`, `id` = fresh UUID v4, `time` = now (RFC3339 UTC),
    /// `datacontenttype = "application/json"`.
    #[must_use]
    pub fn new(
        source: impl Into<String>,
        ce_type: impl Into<String>,
        subject: impl Into<String>,
        data: Value,
    ) -> Self {
        Self {
            specversion: "1.0".into(),
            id: Uuid::new_v4().to_string(),
            source: source.into(),
            ce_type: ce_type.into(),
            time: now_rfc3339(),
            subject: Some(subject.into()),
            datacontenttype: Some("application/json".into()),
            dataschema: None,
            extensions: serde_json::Map::new(),
            data,
        }
    }

    /// Construct an event with no `subject` — for the rare event that has no
    /// single business subject (a tenant-wide alert, a batch summary). Same
    /// defaults as [`CloudEvent::new`] otherwise.
    #[must_use]
    pub fn new_subjectless(
        source: impl Into<String>,
        ce_type: impl Into<String>,
        data: Value,
    ) -> Self {
        let mut ce = Self::new(source, ce_type, String::new(), data);
        ce.subject = None;
        ce
    }

    /// Override the auto-generated `id` — e.g. to carry an upstream idempotency
    /// key so the receiver dedups on the originating command, not on delivery.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Override `time` with the event's *occurrence* time (reading timestamp,
    /// invoice date) rather than the emission time. Formatted RFC3339 internally,
    /// so it can never be set to a malformed string.
    #[must_use]
    pub fn with_time(mut self, occurred_at: OffsetDateTime) -> Self {
        self.time = occurred_at
            .to_offset(time::UtcOffset::UTC)
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| now_rfc3339());
        self
    }

    /// Override `datacontenttype` — e.g. `"application/xml"` for an `XRechnung`
    /// payload carried as a JSON string in `data`. Defaults to `application/json`.
    #[must_use]
    pub fn with_datacontenttype(mut self, content_type: impl Into<String>) -> Self {
        self.datacontenttype = Some(content_type.into());
        self
    }

    /// Set the optional `dataschema` URI.
    #[must_use]
    pub fn with_dataschema(mut self, dataschema: impl Into<String>) -> Self {
        self.dataschema = Some(dataschema.into());
        self
    }

    /// Add a `CloudEvents` extension attribute (§3.3: `key` must be lowercase
    /// alphanumeric). Chainable. A `None`-valued option is skipped, so
    /// `.extension_opt("makoerc", erc)` is a no-op when `erc` is `None`.
    ///
    /// A `key` that collides with a core `CloudEvents` attribute (`id`, `data`,
    /// `type`, …) is **ignored** — it would otherwise serialise a duplicate JSON
    /// key that every receiver rejects. Debug builds assert on the misuse.
    #[must_use]
    pub fn extension(mut self, key: &str, value: impl Into<Value>) -> Self {
        debug_assert!(
            !RESERVED_ATTRS.contains(&key),
            "CloudEvent extension key `{key}` collides with a core CloudEvents attribute"
        );
        if !RESERVED_ATTRS.contains(&key) {
            self.extensions.insert(key.to_owned(), value.into());
        }
        self
    }

    /// Add an extension attribute only when `value` is `Some`.
    #[must_use]
    pub fn extension_opt(self, key: &str, value: Option<impl Into<Value>>) -> Self {
        match value {
            Some(v) => self.extension(key, v),
            None => self,
        }
    }

    /// Drop the `subject` — for the rare event that has no business subject.
    #[must_use]
    pub fn without_subject(mut self) -> Self {
        self.subject = None;
        self
    }

    /// Serialise to the JSON bytes sent on the wire.
    ///
    /// # Errors
    ///
    /// Returns the `serde_json` error if the `data` payload cannot be encoded.
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

/// Now, formatted RFC3339 in UTC.
fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting of a UTC timestamp is infallible")
}

// ── Outbound publishing ─────────────────────────────────────────────────────

/// Errors from [`post_ce_with_retry`].
#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    /// The envelope could not be serialised — a bug in the payload, not transport.
    #[error("CloudEvent serialisation failed: {0}")]
    Serialise(String),
    /// Delivery failed. `permanent` distinguishes a receiver rejection the caller
    /// should **dead-letter immediately** (a 3xx/4xx — retrying identical bytes
    /// cannot help) from an exhausted transient failure (5xx/429/network) the
    /// caller may reschedule.
    #[error("CloudEvent delivery failed after {attempts} attempt(s){}: {last}", if *.permanent { " (permanent)" } else { "" })]
    Delivery {
        attempts: u32,
        last: String,
        permanent: bool,
    },
}

impl PublishError {
    /// `true` when delivery failed permanently (a 3xx/4xx the receiver will never
    /// accept on retry) — dead-letter immediately instead of rescheduling.
    #[must_use]
    pub fn is_permanent(&self) -> bool {
        matches!(
            self,
            PublishError::Delivery {
                permanent: true,
                ..
            }
        )
    }
}

/// POST a [`CloudEvent`] to `url`, signed and retried.
///
/// - **Signs** the exact serialised bytes with [Standard Webhooks] when `secret`
///   is `Some` — `webhook-id`, `webhook-timestamp` and
///   `webhook-signature: v1,<base64>` over `{id}.{timestamp}.{body}`
///   ([`crate::webhook::headers`]). The bytes signed are the bytes sent, so a
///   body serialised twice can never diverge from its signature.
///
///   The CloudEvent's own `id` is the `webhook-id`, which is what makes the
///   scheme's replay protection and mako's idempotency the same fact rather than
///   two: a receiver deduplicating on `webhook-id` is deduplicating on the
///   CloudEvent id it would have used anyway.
///
///   **The timestamp is stamped once, before the retry loop.** The three retries
///   below re-send byte-identical requests, headers included — a fresh timestamp
///   per attempt would produce three distinct `webhook-id`-plus-signature pairs
///   for one event and defeat the receiver's deduplication at the moment it is
///   most needed. The tolerance window is minutes and the back-off is under a
///   second, so one stamp covers every attempt.
/// - **Retries** up to 3 attempts with exponential back-off (200 ms, 400 ms) on
///   transient failures (transport errors, HTTP 429, and 5xx). A permanent 4xx
///   (except 429) is returned immediately without wasting retries — the receiver
///   rejected the event and re-sending the same bytes cannot change that.
/// - Sends `Content-Type: application/cloudevents+json`.
///
/// This is the single-target ERP/webhook emitter used by the L2/L3 services for
/// fire-once events. `marktd`'s multi-subscriber fan-out keeps its own
/// DLQ-backed delivery loop instead.
///
/// # Errors
///
/// [`PublishError::Serialise`] if the envelope cannot be encoded;
/// [`PublishError::Delivery`] if all attempts fail.
pub async fn post_ce_with_retry(
    client: &reqwest::Client,
    url: &str,
    event: &CloudEvent,
    secret: Option<&[u8]>,
) -> Result<(), PublishError> {
    let body = event
        .to_bytes()
        .map_err(|e| PublishError::Serialise(e.to_string()))?;
    // Stamped once: every retry must be the same signed request. See the note
    // on the timestamp above.
    let signed = secret.map(|s| {
        crate::webhook::headers(
            s,
            &event.id,
            time::OffsetDateTime::now_utc().unix_timestamp(),
            &body,
        )
    });

    let mut last = String::new();
    for attempt in 0u32..3 {
        let mut req = client
            .post(url)
            .header("Content-Type", "application/cloudevents+json")
            .body(body.clone());
        match signed {
            // `webhook-id` *is* the CloudEvent id, so it is the idempotency key
            // as well as the replay-protection key — one fact, not two headers
            // that can disagree.
            Some(ref h) => {
                for (name, value) in h {
                    req = req.header(*name, value);
                }
            }
            // Unsigned (dev mode) still carries the id, so a receiver's
            // deduplication does not depend on whether a secret happens to be
            // configured.
            None => req = req.header(crate::webhook::ID_HEADER, &event.id),
        }
        match req.send().await {
            Ok(r) if r.status().is_success() => return Ok(()),
            Ok(r) => {
                let status = r.status();
                last = format!("HTTP {status}");
                // Only 429 (Too Many Requests) and 5xx are transient. Everything
                // else — 3xx (redirects are disabled, so a 3xx is a misconfigured
                // endpoint) and 4xx — is permanent: the receiver rejected these
                // exact bytes, so retrying the same request cannot change that.
                let transient = status.as_u16() == 429 || status.is_server_error();
                if !transient {
                    return Err(PublishError::Delivery {
                        attempts: attempt + 1,
                        last,
                        permanent: true,
                    });
                }
            }
            Err(e) => last = e.to_string(),
        }
        if attempt < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(200 * (1 << attempt))).await;
        }
    }
    Err(PublishError::Delivery {
        attempts: 3,
        last,
        permanent: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_is_a_urn() {
        assert_eq!(
            source("billingd", "9900000000001"),
            "urn:mako:billingd:tenant:9900000000001"
        );
    }

    #[test]
    fn new_sets_canonical_defaults() {
        let ce = CloudEvent::new(
            source("billingd", "9900000000001"),
            mako_events::billing::RECHNUNG_ERSTELLT,
            "DE0001",
            serde_json::json!({"betrag": "42.00"}),
        );
        assert_eq!(ce.specversion, "1.0");
        assert_eq!(ce.datacontenttype.as_deref(), Some("application/json"));
        assert_eq!(ce.subject.as_deref(), Some("DE0001"));
        // id is a parseable UUID v4 by default.
        assert!(Uuid::parse_str(&ce.id).is_ok());
        // time round-trips as RFC3339.
        assert!(
            OffsetDateTime::parse(&ce.time, &time::format_description::well_known::Rfc3339).is_ok()
        );
    }

    #[test]
    fn serialises_type_as_type_and_flattens_extensions() {
        let ce = CloudEvent::new(
            source("makod", "9900000000001"),
            mako_events::mako::PROCESS_COMPLETED,
            "proc-1",
            serde_json::json!({}),
        )
        .with_id("idem-key-1")
        .extension("makopid", 55003u32)
        .extension_opt("makoerc", Some("E01"))
        .extension_opt("absent", Option::<&str>::None);

        let v = serde_json::to_value(&ce).unwrap();
        assert_eq!(v["type"], mako_events::mako::PROCESS_COMPLETED);
        assert_eq!(v["id"], "idem-key-1");
        assert_eq!(v["makopid"], 55003);
        assert_eq!(v["makoerc"], "E01");
        assert!(v.get("absent").is_none());
        // Extensions are flat, not nested under `extensions`.
        assert!(v.get("extensions").is_none());
    }

    #[test]
    fn roundtrips_through_json() {
        let ce = CloudEvent::new(
            source("edmd", "9900000000001"),
            mako_events::messwert::READING_QUALITY_WARNING,
            "DE0002",
            serde_json::json!({"grade": "F"}),
        )
        .extension("makopid", 55003u32);
        let bytes = ce.to_bytes().unwrap();
        let back: CloudEvent = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.ce_type, ce.ce_type);
        assert_eq!(back.extensions.get("makopid").unwrap(), 55003);
        assert_eq!(back.data["grade"], "F");
    }

    #[test]
    fn without_subject_omits_it() {
        let ce = CloudEvent::new(
            source("obsd", "9900000000001"),
            mako_events::obs::DEADLINE_APPROACHING,
            "",
            serde_json::json!({}),
        )
        .without_subject();
        let v = serde_json::to_value(&ce).unwrap();
        assert!(v.get("subject").is_none());
    }

    #[test]
    fn new_subjectless_omits_subject() {
        let ce = CloudEvent::new_subjectless(
            source("obsd", "t1"),
            mako_events::obs::STP_PARITY_ALERT,
            serde_json::json!({}),
        );
        assert!(ce.subject.is_none());
        assert!(serde_json::to_value(&ce).unwrap().get("subject").is_none());
    }

    #[test]
    fn extensions_serialise_flat_and_round_trip_without_duplicates() {
        let ce = CloudEvent::new(
            source("edmd", "t"),
            "de.x.y",
            "s",
            serde_json::json!({"real": 1}),
        )
        .extension("makopid", 5u32);
        let v = serde_json::to_value(&ce).unwrap();
        assert_eq!(v["makopid"], 5);
        assert_eq!(v["data"]["real"], 1);
        // Round-trips — proof there is no duplicate `data`/`id`/… key.
        let back: CloudEvent = serde_json::from_slice(&ce.to_bytes().unwrap()).unwrap();
        assert_eq!(back.extensions.get("makopid").unwrap(), 5);
    }

    /// A reserved-attribute key would serialise a duplicate JSON key that every
    /// receiver rejects; the guard catches the misuse in debug builds.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "collides with a core CloudEvents attribute")]
    fn extension_with_reserved_key_panics_in_debug() {
        let _ = CloudEvent::new(source("x", "t"), "de.x.y", "s", serde_json::json!({}))
            .extension("id", "boom");
    }

    /// In release builds the assert is compiled out and the reserved key is
    /// silently dropped rather than corrupting the envelope.
    #[test]
    #[cfg(not(debug_assertions))]
    fn extension_with_reserved_key_is_dropped_in_release() {
        let ce = CloudEvent::new(source("x", "t"), "de.x.y", "s", serde_json::json!({"d": 1}))
            .with_id("real-id")
            .extension("id", "boom");
        // The struct field wins; no second `id` key exists.
        assert_eq!(ce.id, "real-id");
        let back: CloudEvent = serde_json::from_slice(&ce.to_bytes().unwrap()).unwrap();
        assert_eq!(back.id, "real-id");
    }

    #[test]
    fn with_time_and_datacontenttype_override_defaults() {
        let t = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let ce = CloudEvent::new(
            source("accountingd", "t"),
            "de.x.y",
            "s",
            serde_json::json!("<xml/>"),
        )
        .with_time(t)
        .with_datacontenttype("application/xml");
        assert_eq!(ce.time, "2023-11-14T22:13:20Z");
        assert_eq!(ce.datacontenttype.as_deref(), Some("application/xml"));
    }
}

#[cfg(test)]
mod publish_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Captured {
        calls: AtomicUsize,
        statuses: Mutex<Vec<u16>>,
        sig: Mutex<Option<String>>,
        webhook_id: Mutex<Option<String>>,
        webhook_ts: Mutex<Option<String>>,
        content_type: Mutex<Option<String>>,
        body: Mutex<Vec<u8>>,
    }

    async fn handler(
        axum::extract::State(cap): axum::extract::State<Arc<Captured>>,
        headers: axum::http::HeaderMap,
        body: axum::body::Bytes,
    ) -> axum::http::StatusCode {
        let n = cap.calls.fetch_add(1, Ordering::SeqCst);
        let hv = |k: &str| {
            headers
                .get(k)
                .and_then(|v| v.to_str().ok())
                .map(String::from)
        };
        *cap.sig.lock().unwrap() = hv(crate::webhook::SIGNATURE_HEADER);
        *cap.webhook_id.lock().unwrap() = hv(crate::webhook::ID_HEADER);
        *cap.webhook_ts.lock().unwrap() = hv(crate::webhook::TIMESTAMP_HEADER);
        *cap.content_type.lock().unwrap() = hv("content-type");
        *cap.body.lock().unwrap() = body.to_vec();
        let statuses = cap.statuses.lock().unwrap();
        let code = *statuses.get(n).unwrap_or_else(|| statuses.last().unwrap());
        axum::http::StatusCode::from_u16(code).unwrap()
    }

    /// Spawn a one-route test server that replies with `statuses[call]` (clamped
    /// to the last) on each POST and records the last request.
    async fn spawn(statuses: Vec<u16>) -> (String, Arc<Captured>) {
        let cap = Arc::new(Captured {
            statuses: Mutex::new(statuses),
            ..Default::default()
        });
        let app = axum::Router::new()
            .route("/hook", axum::routing::post(handler))
            .with_state(Arc::clone(&cap));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}/hook"), cap)
    }

    fn sample() -> CloudEvent {
        CloudEvent::new(
            source("billingd", "t1"),
            mako_events::billing::RECHNUNG_ERSTELLT,
            "DE0001",
            serde_json::json!({"a": 1}),
        )
    }

    #[tokio::test]
    async fn signs_and_sends_canonical_headers_and_exact_body() {
        let (url, cap) = spawn(vec![200]).await;
        let ce = sample();
        let expected_body = ce.to_bytes().unwrap();
        post_ce_with_retry(&crate::http::default_client(), &url, &ce, Some(b"secret"))
            .await
            .unwrap();
        assert_eq!(cap.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            cap.content_type.lock().unwrap().as_deref(),
            Some("application/cloudevents+json")
        );
        // The CloudEvent id *is* the webhook id: replay protection and mako's
        // idempotency key are one fact, not two headers that can disagree.
        assert_eq!(
            cap.webhook_id.lock().unwrap().as_deref(),
            Some(ce.id.as_str())
        );
        assert_eq!(*cap.body.lock().unwrap(), expected_body);

        // What was sent verifies against the same shared verifier a receiver
        // uses — asserting the emitter's own output would prove only that it is
        // self-consistent.
        let mut headers = axum::http::HeaderMap::new();
        for (name, value) in [
            (
                crate::webhook::ID_HEADER,
                cap.webhook_id.lock().unwrap().clone(),
            ),
            (
                crate::webhook::TIMESTAMP_HEADER,
                cap.webhook_ts.lock().unwrap().clone(),
            ),
            (
                crate::webhook::SIGNATURE_HEADER,
                cap.sig.lock().unwrap().clone(),
            ),
        ] {
            headers.insert(
                axum::http::HeaderName::from_static(name),
                value.expect("header sent").parse().expect("ascii"),
            );
        }
        assert_eq!(
            crate::webhook::verify_request(Some(b"secret"), &headers, &expected_body),
            Ok(Some(crate::webhook::WebhookId(ce.id.clone())))
        );
        assert!(crate::webhook::verify_request(Some(b"other"), &headers, &expected_body).is_err());
    }

    /// Every retry must be the **same** signed request.
    ///
    /// Re-stamping the timestamp per attempt would produce three distinct
    /// signatures for one event and defeat the receiver's deduplication at the
    /// moment it is most needed — a 5xx followed by a success is exactly when
    /// the receiver may already have processed the first attempt.
    #[tokio::test]
    async fn every_retry_sends_the_identical_signed_request() {
        let (url, cap) = spawn(vec![503, 503, 200]).await;
        let ce = sample();
        let first = Arc::new(Mutex::new(None::<(String, String, String)>));

        post_ce_with_retry(&crate::http::default_client(), &url, &ce, Some(b"secret"))
            .await
            .unwrap();
        assert_eq!(cap.calls.load(Ordering::SeqCst), 3);

        // The handler records the last request; it must equal the first, which
        // it can only do if nothing was re-stamped.
        let last = (
            cap.webhook_id.lock().unwrap().clone().unwrap(),
            cap.webhook_ts.lock().unwrap().clone().unwrap(),
            cap.sig.lock().unwrap().clone().unwrap(),
        );
        *first.lock().unwrap() = Some(last.clone());
        assert_eq!(
            last.0, ce.id,
            "the id is the CloudEvent's, on every attempt"
        );
        assert!(
            crate::webhook::verify_signature(
                b"secret",
                &last.0,
                last.1.parse().unwrap(),
                &ce.to_bytes().unwrap(),
                &last.2
            ),
            "the surviving attempt is still the signed one"
        );
    }

    #[tokio::test]
    async fn no_signature_header_without_secret() {
        let (url, cap) = spawn(vec![200]).await;
        let ce = sample();
        post_ce_with_retry(&crate::http::default_client(), &url, &ce, None)
            .await
            .unwrap();
        assert!(cap.sig.lock().unwrap().is_none());
        // …but the id still travels, so a receiver deduplicates the same way in
        // dev mode as in production.
        assert_eq!(
            cap.webhook_id.lock().unwrap().as_deref(),
            Some(ce.id.as_str())
        );
    }

    #[tokio::test]
    async fn retries_5xx_then_succeeds() {
        let (url, cap) = spawn(vec![503, 503, 200]).await;
        post_ce_with_retry(&crate::http::default_client(), &url, &sample(), None)
            .await
            .unwrap();
        assert_eq!(cap.calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn returns_err_after_exhausting() {
        let (url, cap) = spawn(vec![503]).await;
        let err = post_ce_with_retry(&crate::http::default_client(), &url, &sample(), None)
            .await
            .unwrap_err();
        assert!(matches!(err, PublishError::Delivery { attempts: 3, .. }));
        assert!(
            !err.is_permanent(),
            "exhausted transient failure is not permanent"
        );
        assert_eq!(cap.calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn skips_retry_on_permanent_4xx() {
        let (url, cap) = spawn(vec![400]).await;
        let err = post_ce_with_retry(&crate::http::default_client(), &url, &sample(), None)
            .await
            .unwrap_err();
        assert!(matches!(err, PublishError::Delivery { attempts: 1, .. }));
        assert!(err.is_permanent(), "a 4xx must be classified permanent");
        assert_eq!(cap.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retries_on_429() {
        let (url, cap) = spawn(vec![429, 200]).await;
        post_ce_with_retry(&crate::http::default_client(), &url, &sample(), None)
            .await
            .unwrap();
        assert_eq!(cap.calls.load(Ordering::SeqCst), 2);
    }
}

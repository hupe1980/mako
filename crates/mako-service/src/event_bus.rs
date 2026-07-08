//! `EventBus` trait — pluggable event fan-out for mako services.
//!
//! `marktd` holds an `Arc<dyn EventBus>` selected from the `[eventbus]` TOML
//! section at startup.  Domain code is unchanged when switching backends.
//!
//! ## Backends
//!
//! | Feature | Backend | When to use |
//! |---|---|---|
//! | *(default)* | `WebhookBus` | HTTP POST + HMAC-SHA256 + 72 h retry |
//! | `kafka` | `KafkaBus` | `krafka` producer; use when webhook fan-out is a measured bottleneck (>500 MaLo, >20 subscribers) |
//!
//! ## Kafka activation threshold
//!
//! The `KafkaBus` should only be enabled when the `WebhookBus` is a measured
//! bottleneck.  The documented threshold is **>500 MaLo or >20 active webhook
//! subscribers** — both are confirmed in `crates/mako-service/src/event_bus.rs`.
//!
//! When that threshold is reached, add the `kafka` feature to the `marktd`
//! binary and configure `[event_bus] backend = "kafka"` in `marktd.toml`.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use mako_service::event_bus::{EventBus, WebhookBus, WebhookBusConfig};
//!
//! let bus: Arc<dyn EventBus> = Arc::new(
//!     WebhookBus::new(WebhookBusConfig::default()),
//! );
//!
//! // In marktd fanout:
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! bus.publish("de.mako.process.initiated", serde_json::json!({
//!     "process_id": "...",
//!     "pid": 55001,
//! })).await.unwrap();
//! # });
//! ```

use std::{future::Future, pin::Pin, sync::Arc};

use serde_json::Value;

// ── Trait ─────────────────────────────────────────────────────────────────────

/// Async event publication.
///
/// All backends must be `Send + Sync + 'static` so they can be held in
/// `Arc<dyn EventBus>` and shared across Tokio tasks.
///
/// The return type uses `Pin<Box<dyn Future>>` (not AFIT) to keep the trait
/// dyn-compatible in Rust 1.89.  AFIT is not dyn-safe until trait-upcasting
/// is stabilised.
pub trait EventBus: Send + Sync + 'static {
    /// Publish an event.
    ///
    /// - `ce_type` — CloudEvents `type` (e.g. `"de.mako.process.initiated"`)
    /// - `payload` — JSON payload (typically a serialised `MarktEvent`)
    ///
    /// Implementations must be idempotent with respect to the event `id` field
    /// inside the payload.
    ///
    /// At-least-once delivery is required; exactly-once is not guaranteed.
    fn publish(
        &self,
        ce_type: &str,
        payload: Value,
    ) -> Pin<Box<dyn Future<Output = Result<(), EventBusError>> + Send + '_>>;

    /// Backend name for health-check and metrics labels.
    fn backend_name(&self) -> &'static str;
}

// ── Errors ────────────────────────────────────────────────────────────────────

/// Errors returned by [`EventBus::publish`].
#[derive(Debug, thiserror::Error)]
pub enum EventBusError {
    /// Serialisation failure — payload could not be encoded for the backend.
    #[error("EventBus serialisation error: {0}")]
    Serialise(String),

    /// Transport failure — network, broker, or connection error.
    #[error("EventBus transport error ({backend}): {cause}")]
    Transport {
        backend: &'static str,
        cause: String,
    },
}

// ── WebhookBus ────────────────────────────────────────────────────────────────

/// Configuration for [`WebhookBus`].
#[derive(Debug, Clone)]
pub struct WebhookBusConfig {
    /// HTTP request timeout per delivery attempt.
    pub delivery_timeout: std::time::Duration,
    /// Maximum retry attempts (exponential back-off).
    ///
    /// The retry window is approximately `2^max_attempts` seconds.
    /// Default 8 ≈ 256 s ≈ 4 minutes; for 72 h total, the caller must enqueue
    /// retries independently.
    pub max_retry_attempts: u32,
}

impl Default for WebhookBusConfig {
    fn default() -> Self {
        Self {
            delivery_timeout: std::time::Duration::from_secs(10),
            max_retry_attempts: 8,
        }
    }
}

/// HTTP webhook-based event bus.
///
/// Compatible with the existing `marktd` fanout worker.  Each `publish` call
/// enqueues delivery to all registered subscribers via HMAC-signed HTTP POST.
///
/// This is the default backend.  Activate Kafka only when webhook fan-out is a
/// measured bottleneck (>500 MaLo, >20 subscribers).
#[derive(Clone)]
pub struct WebhookBus {
    config: WebhookBusConfig,
    /// MPSC sender to the `marktd::fanout` worker task.
    /// `None` in test / dev mode when no fanout worker is running.
    sender: Option<Arc<tokio::sync::mpsc::UnboundedSender<Value>>>,
}

impl WebhookBus {
    /// Create a new `WebhookBus` with default configuration (no fanout worker).
    ///
    /// Wire up the fanout sender via [`WebhookBus::with_sender`] in production.
    pub fn new(config: WebhookBusConfig) -> Self {
        Self {
            config,
            sender: None,
        }
    }

    /// Attach a fanout MPSC sender.
    ///
    /// The sender delivers events to the `marktd::fanout::spawn` worker task.
    pub fn with_sender(mut self, sender: tokio::sync::mpsc::UnboundedSender<Value>) -> Self {
        self.sender = Some(Arc::new(sender));
        self
    }

    /// Fanout configuration.
    pub fn config(&self) -> &WebhookBusConfig {
        &self.config
    }
}

impl EventBus for WebhookBus {
    fn backend_name(&self) -> &'static str {
        "webhook"
    }

    fn publish(
        &self,
        ce_type: &str,
        payload: Value,
    ) -> Pin<Box<dyn Future<Output = Result<(), EventBusError>> + Send + '_>> {
        let ce_type = ce_type.to_owned();
        let sender = self.sender.clone();
        Box::pin(async move {
            let _bytes = serde_json::to_vec(&payload)
                .map_err(|e| EventBusError::Serialise(e.to_string()))?;

            if let Some(ref tx) = sender {
                tx.send(payload).map_err(|e| EventBusError::Transport {
                    backend: "webhook",
                    cause: e.to_string(),
                })?;
            } else {
                tracing::debug!(
                    ce_type = %ce_type,
                    backend = "webhook",
                    "EventBus::publish (no fanout sender configured — dev mode)",
                );
            }

            Ok(())
        })
    }
}

// ── NoopBus ───────────────────────────────────────────────────────────────────

/// No-op event bus — silently discards all events.
///
/// Use in unit tests and in development setups where no subscribers are
/// configured.
#[derive(Clone, Copy, Default)]
pub struct NoopBus;

impl EventBus for NoopBus {
    fn backend_name(&self) -> &'static str {
        "noop"
    }

    fn publish(
        &self,
        _ce_type: &str,
        _payload: Value,
    ) -> Pin<Box<dyn Future<Output = Result<(), EventBusError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
}

// ── KafkaBus (feature-gated) ──────────────────────────────────────────────────

/// Kafka event bus using [`krafka`] (pure Rust, MSRV 1.88, zero-unsafe).
///
/// Activate with `features = ["kafka"]`.  One topic per CloudEvents `type`, e.g.
/// `de.mako.process.initiated` → Kafka topic `de.mako.process.initiated`.
///
/// Uses `krafka`\'s high-throughput producer with LZ4 compression, built-in
/// exponential-backoff retry on leader changes, and idempotent delivery.
///
/// **When to use:** only when webhook fan-out is a measured bottleneck
/// (>500 MaLo, >20 active subscribers).
///
/// **Kafka version requirement:** Kafka 3.9+.
///
/// # Configuration
///
/// ```toml
/// [event_bus]
/// backend           = "kafka"
/// bootstrap_servers = "kafka-0:9092,kafka-1:9092"
/// client_id         = "mako-eventbus"
/// # Optional SASL/SCRAM-SHA-256 (implies TLS):
/// # sasl_username = "mako"
/// # sasl_password = "env:KAFKA_PASSWORD"
/// # Compression codec: "none" | "gzip" | "snappy" | "lz4" (default: "lz4")
/// # compression = "lz4"
/// ```
#[cfg(feature = "kafka")]
pub mod kafka {
    use std::{future::Future, pin::Pin, sync::Arc};

    use krafka::producer::{Acks, Producer};
    use krafka::protocol::Compression;

    use super::{EventBus, EventBusError};

    /// Compression codec for the Kafka producer.
    ///
    /// LZ4 is the recommended default: fastest pure-Rust codec, minimal
    /// latency impact on the hot path.
    #[derive(Debug, Clone, Copy, Default)]
    pub enum KafkaCompression {
        /// No compression — maximum throughput, largest messages.
        None,
        /// Gzip — best ratio, slowest.
        Gzip,
        /// Snappy — good balance, pure Rust via `snap`.
        Snappy,
        /// LZ4 — fastest, pure Rust via `lz4_flex`. **Recommended.**
        #[default]
        Lz4,
    }

    impl From<KafkaCompression> for Compression {
        fn from(c: KafkaCompression) -> Self {
            match c {
                KafkaCompression::None => Compression::None,
                KafkaCompression::Gzip => Compression::Gzip,
                KafkaCompression::Snappy => Compression::Snappy,
                KafkaCompression::Lz4 => Compression::Lz4,
            }
        }
    }

    /// SASL authentication mode for [`KafkaBus`].
    ///
    /// SASL authentication automatically enables TLS on the broker connection.
    /// For plain (unauthenticated) connections, use [`SaslConfig::None`].
    #[derive(Debug, Clone, Default)]
    pub enum SaslConfig {
        /// No authentication — plaintext broker connection.
        /// Only use on internal networks with mTLS at the network layer.
        #[default]
        None,
        /// SASL/SCRAM-SHA-256 (recommended for most deployments).
        ScramSha256 { username: String, password: String },
        /// SASL/SCRAM-SHA-512 (higher security, same protocol).
        ScramSha512 { username: String, password: String },
        /// SASL/PLAIN (username + password without hashing).
        /// Only suitable with TLS; the credentials are sent in plaintext.
        Plain { username: String, password: String },
    }

    /// Configuration for [`KafkaBus`].
    #[derive(Debug, Clone)]
    pub struct KafkaBusConfig {
        /// Comma-separated Kafka bootstrap servers.
        ///
        /// Example: `"kafka-0:9092,kafka-1:9092"`
        pub bootstrap_servers: String,
        /// SASL authentication (default: none — plaintext connection).
        ///
        /// All SASL variants automatically enable TLS on the broker connection.
        pub sasl: SaslConfig,
        /// Compression codec (default: LZ4).
        pub compression: KafkaCompression,
        /// `client.id` sent to the broker (default: `"mako-eventbus"`).
        pub client_id: String,
    }

    impl Default for KafkaBusConfig {
        fn default() -> Self {
            Self {
                bootstrap_servers: "localhost:9092".to_owned(),
                sasl: SaslConfig::None,
                compression: KafkaCompression::Lz4,
                client_id: "mako-eventbus".to_owned(),
            }
        }
    }

    /// Kafka-backed event bus using [`krafka`].
    ///
    /// Serialises each CloudEvent as JSON and produces it to the Kafka topic
    /// matching the CE `type`.  Uses `krafka`\'s built-in:
    ///
    /// - **LZ4 batched compression** (pure Rust, 5 ms linger)
    /// - **Retry with exponential backoff** on leader changes
    /// - **`Acks::Leader`** — durable delivery without full ISR wait
    ///
    /// Use [`super::WebhookBus`] unless Kafka is specifically required.
    #[derive(Clone)]
    pub struct KafkaBus {
        producer: Arc<Producer>,
    }

    /// Errors returned by [`KafkaBus::connect`].
    #[derive(Debug, thiserror::Error)]
    pub enum KafkaBusError {
        /// Initial broker connection or metadata fetch failed.
        #[error("KafkaBus: broker connect failed: {0}")]
        Connect(String),
        /// SASL/PLAIN configuration is invalid (e.g. empty username).
        #[error("KafkaBus: SASL PLAIN config error: {0}")]
        SaslConfig(String),
    }

    impl KafkaBus {
        /// Connect to Kafka and return a `KafkaBus`.
        ///
        /// # Errors
        ///
        /// Returns [`KafkaBusError::Connect`] when the broker connection or
        /// initial metadata fetch fails at startup.
        /// Returns [`KafkaBusError::SaslConfig`] when the SASL/PLAIN
        /// configuration is rejected by `krafka` (e.g. empty credentials).
        pub async fn connect(config: KafkaBusConfig) -> Result<Self, KafkaBusError> {
            let compression = Compression::from(config.compression);

            let producer = match config.sasl {
                SaslConfig::None => Producer::builder()
                    .bootstrap_servers(config.bootstrap_servers)
                    .client_id(config.client_id)
                    .compression(compression)
                    .linger(std::time::Duration::from_millis(5))
                    .acks(Acks::Leader)
                    .build()
                    .await
                    .map_err(|e| KafkaBusError::Connect(e.to_string()))?,
                SaslConfig::ScramSha256 { username, password } => Producer::builder()
                    .bootstrap_servers(config.bootstrap_servers)
                    .client_id(config.client_id)
                    .compression(compression)
                    .linger(std::time::Duration::from_millis(5))
                    .acks(Acks::Leader)
                    .sasl_scram_sha256(username, password)
                    .build()
                    .await
                    .map_err(|e| KafkaBusError::Connect(e.to_string()))?,
                SaslConfig::ScramSha512 { username, password } => Producer::builder()
                    .bootstrap_servers(config.bootstrap_servers)
                    .client_id(config.client_id)
                    .compression(compression)
                    .linger(std::time::Duration::from_millis(5))
                    .acks(Acks::Leader)
                    .sasl_scram_sha512(username, password)
                    .build()
                    .await
                    .map_err(|e| KafkaBusError::Connect(e.to_string()))?,
                SaslConfig::Plain { username, password } => Producer::builder()
                    .bootstrap_servers(config.bootstrap_servers)
                    .client_id(config.client_id)
                    .compression(compression)
                    .linger(std::time::Duration::from_millis(5))
                    .acks(Acks::Leader)
                    .sasl_plain(username, password)
                    .map_err(|e| KafkaBusError::SaslConfig(e.to_string()))?
                    .build()
                    .await
                    .map_err(|e| KafkaBusError::Connect(e.to_string()))?,
            };

            Ok(Self {
                producer: Arc::new(producer),
            })
        }

        /// Gracefully flush and close the producer.
        ///
        /// Call during service shutdown to ensure in-flight batches are
        /// delivered before the process exits.
        pub async fn close(self) {
            if let Ok(producer) = Arc::try_unwrap(self.producer) {
                producer.close().await;
            }
        }
    }

    impl EventBus for KafkaBus {
        fn backend_name(&self) -> &'static str {
            "kafka"
        }

        fn publish(
            &self,
            ce_type: &str,
            payload: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = Result<(), EventBusError>> + Send + '_>> {
            let topic = ce_type.to_owned();
            let producer = Arc::clone(&self.producer);
            Box::pin(async move {
                let bytes = serde_json::to_vec(&payload)
                    .map_err(|e| EventBusError::Serialise(e.to_string()))?;

                // Produce to the topic named after the CloudEvents type.
                // krafka handles batching, linger timer, LZ4 compression,
                // and retry on leader changes internally.
                // We discard the RecordMetadata (partition/offset) — at-least-once
                // delivery is all the EventBus requires; offset tracking is the
                // subscriber's responsibility.
                let _meta = producer
                    .send(
                        &topic,
                        None::<&[u8]>, // key: None — no partition affinity
                        bytes.as_slice(),
                    )
                    .await
                    .map_err(|e| EventBusError::Transport {
                        backend: "kafka",
                        cause: e.to_string(),
                    })?;

                Ok(())
            })
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_bus_always_succeeds() {
        let bus = NoopBus;
        bus.publish(
            "de.mako.process.initiated",
            serde_json::json!({"process_id": "test"}),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn webhook_bus_enqueues_without_error() {
        let bus = WebhookBus::new(WebhookBusConfig::default());
        bus.publish(
            "de.mako.process.initiated",
            serde_json::json!({"process_id": "test", "pid": 55001}),
        )
        .await
        .unwrap();
        assert_eq!(bus.backend_name(), "webhook");
    }

    #[test]
    fn noop_bus_name() {
        assert_eq!(NoopBus.backend_name(), "noop");
    }
}

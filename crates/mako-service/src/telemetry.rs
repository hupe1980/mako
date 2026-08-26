//! Structured logging and optional OpenTelemetry OTLP/gRPC tracing initializer.
//!
//! Every mako service calls [`init_tracing`] once at startup instead of
//! setting up `tracing_subscriber` manually.  This centralises:
//!
//! - JSON-formatted structured logs with `service`, `level`, `target`, `trace_id`
//! - `RUST_LOG` / `log_level` env-filter
//! - Optional OpenTelemetry OTLP export (feature `otel`) — spans are forwarded
//!   to any OTel-compatible backend (Jaeger, Tempo, OTLP collector, …)
//! - W3C `traceparent` / `tracestate` propagation (feature `otel`)
//!
//! # Usage
//!
//! ```rust,no_run
//! use mako_service::telemetry::{init_tracing, OtelConfig};
//!
//! #[tokio::main]
//! async fn main() {
//!     // Without OpenTelemetry (feature not enabled or endpoint not configured)
//!     let _guard = init_tracing("myservice", "info", None);
//!
//!     // With OpenTelemetry
//!     let otel = OtelConfig {
//!         endpoint:     "http://otel-collector:4317".into(),
//!         service_name: "myservice".into(),
//!     };
//!     let _guard = init_tracing("myservice", "info", Some(&otel));
//!     // hold _guard until shutdown — it flushes spans on drop
//! }
//! ```
//!
//! # Panics
//!
//! Panics if the global tracing subscriber is already set (only one call per
//! process is allowed).

use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt as _, util::SubscriberInitExt as _};

/// An extra `tracing` layer a daemon installs beside the standard ones.
///
/// Typed against the bare [`tracing_subscriber::Registry`] because it is added
/// **first**, before the filter and the formatter: a layer added later would be
/// typed against whatever `Layered<…>` stack precedes it, which no daemon can
/// name. Added first, the type is one every caller can write down.
///
/// It exists because a library can emit a metric without choosing an exporter.
/// `agentplane` publishes its whole instrument catalogue as `tracing` events on
/// a dedicated target and leaves the bridge to whoever embeds it — so without a
/// seam like this, an embedder gets a plane whose counters are all dark and no
/// place to plug a collector in.
pub type ExtraLayer =
    Box<dyn tracing_subscriber::Layer<tracing_subscriber::Registry> + Send + Sync>;

// ── Public types ──────────────────────────────────────────────────────────────

/// Configuration for the OpenTelemetry OTLP exporter.
///
/// Populated from the `[otel]` section of each service's TOML config.
#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct OtelConfig {
    /// OTLP gRPC endpoint, e.g. `"http://otel-collector:4317"`.
    /// Required when OpenTelemetry export is desired.
    #[serde(default)]
    pub endpoint: String,
    /// Logical service name emitted in `service.name` resource attribute.
    /// Defaults to the service binary name if empty.
    #[serde(default)]
    pub service_name: String,
}

impl OtelConfig {
    /// `true` when an endpoint is configured (non-empty after trim).
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        !self.endpoint.trim().is_empty()
    }
}

/// Drop guard that flushes and shuts down the `OTel` tracer provider on drop.
///
/// Hold this value for the lifetime of the process:
///
/// ```rust,no_run
/// # use mako_service::telemetry::{init_tracing, OtelConfig};
/// # let otel = OtelConfig::default();
/// let _guard = init_tracing("svc", "info", Some(&otel));
/// // … run service …
/// // _guard dropped here → provider.shutdown() called
/// ```
pub struct OtelGuard {
    #[cfg(feature = "otel")]
    provider: opentelemetry_sdk::trace::SdkTracerProvider,
    _priv: (),
}

impl Drop for OtelGuard {
    fn drop(&mut self) {
        #[cfg(feature = "otel")]
        {
            if let Err(e) = self.provider.shutdown() {
                eprintln!("OTel tracer provider shutdown error: {e}");
            }
        }
    }
}

// ── init_tracing ──────────────────────────────────────────────────────────────

/// Initialise the global `tracing` subscriber.
///
/// - Always: JSON structured logs, `RUST_LOG`-controlled filter.
/// - With `feature = "otel"` and a non-empty `otel.endpoint`:
///   OTLP/gRPC span export, W3C `traceparent` propagation,
///   `trace_id` / `span_id` injected into every log line.
///
/// # Panics
///
/// Panics if called more than once per process.
#[must_use]
pub fn init_tracing(service_name: &str, log_level: &str, otel: Option<&OtelConfig>) -> OtelGuard {
    init_tracing_with(service_name, log_level, otel, None)
}

/// [`init_tracing`], plus one daemon-supplied layer.
///
/// `extra` is installed directly on the registry, ahead of the filter and the
/// formatter — see [`ExtraLayer`] for why that ordering is the one that gives
/// the type a name.
///
/// # Panics
///
/// Panics if called more than once per process.
#[must_use]
pub fn init_tracing_with(
    service_name: &str,
    log_level: &str,
    otel: Option<&OtelConfig>,
    extra: Option<ExtraLayer>,
) -> OtelGuard {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level));

    let fmt_layer = fmt::layer()
        .json()
        .with_target(true)
        .with_thread_ids(false)
        .with_current_span(true);

    #[cfg(feature = "otel")]
    {
        let otel_active = otel.is_some_and(OtelConfig::is_enabled);
        if otel_active {
            let cfg = otel.expect("checked above");
            match build_otel_provider(cfg, service_name) {
                Ok(provider) => {
                    use opentelemetry::global;
                    use opentelemetry::trace::TracerProvider as _;
                    use opentelemetry_sdk::propagation::TraceContextPropagator;

                    // W3C traceparent propagation
                    global::set_text_map_propagator(TraceContextPropagator::new());

                    let svc_name = if cfg.service_name.is_empty() {
                        service_name.to_owned()
                    } else {
                        cfg.service_name.clone()
                    };

                    let otel_layer =
                        tracing_opentelemetry::layer().with_tracer(provider.tracer(svc_name));

                    tracing_subscriber::registry()
                        .with(extra)
                        .with(filter)
                        .with(fmt_layer)
                        .with(otel_layer)
                        .init();

                    tracing::info!(
                        service = service_name,
                        otel_endpoint = cfg.endpoint.as_str(),
                        "OpenTelemetry OTLP exporter active",
                    );

                    return OtelGuard {
                        provider,
                        _priv: (),
                    };
                }
                Err(e) => {
                    // Fall through to plain logging — never block startup on OTel
                    tracing_subscriber::registry()
                        .with(extra)
                        .with(filter)
                        .with(fmt_layer)
                        .init();
                    tracing::warn!(error = %e, "OTel pipeline init failed — falling back to plain logging");
                    // We need to return a guard even without provider
                    return OtelGuard {
                        provider: opentelemetry_sdk::trace::SdkTracerProvider::default(),
                        _priv: (),
                    };
                }
            }
        }
    }

    // Plain JSON logging (no OTel)
    tracing_subscriber::registry()
        .with(extra)
        .with(filter)
        .with(fmt_layer)
        .init();

    OtelGuard {
        #[cfg(feature = "otel")]
        provider: opentelemetry_sdk::trace::SdkTracerProvider::default(),
        _priv: (),
    }
}

// ── OTel provider builder (feature-gated) ────────────────────────────────────

#[cfg(feature = "otel")]
fn build_otel_provider(
    config: &OtelConfig,
    service_name: &str,
) -> Result<opentelemetry_sdk::trace::SdkTracerProvider, Box<dyn std::error::Error + Send + Sync>> {
    use opentelemetry::KeyValue;
    use opentelemetry_otlp::{SpanExporter, WithExportConfig};
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use opentelemetry_semantic_conventions::resource::SERVICE_NAME;

    let svc = if config.service_name.is_empty() {
        service_name.to_owned()
    } else {
        config.service_name.clone()
    };

    let exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&config.endpoint)
        .build()?;

    let resource = Resource::builder()
        .with_attribute(KeyValue::new(SERVICE_NAME, svc))
        .build();

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();

    Ok(provider)
}

// ── init_tracing_from_env ─────────────────────────────────────────────────────

/// Initialize structured logging from environment variables — the standard
/// one-liner for **all** mako services.
///
/// Replaces the weaker `tracing_subscriber::fmt::init()` call:
///
/// ```rust,no_run
/// // Old (no OTel, ignores LOG_LEVEL env var):
/// tracing_subscriber::fmt::init();
///
/// // New — structured JSON, env-configurable level, optional OTel:
/// # use mako_service::telemetry::init_tracing_from_env;
/// let _guard = init_tracing_from_env("my-service");
/// ```
///
/// ## Environment variables
///
/// | Variable | Effect |
/// |---|---|
/// | `<SERVICE>_LOG_LEVEL` | Log level filter for this service specifically (e.g. `MARKTD_LOG_LEVEL`) |
/// | `LOG_LEVEL` or `RUST_LOG` | Log level filter for all services (default: `"info"`) |
/// | `OTEL_EXPORTER_OTLP_ENDPOINT` | Enables OTLP trace export when set |
/// | `OTEL_SERVICE_NAME` | Overrides `service_name` in trace metadata |
///
/// ## Important — keep the guard alive
///
/// The returned [`OtelGuard`] **must** be bound to `_guard` (not `_`) so it
/// lives until the end of `main`:
///
/// ```rust,no_run
/// # use mako_service::telemetry::init_tracing_from_env;
/// let _guard = init_tracing_from_env("accountingd");
/// //  ^^^^^^ not `_` — that would drop immediately!
/// ```
///
/// # Panics
///
/// Panics if called more than once per process.
#[must_use]
pub fn init_tracing_from_env(service_name: &str) -> OtelGuard {
    // Service-prefixed first: every container image sets `<SERVICE>_LOG_LEVEL`,
    // and `load_config` deliberately ignores that key so it does not collide
    // with `deny_unknown_fields` — which left it read by nobody at all.
    let prefixed = format!(
        "{}_LOG_LEVEL",
        service_name.to_uppercase().replace('-', "_")
    );
    let level = std::env::var(&prefixed)
        .or_else(|_| std::env::var("LOG_LEVEL"))
        .or_else(|_| std::env::var("RUST_LOG"))
        .unwrap_or_else(|_| "info".to_owned());

    let otel_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
    let otel_svc = std::env::var("OTEL_SERVICE_NAME")
        .ok()
        .unwrap_or_else(|| service_name.to_owned());
    let otel = otel_endpoint.map(|ep| OtelConfig {
        endpoint: ep,
        service_name: otel_svc,
    });

    init_tracing_with(service_name, &level, otel.as_ref(), None)
}

/// [`init_tracing_from_env`], plus one daemon-supplied layer.
///
/// # Panics
///
/// Panics if called more than once per process.
#[must_use]
pub fn init_tracing_from_env_with(service_name: &str, extra: Option<ExtraLayer>) -> OtelGuard {
    let prefixed = format!(
        "{}_LOG_LEVEL",
        service_name.to_uppercase().replace('-', "_")
    );
    let level = std::env::var(&prefixed)
        .or_else(|_| std::env::var("LOG_LEVEL"))
        .or_else(|_| std::env::var("RUST_LOG"))
        .unwrap_or_else(|_| "info".to_owned());

    let otel_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
    let otel_svc = std::env::var("OTEL_SERVICE_NAME")
        .ok()
        .unwrap_or_else(|| service_name.to_owned());
    let otel = otel_endpoint.map(|ep| OtelConfig {
        endpoint: ep,
        service_name: otel_svc,
    });

    init_tracing_with(service_name, &level, otel.as_ref(), extra)
}

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

use tracing_subscriber::{
    EnvFilter, Layer as _, fmt, layer::SubscriberExt as _, util::SubscriberInitExt as _,
};

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

/// How a service encodes its log records.
///
/// JSON is the default and the only format a log pipeline should be fed: every
/// field of a `tracing` event survives it, which is what makes `malo_id`,
/// `process_id` and `correlation_id` queryable after the fact. `Pretty` and
/// `Compact` exist for a terminal, where the JSON is unreadable.
///
/// Resolved from `<SERVICE>_LOG_FORMAT`, then `LOG_FORMAT`, by
/// [`init_tracing_from_env_with`]. The name is matched case-insensitively and
/// an unrecognised value falls back to JSON with a warning rather than
/// refusing to start — a log-encoding typo must not stop a daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogFormat {
    /// One JSON object per record. The default, and what a container emits.
    #[default]
    Json,
    /// Multi-line human-readable output, for a terminal.
    Pretty,
    /// Single-line human-readable output, for a terminal.
    Compact,
}

impl LogFormat {
    /// Parse a `LOG_FORMAT` value, case-insensitively.
    ///
    /// Returns `None` for an unrecognised value so the caller can say what it
    /// ignored; every caller here falls back to [`LogFormat::Json`].
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "json" => Some(Self::Json),
            "pretty" => Some(Self::Pretty),
            "compact" => Some(Self::Compact),
            _ => None,
        }
    }

    /// The name this variant parses from.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Pretty => "pretty",
            Self::Compact => "compact",
        }
    }
}

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
    init_tracing_with(service_name, log_level, LogFormat::default(), otel, None)
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
    format: LogFormat,
    otel: Option<&OtelConfig>,
    extra: Option<ExtraLayer>,
) -> OtelGuard {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level));

    // Boxed because the three builders are three different types; the
    // subscriber stack below is then written once for all of them.
    let fmt_layer = match format {
        LogFormat::Json => fmt::layer()
            .json()
            .with_target(true)
            .with_thread_ids(false)
            .with_current_span(true)
            .boxed(),
        LogFormat::Pretty => fmt::layer().pretty().with_target(true).boxed(),
        LogFormat::Compact => fmt::layer().compact().with_target(true).boxed(),
    };

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
    init_tracing_from_env_with(service_name, None)
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

    // Same precedence as the level. `load_config` ignores both keys in its env
    // layer, so a config struct carrying `deny_unknown_fields` is not broken by
    // their presence — which is also why they are read here and not from TOML.
    let prefixed_format = format!(
        "{}_LOG_FORMAT",
        service_name.to_uppercase().replace('-', "_")
    );
    let format = match std::env::var(&prefixed_format)
        .or_else(|_| std::env::var("LOG_FORMAT"))
        .ok()
    {
        None => LogFormat::default(),
        Some(v) => LogFormat::parse(&v).unwrap_or_else(|| {
            // The subscriber does not exist yet, so this cannot be an event.
            eprintln!(
                "{service_name}: LOG_FORMAT={v:?} is not one of json/pretty/compact — using json"
            );
            LogFormat::Json
        }),
    };

    let otel_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
    let otel_svc = std::env::var("OTEL_SERVICE_NAME")
        .ok()
        .unwrap_or_else(|| service_name.to_owned());
    let otel = otel_endpoint.map(|ep| OtelConfig {
        endpoint: ep,
        service_name: otel_svc,
    });

    init_tracing_with(service_name, &level, format, otel.as_ref(), extra)
}

#[cfg(test)]
mod tests {
    use super::LogFormat;

    /// JSON is the default, because a container's logs are read by a pipeline
    /// and not by a person — every field of an event survives it.
    #[test]
    fn the_default_is_json() {
        assert_eq!(LogFormat::default(), LogFormat::Json);
    }

    /// Operators set this from a shell and a Dockerfile, where case is not
    /// something to be exact about.
    #[test]
    fn the_name_is_matched_case_insensitively_and_trimmed() {
        for (input, want) in [
            ("json", LogFormat::Json),
            ("JSON", LogFormat::Json),
            ("  Json  ", LogFormat::Json),
            ("pretty", LogFormat::Pretty),
            ("PRETTY", LogFormat::Pretty),
            ("compact", LogFormat::Compact),
        ] {
            assert_eq!(LogFormat::parse(input), Some(want), "parsing {input:?}");
        }
    }

    /// An unrecognised value is reported, not guessed at and not fatal: the
    /// caller falls back to JSON and says what it ignored. Answering `Some`
    /// here would make a typo silently select a format.
    #[test]
    fn an_unrecognised_name_does_not_resolve() {
        for input in ["", "plain", "logfmt", "js on", "pretty-json"] {
            assert_eq!(LogFormat::parse(input), None, "parsing {input:?}");
        }
    }

    /// Every variant parses from the name it prints, so a value read back out
    /// of a config dump selects the same format it came from.
    #[test]
    fn every_variant_round_trips_through_its_name() {
        for f in [LogFormat::Json, LogFormat::Pretty, LogFormat::Compact] {
            assert_eq!(LogFormat::parse(f.as_str()), Some(f));
        }
    }
}

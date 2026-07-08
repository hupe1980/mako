//! Prometheus metrics — `GET /metrics` handler + Tower recording middleware.
//!
//! Enabled by the `metrics` Cargo feature.
//!
//! Registers two standard metrics on the default Prometheus registry the first
//! time [`init_metrics`] is called (idempotent):
//!
//! | Name | Type | Labels |
//! |---|---|---|
//! | `mako_http_requests_total` | CounterVec | `method`, `path`, `status` |
//! | `mako_http_request_duration_seconds` | HistogramVec | `method`, `path` |
//!
//! ## Usage
//!
//! ```rust,no_run
//! use mako_service::ServiceBuilder;
//! use mako_service::metrics::init_metrics;
//!
//! init_metrics();
//!
//! let app = ServiceBuilder::new()
//!     .with_health(|| async { true })
//!     .with_metrics()       // mounts GET /metrics + recording middleware
//!     .build();
//! ```

use std::sync::OnceLock;
use std::time::Instant;

use axum::{extract::Request, middleware::Next, response::Response};
use prometheus::{
    CounterVec, Encoder, HistogramVec, TextEncoder, register_counter_vec, register_histogram_vec,
};

static REQUESTS_TOTAL: OnceLock<CounterVec> = OnceLock::new();
static REQUEST_DURATION: OnceLock<HistogramVec> = OnceLock::new();

/// Register `mako_http_*` metrics on the default Prometheus registry.
///
/// Idempotent — safe to call multiple times (only registers once).
/// Call this once at service startup, before serving traffic.
pub fn init_metrics() {
    REQUESTS_TOTAL.get_or_init(|| {
        register_counter_vec!(
            "mako_http_requests_total",
            "Total HTTP requests handled by this mako service",
            &["method", "path", "status"]
        )
        .expect("mako_http_requests_total registration failed")
    });
    REQUEST_DURATION.get_or_init(|| {
        register_histogram_vec!(
            "mako_http_request_duration_seconds",
            "HTTP request latency histogram",
            &["method", "path"],
            vec![
                0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0
            ]
        )
        .expect("mako_http_request_duration_seconds registration failed")
    });
}

/// Record a single HTTP request observation.
///
/// Silently no-ops when [`init_metrics`] has not been called yet.
pub fn record_request(method: &str, path: &str, status: u16, duration_secs: f64) {
    if let Some(c) = REQUESTS_TOTAL.get() {
        c.with_label_values(&[method, path, &status.to_string()])
            .inc();
    }
    if let Some(h) = REQUEST_DURATION.get() {
        h.with_label_values(&[method, path]).observe(duration_secs);
    }
}

/// Axum handler: `GET /metrics` — encodes Prometheus default registry as text.
pub async fn metrics_handler() -> impl axum::response::IntoResponse {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::with_capacity(4096);
    let _ = encoder.encode(&metric_families, &mut buffer);
    (
        axum::http::StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        buffer,
    )
}

/// Axum `from_fn` middleware that records HTTP request metrics.
///
/// Normalises path labels to avoid high-cardinality explosion from IDs:
/// any path segment that is a UUID, pure-digit string, or longer than 24
/// chars is replaced with `{id}`.
pub async fn recording_middleware(req: Request, next: Next) -> Response {
    let method = req.method().to_string();
    let raw_path = req.uri().path().to_string();
    let path = normalise_path(&raw_path);
    let start = Instant::now();
    let response = next.run(req).await;
    let duration = start.elapsed().as_secs_f64();
    let status = response.status().as_u16();
    record_request(&method, &path, status, duration);
    response
}

/// Replace variable path segments with `{id}` to bound label cardinality.
fn normalise_path(path: &str) -> String {
    let segments: Vec<&str> = path.split('/').collect();
    let normalised: Vec<&str> = segments
        .iter()
        .map(|seg| {
            if seg.is_empty() {
                // Preserve empty segments produced by leading/trailing `/`
                *seg
            } else if !seg.contains('.') && is_variable_segment(seg) {
                "{id}"
            } else {
                seg
            }
        })
        .collect();
    normalised.join("/")
}

fn is_variable_segment(s: &str) -> bool {
    // UUID pattern (with dashes)
    if s.len() == 36 && s.chars().filter(|&c| c == '-').count() == 4 {
        return true;
    }
    // Pure digits (IDs, years-as-segment, etc.) > 4 chars
    if s.len() > 4 && s.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    // Very long opaque tokens
    if s.len() > 30 {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_keeps_short_paths() {
        assert_eq!(normalise_path("/health/live"), "/health/live");
        assert_eq!(normalise_path("/api/v1/malo"), "/api/v1/malo");
    }

    #[test]
    fn normalise_replaces_uuid() {
        let path = "/api/v1/malo/550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(normalise_path(path), "/api/v1/malo/{id}");
    }

    #[test]
    fn normalise_replaces_digit_ids() {
        let path = "/api/v1/malo/51238696780";
        assert_eq!(normalise_path(path), "/api/v1/malo/{id}");
    }
}

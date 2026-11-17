//! Prometheus metrics — `GET /metrics` handler + Tower recording middleware.
//!
//! Enabled by the `metrics` Cargo feature.
//!
//! Registers two standard metrics on the default Prometheus registry the first
//! time [`init_metrics`] is called (idempotent):
//!
//! | Name | Type | Labels |
//! |---|---|---|
//! | `mako_http_requests_total` | `CounterVec` | `method`, `path`, `status` |
//! | `mako_http_request_duration_seconds` | `HistogramVec` | `method`, `path` |
//! | `mako_bo4e_fractional_json_number_total` | `PullingGauge` | — |
//!
//! ## Why the BO4E counter is here
//!
//! Every BO4E amount is a decimal, and a **fractional** JSON number has been
//! through `f64` before any deserializer sees it, so its scale is already gone.
//! mako does not choose how a counterparty encodes its JSON, and refusing the
//! shape would refuse a producer BO4E permits, so the BO4E gate
//! (`mako_markt::bo4e::decode`) admits it and counts it. This exports the
//! count: a producer sending floats shows up as a rising series instead of as
//! cents that do not reconcile.
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
    CounterVec, Encoder, HistogramVec, PullingGauge, TextEncoder, register_counter_vec,
    register_histogram_vec,
};

static REQUESTS_TOTAL: OnceLock<CounterVec> = OnceLock::new();
static REQUEST_DURATION: OnceLock<HistogramVec> = OnceLock::new();
static BO4E_JSON_NUMBERS: OnceLock<()> = OnceLock::new();

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
    BO4E_JSON_NUMBERS.get_or_init(|| {
        // Pulled at scrape time: the counter lives in the BO4E gate as a
        // process-wide atomic, so there is nothing for this crate to increment.
        let gauge = PullingGauge::new(
            "mako_bo4e_fractional_json_number_total",
            "Fractional JSON numbers admitted by the BO4E gate \
             (each has already lost its scale to f64)",
            // Prometheus speaks `f64`. The count would have to pass 2^53 reads
            // in one process lifetime to lose a digit, and long before that the
            // series has already said what it exists to say.
            #[expect(
                clippy::cast_precision_loss,
                reason = "a count or ratio reported as a float, never a money amount"
            )]
            Box::new(|| mako_markt::bo4e::fractional_json_number_count() as f64),
        )
        .expect("mako_bo4e_fractional_json_number_total construction failed");
        prometheus::register(Box::new(gauge))
            .expect("mako_bo4e_fractional_json_number_total registration failed");
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
/// The `path` label is the **matched route template** (`/nachrechnen/{token}`),
/// never the request path: a path segment can be a credential — portald's
/// public verify token is one — and `/metrics` is scraped by whoever can reach
/// it. A request no route matched is labelled `{unmatched}`, which also bounds
/// the label set against a caller inventing paths.
pub async fn recording_middleware(req: Request, next: Next) -> Response {
    let method = req.method().to_string();
    let path = route_label(&req);
    let start = Instant::now();
    let response = next.run(req).await;
    let duration = start.elapsed().as_secs_f64();
    let status = response.status().as_u16();
    record_request(&method, &path, status, duration);
    response
}

/// The route template a request matched, or `{unmatched}`.
fn route_label(req: &Request) -> String {
    req.extensions()
        .get::<axum::extract::MatchedPath>()
        .map_or_else(|| "{unmatched}".to_owned(), |m| m.as_str().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate's fractional-number count is on `/metrics`, and it counts.
    ///
    /// Registered as a pulling gauge, so a scrape that does not reach the gate
    /// reports a flat zero and looks exactly like a clean feed.
    #[test]
    fn the_bo4e_fractional_number_counter_is_exported_and_rises() {
        use rubo4e::current::Rechnung;

        init_metrics();
        let before = mako_markt::bo4e::fractional_json_number_count();
        let _: Rechnung =
            mako_markt::bo4e::decode(serde_json::json!({"gesamtbrutto": {"wert": 29.5}}))
                .expect("a JSON number is lawful BO4E");
        assert!(
            mako_markt::bo4e::fractional_json_number_count() > before,
            "a price read from a fractional JSON number must be counted"
        );

        let mut buffer = Vec::new();
        Encoder::encode(&TextEncoder::new(), &prometheus::gather(), &mut buffer)
            .expect("encode the default registry");
        let text = String::from_utf8(buffer).expect("utf-8");
        assert!(
            text.contains("mako_bo4e_fractional_json_number_total"),
            "the counter must appear on /metrics"
        );
    }

    /// A token-shaped segment never reaches a label: the label is the route
    /// template, and a path no route matches is `{unmatched}`.
    #[tokio::test]
    async fn the_path_label_is_the_route_template_never_the_request_path() {
        use tower::ServiceExt as _;
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let record = std::sync::Arc::clone(&seen);
        let app = axum::Router::new()
            .route(
                "/nachrechnen/{token}",
                axum::routing::get(|| async { "ok" }),
            )
            .layer(axum::middleware::from_fn(
                move |req: Request, next: Next| {
                    let record = std::sync::Arc::clone(&record);
                    async move {
                        record.lock().expect("lock").push(route_label(&req));
                        next.run(req).await
                    }
                },
            ));
        for uri in [
            "/nachrechnen/q3Zf9-xY_7aBcDeFgHiJkL",
            "/elsewhere/q3Zf9-xY_7aBcDeFgHiJkL",
        ] {
            let req = axum::http::Request::get(uri)
                .body(axum::body::Body::empty())
                .expect("request");
            let _ = app.clone().oneshot(req).await.expect("responds");
        }
        let labels = seen.lock().expect("lock").clone();
        assert_eq!(labels, vec!["/nachrechnen/{token}", "{unmatched}"]);
        assert!(labels.iter().all(|l| !l.contains("q3Zf9")));
    }
}

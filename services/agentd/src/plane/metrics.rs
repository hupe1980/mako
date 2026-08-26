//! agentplane's instrument catalogue, on mako's Prometheus registry.
//!
//! agentplane measures itself and deliberately picks no exporter: it emits every
//! instrument in [`agentplane::runtime::metrics::CATALOGUE`] as a `tracing` event
//! on the [`METRIC`] target, with fixed fields (`metric`, `kind`, `unit`,
//! `value`, `dim`, `tenant`), and leaves the collector to whoever embeds it. That
//! is right upstream — a library that chose Prometheus would be wrong for
//! everyone who does not — and it makes the bridge the embedder's job. Without
//! one every counter goes into the log stream and nowhere else, and an operator
//! cannot tell *this never happens* from *nothing reports it*.
//!
//! One `tracing` layer, installed on the registry before the filter, turning each
//! metric event into a series on the registry `GET /metrics` already serves: a
//! counter into an `IntCounterVec`, a gauge into an `IntGaugeVec`, both labelled
//! by the instrument's declared dimension. Names are derived from the catalogue
//! rather than restated in a table here, so an instrument added upstream appears
//! without an edit.
//!
//! ## What it deliberately does not do
//!
//! **No durations.** agentplane refuses to measure them in-crate and the reason
//! carries over: a replayed run would re-measure a call it did not make. Latency
//! comes from the spans `runtime::telemetry` emits, computed by a collector that
//! can exclude replays — an in-process histogram could not.
//!
//! **No tenant label.** One plane per process here, the tenant is on every log
//! line, and a label whose cardinality is one costs a dimension for nothing.
//!
//! ## One operational constraint
//!
//! The events are emitted at `info` and this layer sits under the `EnvFilter`,
//! which is a **global** filter — so a deployment running at `warn` silences the
//! metrics with the logs, and the series flatline rather than disappear. Admit
//! the target explicitly:
//!
//! ```text
//! LOG_LEVEL="warn,agentplane.metric=info"
//! ```

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use agentplane::runtime::metrics::{Instrument, Kind, METRIC};
use prometheus::{IntCounterVec, IntGaugeVec, register_int_counter_vec, register_int_gauge_vec};
use tracing::field::{Field, Visit};

/// The label every series carries when its instrument declares no dimension.
///
/// Prometheus has no notion of a label-less member of a labelled family, and
/// registering two families per instrument — one with the label, one without —
/// would make every query need to know which. One family, one label, empty when
/// the instrument has no dimension.
const NO_DIMENSION: &str = "dim";

/// Registered families, by Prometheus name.
///
/// Registration is idempotent per process and the map is the memo: `prometheus`
/// refuses a duplicate registration, and looking one up is cheaper than handling
/// that refusal on every event.
type Registry = (HashMap<String, IntCounterVec>, HashMap<String, IntGaugeVec>);

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new((HashMap::new(), HashMap::new())))
}

/// The Prometheus name for one agentplane instrument.
///
/// `agentplane.runs` → `agentd_agentplane_runs_total`. Derived rather than
/// mapped: a table of 23 names here would be a second catalogue, and the first
/// instrument upstream adds would be missing from it silently.
#[must_use]
pub fn prometheus_name(i: &Instrument) -> String {
    let base = format!("agentd_{}", i.name.replace(['.', '-'], "_"));
    match i.kind {
        // The convention Prometheus tooling assumes for a monotonic series.
        Kind::Counter => format!("{base}_total"),
        Kind::Gauge => base,
    }
}

/// The instrument in agentplane's catalogue with this name.
fn instrument(name: &str) -> Option<&'static Instrument> {
    agentplane::runtime::metrics::CATALOGUE
        .iter()
        .find(|i| i.name == name)
}

/// One metric event's fields, as the layer reads them off the record.
#[derive(Debug, Default)]
struct MetricEvent {
    metric: Option<String>,
    value: u64,
    dim: String,
}

impl Visit for MetricEvent {
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "value" {
            self.value = value;
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if field.name() == "value" {
            self.value = u64::try_from(value).unwrap_or(0);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "metric" => self.metric = Some(value.to_owned()),
            "dim" => self.dim = value.to_owned(),
            _ => {}
        }
    }

    /// `tracing` records a `&str` field through `record_debug` when it was
    /// captured by `Display`/`Debug` rather than as a primitive, and the
    /// upstream emitter uses shorthand field syntax for `dim`. Reading only
    /// `record_str` left `dim` empty for every counter that has one — every
    /// run outcome collapsing into a single unlabelled series.
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let rendered = format!("{value:?}");
        let unquoted = rendered
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(&rendered)
            .to_owned();
        match field.name() {
            "metric" => self.metric = Some(unquoted),
            "dim" => self.dim = unquoted,
            _ => {}
        }
    }
}

/// The layer itself.
///
/// Stateless: everything it needs is in the event and in agentplane's
/// catalogue.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlaneMetrics;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for PlaneMetrics {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if event.metadata().target() != METRIC {
            return;
        }
        let mut read = MetricEvent::default();
        event.record(&mut read);
        let Some(name) = read.metric.as_deref() else {
            return;
        };
        // An instrument this build's agentplane does not declare is not
        // registered under a guessed kind: a counter recorded as a gauge reads
        // as a level that keeps rising, which is worse than a missing series.
        let Some(i) = instrument(name) else {
            return;
        };
        record(i, &read.dim, read.value);
    }
}

/// Register-on-first-sight, then observe.
fn record(i: &Instrument, dim: &str, value: u64) {
    let key = prometheus_name(i);
    let Ok(mut families) = registry().lock() else {
        // A poisoned metrics mutex must not take the plane down with it.
        return;
    };
    match i.kind {
        Kind::Counter => {
            let family = families.0.entry(key.clone()).or_insert_with(|| {
                register_int_counter_vec!(key.clone(), i.description.to_owned(), &[NO_DIMENSION])
                    .unwrap_or_else(|e| panic!("register {key}: {e}"))
            });
            family.with_label_values(&[dim]).inc_by(value);
        }
        Kind::Gauge => {
            let family = families.1.entry(key.clone()).or_insert_with(|| {
                register_int_gauge_vec!(key.clone(), i.description.to_owned(), &[NO_DIMENSION])
                    .unwrap_or_else(|e| panic!("register {key}: {e}"))
            });
            family
                .with_label_values(&[dim])
                .set(i64::try_from(value).unwrap_or(i64::MAX));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Counters get `_total`, gauges do not, and the dots become underscores.
    #[test]
    fn names_follow_the_prometheus_convention() {
        assert_eq!(
            prometheus_name(&agentplane::runtime::metrics::RUNS),
            "agentd_agentplane_runs_total"
        );
        assert_eq!(
            prometheus_name(&agentplane::runtime::metrics::OPEN_CASES),
            "agentd_agentplane_cases_open"
        );
        assert_eq!(
            prometheus_name(&agentplane::runtime::metrics::DEADLINE_BREACHES),
            "agentd_agentplane_deadlines_breached_total"
        );
    }

    /// **Every instrument in the catalogue gets a distinct, valid series name.**
    ///
    /// A collision would make two instruments share a family, and the second
    /// registration would then fail against a description that does not describe
    /// it — silently, since a failed registration here falls back to whatever
    /// was registered first.
    #[test]
    fn every_catalogue_entry_maps_to_a_distinct_valid_name() {
        let mut seen = std::collections::BTreeSet::new();
        for i in agentplane::runtime::metrics::CATALOGUE {
            let name = prometheus_name(i);
            assert!(
                name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
                "`{name}` is not a valid Prometheus metric name"
            );
            assert!(seen.insert(name.clone()), "`{name}` is claimed twice");
        }
        assert_eq!(
            seen.len(),
            agentplane::runtime::metrics::CATALOGUE.len(),
            "one series per instrument"
        );
    }

    /// The bridge records what the emitter emits, including the dimension.
    ///
    /// Asserted against the *real* field shape rather than a hand-built event:
    /// the upstream emitter uses `tracing`'s shorthand for `dim`, which arrives
    /// through `record_debug` rather than `record_str`, and a visitor reading
    /// only the latter collapses every labelled counter into one series.
    #[test]
    fn a_metric_event_reaches_prometheus_with_its_dimension() {
        use tracing_subscriber::layer::SubscriberExt as _;

        let subscriber = tracing_subscriber::registry().with(PlaneMetrics);
        tracing::subscriber::with_default(subscriber, || {
            let i = agentplane::runtime::metrics::RUNS;
            tracing::info!(
                target: METRIC,
                metric = i.name,
                kind = i.kind.as_str(),
                unit = i.unit,
                value = 1_u64,
                dim = "failed",
                tenant = "",
            );
        });

        let name = prometheus_name(&agentplane::runtime::metrics::RUNS);
        let families = prometheus::gather();
        let family = families
            .iter()
            .find(|f| f.name() == name)
            .unwrap_or_else(|| panic!("`{name}` was never registered"));
        let sample = family
            .get_metric()
            .iter()
            .find(|m| {
                m.get_label()
                    .iter()
                    .any(|l| l.name() == NO_DIMENSION && l.value() == "failed")
            })
            .expect("the run outcome must survive as a label, not be flattened away");
        assert!(sample.get_counter().value() >= 1.0);
    }

    /// An event on another target is not a metric, however it is shaped.
    ///
    /// Asserted on **one named family** rather than on the size of the default
    /// registry: that registry is process-global and every other test in this
    /// binary registers into it, in parallel, so a before/after count is a race
    /// rather than a property. The instrument here is used by no other test, so
    /// its absence is the claim.
    #[test]
    fn an_ordinary_log_line_registers_nothing() {
        use tracing_subscriber::layer::SubscriberExt as _;

        let name = prometheus_name(&agentplane::runtime::metrics::QUARANTINES);
        let subscriber = tracing_subscriber::registry().with(PlaneMetrics);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(metric = "agentplane.quarantines", value = 1_u64, dim = "");
        });
        assert!(
            !prometheus::gather().iter().any(|f| f.name() == name),
            "a log line that happens to carry a `metric` field is still a log line — \
             `{name}` must not exist because of one"
        );
    }
}

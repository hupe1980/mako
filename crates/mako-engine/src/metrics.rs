//! [`EngineMetrics`] — process-level event counters for Prometheus export.
//!
//! Provides a **process-global** set of [`std::sync::atomic::AtomicU64`]
//! counters that the engine and domain handlers increment at runtime. The
//! [`metrics_api`] handler reads them via [`EngineMetrics::global()`] without
//! any I/O and renders them in Prometheus text format.
//!
//! ## Design rationale
//!
//! The mako-engine is a single-process daemon (`makod`). A process-global
//! static is the simplest, lowest-overhead counter mechanism that:
//!
//! - requires **zero allocations** on the hot path (every command dispatch),
//! - is **async-safe** (atomics need no async context),
//! - imposes **no external dependency** (no `prometheus` crate in the engine),
//! - is **observable** from `metrics_api` via a simple method call.
//!
//! The trade-off: counters reset on process restart (they are not persisted).
//! For a single-process daemon this is acceptable — Prometheus's `rate()`
//! function handles counter resets automatically.
//!
//! ## Usage
//!
//! ### Incrementing a counter
//!
//! ```rust
//! use mako_engine::metrics::{EngineMetrics, ProcessOutcome};
//!
//! // In a workflow handle() or apply() implementation:
//! EngineMetrics::global().process_initiated("gpke");
//! EngineMetrics::global().process_completed("gpke", ProcessOutcome::Accepted);
//! EngineMetrics::global().validation_failed("utilmd", "S2.1");
//! ```
//!
//! ### Reading counters (metrics endpoint)
//!
//! ```rust,ignore
//! let metrics = mako_engine::metrics::EngineMetrics::global();
//! let snapshot = metrics.snapshot();
//! // Render snapshot to Prometheus text format.
//! ```
//!
//! [`metrics_api`]: https://docs.rs/makod

use std::{
    collections::HashMap,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

// ── ProcessOutcome ────────────────────────────────────────────────────────────

/// Terminal outcome of a MaKo process instance.
///
/// Used as the `result` label on [`EngineMetrics::process_completed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProcessOutcome {
    /// The counterparty accepted the request (Bestätigung / positive APERAK).
    Accepted,
    /// The counterparty rejected the request (Ablehnung / negative APERAK).
    Rejected,
    /// The process timed out before a response arrived (24h / 5 WD / 10 WD).
    Timeout,
    /// The process was cancelled by the originating ERP before completion.
    Cancelled,
}

impl ProcessOutcome {
    /// Prometheus label value for this outcome.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
        }
    }

    /// All variants in a fixed order, for metric exposition.
    pub const ALL: &'static [Self] = &[
        Self::Accepted,
        Self::Rejected,
        Self::Timeout,
        Self::Cancelled,
    ];
}

// ── MetricVec ─────────────────────────────────────────────────────────────────

/// A map of label strings → `AtomicU64` counters.
///
/// `MetricVec` is append-only: new label combinations are registered on first
/// increment and are never removed (counters remain at 0 once created).
#[derive(Default)]
struct MetricVec {
    inner: std::sync::RwLock<HashMap<Box<str>, Arc<AtomicU64>>>,
}

impl MetricVec {
    fn increment(&self, label: &str) {
        // Fast path: label already registered — just increment.
        {
            let guard = self.inner.read().expect("MetricVec RwLock poisoned");
            if let Some(counter) = guard.get(label) {
                counter.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        // Slow path: first increment for this label — register + increment.
        let mut guard = self.inner.write().expect("MetricVec RwLock poisoned");
        let counter = guard
            .entry(label.into())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)));
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Snapshot all label → value pairs, sorted by label for deterministic output.
    fn snapshot(&self) -> Vec<(Box<str>, u64)> {
        let guard = self.inner.read().expect("MetricVec RwLock poisoned");
        let mut pairs: Vec<(Box<str>, u64)> = guard
            .iter()
            .map(|(k, v)| (k.clone(), v.load(Ordering::Relaxed)))
            .collect();
        pairs.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
        pairs
    }
}

// ── EngineMetrics ─────────────────────────────────────────────────────────────

/// Process-global engine metrics counters.
///
/// Access via [`EngineMetrics::global()`]. The global instance is initialised
/// once on first access using [`OnceLock`] and lives for the process lifetime.
///
/// ## Counter naming (maps 1:1 to Prometheus metric names)
///
/// | Method | Prometheus metric | Labels |
/// |---|---|---|
/// | [`process_initiated`] | `makod_process_initiated_total` | `family` |
/// | [`process_completed`] | `makod_process_completed_total` | `family`, `result` |
/// | [`validation_failed`] | `makod_validation_failed_total` | `message_type`, `release` |
/// | [`outbox_delivery_attempted`] | `makod_outbox_delivery_attempts_total` | `result` |
/// | [`deadline_fired`] | `makod_deadline_fired_total` | `family` |
/// | [`dead_letter_recorded`] | `makod_dead_letter_recorded_total` | `reason` |
///
/// For `makod_dead_letter_recorded_total`, the `reason` label is:
/// - `unknown_pid:<N>` when `DeadLetterReason::UnknownPid { pid: N, .. }` — one label per
///   distinct PID, enabling per-PID alerting
/// - a short category string (`unknown_conversation`, `version_mismatch`, etc.)
///   for all other reason variants
///
/// [`process_initiated`]: EngineMetrics::process_initiated
/// [`process_completed`]: EngineMetrics::process_completed
/// [`validation_failed`]: EngineMetrics::validation_failed
/// [`outbox_delivery_attempted`]: EngineMetrics::outbox_delivery_attempted
/// [`deadline_fired`]: EngineMetrics::deadline_fired
/// [`dead_letter_recorded`]: EngineMetrics::dead_letter_recorded
pub struct EngineMetrics {
    /// `makod_process_initiated_total{family}` — incremented when a new
    /// process is spawned via `Process::execute(InitiateXxx)`.
    process_initiated: MetricVec,

    /// `makod_process_completed_total{family,result}` — incremented when a
    /// process reaches a terminal state.
    process_completed: MetricVec,

    /// `makod_validation_failed_total{message_type,release}` — incremented
    /// when an inbound EDIFACT message fails AHB validation.
    validation_failed: MetricVec,

    /// `makod_outbox_delivery_attempts_total{result}` — incremented by the
    /// AS4 sender on every delivery attempt.
    outbox_delivery_attempts: MetricVec,

    /// `makod_deadline_fired_total{family}` — incremented when a deadline
    /// scheduler fires a `TimeoutExpired` command.
    deadline_fired: MetricVec,

    /// `makod_dead_letter_recorded_total{reason}` — incremented when a message
    /// is sent to the dead-letter sink.
    dead_letter_recorded: MetricVec,
}

impl EngineMetrics {
    fn new() -> Self {
        Self {
            process_initiated: MetricVec::default(),
            process_completed: MetricVec::default(),
            validation_failed: MetricVec::default(),
            outbox_delivery_attempts: MetricVec::default(),
            deadline_fired: MetricVec::default(),
            dead_letter_recorded: MetricVec::default(),
        }
    }

    /// Return the process-global [`EngineMetrics`] instance.
    ///
    /// The instance is initialised lazily on first call. Subsequent calls
    /// return the same instance with zero allocation.
    #[must_use]
    pub fn global() -> &'static Self {
        static GLOBAL: OnceLock<EngineMetrics> = OnceLock::new();
        GLOBAL.get_or_init(Self::new)
    }

    // ── Increment methods ─────────────────────────────────────────────────────

    /// Increment `makod_process_initiated_total{family=<family>}`.
    ///
    /// Call once when a domain workflow receives its first initiating command
    /// (e.g. `LfAnmeldungCommand::InitiateAnmeldung`).
    ///
    /// `family` is the [`EngineModule::name`] value (`"gpke"`, `"wim"`, etc.).
    ///
    /// [`EngineModule::name`]: crate::builder::EngineModule::name
    pub fn process_initiated(&self, family: &str) {
        self.process_initiated.increment(family);
    }

    /// Increment `makod_process_completed_total{family=<family>,result=<result>}`.
    ///
    /// Call once when a workflow transitions to a **terminal state**
    /// (`Active`, `Rejected`, timeout, or cancellation).
    pub fn process_completed(&self, family: &str, outcome: ProcessOutcome) {
        let label = format!("{family},{}", outcome.label());
        self.process_completed.increment(&label);
    }

    /// Increment `makod_validation_failed_total{message_type=<type>,release=<rel>}`.
    ///
    /// Call when an inbound message fails `validate()` or `validate_against()`.
    pub fn validation_failed(&self, message_type: &str, release: &str) {
        let label = format!("{message_type},{release}");
        self.validation_failed.increment(&label);
    }

    /// Increment `makod_outbox_delivery_attempts_total{result=<result>}`.
    ///
    /// Call in the AS4 sender after every delivery attempt.
    /// `result` should be one of `"ok"`, `"transport_error"`, `"partner_unknown"`.
    pub fn outbox_delivery_attempted(&self, result: &str) {
        self.outbox_delivery_attempts.increment(result);
    }

    /// Increment `makod_deadline_fired_total{family=<family>}`.
    ///
    /// Call in the deadline scheduler when it dispatches a `TimeoutExpired`.
    pub fn deadline_fired(&self, family: &str) {
        self.deadline_fired.increment(family);
    }

    /// Increment `makod_dead_letter_recorded_total{reason=<reason>}`.
    ///
    /// Call in the dead-letter sink when `reject()` is invoked.
    /// `reason` should match [`DeadLetterReason`]'s label string.
    ///
    /// [`DeadLetterReason`]: crate::dead_letter::DeadLetterReason
    pub fn dead_letter_recorded(&self, reason: &str) {
        self.dead_letter_recorded.increment(reason);
    }

    // ── Snapshot ──────────────────────────────────────────────────────────────

    /// Return a snapshot of all counters as a [`MetricsSnapshot`].
    ///
    /// This is a **read-only** operation that does not reset any counters.
    /// Counters are monotonically increasing; Prometheus's `rate()` handles
    /// counter resets on process restart automatically.
    #[must_use]
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            process_initiated: self.process_initiated.snapshot(),
            process_completed: self.process_completed.snapshot(),
            validation_failed: self.validation_failed.snapshot(),
            outbox_delivery_attempts: self.outbox_delivery_attempts.snapshot(),
            deadline_fired: self.deadline_fired.snapshot(),
            dead_letter_recorded: self.dead_letter_recorded.snapshot(),
        }
    }
}

// ── MetricsSnapshot ───────────────────────────────────────────────────────────

/// A point-in-time snapshot of all [`EngineMetrics`] counters.
///
/// Obtained via [`EngineMetrics::snapshot()`]. All fields are `Vec` of
/// `(label, count)` pairs sorted by label for deterministic Prometheus output.
///
/// The `label` field uses a `","` separator for multi-label metrics
/// (e.g. `"gpke,accepted"` for `{family="gpke",result="accepted"}`).
/// The [`render_prometheus`] function splits them appropriately.
///
/// [`render_prometheus`]: MetricsSnapshot::render_prometheus
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    /// `(family, count)` pairs for `makod_process_initiated_total`.
    pub process_initiated: Vec<(Box<str>, u64)>,
    /// `("family,result", count)` pairs for `makod_process_completed_total`.
    pub process_completed: Vec<(Box<str>, u64)>,
    /// `("message_type,release", count)` pairs for `makod_validation_failed_total`.
    pub validation_failed: Vec<(Box<str>, u64)>,
    /// `(result, count)` pairs for `makod_outbox_delivery_attempts_total`.
    pub outbox_delivery_attempts: Vec<(Box<str>, u64)>,
    /// `(family, count)` pairs for `makod_deadline_fired_total`.
    pub deadline_fired: Vec<(Box<str>, u64)>,
    /// `(reason, count)` pairs for `makod_dead_letter_recorded_total`.
    pub dead_letter_recorded: Vec<(Box<str>, u64)>,
}

impl MetricsSnapshot {
    /// Render this snapshot to Prometheus text exposition format (v0.0.4).
    ///
    /// The output follows the format:
    /// ```text
    /// # HELP <metric_name> <description>
    /// # TYPE <metric_name> counter
    /// <metric_name>{<labels>} <value>
    /// ```
    ///
    /// Multi-label metrics use a `","` separator in the internal label string,
    /// which is split into separate `key="value"` pairs in the output.
    #[must_use]
    pub fn render_prometheus(&self) -> String {
        let mut out = String::with_capacity(4096);

        Self::write_counter_vec(
            &mut out,
            "makod_process_initiated_total",
            "Total number of MaKo process instances initiated, by process family.",
            &["family"],
            &self.process_initiated,
        );
        Self::write_counter_vec(
            &mut out,
            "makod_process_completed_total",
            "Total number of MaKo process instances that reached a terminal state.",
            &["family", "result"],
            &self.process_completed,
        );
        Self::write_counter_vec(
            &mut out,
            "makod_validation_failed_total",
            "Total number of inbound EDIFACT messages that failed AHB validation.",
            &["message_type", "release"],
            &self.validation_failed,
        );
        Self::write_counter_vec(
            &mut out,
            "makod_outbox_delivery_attempts_total",
            "Total number of AS4 outbox delivery attempts.",
            &["result"],
            &self.outbox_delivery_attempts,
        );
        Self::write_counter_vec(
            &mut out,
            "makod_deadline_fired_total",
            "Total number of regulatory deadlines fired (TimeoutExpired dispatched).",
            &["family"],
            &self.deadline_fired,
        );
        Self::write_counter_vec(
            &mut out,
            "makod_dead_letter_recorded_total",
            "Total number of messages sent to the durable dead-letter sink.",
            &["reason"],
            &self.dead_letter_recorded,
        );

        out
    }

    /// Write a `counter` metric family to `out`.
    ///
    /// `label_names` specifies the label key names in order.  Each entry in
    /// `pairs` has a label value that is either a bare string (single-label
    /// metrics) or a `","` separated string (multi-label metrics, split in
    /// order of `label_names`).
    fn write_counter_vec(
        out: &mut String,
        name: &str,
        help: &str,
        label_names: &[&str],
        pairs: &[(Box<str>, u64)],
    ) {
        if pairs.is_empty() {
            return;
        }
        out.push_str("# HELP ");
        out.push_str(name);
        out.push(' ');
        out.push_str(help);
        out.push('\n');
        out.push_str("# TYPE ");
        out.push_str(name);
        out.push_str(" counter\n");

        for (label_str, count) in pairs {
            let values: Vec<&str> = label_str.splitn(label_names.len(), ',').collect();
            out.push_str(name);
            out.push('{');
            for (i, (key, val)) in label_names.iter().zip(values.iter()).enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(key);
                out.push_str("=\"");
                // Escape backslash, double-quote, and newline per Prometheus spec.
                for ch in val.chars() {
                    match ch {
                        '\\' => out.push_str(r"\\"),
                        '"' => out.push_str(r#"\""#),
                        '\n' => out.push_str(r"\n"),
                        _ => out.push(ch),
                    }
                }
                out.push('"');
            }
            out.push_str("} ");
            let _ = std::fmt::Write::write_fmt(out, format_args!("{count}"));
            out.push('\n');
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_metrics() -> EngineMetrics {
        EngineMetrics::new()
    }

    #[test]
    fn process_initiated_increments_by_family() {
        let m = fresh_metrics();
        m.process_initiated("gpke");
        m.process_initiated("gpke");
        m.process_initiated("wim");

        let snap = m.snapshot();
        assert_eq!(snap.process_initiated.len(), 2);

        let gpke = snap
            .process_initiated
            .iter()
            .find(|(k, _)| k.as_ref() == "gpke");
        assert_eq!(gpke.map(|(_, v)| *v), Some(2));

        let wim = snap
            .process_initiated
            .iter()
            .find(|(k, _)| k.as_ref() == "wim");
        assert_eq!(wim.map(|(_, v)| *v), Some(1));
    }

    #[test]
    fn process_completed_uses_composite_label() {
        let m = fresh_metrics();
        m.process_completed("gpke", ProcessOutcome::Accepted);
        m.process_completed("gpke", ProcessOutcome::Rejected);
        m.process_completed("gpke", ProcessOutcome::Accepted);
        m.process_completed("wim", ProcessOutcome::Timeout);

        let snap = m.snapshot();
        let accepted = snap
            .process_completed
            .iter()
            .find(|(k, _)| k.as_ref() == "gpke,accepted");
        assert_eq!(accepted.map(|(_, v)| *v), Some(2));

        let timeout = snap
            .process_completed
            .iter()
            .find(|(k, _)| k.as_ref() == "wim,timeout");
        assert_eq!(timeout.map(|(_, v)| *v), Some(1));
    }

    #[test]
    fn snapshot_returns_zero_for_unincremented_metric() {
        let m = fresh_metrics();
        // No increments — snapshot should be empty.
        let snap = m.snapshot();
        assert!(snap.process_initiated.is_empty());
        assert!(snap.process_completed.is_empty());
    }

    #[test]
    fn render_prometheus_omits_empty_metric_families() {
        let m = fresh_metrics();
        m.process_initiated("gpke");

        let output = m.snapshot().render_prometheus();

        // Only the incremented family should appear.
        assert!(
            output.contains("makod_process_initiated_total"),
            "initiated must appear"
        );
        assert!(
            !output.contains("makod_process_completed_total"),
            "completed must be absent"
        );
        assert!(
            !output.contains("makod_validation_failed_total"),
            "validation must be absent"
        );
    }

    #[test]
    fn render_prometheus_formats_labels_correctly() {
        let m = fresh_metrics();
        m.process_initiated("gpke");
        m.process_completed("gpke", ProcessOutcome::Accepted);
        m.validation_failed("utilmd", "S2.1");

        let output = m.snapshot().render_prometheus();

        assert!(
            output.contains(r#"makod_process_initiated_total{family="gpke"} 1"#),
            "single-label format must match; output:\n{output}"
        );
        assert!(
            output.contains(r#"makod_process_completed_total{family="gpke",result="accepted"} 1"#),
            "two-label format must match; output:\n{output}"
        );
        assert!(
            output.contains(
                r#"makod_validation_failed_total{message_type="utilmd",release="S2.1"} 1"#
            ),
            "message_type+release format must match; output:\n{output}"
        );
    }

    #[test]
    fn render_prometheus_escapes_special_chars_in_label_values() {
        let m = fresh_metrics();
        // Inject a label value with a backslash and a double-quote.
        m.outbox_delivery_attempted("ok");
        m.dead_letter_recorded("unknown_pid:13002");

        let output = m.snapshot().render_prometheus();
        assert!(
            output.contains(r#"result="ok""#),
            "plain label must survive; output:\n{output}"
        );
        assert!(
            output.contains(r#"reason="unknown_pid:13002""#),
            "reason label must survive; output:\n{output}"
        );
    }

    #[test]
    fn counters_are_monotonically_increasing() {
        let m = fresh_metrics();
        for _ in 0..100 {
            m.deadline_fired("gpke");
        }
        let snap = m.snapshot();
        let gpke = snap
            .deadline_fired
            .iter()
            .find(|(k, _)| k.as_ref() == "gpke");
        assert_eq!(gpke.map(|(_, v)| *v), Some(100));
    }

    #[test]
    fn snapshot_sorted_by_label() {
        let m = fresh_metrics();
        // Insert in reverse order to verify sort.
        m.process_initiated("wim");
        m.process_initiated("mabis");
        m.process_initiated("geli-gas");
        m.process_initiated("gpke");

        let snap = m.snapshot();
        let labels: Vec<&str> = snap
            .process_initiated
            .iter()
            .map(|(k, _)| k.as_ref())
            .collect();
        let mut sorted = labels.clone();
        sorted.sort_unstable();
        assert_eq!(labels, sorted, "snapshot must be sorted by label");
    }
}

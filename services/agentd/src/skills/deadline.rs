//! APERAK / response-window deadline triage — arithmetic, not inference.
//!
//! `obsd` already knows which processes are past their Frist: it computes the
//! deadline with the same BDEW Werktage calendar this service uses, and
//! `list_overdue_processes` returns them ordered most-urgent-first with
//! `deadline_at` and `partner_mp_id` on every row.
//!
//! What remains is a subtraction and three comparisons. This module is those.
//!
//! ## Severity
//!
//! The bands, stated where they can be tested rather than in a prompt:
//!
//! | Band | Condition |
//! |---|---|
//! | `BREACH` | the deadline has passed |
//! | `CRITICAL` | under 30 minutes remain |
//! | `WARNING` | under 2 hours remain |
//! | `COMPLIANT` | more than 2 hours remain |
//!
//! ## What the skill classifies
//!
//! The union of the **triggering event** and `obsd`'s overdue list, deduplicated
//! by `process_id`; the alert's own row wins, because its `due_at` came from the
//! event that caused this run.
//!
//! Both halves are needed. `list_overdue_processes` returns only rows already
//! past their Frist, so classifying it alone answers `BREACH` every time and
//! leaves the other three bands unreachable — while saying nothing about the
//! process the run was woken for, whose `de.obs.deadline.approaching` carries a
//! deadline still in the future.

use std::collections::BTreeMap;

use agentplane::core::{Outcome, Skill, SkillDescriptor, SkillError, Tainted, Timestamp};
use agentplane::runtime::StepCtx;
use agentplane::tools::ToolId;
use serde_json::{Value, json};

/// Under this much time remaining, a Frist is `CRITICAL`.
const CRITICAL_SECS: i64 = 30 * 60;
/// Under this much, `WARNING`.
const WARNING_SECS: i64 = 2 * 60 * 60;

/// Deadline triage for `de.obs.deadline.approaching`, `de.mako.aperak.timeout`
/// and `de.mako.process.failed`.
#[derive(Debug, Default)]
pub struct DeadlineTriage;

impl DeadlineTriage {
    /// The capability this skill provides, matching the manifest's
    /// `spec.capabilities.provides`.
    pub const CAPABILITY: &'static str = "deadline.alert";
    /// The skill name, matching `metadata.name`.
    pub const NAME: &'static str = "deadline-alert-agent";

    /// The skill holds nothing — no catalogue, no transport.
    /// `StepCtx::call_tool` dispatches through the plane's own catalogue, the
    /// one derived from the manifests, so this skill's reach is provably its
    /// manifest's reach.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// How urgent one overdue process is.
///
/// Ordered worst-first so a `max` over the set is the run's headline severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// More than two hours remain.
    Compliant,
    /// Under two hours remain.
    Warning,
    /// Under thirty minutes remain.
    Critical,
    /// The deadline has passed.
    Breach,
}

impl Severity {
    /// How many bands there are. Named so the run's own note cannot claim a
    /// different number from the enum — it said "3" while there were four.
    pub const COUNT: usize = 4;

    /// The wire spelling, as the alert carries it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Compliant => "COMPLIANT",
            Self::Warning => "WARNING",
            Self::Critical => "CRITICAL",
            Self::Breach => "BREACH",
        }
    }
}

/// Classify one deadline against the current instant.
///
/// Split out from the effectful path precisely so it can be tested without a
/// runtime, a model, or a network — the property the prompt version could not
/// have.
#[must_use]
pub fn classify(remaining_secs: i64) -> Severity {
    if remaining_secs < 0 {
        Severity::Breach
    } else if remaining_secs < CRITICAL_SECS {
        Severity::Critical
    } else if remaining_secs < WARNING_SECS {
        Severity::Warning
    } else {
        Severity::Compliant
    }
}

/// The triggering event as a classifiable row, when it names a deadline.
///
/// `de.obs.deadline.approaching` carries `due_at`, not `deadline_at` — the
/// field this skill classifies on — so the shapes are reconciled here rather
/// than by hoping two services agree on a key. Anything without a readable
/// deadline yields `None` and the event is simply reported back as the trigger.
fn trigger_as_row(payload: &Value) -> Option<Value> {
    let due_at = payload.get("due_at").and_then(Value::as_str)?;
    Some(json!({
        "process_id":      payload.get("process_id"),
        "pid":             payload.get("pid"),
        "state":           payload.get("state"),
        "partner_mp_id":   payload.get("partner_mp_id"),
        "deadline_at":     due_at,
        "deadline_source": payload.get("deadline_source"),
    }))
}

/// Seconds from `now` until `deadline`, or `None` when the timestamp is
/// unparseable.
///
/// A row whose `deadline_at` mako cannot read is **not** silently treated as
/// compliant: the caller counts it as unclassifiable and says so. Guessing the
/// safe direction here would hide an obsd projection bug behind a green alert.
fn remaining_secs(now: Timestamp, deadline: &str) -> Option<i64> {
    use time::format_description::well_known::Rfc3339;
    let parsed = Timestamp::parse(deadline, &Rfc3339).ok()?;
    Some(parsed.unix_timestamp() - now.unix_timestamp())
}

#[async_trait::async_trait]
impl Skill for DeadlineTriage {
    fn descriptor(&self) -> SkillDescriptor {
        SkillDescriptor::new(Self::NAME).provides(Self::CAPABILITY)
    }

    async fn invoke(
        &self,
        cx: &mut StepCtx<'_>,
        input: Tainted<Value>,
    ) -> Result<Outcome, SkillError> {
        // The tool call is a governed, journaled effect exactly as it would be
        // from a model's turn: same catalogue, same policy gate, same replay.
        // What is gone is the turn.
        //
        // `call_tool` goes through the plane's catalogue — the one `try_build`
        // checked against every manifest — so this skill's reach is provably its
        // manifest's reach, and cannot drift from it.
        let overdue = cx
            .call_tool(
                ToolId::new("obsd", "list_overdue_processes"),
                Tainted::trusted(json!({})),
            )
            .await
            .map_err(|e| SkillError::Other(format!("obsd/list_overdue_processes: {e}")))?;

        // The clock is an effect too, so a replay classifies against the instant
        // the original run saw. A wall-clock read here would make a replayed run
        // reach a different severity and report divergence against itself.
        let now = cx
            .now()
            .await
            .map_err(|e| SkillError::Other(format!("clock: {e}")))?;

        let mut rows = overdue
            .peek()
            .get("overdue")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        // The triggering event is a row too. `de.obs.deadline.approaching`
        // carries the deadline it is warning about, and it is the reason this
        // run exists — answering only about *other* processes leaves the alert
        // unanswered.
        //
        // It goes first so the dedup below keeps its `due_at`: the event's
        // instant came from the projection at alert time and the tool's from
        // whenever it ran.
        if let Some(trigger_row) = trigger_as_row(input.peek()) {
            rows.insert(0, trigger_row);
        }

        let mut worst = Severity::Compliant;
        let mut by_partner: BTreeMap<String, usize> = BTreeMap::new();
        let mut classified: Vec<Value> = Vec::with_capacity(rows.len());
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut unclassifiable = 0usize;

        for row in &rows {
            // One verdict per process. The alert and the overdue list overlap
            // whenever the warned process has since breached, and counting it
            // twice would double it in `by_partner_mp_id`.
            if let Some(id) = row.get("process_id").and_then(Value::as_str)
                && !seen.insert(id.to_owned())
            {
                continue;
            }
            let deadline_at = row.get("deadline_at").and_then(Value::as_str);
            let Some(secs) = deadline_at.and_then(|d| remaining_secs(now, d)) else {
                unclassifiable += 1;
                continue;
            };
            let severity = classify(secs);
            worst = worst.max(severity);

            // "Identify the responsible market participant" was a step in the
            // prompt. It is a field.
            let partner = row
                .get("partner_mp_id")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            *by_partner.entry(partner.to_owned()).or_default() += 1;

            classified.push(json!({
                "process_id":      row.get("process_id"),
                "pid":             row.get("pid"),
                "state":           row.get("state"),
                "partner_mp_id":   partner,
                "deadline_at":     deadline_at,
                // The Festlegung the instant came from, where obsd supplied one.
                // A recommendation that cites the rule beats one that asserts a
                // number.
                "deadline_source": row.get("deadline_source"),
                "remaining_secs":  secs,
                "severity":        severity.as_str(),
            }));
        }

        // Adjacent to the effects it explains, so reasoning-versus-action
        // mismatch stays detectable after the fact.
        // `obsd` caps its overdue list and says when the cap bit. A run that
        // reported 500 of ten thousand as though it were the whole picture would
        // be wrong in the calmest possible way.
        let saturated = overdue
            .peek()
            .get("saturated")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        cx.note(format!(
            "classified {} process(es) against {} severity band(s); worst = {}{}{}",
            classified.len(),
            Severity::COUNT,
            worst.as_str(),
            if unclassifiable > 0 {
                format!("; {unclassifiable} row(s) had an unreadable deadline_at")
            } else {
                String::new()
            },
            if saturated {
                "; obsd's overdue list was saturated — at least the cap was waiting"
            } else {
                ""
            }
        ))
        .await
        .map_err(|e| SkillError::Other(format!("note: {e}")))?;

        // The triggering event is reported back verbatim so a consumer can
        // correlate the alert with what caused it, and it keeps the input's own
        // label rather than being promoted on the way through.
        let trigger = input.peek().clone();

        Ok(Outcome::done(overdue.map(|_| {
            json!({
                "deadline_status":  worst.as_str(),
                "at_risk_count":    classified.len(),
                "unclassifiable":   unclassifiable,
                // `true` means obsd returned a full batch: at least the cap was
                // waiting, never that the cap was all there was.
                "truncated":        saturated,
                "by_partner_mp_id": by_partner,
                "processes":        classified,
                "trigger":          trigger,
            })
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The bands, at and around every boundary.
    ///
    /// The classification is code, so "is 29 minutes CRITICAL" is answerable
    /// without calling a model.
    #[test]
    fn severity_bands_are_exact_at_their_boundaries() {
        assert_eq!(classify(-1), Severity::Breach);
        assert_eq!(classify(-86_400), Severity::Breach);
        // Zero remaining has not passed yet.
        assert_eq!(classify(0), Severity::Critical);
        assert_eq!(classify(29 * 60), Severity::Critical);
        assert_eq!(classify(CRITICAL_SECS - 1), Severity::Critical);
        assert_eq!(classify(CRITICAL_SECS), Severity::Warning);
        assert_eq!(classify(WARNING_SECS - 1), Severity::Warning);
        assert_eq!(classify(WARNING_SECS), Severity::Compliant);
        assert_eq!(classify(i64::MAX), Severity::Compliant);
    }

    /// Worst-first ordering is what makes `max` the headline severity.
    #[test]
    fn severity_orders_worst_last_so_max_is_the_headline() {
        assert!(Severity::Breach > Severity::Critical);
        assert!(Severity::Critical > Severity::Warning);
        assert!(Severity::Warning > Severity::Compliant);

        let set = [Severity::Warning, Severity::Breach, Severity::Compliant];
        assert_eq!(
            set.into_iter().max().expect("non-empty"),
            Severity::Breach,
            "one breach in the set makes the run a breach"
        );
    }

    /// An unreadable `deadline_at` is not compliance.
    #[test]
    fn an_unparseable_deadline_is_not_classified() {
        let now = Timestamp::from_unix_timestamp(1_800_000_000).expect("valid instant");
        assert_eq!(remaining_secs(now, "not a timestamp"), None);
        assert_eq!(remaining_secs(now, ""), None);
        // A `time` component array is not a timestamp either — the guard that
        // keeps one off a wire is `xtask check-wire-timestamps`.
        assert_eq!(remaining_secs(now, "[2027,15,8,0,0,0,0,0,0]"), None);
    }

    /// The triggering alert is classified, not just echoed back.
    ///
    /// `obsd`'s overdue list is past its Frist by construction, so classifying
    /// it alone answers `BREACH` every time and makes the other three bands
    /// unreachable — while saying nothing about the process the run was woken
    /// for.
    #[test]
    fn the_triggering_alert_becomes_a_classifiable_row() {
        let row = trigger_as_row(&json!({
            "process_id":      "6f1c2a3e-0000-4000-8000-000000000001",
            "pid":             55001,
            "partner_mp_id":   "9900357000004",
            "due_at":          "2027-01-15T08:00:00Z",
            "deadline_source": "BK6-24-174 GPKE Teil 2, SD Lieferbeginn",
        }))
        .expect("an alert carrying due_at is a row");

        assert_eq!(
            row.get("deadline_at").and_then(Value::as_str),
            Some("2027-01-15T08:00:00Z"),
            "`due_at` on the event is `deadline_at` to the classifier"
        );
        assert_eq!(
            row.get("deadline_source").and_then(Value::as_str),
            Some("BK6-24-174 GPKE Teil 2, SD Lieferbeginn"),
            "the Festlegung travels into the recommendation"
        );
    }

    /// An event that names no deadline is not invented into one.
    #[test]
    fn an_event_without_a_deadline_is_not_a_row() {
        assert!(trigger_as_row(&json!({ "process_id": "x" })).is_none());
        assert!(trigger_as_row(&json!("a string")).is_none());
        assert!(trigger_as_row(&json!({ "due_at": 1_700_000_000 })).is_none());
    }

    /// The band count in the run's note is the enum's, not a literal.
    ///
    /// It said "3" while there were four, which is the smallest possible version
    /// of the failure this whole module exists to prevent: a threshold stated in
    /// prose disagreeing with the code that applies it.
    #[test]
    fn the_band_count_matches_the_enum() {
        let bands = [
            Severity::Compliant,
            Severity::Warning,
            Severity::Critical,
            Severity::Breach,
        ];
        assert_eq!(Severity::COUNT, bands.len());
        let spellings: std::collections::BTreeSet<_> = bands.iter().map(|b| b.as_str()).collect();
        assert_eq!(spellings.len(), Severity::COUNT, "each band is distinct");
    }

    /// The arithmetic, in both directions.
    #[test]
    fn remaining_seconds_are_signed_around_now() {
        let now = Timestamp::from_unix_timestamp(1_800_000_000).expect("valid instant");
        // 2026-01-15T08:00:00Z is well before that instant.
        let past = remaining_secs(now, "2026-01-15T08:00:00Z").expect("parses");
        assert!(past < 0, "a passed deadline is negative, got {past}");
        assert_eq!(classify(past), Severity::Breach);

        let future = remaining_secs(now, "2087-01-15T08:00:00Z").expect("parses");
        assert!(future > 0, "a future deadline is positive, got {future}");
        assert_eq!(classify(future), Severity::Compliant);
    }
}

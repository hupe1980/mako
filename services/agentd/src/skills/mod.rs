//! Specialists whose work is computation, written as code rather than prompts.
//!
//! A specialist belongs here when its procedure is a total function of data the
//! tools return: arithmetic, threshold classification, field extraction, set
//! logic. The deadline monitor is the model case — "< 30 min remaining is
//! CRITICAL" is a subtraction and a comparison. As a prompt it costs a model
//! call per deadline event, can apply a threshold differently next time, and
//! cannot be regression-tested in CI; as code it is cheap, deterministic and
//! testable.
//!
//! Judgement over open-ended input — a counterparty's free-text objection, an
//! unfamiliar failure, an operator narrative — keeps its model.
//!
//! Code keeps everything the runtime provides: the tool call is still a
//! journaled effect dispatched through the policy gate, the clock read is still
//! an effect so a replay sees the original instant, and the manifest still
//! governs grants, ceilings and egress. A specialist here writes `models: {}`
//! in its manifest — agentplane's *no inference at all, declared on purpose*,
//! distinct from model wiring somebody forgot.
//!
//! # Reaching a tool
//!
//! Through [`StepCtx::call_tool`](agentplane::runtime::StepCtx::call_tool),
//! which dispatches via the plane's own catalogue — the one `try_build` derived
//! from the manifests and checked against them. A skill holds no catalogue and
//! no transport, so its reach is provably its manifest's reach: a private
//! catalogue could grant reach the manifest never described, or laxer reach (a
//! `read_only` entry for a tool the manifest calls mutating would skip the
//! whole-value taint gate and add retry-on-timeout).

use agentplane::runtime::Agent;

pub mod deadline;
pub mod gabi;

pub use deadline::DeadlineTriage;
pub use gabi::GabiAllocationTriage;

/// Every specialist whose conduct is Rust — the one list of them.
///
/// A name here is a manifest with no `execution` block whose behaviour is a
/// skill in this module, registered in `Plane::new`, and whose worklist row
/// text is a builder this module owns. Each coded specialist opens a row, so
/// this is also the list of coded specialists that triage. `skills::tests`
/// holds it equal to the manifests without `execution`, in both directions.
pub const CODED: &[&str] = &[DeadlineTriage::NAME, GabiAllocationTriage::NAME];

/// Give `agent` the coded skill a [`CODED`] name runs.
///
/// No wiring: `StepCtx::call_tool` dispatches through the plane's own
/// catalogue, which `try_build` derived from the manifests and checked against
/// them. A skill that carried its own could grant itself reach the declaration
/// never described.
///
/// # Errors
///
/// `agent` back, unchanged, when `name` runs no coded skill. `Plane::new`
/// refuses to assemble on that answer for a [`CODED`] name, and
/// `skills::tests` holds the arms here equal to [`CODED`].
#[must_use = "the agent carries the skill only in the returned value"]
pub fn register(name: &str, agent: Agent) -> Result<Agent, Agent> {
    match name {
        DeadlineTriage::NAME => Ok(agent.skill(DeadlineTriage::new())),
        GabiAllocationTriage::NAME => Ok(agent.skill(GabiAllocationTriage::new())),
        _ => Err(agent),
    }
}

#[cfg(test)]
mod tests {
    use super::{CODED, DeadlineTriage, GabiAllocationTriage, deadline, gabi, register};
    use crate::plane::{MACHINE_GENERATED, find_manifest, manifests};
    use agentplane::runtime::Agent;

    /// [`CODED`] is exactly the manifests that declare no `execution`.
    ///
    /// Both directions, because each failure is silent at runtime: a manifest
    /// with no `execution` missing from the list has no skill registered and
    /// finds nothing to invoke at first dispatch; a listed name whose manifest
    /// declares `execution` registers a skill the runtime never runs; a listed
    /// name with no manifest at all registers nothing and hides a rename.
    #[test]
    fn the_coded_list_is_exactly_the_manifests_without_execution() {
        let unlisted: Vec<&str> = manifests()
            .iter()
            .filter(|(name, m)| m.spec.execution.is_none() && !CODED.contains(&name.as_str()))
            .map(|(name, _)| name.as_str())
            .collect();
        assert!(
            unlisted.is_empty(),
            "these manifests declare no `execution` and are not in `skills::CODED`: {unlisted:?}"
        );
        for name in CODED {
            let m = find_manifest(name)
                .unwrap_or_else(|| panic!("`skills::CODED` names {name}, which has no manifest"));
            assert!(
                m.spec.execution.is_none(),
                "`skills::CODED` names {name}, whose manifest declares `execution`"
            );
        }
        let mut seen = std::collections::BTreeSet::new();
        assert!(
            CODED.iter().all(|n| seen.insert(*n)),
            "`skills::CODED` names a specialist twice: {CODED:?}"
        );
    }

    /// Every text a coded skill can file as a worklist row, keyed by the
    /// specialist that files it — the summary builders the skills themselves
    /// call, not copies of their sentences.
    fn row_texts(name: &str) -> Option<Vec<String>> {
        match name {
            DeadlineTriage::NAME => Some(vec![
                deadline::breach_summary(1, 0),
                deadline::breach_summary(3, 2),
            ]),
            GabiAllocationTriage::NAME => Some(vec![gabi::TRIAGE_SUMMARY.to_owned()]),
            _ => None,
        }
    }

    /// [`register`] attaches a skill for exactly the [`CODED`] names.
    ///
    /// A [`CODED`] name without an arm makes `Plane::new` refuse to assemble;
    /// an arm for a name outside [`CODED`] is a skill `Plane::new` never asks
    /// for, beside a manifest whose `execution` the runtime runs instead.
    #[test]
    fn a_skill_is_registered_for_every_coded_name_and_no_other() {
        for (name, m) in manifests() {
            let registered = register(name, Agent::new(m)).is_ok();
            assert_eq!(
                registered,
                CODED.contains(&name.as_str()),
                "{name}: `skills::register` {} skill, and `skills::CODED` {} it",
                if registered {
                    "attaches a"
                } else {
                    "attaches no"
                },
                if CODED.contains(&name.as_str()) {
                    "names"
                } else {
                    "does not name"
                },
            );
        }
    }

    /// No coded specialist's row says a model wrote it.
    ///
    /// Complete over the catalog: [`CODED`] is every manifest without
    /// `execution`, and each must name its row texts here. A coded specialist
    /// missing from [`row_texts`] fails rather than passing unexamined, and a row text opening with [`MACHINE_GENERATED`] is a false
    /// disclosure — the worse direction, because a marking on rows no model
    /// touched is a marking an operator learns to skip.
    #[test]
    fn no_coded_specialists_row_carries_the_machine_generated_disclosure() {
        for name in CODED {
            let texts = row_texts(name).unwrap_or_else(|| {
                panic!(
                    "{name} is a coded specialist whose row texts this test does not know; \
                     name its summary builder in `row_texts`"
                )
            });
            for text in texts {
                assert!(
                    !text.starts_with(MACHINE_GENERATED),
                    "{name} files a row disclosed as AI-generated, and no model wrote it: {text}"
                );
                assert!(!text.is_empty(), "{name} files an empty row");
            }
        }
    }
}

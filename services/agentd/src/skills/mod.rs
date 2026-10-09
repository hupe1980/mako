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

pub mod deadline;
pub mod gabi;

pub use deadline::DeadlineTriage;
pub use gabi::GabiAllocationTriage;

//! Specialists whose work is computation, written as code rather than prompts.
//!
//! # Why this module exists
//!
//! An agent runtime makes a model's actions survivable. It does not make a model
//! the right tool for every job, and the cutover to `agentplane` quietly assumed
//! it was: twenty-seven of twenty-eight specialists were declared
//! `execution: { kind: tool-calling }`, including several whose entire procedure
//! is arithmetic over a field another mako service had already computed.
//!
//! The clearest case was the deadline monitor. Its prompt asked a model to
//! "classify by severity: < 30 min remaining CRITICAL, < 2h WARNING, already
//! overdue BREACH" and to "identify the responsible market participant" — which
//! is a subtraction, three comparisons, and reading `partner_mp_id` out of the
//! JSON the tool returned. That cost a frontier-model call with an eight-turn
//! ceiling and a 200 000-token budget, and returned **prose** that no consumer
//! could parse.
//!
//! Three things are wrong with that, and only the first is about money:
//!
//! 1. **It is not cheap.** A run per deadline event, at a rate set by how
//!    close mako's counterparties are to their Fristen.
//! 2. **It is not reliable.** A threshold a model applies is a threshold that
//!    can be applied differently next time, and BNetzA monitoring is not a place
//!    to discover that.
//! 3. **It is not testable.** A regression in "is 29 minutes CRITICAL" is only
//!    observable by running the model, which means it is not observable in CI.
//!
//! Code has none of those properties. What it keeps is everything the runtime
//! provides: the tool call is still a journaled effect dispatched through the
//! policy gate, the clock read is still an effect so a replay sees the instant
//! the original run saw, and the manifest still governs the grants, the
//! ceilings and the egress. **Governance is not what the model was buying.**
//!
//! # What belongs here, and what does not
//!
//! A specialist belongs here when its procedure is a total function of data the
//! tools return: arithmetic, threshold classification, field extraction, set
//! logic. It does **not** belong here when the task is judgement over open-ended
//! input — reading a counterparty's free-text objection, diagnosing an
//! unfamiliar failure, writing an operator narrative. Those keep their models,
//! and the audit that produced this module left them alone.
//!
//! The declaration says which is which: a specialist here writes `models: {}` in
//! its manifest, which agentplane documents as *no inference at all, declared on
//! purpose* — the thing that distinguishes a rules-only agent from one whose
//! model wiring somebody forgot.
//!
//! # Reaching a tool
//!
//! Through [`StepCtx::call_tool`](agentplane::runtime::StepCtx::call_tool),
//! which dispatches via the plane's own catalogue — the one `try_build` derived
//! from the manifests and checked against them. A skill therefore holds no
//! catalogue and no transport, and its reach is provably its manifest's reach.
//!
//! A skill that carried its own catalogue would be free to grant itself reach
//! the manifest never described — and worse, reach that is *laxer*: a
//! `read_only` entry for a tool the manifest calls mutating would exempt it
//! from the whole-value taint gate and add retry-on-timeout. Dispatching
//! through the plane removes the choice.

pub mod deadline;
pub mod gabi;

pub use deadline::DeadlineTriage;
pub use gabi::GabiAllocationTriage;

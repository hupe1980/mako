//! [`MessageAdapter`] — cross-format-version message-to-command translation.
//!
//! # Problem
//!
//! A GPKE process started under `FV2025-10-01` may still be in-flight when
//! `FV2026-10-01` goes live.  The counterparty begins sending APERAK messages
//! in the new AHB format before the process completes.  The field that signals
//! acceptance may have moved (e.g. a new qualifier in BGM DE 1001), or an
//! additional mandatory DTM has been added.
//!
//! Without an explicit adapter, each workflow handles this ad-hoc inside its
//! command constructor, making the mapping invisible, untested, and easy to
//! forget when a new release cycle arrives.
//!
//! # Solution
//!
//! `MessageAdapter<W>` is the type-system home for all format-version-specific
//! translation logic.  An adapter declares which format versions it can handle
//! (`accepts_format_version`), receives a parsed `AnyMessage`, and returns the
//! domain command to dispatch.
//!
//! Adapters are registered in an [`AdapterRegistry`] at engine startup, and
//! the registry reports which of the binary's known format versions no adapter
//! claims ([`AdapterRegistry::uncovered_format_versions`]).
//!
//! # Example
//!
//! ```rust,ignore
//! use mako_engine::message_adapter::{AdapterRegistry, MessageAdapter};
//! use mako_engine::version::FormatVersion;
//! use mako_engine::error::EngineError;
//!
//! struct GpkeAperakAdapter;
//!
//! impl MessageAdapter<GpkeWorkflow> for GpkeAperakAdapter {
//!     fn accepts_format_version(&self, fv: &FormatVersion) -> bool {
//!         matches!(fv.as_str(), "FV2025-10-01" | "FV2026-10-01")
//!     }
//!
//!     fn adapt(
//!         &self,
//!         msg: &dyn std::any::Any,
//!         fv: &FormatVersion,
//!     ) -> Result<GpkeCommand, EngineError> {
//!         // parse `msg` as APERAK and construct the appropriate command
//!         Ok(GpkeCommand::ReceiveAperak { positive: true })
//!     }
//! }
//!
//! let mut registry: AdapterRegistry<GpkeWorkflow> = AdapterRegistry::new();
//! registry.register(GpkeAperakAdapter);
//! ```

use crate::{error::EngineError, version::FormatVersion, workflow::Workflow};

// ── MessageAdapter trait ──────────────────────────────────────────────────────

/// Translates a parsed EDIFACT message into a domain command for workflow `W`.
///
/// Implement one `MessageAdapter` per (message type, format version range)
/// combination that your workflow needs to handle.  An adapter that handles
/// multiple format versions via internal branching is also valid.
///
/// # Thread safety
///
/// Adapters must be `Send + Sync + 'static` because they are stored in an
/// [`AdapterRegistry`] that is shared across async tasks.
pub trait MessageAdapter<W: Workflow>: Send + Sync + 'static {
    /// Returns `true` when this adapter can translate messages formatted under
    /// `fv`.
    ///
    /// The [`AdapterRegistry`] calls this during validation to confirm that
    /// every format version the binary knows about is covered.
    fn accepts_format_version(&self, fv: &FormatVersion) -> bool;

    /// Translate a raw parsed message into a domain command.
    ///
    /// `fv` is the format version detected from the wire message (e.g. from
    /// `EdiEnergyMessage::detect_release`).  Use it to select the correct
    /// field mapping when the adapter handles multiple format versions.
    ///
    /// # Errors
    ///
    /// Return [`EngineError::Workflow`] when the message is structurally valid
    /// but semantically inappropriate for this command (e.g. wrong PID).
    ///
    /// Return [`EngineError::Deserialization`] when a required field is absent
    /// or malformed.
    fn adapt(&self, raw: &dyn std::any::Any, fv: &FormatVersion)
    -> Result<W::Command, EngineError>;
}

// ── AdapterRegistry ───────────────────────────────────────────────────────────

/// Runtime registry of [`MessageAdapter`]s for a single workflow type `W`.
///
/// Adapters are registered at startup via [`AdapterRegistry::register`].
/// After all adapters are registered, call
/// [`AdapterRegistry::uncovered_format_versions`] to confirm that every format
/// version the binary knows about is claimed by at least one adapter.
///
/// # Example
///
/// ```rust,ignore
/// use mako_engine::message_adapter::AdapterRegistry;
///
/// let mut registry: AdapterRegistry<MyWorkflow> = AdapterRegistry::new();
/// registry.register(MyFV2025Adapter);
/// registry.register(MyFV2026Adapter);
/// assert!(
///     registry
///         .uncovered_format_versions(&[
///             FormatVersion::new("FV2025-10-01"),
///             FormatVersion::new("FV2026-10-01"),
///         ])
///         .is_empty(),
///     "every format version must have a registered adapter",
/// );
/// ```
pub struct AdapterRegistry<W: Workflow> {
    adapters: Vec<Box<dyn MessageAdapter<W>>>,
}

impl<W: Workflow> Default for AdapterRegistry<W> {
    fn default() -> Self {
        Self::new()
    }
}

impl<W: Workflow> AdapterRegistry<W> {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            adapters: Vec::new(),
        }
    }

    /// Register an adapter.
    ///
    /// Multiple adapters can be registered.  When [`AdapterRegistry::dispatch`]
    /// is called, the first adapter that returns `true` from
    /// `accepts_format_version` is used.  Register the most specific adapters
    /// first.
    pub fn register(&mut self, adapter: impl MessageAdapter<W>) {
        self.adapters.push(Box::new(adapter));
    }

    /// Dispatch `raw` to the first adapter that accepts `fv`.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Workflow`] wrapping
    /// `WorkflowError::other("no adapter registered for format version …")`
    /// when no registered adapter claims `fv`.
    ///
    /// Propagates the adapter's own error otherwise.
    pub fn dispatch(
        &self,
        raw: &dyn std::any::Any,
        fv: &FormatVersion,
    ) -> Result<W::Command, EngineError> {
        for adapter in &self.adapters {
            if adapter.accepts_format_version(fv) {
                return adapter.adapt(raw, fv);
            }
        }
        Err(EngineError::Workflow(crate::error::WorkflowError::other(
            format!("no adapter registered for format version {fv}"),
        )))
    }

    /// Format versions in `known_fvs` that no registered adapter claims.
    ///
    /// `known_fvs` is the set of BDEW format versions the running binary has
    /// profiles for. An empty return value means every one of them can be
    /// parsed into this workflow's command type.
    ///
    /// A non-empty one is a **startup** error, not a runtime one: the gap is a
    /// release the binary knows about and this workflow cannot answer, so the
    /// first counterparty message in that format would dead-letter. `makod`
    /// refuses to boot on it rather than discovering it on the wire.
    ///
    /// The check is deliberately not conditioned on anything a workflow
    /// declares. A process may be started under any known format version and a
    /// counterparty may reply under any later one, so every known format
    /// version has to be covered — there is no narrower set that is safe.
    #[must_use]
    pub fn uncovered_format_versions(&self, known_fvs: &[FormatVersion]) -> Vec<FormatVersion> {
        known_fvs
            .iter()
            .filter(|fv| !self.adapters.iter().any(|a| a.accepts_format_version(fv)))
            .cloned()
            .collect()
    }

    /// Returns the number of registered adapters.
    #[must_use]
    pub fn len(&self) -> usize {
        self.adapters.len()
    }

    /// Returns `true` when no adapters are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.adapters.is_empty()
    }

    /// Returns a list of all format versions for which at least one adapter
    /// returns `true` from `accepts_format_version`, out of the given
    /// `candidate_fvs` set.
    #[must_use]
    pub fn covered_versions<'a>(
        &self,
        candidate_fvs: &'a [FormatVersion],
    ) -> Vec<&'a FormatVersion> {
        candidate_fvs
            .iter()
            .filter(|fv| self.adapters.iter().any(|a| a.accepts_format_version(fv)))
            .collect()
    }
}

impl<W: Workflow> std::fmt::Debug for AdapterRegistry<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdapterRegistry")
            .field("adapters", &self.adapters.len())
            .finish()
    }
}

// ── Blanket impl for closures ─────────────────────────────────────────────────

/// A simple function-based adapter constructed via
/// [`FnAdapter::new`].
///
/// Use this for lightweight adapters that do not need to carry state.
///
/// # Example
///
/// ```rust,ignore
/// use mako_engine::message_adapter::{AdapterRegistry, FnAdapter};
///
/// let mut registry: AdapterRegistry<MyWorkflow> = AdapterRegistry::new();
/// registry.register(FnAdapter::new(
///     |fv| fv.as_str() == "FV2025-10-01",
///     |raw, _fv| {
///         // cast raw and construct command
///         Ok(MyCommand::Received)
///     },
/// ));
/// ```
pub struct FnAdapter<W: Workflow, A, D>
where
    A: Fn(&FormatVersion) -> bool + Send + Sync + 'static,
    D: Fn(&dyn std::any::Any, &FormatVersion) -> Result<W::Command, EngineError>
        + Send
        + Sync
        + 'static,
{
    accepts: A,
    adapt: D,
    _phantom: std::marker::PhantomData<W>,
}

impl<W: Workflow, A, D> FnAdapter<W, A, D>
where
    A: Fn(&FormatVersion) -> bool + Send + Sync + 'static,
    D: Fn(&dyn std::any::Any, &FormatVersion) -> Result<W::Command, EngineError>
        + Send
        + Sync
        + 'static,
{
    /// Construct an adapter from two closures.
    pub fn new(accepts: A, adapt: D) -> Self {
        Self {
            accepts,
            adapt,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<W: Workflow, A, D> MessageAdapter<W> for FnAdapter<W, A, D>
where
    A: Fn(&FormatVersion) -> bool + Send + Sync + 'static,
    D: Fn(&dyn std::any::Any, &FormatVersion) -> Result<W::Command, EngineError>
        + Send
        + Sync
        + 'static,
{
    fn accepts_format_version(&self, fv: &FormatVersion) -> bool {
        (self.accepts)(fv)
    }

    fn adapt(
        &self,
        raw: &dyn std::any::Any,
        fv: &FormatVersion,
    ) -> Result<W::Command, EngineError> {
        (self.adapt)(raw, fv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        error::WorkflowError,
        version::FormatVersion,
        workflow::{CommandPayload, EventPayload, Workflow},
    };

    // ── Minimal test workflow ─────────────────────────────────────────────────

    #[derive(Debug, Default, Clone)]
    struct TestState;

    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    enum TestEvent {
        Fired,
    }
    impl EventPayload for TestEvent {
        fn event_type(&self) -> &'static str {
            "Fired"
        }
    }

    #[derive(Debug)]
    enum TestCommand {
        Fire,
    }
    impl CommandPayload for TestCommand {}

    struct TestWorkflow;
    impl Workflow for TestWorkflow {
        type State = TestState;
        type Event = TestEvent;
        type Command = TestCommand;

        fn apply(state: Self::State, _event: &Self::Event) -> Self::State {
            state
        }
        fn handle(
            _state: &Self::State,
            _cmd: Self::Command,
        ) -> Result<crate::workflow::WorkflowOutput<Self::Event>, WorkflowError> {
            Ok(vec![TestEvent::Fired].into())
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[test]
    fn fn_adapter_accepts_correct_fv() {
        let adapter: FnAdapter<TestWorkflow, _, _> = FnAdapter::new(
            |fv| fv.as_str() == "FV2025-10-01",
            |_raw, _fv| Ok(TestCommand::Fire),
        );
        let fv25 = FormatVersion::new("FV2025-10-01");
        let fv26 = FormatVersion::new("FV2026-10-01");
        assert!(adapter.accepts_format_version(&fv25));
        assert!(!adapter.accepts_format_version(&fv26));
    }

    #[test]
    fn registry_dispatches_to_first_matching_adapter() {
        let mut registry: AdapterRegistry<TestWorkflow> = AdapterRegistry::new();
        registry.register(FnAdapter::new(
            |fv| fv.as_str() == "FV2025-10-01",
            |_raw, _fv| Ok(TestCommand::Fire),
        ));
        let fv = FormatVersion::new("FV2025-10-01");
        // `()` as the "raw" message — the adapter ignores it.
        let result = registry.dispatch(&() as &dyn std::any::Any, &fv);
        assert!(result.is_ok(), "dispatch must succeed for registered FV");
    }

    #[test]
    fn registry_errors_on_unregistered_fv() {
        let registry: AdapterRegistry<TestWorkflow> = AdapterRegistry::new();
        let fv = FormatVersion::new("FV2025-10-01");
        let result = registry.dispatch(&() as &dyn std::any::Any, &fv);
        assert!(result.is_err(), "must return Err for unregistered FV");
    }

    #[test]
    fn every_known_format_version_covered_reports_no_gap() {
        let mut registry: AdapterRegistry<TestWorkflow> = AdapterRegistry::new();
        registry.register(FnAdapter::new(
            |fv| matches!(fv.as_str(), "FV2025-10-01" | "FV2026-10-01"),
            |_raw, _fv| Ok(TestCommand::Fire),
        ));
        let known = vec![
            FormatVersion::new("FV2025-10-01"),
            FormatVersion::new("FV2026-10-01"),
        ];
        assert!(registry.uncovered_format_versions(&known).is_empty());
    }

    #[test]
    fn a_format_version_no_adapter_claims_is_reported() {
        let mut registry: AdapterRegistry<TestWorkflow> = AdapterRegistry::new();
        registry.register(FnAdapter::new(
            |fv| fv.as_str() == "FV2025-10-01",
            |_raw, _fv| Ok(TestCommand::Fire),
        ));
        let known = vec![
            FormatVersion::new("FV2025-10-01"),
            FormatVersion::new("FV2026-10-01"), // no adapter → gap
        ];
        assert_eq!(
            registry.uncovered_format_versions(&known),
            vec![FormatVersion::new("FV2026-10-01")]
        );
    }

    /// An empty `known_fvs` cannot fail, which is why the caller — not this
    /// function — is responsible for supplying the real profile list.
    ///
    /// `makod::startup::validate_adapter_coverage` refuses an empty
    /// `known_fvs()` for exactly this reason: a coverage check whose input is
    /// empty reports "covered" for a registry with no adapters at all.
    #[test]
    fn an_empty_known_set_reports_no_gap_even_for_an_empty_registry() {
        let registry: AdapterRegistry<TestWorkflow> = AdapterRegistry::new();
        assert!(registry.uncovered_format_versions(&[]).is_empty());
    }

    #[test]
    fn covered_versions_returns_subset() {
        let mut registry: AdapterRegistry<TestWorkflow> = AdapterRegistry::new();
        registry.register(FnAdapter::new(
            |fv| fv.as_str() == "FV2025-10-01",
            |_raw, _fv| Ok(TestCommand::Fire),
        ));
        let candidates = vec![
            FormatVersion::new("FV2025-10-01"),
            FormatVersion::new("FV2026-10-01"),
        ];
        let covered = registry.covered_versions(&candidates);
        assert_eq!(covered.len(), 1);
        assert_eq!(covered[0].as_str(), "FV2025-10-01");
    }
}

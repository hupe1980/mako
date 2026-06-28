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
//! Adapters are registered in an [`AdapterRegistry`] at engine startup.  The
//! registry validates at registration time that all format versions in the
//! workflow's [`WorkflowVersionPolicy`] have a registered adapter.
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

use crate::{
    error::EngineError,
    version::{FormatVersion, WorkflowVersionPolicy},
    workflow::Workflow,
};

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
    /// every format version declared by the workflow's
    /// [`WorkflowVersionPolicy`] is covered.
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
/// After all adapters are registered, call [`AdapterRegistry::validate_policy`]
/// to confirm that every format version declared in the workflow's
/// [`WorkflowVersionPolicy`] is covered by at least one adapter.
///
/// # Example
///
/// ```rust,ignore
/// use mako_engine::message_adapter::AdapterRegistry;
///
/// let mut registry: AdapterRegistry<MyWorkflow> = AdapterRegistry::new();
/// registry.register(MyFV2025Adapter);
/// registry.register(MyFV2026Adapter);
/// registry
///     .validate_policy(
///         &MyWorkflow::version_policy(),
///         &[
///             FormatVersion::new("FV2025-10-01"),
///             FormatVersion::new("FV2026-10-01"),
///         ],
///     )
///     .expect("all format versions must have a registered adapter");
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

    /// Validate that every format version in `known_fvs` is covered by at
    /// least one registered adapter, according to `policy`.
    ///
    /// `known_fvs` is typically the set of all registered BDEW profiles for
    /// the workflow's message type.  In practice, call this at engine startup
    /// with the format versions returned by `ReleaseRegistry::all_profiles()`.
    ///
    /// # Behaviour per policy
    ///
    /// | Policy | Validation rule |
    /// |--------|-----------------|
    /// | `Pinned` | All `known_fvs` must be covered. A Pinned workflow can be
    ///   started under any known FV; every one of them must have an adapter. |
    /// | `ForwardCompatible` | Same — all `known_fvs` must be covered so the
    ///   workflow can handle messages in every FV it may encounter. |
    /// | `Explicit(list)` | Only the explicitly listed FVs must be covered. |
    ///
    /// Passing an empty `known_fvs` slice skips all coverage checks and
    /// always returns `Ok(())`.
    ///
    /// # Errors
    ///
    /// Returns a non-empty list of uncovered format versions.  The engine
    /// should treat this as a startup error rather than a runtime error.
    pub fn validate_policy(
        &self,
        policy: &WorkflowVersionPolicy,
        known_fvs: &[FormatVersion],
    ) -> Result<(), Vec<FormatVersion>> {
        let must_cover: &[FormatVersion] = match policy {
            // Pinned and ForwardCompatible both require coverage of every
            // currently-known FV.  (For Pinned, any of the known FVs may be
            // used as the process creation FV; for ForwardCompatible, the
            // workflow accepts all of them.)
            WorkflowVersionPolicy::Pinned | WorkflowVersionPolicy::ForwardCompatible => known_fvs,

            // Explicit lists the exact FVs that need coverage.
            WorkflowVersionPolicy::Explicit(required) => required.as_slice(),
        };

        let uncovered: Vec<FormatVersion> = must_cover
            .iter()
            .filter(|fv| !self.adapters.iter().any(|a| a.accepts_format_version(fv)))
            .cloned()
            .collect();

        if uncovered.is_empty() {
            Ok(())
        } else {
            Err(uncovered)
        }
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
        version::{FormatVersion, WorkflowVersionPolicy},
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
    fn validate_policy_explicit_all_covered() {
        let mut registry: AdapterRegistry<TestWorkflow> = AdapterRegistry::new();
        registry.register(FnAdapter::new(
            |fv| matches!(fv.as_str(), "FV2025-10-01" | "FV2026-10-01"),
            |_raw, _fv| Ok(TestCommand::Fire),
        ));
        let policy = WorkflowVersionPolicy::Explicit(vec![
            FormatVersion::new("FV2025-10-01"),
            FormatVersion::new("FV2026-10-01"),
        ]);
        let known = vec![
            FormatVersion::new("FV2025-10-01"),
            FormatVersion::new("FV2026-10-01"),
        ];
        assert!(registry.validate_policy(&policy, &known).is_ok());
    }

    #[test]
    fn validate_policy_explicit_gap_detected() {
        let mut registry: AdapterRegistry<TestWorkflow> = AdapterRegistry::new();
        // Only FV2025 adapter registered.
        registry.register(FnAdapter::new(
            |fv| fv.as_str() == "FV2025-10-01",
            |_raw, _fv| Ok(TestCommand::Fire),
        ));
        let policy = WorkflowVersionPolicy::Explicit(vec![
            FormatVersion::new("FV2025-10-01"),
            FormatVersion::new("FV2026-10-01"), // <-- no adapter
        ]);
        let known = vec![
            FormatVersion::new("FV2025-10-01"),
            FormatVersion::new("FV2026-10-01"),
        ];
        let result = registry.validate_policy(&policy, &known);
        assert!(result.is_err());
        let gaps = result.unwrap_err();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].as_str(), "FV2026-10-01");
    }

    #[test]
    fn validate_policy_pinned_empty_known_fvs_always_ok() {
        // When no known FVs are supplied, there is nothing to validate —
        // even an empty registry passes.  Callers should always provide the
        // actual registered profile list for meaningful coverage checks.
        let registry: AdapterRegistry<TestWorkflow> = AdapterRegistry::new();
        assert!(
            registry
                .validate_policy(&WorkflowVersionPolicy::Pinned, &[])
                .is_ok()
        );
    }

    #[test]
    fn validate_policy_pinned_with_known_fvs_detects_gap() {
        // Pinned policy with known FVs: all must be covered.
        let mut registry: AdapterRegistry<TestWorkflow> = AdapterRegistry::new();
        registry.register(FnAdapter::new(
            |fv| fv.as_str() == "FV2025-10-01",
            |_raw, _fv| Ok(TestCommand::Fire),
        ));
        let known = vec![
            FormatVersion::new("FV2025-10-01"),
            FormatVersion::new("FV2026-10-01"), // no adapter → gap
        ];
        let result = registry.validate_policy(&WorkflowVersionPolicy::Pinned, &known);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            vec![FormatVersion::new("FV2026-10-01")]
        );
    }

    #[test]
    fn validate_policy_forward_compatible_with_known_fvs_detects_gap() {
        // ForwardCompatible must cover every known FV.
        let mut registry: AdapterRegistry<TestWorkflow> = AdapterRegistry::new();
        registry.register(FnAdapter::new(
            |fv| fv.as_str() == "FV2025-10-01",
            |_raw, _fv| Ok(TestCommand::Fire),
        ));
        let known = vec![
            FormatVersion::new("FV2025-10-01"),
            FormatVersion::new("FV2026-10-01"), // no adapter → gap
        ];
        let result = registry.validate_policy(&WorkflowVersionPolicy::ForwardCompatible, &known);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            vec![FormatVersion::new("FV2026-10-01")]
        );
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

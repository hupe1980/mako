//! Integration test: cross-format-version dispatch with `ForwardCompatible` policy.
//!
//! Validates that a process started under one format version can correctly
//! receive and process messages formatted under a later format version when
//! the `ForwardCompatible` version policy is active.
//!
//! # Scenario
//!
//! 1. A `CrossFvWorkflow` is started under `FV2025-10-01`.
//! 2. Two adapters are registered: one for `FV2025-10-01`, one for `FV2026-10-01`.
//! 3. `validate_policy(ForwardCompatible, known_fvs)` succeeds when both FVs are covered.
//! 4. `dispatch()` succeeds when called with a message under either FV.
//! 5. The workflow state correctly reflects commands dispatched from both FVs.

use mako_engine::{
    error::EngineError,
    event_store::InMemoryEventStore,
    ids::TenantId,
    message_adapter::{AdapterRegistry, FnAdapter},
    process::Process,
    version::{FormatVersion, WorkflowId, WorkflowVersionPolicy},
    workflow::{CommandPayload, EventPayload, Workflow},
};
use serde::{Deserialize, Serialize};

// ── Test domain ───────────────────────────────────────────────────────────────

/// Minimal event for the cross-FV test.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum CrossFvEvent {
    AperakReceived { positive: bool, fv: String },
}

impl EventPayload for CrossFvEvent {
    fn event_type(&self) -> &'static str {
        "AperakReceived"
    }
}

/// Minimal command — carries what the adapter extracted from the message.
#[derive(Debug)]
struct ReceiveAperak {
    positive: bool,
    fv: String,
}
impl CommandPayload for ReceiveAperak {}

/// State for the cross-FV workflow.
#[derive(Debug, Default, Clone)]
struct CrossFvState {
    aperak_count: u32,
    last_positive: Option<bool>,
    last_fv: Option<String>,
}

/// Workflow that accepts APERAK commands across format versions.
struct CrossFvWorkflow;

impl Workflow for CrossFvWorkflow {
    type State = CrossFvState;
    type Event = CrossFvEvent;
    type Command = ReceiveAperak;

    fn apply(mut state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            CrossFvEvent::AperakReceived { positive, fv } => {
                state.aperak_count += 1;
                state.last_positive = Some(*positive);
                state.last_fv = Some(fv.clone());
            }
        }
        state
    }

    fn handle(
        _state: &Self::State,
        command: Self::Command,
    ) -> Result<mako_engine::workflow::WorkflowOutput<Self::Event>, mako_engine::error::WorkflowError>
    {
        Ok(vec![CrossFvEvent::AperakReceived {
            positive: command.positive,
            fv: command.fv,
        }]
        .into())
    }

    // Use the default version_policy() = ForwardCompatible.
}

// ── Simulated message type ────────────────────────────────────────────────────

/// Simulated inbound message. Stands in for `AnyMessage` to keep this test
/// self-contained (no `edi-energy` crate dependency).
struct SimulatedAperak {
    positive: bool,
}

// ── Known FV list (analogous to ReleaseRegistry::all_profiles()) ──────────────

fn known_fvs() -> Vec<FormatVersion> {
    vec![
        FormatVersion::new("FV2025-10-01"),
        FormatVersion::new("FV2026-10-01"),
    ]
}

// ── Registry builder ─────────────────────────────────────────────────────────

fn build_adapter_registry() -> AdapterRegistry<CrossFvWorkflow> {
    let mut registry = AdapterRegistry::new();

    // Adapter for the current format version (FV2025-10-01).
    registry.register(FnAdapter::new(
        |fv: &FormatVersion| fv.as_str() == "FV2025-10-01",
        |raw: &dyn std::any::Any, fv: &FormatVersion| {
            raw.downcast_ref::<SimulatedAperak>()
                .map(|msg| ReceiveAperak {
                    positive: msg.positive,
                    fv: fv.as_str().to_owned(),
                })
                .ok_or_else(|| EngineError::Deserialization("unexpected message type".into()))
        },
    ));

    // Adapter for the next format version (FV2026-10-01).
    // In a real deployment this adapter would handle new field layouts.
    registry.register(FnAdapter::new(
        |fv: &FormatVersion| fv.as_str() == "FV2026-10-01",
        |raw: &dyn std::any::Any, fv: &FormatVersion| {
            raw.downcast_ref::<SimulatedAperak>()
                .map(|msg| ReceiveAperak {
                    positive: msg.positive,
                    fv: fv.as_str().to_owned(),
                })
                .ok_or_else(|| EngineError::Deserialization("unexpected message type".into()))
        },
    ));

    registry
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[test]
fn validate_policy_passes_for_forward_compatible_with_both_fvs_registered() {
    let registry = build_adapter_registry();
    assert!(
        registry
            .validate_policy(&WorkflowVersionPolicy::ForwardCompatible, &known_fvs())
            .is_ok(),
        "validate_policy must succeed when all known_fvs are covered by adapters"
    );
}

#[test]
fn validate_policy_fails_when_future_fv_adapter_is_missing() {
    let mut registry: AdapterRegistry<CrossFvWorkflow> = AdapterRegistry::new();

    // Register ONLY the current FV adapter — FV2026-10-01 is missing.
    registry.register(FnAdapter::new(
        |fv: &FormatVersion| fv.as_str() == "FV2025-10-01",
        |_: &dyn std::any::Any, fv: &FormatVersion| {
            Ok(ReceiveAperak {
                positive: true,
                fv: fv.as_str().to_owned(),
            })
        },
    ));

    let result = registry.validate_policy(&WorkflowVersionPolicy::ForwardCompatible, &known_fvs());
    assert!(
        result.is_err(),
        "validate_policy must detect the missing FV2026-10-01 adapter"
    );
    let uncovered = result.unwrap_err();
    assert_eq!(uncovered.len(), 1);
    assert_eq!(uncovered[0].as_str(), "FV2026-10-01");
}

#[test]
fn dispatch_succeeds_for_current_fv() {
    let registry = build_adapter_registry();
    let fv_current = FormatVersion::new("FV2025-10-01");
    let msg = SimulatedAperak { positive: true };

    let command = registry
        .dispatch(&msg as &dyn std::any::Any, &fv_current)
        .expect("dispatch must succeed for FV2025-10-01");

    assert!(command.positive);
    assert_eq!(command.fv, "FV2025-10-01");
}

#[test]
fn dispatch_succeeds_for_future_fv() {
    let registry = build_adapter_registry();
    let fv_future = FormatVersion::new("FV2026-10-01");
    let msg = SimulatedAperak { positive: false };

    let command = registry
        .dispatch(&msg as &dyn std::any::Any, &fv_future)
        .expect("dispatch must succeed for FV2026-10-01 with ForwardCompatible policy");

    assert!(!command.positive);
    assert_eq!(command.fv, "FV2026-10-01");
}

#[tokio::test]
async fn process_handles_cross_fv_command_sequence() {
    let store = InMemoryEventStore::new();
    let process = Process::<CrossFvWorkflow, _>::new(
        store,
        TenantId::new(),
        WorkflowId::new("cross-fv-test", "FV2025-10-01"),
    );
    let registry = build_adapter_registry();

    // Step 1: dispatch via current FV adapter (process start FV).
    let fv_current = FormatVersion::new("FV2025-10-01");
    let msg_current = SimulatedAperak { positive: true };
    let cmd_current = registry
        .dispatch(&msg_current as &dyn std::any::Any, &fv_current)
        .unwrap();

    let envelopes1 = process.execute(cmd_current).await.unwrap();
    assert_eq!(
        envelopes1.len(),
        1,
        "first command must produce exactly one event"
    );

    // Step 2: dispatch via future FV adapter (simulates post-Oct-2026 scenario).
    let fv_future = FormatVersion::new("FV2026-10-01");
    let msg_future = SimulatedAperak { positive: false };
    let cmd_future = registry
        .dispatch(&msg_future as &dyn std::any::Any, &fv_future)
        .unwrap();

    let envelopes2 = process.execute(cmd_future).await.unwrap();
    assert_eq!(
        envelopes2.len(),
        1,
        "second command must produce exactly one event"
    );

    // State must reflect both commands.
    let state = process.state().await.unwrap();
    assert_eq!(state.aperak_count, 2, "two commands → two events applied");
    assert_eq!(
        state.last_fv.as_deref(),
        Some("FV2026-10-01"),
        "last event must have FV2026-10-01"
    );
    assert_eq!(
        state.last_positive,
        Some(false),
        "last event must carry positive=false"
    );
}

#[test]
fn future_fv_not_in_known_list_cannot_be_dispatched() {
    let registry = build_adapter_registry();
    let fv_unknown = FormatVersion::new("FV2027-10-01");
    let msg = SimulatedAperak { positive: true };

    let result = registry.dispatch(&msg as &dyn std::any::Any, &fv_unknown);
    assert!(
        result.is_err(),
        "dispatch must fail for a FV with no registered adapter"
    );
}

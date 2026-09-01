//! Regulatory reachability and trust contracts for GaBi final-allocation triage.

use agentplane::core::Trust;
use serde_json::json;

#[test]
fn gabi_routes_only_the_event_the_platform_emits() {
    let specialist = agentd::builtin::find("gabi-gas-agent").expect("compiled specialist");
    assert_eq!(
        specialist.trigger_patterns,
        &[mako_events::gabi::ALOCAT_MISSING]
    );
    assert!(
        !specialist
            .trigger_patterns
            .iter()
            .any(|pattern| pattern.contains("imbalance") || pattern.contains("nomination"))
    );
}

#[test]
fn deterministic_gabi_triage_has_no_model_or_tool_authority() {
    let manifest = agentd::plane::find_manifest("gabi-gas-agent").expect("manifest");
    assert!(manifest.spec.execution.is_none());
    assert!(manifest.spec.tools.is_empty());
    let models = manifest
        .spec
        .models
        .as_ref()
        .expect("models: {} is explicit");
    assert!(models.privileged.is_none() && models.quarantined.is_none());
}

#[test]
fn alocat_authority_fields_require_semantic_validation() {
    let valid = agentd::plane::label::routing_envelope(&json!({
        "gas_day": "2026-08-06",
        "sender_eic": "11XRWENET-----1E",
        "receiver_eic": "11YN00000000TH2M",
        "synthetic_pid": "13013",
        "deadline_label": "counterparty-controlled text"
    }))
    .expect("validated routing envelope");
    assert_eq!(valid.label().trust, Trust::Trusted);
    let fields = valid.peek().as_object().expect("object");
    assert!(fields.contains_key("synthetic_pid"));
    assert!(!fields.contains_key("deadline_label"));

    assert!(
        agentd::plane::label::routing_envelope(&json!({
            "gas_day": "2026-02-30",
            "sender_eic": "11XRWENET-----1X",
            "synthetic_pid": "13A13"
        }))
        .is_none()
    );
}

//! Shared fixtures for the renderer conformance tests.

use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
use mako_engine::outbox::OutboxMessage;
use makod::config::PartyConfig;
use makod::party_registry::MpIdRegistry;

/// A registry with one primary party in the NB role.
pub fn registry(mp_id: &str) -> MpIdRegistry {
    let party = PartyConfig {
        mp_id: mp_id.to_owned(),
        roles: vec!["NB".to_owned()],
        primary: true,
        agency: None,
    };
    MpIdRegistry::from_config(&[party]).expect("test registry")
}

/// An outbox message carrying `payload` for `recipient`.
pub fn outbox_message(
    message_type: &str,
    recipient: &str,
    payload: serde_json::Value,
) -> OutboxMessage {
    OutboxMessage::new(
        StreamId::new("process/conformance"),
        ProcessId::new(),
        TenantId::new(),
        CorrelationId::new(),
        ConversationId::new(),
        EventId::new(),
        message_type,
        recipient,
        payload,
    )
}

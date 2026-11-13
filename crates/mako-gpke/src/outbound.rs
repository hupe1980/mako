//! The column fields a caller supplies beside a workflow's own, read into the
//! typed outbound payload at the command.
//!
//! A command that sends an ORDERS takes the Anwendungsfall's column fields
//! (`grund`, `wertzeitpunkt`, …) as JSON from the door. They land at the top of
//! the payload, where the renderer reads them; read into the typed struct
//! here, a key the renderer does not read is refused when the command is
//! handled — not when the outbox worker renders it, after the process has
//! already recorded the message as sent.

use mako_engine::error::WorkflowError;
use mako_engine::payload::OutboundPayload;

/// `base` with the top-level fields of `spalte` laid over it, read back as
/// `P`. `spalte` is an object or `null`; any other shape, or a key `P` does
/// not declare, is refused naming it.
pub(crate) fn mit_spalte<P: OutboundPayload>(
    base: &P,
    spalte: &serde_json::Value,
) -> Result<P, WorkflowError> {
    let mut body = serde_json::to_value(base)
        .map_err(|e| WorkflowError::rejected(format!("outbound payload: {e}")))?;
    match (body.as_object_mut(), spalte) {
        (_, serde_json::Value::Null) => {}
        (Some(target), serde_json::Value::Object(extra)) => {
            target.extend(extra.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        _ => {
            return Err(WorkflowError::rejected(format!(
                "the {} column fields must be a JSON object",
                P::MESSAGE_TYPE
            )));
        }
    }
    P::from_payload(&body).map_err(|e| {
        WorkflowError::rejected(format!(
            "the {} column fields are not ones its renderer reads: {e}",
            P::MESSAGE_TYPE
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_engine::payload::OrdersPayload;

    /// A caller-supplied key the renderer does not read is refused at the
    /// command, by name.
    #[test]
    fn an_unread_column_key_is_refused_by_name() {
        let base = OrdersPayload {
            pid: Some(17_004),
            ..Default::default()
        };
        let ok = mit_spalte(&base, &serde_json::json!({ "grund": "Z46" })).expect("declared");
        assert_eq!(ok.grund.as_deref(), Some("Z46"));
        assert_eq!(ok.pid, Some(17_004));
        let err = mit_spalte(&base, &serde_json::json!({ "grnd": "Z46" })).expect_err("refused");
        assert!(err.to_string().contains("grnd"), "{err}");
        assert!(mit_spalte(&base, &serde_json::Value::Null).is_ok());
        assert!(mit_spalte(&base, &serde_json::json!("x")).is_err());
    }
}

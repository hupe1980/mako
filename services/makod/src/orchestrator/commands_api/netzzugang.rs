//! §20b EnWG Netzzugangsplattform commands.
//!
//! Split out of the flat `commands_api` module; shared state, types, and
//! process-dispatch helpers live in `super`.

use super::*;

// ── §20b EnWG Netzzugangsplattform commands ───────────────────────────────────

/// Shared dispatch for the `netzzugang.*` commands (§20b EnWG Abs. 2 Nr. 1–3).
///
/// Validates the request, projects it into the marktd registry
/// (`netzzugang_antraege`, status `erfasst`, best-effort) and enqueues a
/// `NetzzugangAntrag` outbox message; the [`crate::netzzugang`] sender delivers
/// it with at-least-once semantics and advances the projection.
pub(super) async fn dispatch_netzzugang(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    antrag_typ: NetzzugangAntragTyp,
) -> Result<DispatchOutcome, DispatchError> {
    let field = |name: &str| -> Result<String, DispatchError> {
        payload
            .get(name)
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| DispatchError::InvalidPayload(format!("{name} is required")))
    };
    let netzanschluss_id = field("netzanschluss_id")?;
    let nb_mp_id = field("nb_mp_id")?;
    let antragsteller_ref = field("antragsteller_ref")?;

    // §42c registrations have exactly one action; the two Bestellung use cases
    // carry the statutory Bestellung/Änderung/Abbestellung triple.
    let aktion = if antrag_typ == NetzzugangAntragTyp::EnergySharingVereinbarung {
        NetzzugangAktion::Registrierung
    } else {
        match field("aktion")?.as_str() {
            "bestellung" => NetzzugangAktion::Bestellung,
            "aenderung" => NetzzugangAktion::Aenderung,
            "abbestellung" => NetzzugangAktion::Abbestellung,
            other => {
                return Err(DispatchError::InvalidPayload(format!(
                    "aktion must be bestellung|aenderung|abbestellung, got {other}"
                )));
            }
        }
    };

    let antrag = NetzzugangAntrag {
        id: uuid::Uuid::new_v4(),
        tenant: state.sender_party_id.clone(),
        antrag_typ,
        aktion,
        netzanschluss_id,
        nb_mp_id: nb_mp_id.clone(),
        antragsteller_ref,
        status: NetzzugangStatus::Erfasst,
        payload: payload
            .get("details")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        platform_ref: None,
        created_at: time::OffsetDateTime::now_utc(),
        submitted_at: None,
    };

    // Best-effort projection — the outbox delivery does not depend on it.
    if let Some(marktd) = &state.marktd_client
        && let Err(e) = marktd.upsert_netzzugang_antrag(&antrag).await
    {
        tracing::warn!(
            antrag_id = %antrag.id,
            error = %e,
            "netzzugang: marktd projection upsert failed (non-fatal)",
        );
    }

    let process_id = ProcessId::new();
    let msg = mako_engine::outbox::OutboxMessage::new(
        mako_engine::ids::StreamId::new("netzzugang"),
        process_id,
        state.tenant_id,
        mako_engine::ids::CorrelationId::new(),
        mako_engine::ids::ConversationId::new(),
        mako_engine::ids::EventId::new(),
        crate::netzzugang::NETZZUGANG_MESSAGE_TYPE,
        nb_mp_id,
        serde_json::to_value(&antrag)
            .map_err(|e| DispatchError::InvalidPayload(format!("serialize: {e}")))?,
    );
    mako_engine::outbox::OutboxStore::enqueue(state.store.as_ref(), &[msg])
        .await
        .map_err(DispatchError::Engine)?;

    Ok(DispatchOutcome::Spawned { process_id })
}

pub(super) fn cmd_netzzugang_zaehlpunktanordnung<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_netzzugang(
        s,
        p,
        NetzzugangAntragTyp::Zaehlpunktanordnung,
    ))
}

pub(super) fn cmd_netzzugang_verrechnungskonzept<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_netzzugang(
        s,
        p,
        NetzzugangAntragTyp::Verrechnungskonzept,
    ))
}

pub(super) fn cmd_netzzugang_energysharing<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_netzzugang(
        s,
        p,
        NetzzugangAntragTyp::EnergySharingVereinbarung,
    ))
}

//! API-Webdienste Strom server for `makod`.
//!
//! This module wires the `ControlMeasuresHandler`, `MaloIdentHandler`, and
//! `WimOrderHandler` traits from `energy-api` into a single axum [`Router`]
//! that is mounted when `--api-webdienste-addr` is set.
//!
//! ## Architecture
//!
//! `makod` plays three roles on the API-Webdienste Strom server:
//!
//! 1. **NB (Netzbetreiber)** for the MaLo Identification API: the LF sends
//!    `POST /maloId/request/v1`; `makod` looks up the MaLo and delivers the
//!    callback asynchronously via `MaloIdentSender`.
//!
//! 2. **MSB (Messstellenbetreiber)** for the Control Measures API: an NB or LF
//!    sends `POST /steuerbefehl/konfiguration/` or `/initialZustand/`; `makod`
//!    spawns a `WimSteuerungsauftragWorkflow` process, returns `202 Accepted`,
//!    and tracks the 5-Werktage response window.
//!
//! 3. **MSB (Messstellenbetreiber)** for the WiM Order API: a NB sends
//!    `POST /wimBestellung/v1/anmeldung/`; `makod` spawns a
//!    `WimDeviceChangeWorkflow` process (PID 55042 — WiM MSB Anmeldung Strom; REST
//!    channel for the same process family as UTILMD 55042),
//!    returns `202 Accepted`, and tracks the 5-Werktage APERAK window.
//!
//! ## API surface
//!
//! | API                    | Path prefix              | Handler trait             |
//! |------------------------|--------------------------|---------------------------|
//! | Control Measures v1    | `/steuerbefehl/`         | `ControlMeasuresHandler`  |
//! | MaLo Identification v1 | `/maloId/`               | `MaloIdentHandler`        |
//! | WiM Order v1           | `/wimBestellung/v1/`     | `WimOrderHandler`         |
//!
//! ## Authentication and caller identity
//!
//! Two separate things, both required.
//!
//! **Authorization to reach the port.** Every route sits behind bearer/OIDC
//! authentication plus the Cedar `UseWebdienste` action. This is on by default;
//! `--webdienste-allow-unauthenticated` removes it for deployments that
//! terminate mTLS at a fronting proxy and enforce access there, and `makod`
//! refuses to start the port without either.
//!
//! **Who is calling.** BDEW API-Webdienste identify the market participant by
//! their mTLS client certificate, which the proxy validates and terminates —
//! nothing in the request body carries it. The proxy forwards the certificate's
//! Marktpartner-ID in [`CLIENT_MP_ID_HEADER`], and the Control Measures
//! handlers refuse a request without it: the Endantwort to a §14a EnWG
//! Steuerungsauftrag is addressed to whoever sent it, so an unattributable
//! order cannot be accepted.
//!
//! ```nginx
//! # Nginx terminating BDEW PKI mTLS
//! proxy_set_header X-Mako-Client-MP-ID $ssl_client_s_dn_cn;
//! ```
//!
//! The WiM Order API carries `netzbetreiber_id` in its request body as well.
//! That is an *assertion*, not an authentication: whoever holds a client
//! certificate can put any Netzbetreiber's Marktpartner-ID in it. The handler
//! therefore requires the header like the others and refuses the request when
//! the two disagree — the same rule `edi-energy` enforces between `UNB` and
//! `NAD+MS` (Allgemeine Festlegungen §2.13), for the same reason: the transport
//! authenticates the envelope while the business logic reads the body.

use std::sync::Arc;

use crate::api_bridge::{location_id_to_domain, party_id_to_marktpartner};
use axum::Router;
use energy_api::models::electricity::{IdentificationParameter, WimAnmeldungRequest};
use energy_api::server::{control_measures, malo_ident, wim_order};
use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
use mako_engine::inbox::{InboxClaim, InboxStore as _};
use mako_engine::outbox::{OutboxMessage, OutboxStore as _};
use mako_engine::store_slatedb::SlateDbStore;
use mako_engine::types::MeLo;
use mako_wim::geraetewechsel::{
    DeviceChangeCommand, WORKFLOW_NAME as DEVICE_CHANGE_WORKFLOW_NAME, WimDeviceChangeWorkflow,
};
use mako_wim::steuerungsauftrag::{
    SteuerungsauftragCommand, WORKFLOW_NAME as STEUERUNGSAUFTRAG_WORKFLOW_NAME,
    WimSteuerungsauftragWorkflow,
};
use serde_json::json;
use tracing::info;

// ── Caller identity ───────────────────────────────────────────────────────────

/// Header naming the Marktpartner-ID of the calling market participant.
///
/// The BDEW API-Webdienste identify the caller by their mTLS client
/// certificate, which is validated and terminated by the fronting proxy. The
/// proxy forwards the certificate's Marktpartner-ID in this header; nothing in
/// the request body carries it, because the specification does not put it there.
pub const CLIENT_MP_ID_HEADER: &str = "x-mako-client-mp-id";

/// Whether the deployment has declared that a proxy sets
/// [`CLIENT_MP_ID_HEADER`] and strips any copy the client sent.
#[derive(Clone, Copy)]
pub struct TrustClientMpId(pub bool);

tokio::task_local! {
    /// Marktpartner-ID of the authenticated caller for the current request.
    ///
    /// A task-local rather than a handler argument because the `energy-api`
    /// handler traits take only the request's business fields — the transport
    /// identity is deliberately outside their signatures. Same shape as the
    /// engine's `traceparent` propagation.
    static CLIENT_MP_ID: Option<String>;
}

/// Marktpartner-ID of the caller, when the proxy supplied one.
fn caller_mp_id() -> Option<String> {
    CLIENT_MP_ID.try_with(Clone::clone).ok().flatten()
}

/// Error returned when the authenticated caller and the party named in the
/// request body are different market participants.
///
/// The certificate is the authenticated fact; the body field is a claim. A
/// mismatch means one participant is placing an order in another's name, and
/// the MSB's answer — the UTILMD Bestätigung and the APERAK — would be
/// addressed to the party named in the body, who never ordered anything.
fn caller_mismatch_error(caller: &str, claimed: &str) -> energy_api::Error {
    tracing::warn!(
        caller_mp_id = %caller,
        claimed_mp_id = %claimed,
        "API-Webdienste: request body names a different Marktpartner than the \
         authenticated client certificate — refusing",
    );
    energy_api::Error::Http {
        status: 403,
        // Deliberately generic: the caller already proved possession of a
        // client certificate, so naming which value we expected tells them
        // nothing they do not know, and naming the other party would confirm
        // an identifier they merely guessed.
        body: "the Marktpartner-ID in the request body does not match the \
               authenticated client certificate"
            .to_owned(),
    }
}

/// Error returned when a request that must be attributed carries no caller ID.
///
/// Fail-closed on purpose. These endpoints receive **orders**: a §14a EnWG
/// Steuerungsauftrag and a WiM Anmeldung both have to be answered to whoever
/// sent them, and the answer is addressed with this value. Substituting the
/// operator's own Marktpartner-ID produces a confirmation addressed to
/// ourselves: the ordering party is never told the control action was carried
/// out, and the §14a billing event names the wrong party.
fn missing_caller_error() -> energy_api::Error {
    energy_api::Error::Http {
        status: 400,
        body: format!(
            "missing {CLIENT_MP_ID_HEADER}: the calling market participant's \
             Marktpartner-ID must be supplied by the mTLS-terminating proxy. \
             The response to this order is addressed with it, so it cannot be \
             inferred."
        ),
    }
}

// ── MakodApiHandler ───────────────────────────────────────────────────────────

/// Handler state for the API-Webdienste Strom server.
///
/// - `store`          — shared SlateDB instance for inbox idempotency, outbox
///   persistence, and event-sourced workflow dispatch.
/// - `tenant_id`      — the operator's [`TenantId`], derived from their BDEW
///   code / MP-ID via [`TenantId::from_party_id`].
///
/// The calling market participant's Marktpartner-ID is **not** state: it
/// arrives per request in [`CLIENT_MP_ID_HEADER`], set by the mTLS-terminating
/// proxy. Holding it on the handler would make every inbound order look as
/// though this operator had sent it.
#[derive(Clone)]
pub struct MakodApiHandler {
    pub store: SlateDbStore,
    pub tenant_id: TenantId,
}

// ── MaloIdentHandler ──────────────────────────────────────────────────────────

impl malo_ident::MaloIdentHandler for MakodApiHandler {
    /// NB receives a MaLo-ID identification request from the LF.
    ///
    /// 1. Inbox idempotency guard — rejects duplicate `tx_id` values.
    /// 2. Enqueues a `MaloIdentCallback` message for the outbox worker.
    /// 3. Returns `Ok(())` → axum sends `202 Accepted` to the LF.
    fn on_request(
        &self,
        tx_id: String,
        _creation_dt: String,
        sender_market_partner_id: String,
        params: IdentificationParameter,
    ) -> impl std::future::Future<Output = Result<(), energy_api::Error>> + Send {
        let store = self.store.clone();
        let tenant_id = self.tenant_id;
        async move {
            // Idempotency check — `accept` returns true the first time only.
            let inbox_key = format!("maloid:{tenant_id}:{tx_id}");
            match claim_inbox(&store, &inbox_key).await? {
                InboxClaim::Duplicate => {
                    info!(
                        tx_id,
                        "duplicate MaLo-ID request — returning early (idempotent)"
                    );
                    return Ok(());
                }
                InboxClaim::InFlight => {
                    // A concurrent request holds an unsettled claim. Not a
                    // duplicate — nothing is recorded yet — so answering `Ok`
                    // would report work that may still fail.
                    info!(
                        tx_id,
                        "MaLo-ID request already in flight — refusing the concurrent request"
                    );
                    return Err(energy_api::Error::Http {
                        status: 409,
                        body: "a request with this transactionId is already in flight".to_owned(),
                    });
                }
                InboxClaim::Claimed => {}
            }

            // Enqueue outbox message for async callback delivery.
            let payload = json!({
                "tx_id":                    tx_id,
                "tenant_id":                tenant_id.to_string(),
                "sender_market_partner_id": sender_market_partner_id,
                "params":                   serde_json::to_value(&params).unwrap_or_default(),
            });
            let msg = OutboxMessage::new(
                StreamId::new("api-webdienste/maloid"),
                ProcessId::new(),
                tenant_id,
                CorrelationId::new(),
                ConversationId::new(),
                EventId::new(),
                "MaloIdentCallback",
                "internal://malo-ident-callback",
                payload,
            );
            if let Err(e) = enqueue_callback(&store, msg).await {
                settle_abandoned(&store, &inbox_key).await;
                tracing::error!("outbox enqueue error: {e}");
                return Err(energy_api::Error::Http {
                    status: 500,
                    body: "internal error".to_string(),
                });
            }
            settle_accepted(&store, &inbox_key).await;

            info!(
                tx_id,
                "MaLo-ID request accepted and queued for async lookup"
            );
            Ok(())
        }
    }
}

/// Enqueue the MaLo-ID callback for the outbox worker.
async fn enqueue_callback(
    store: &SlateDbStore,
    msg: OutboxMessage,
) -> Result<(), mako_engine::error::EngineError> {
    #[cfg(test)]
    if OUTBOX_FAULT.try_with(|f| *f).unwrap_or(false) {
        return Err(mako_engine::error::EngineError::outbox(
            "injected outbox fault",
        ));
    }
    store.enqueue(&[msg]).await
}

#[cfg(test)]
tokio::task_local! {
    /// Test-only fault point: while set, the MaLo-ID callback enqueue fails.
    static OUTBOX_FAULT: bool;
}

/// Claim `key` for this request, mapping a store failure to a 500.
///
/// Fail-closed: a storage error is an error, never "first seen".
async fn claim_inbox(store: &SlateDbStore, key: &str) -> Result<InboxClaim, energy_api::Error> {
    store
        .as_inbox_store()
        .claim(key)
        .await
        .map_err(|e| energy_api::Error::Http {
            status: 500,
            body: format!("inbox error: {e}"),
        })
}

/// Settle `key` as handled, once the request is durably recorded.
///
/// A settlement failure is logged rather than returned: the work is already
/// done, so failing the request would report that nothing happened when
/// something did. The claim's lease expires and a retry is reprocessed, which
/// is the safe direction.
async fn settle_accepted(store: &SlateDbStore, key: &str) {
    if let Err(e) = store.as_inbox_store().accept(key).await {
        tracing::error!(inbox_key = %key, error = %e, "inbox accept failed");
    }
}

/// Release `key` because nothing durable recorded the request.
///
/// This is what makes a retry a recovery path: without it the key stays
/// claimed and the caller's next attempt is answered as a duplicate of work
/// that never happened.
async fn settle_abandoned(store: &SlateDbStore, key: &str) {
    if let Err(e) = store.as_inbox_store().abandon(key).await {
        tracing::error!(inbox_key = %key, error = %e, "inbox abandon failed");
    }
}

// ── ControlMeasuresHandler ────────────────────────────────────────────────────

impl control_measures::ControlMeasuresHandler for MakodApiHandler {
    /// MSB receives a power-regulation command from NB/LF.
    ///
    /// 1. Inbox idempotency guard — rejects duplicate `tx_id` values.
    /// 2. Spawns a `WimSteuerungsauftragWorkflow` process.
    /// 3. Executes `ReceiveKonfiguration` — writes the `KonfigurationReceived` event.
    /// 4. Returns `Ok(())` → axum sends `202 Accepted`.
    ///
    /// The ERP completes the cycle via:
    /// - `wim.steuerungsauftrag.bestaetigen` — send final positive response
    /// - `wim.steuerungsauftrag.ablehnen`    — send final negative response
    fn on_konfiguration(
        &self,
        tx_id: String,
        _creation_dt: String,
        location_id: energy_api::models::electricity::LocationId,
        command: energy_api::models::electricity::CommandControl,
    ) -> impl std::future::Future<Output = Result<(), energy_api::Error>> + Send {
        let store = self.store.clone();
        let tenant_id = self.tenant_id;
        let caller = caller_mp_id();
        async move {
            // Whoever sent the order is who the Endantwort goes back to.
            let sender = caller.ok_or_else(missing_caller_error)?;
            // Idempotency — accept only the first delivery of this tx_id.
            let inbox_key = format!("steuerungsauftrag:{tenant_id}:{tx_id}");
            match claim_inbox(&store, &inbox_key).await? {
                InboxClaim::Duplicate => {
                    info!(
                        tx_id,
                        "duplicate Steuerungsauftrag konfiguration — returning early (idempotent)"
                    );
                    return Ok(());
                }
                InboxClaim::InFlight => {
                    // A concurrent request holds an unsettled claim. Not a
                    // duplicate — nothing is recorded yet — so answering `Ok`
                    // would report work that may still fail.
                    info!(
                        tx_id,
                        "Steuerungsauftrag konfiguration already in flight — refusing the concurrent request"
                    );
                    return Err(energy_api::Error::Http {
                        status: 409,
                        body: "a request with this transactionId is already in flight".to_owned(),
                    });
                }
                InboxClaim::Claimed => {}
            }

            let domain_cmd = SteuerungsauftragCommand::ReceiveKonfiguration {
                tx_id: tx_id.clone(),
                sender_mp_id: party_id_to_marktpartner(sender),
                location_id: location_id_to_domain(&location_id),
                execution_time_from: command.execution_time_from.clone(),
                max_power_kw: command.maximum_power_value.0.clone(),
                execution_time_until: command.execution_time_until.clone(),
                // The BDEW Control Measures `CommandControl` body has three
                // fields — maximum power, from, until — and no
                // Konfigurationsprodukt. There is therefore nothing for the M1
                // eligibility guard to check on this channel; it is not a
                // trust decision, and the guard is not "skipped" so much as
                // inapplicable. The AS4 ORDERS carries the code and is checked.
                produkt_code: None,
            };

            if let Err(e) =
                spawn_steuerungsauftrag(store.clone(), tenant_id, &tx_id, domain_cmd).await
            {
                settle_abandoned(&store, &inbox_key).await;
                return Err(energy_api::Error::Http {
                    status: 500,
                    body: e.to_string(),
                });
            }
            settle_accepted(&store, &inbox_key).await;

            info!(
                tx_id,
                location_id = %location_id,
                max_power_kw = %command.maximum_power_value.0,
                "Control Measures konfiguration accepted — WimSteuerungsauftrag process spawned"
            );
            Ok(())
        }
    }

    /// MSB receives a reset command from NB/LF.
    fn on_initial_zustand(
        &self,
        tx_id: String,
        _creation_dt: String,
        location_id: energy_api::models::electricity::LocationId,
        command: energy_api::models::electricity::CommandRegular,
    ) -> impl std::future::Future<Output = Result<(), energy_api::Error>> + Send {
        let store = self.store.clone();
        let tenant_id = self.tenant_id;
        let caller = caller_mp_id();
        async move {
            let sender = caller.ok_or_else(missing_caller_error)?;
            let inbox_key = format!("steuerungsauftrag:{tenant_id}:{tx_id}");
            match claim_inbox(&store, &inbox_key).await? {
                InboxClaim::Duplicate => {
                    info!(
                        tx_id,
                        "duplicate Steuerungsauftrag initialZustand — returning early (idempotent)"
                    );
                    return Ok(());
                }
                InboxClaim::InFlight => {
                    // A concurrent request holds an unsettled claim. Not a
                    // duplicate — nothing is recorded yet — so answering `Ok`
                    // would report work that may still fail.
                    info!(
                        tx_id,
                        "Steuerungsauftrag initialZustand already in flight — refusing the concurrent request"
                    );
                    return Err(energy_api::Error::Http {
                        status: 409,
                        body: "a request with this transactionId is already in flight".to_owned(),
                    });
                }
                InboxClaim::Claimed => {}
            }

            let domain_cmd = SteuerungsauftragCommand::ReceiveInitialZustand {
                tx_id: tx_id.clone(),
                sender_mp_id: party_id_to_marktpartner(sender),
                location_id: location_id_to_domain(&location_id),
                execution_time_from: command.execution_time_from.clone(),
            };

            if let Err(e) =
                spawn_steuerungsauftrag(store.clone(), tenant_id, &tx_id, domain_cmd).await
            {
                settle_abandoned(&store, &inbox_key).await;
                return Err(energy_api::Error::Http {
                    status: 500,
                    body: e.to_string(),
                });
            }
            settle_accepted(&store, &inbox_key).await;

            info!(
                tx_id,
                location_id = %location_id,
                "Control Measures initialZustand accepted — WimSteuerungsauftrag process spawned"
            );
            Ok(())
        }
    }
}

// ── WimOrderHandler ───────────────────────────────────────────────────────────

impl wim_order::WimOrderHandler for MakodApiHandler {
    /// MSB receives an iMS Universalbestellprozess order from a NB via REST
    /// (PID 55042 — WiM MSB Anmeldung Strom, REST transport).
    ///
    /// 1. Attribution — the client certificate's Marktpartner-ID must be
    ///    present and equal to `netzbetreiber_id` in the body.
    /// 2. Inbox idempotency guard — rejects duplicate `tx_id` values.
    /// 3. Converts the REST payload to a `DeviceChangeCommand::ReceiveRestOrder`.
    /// 4. Spawns a `WimDeviceChangeWorkflow` process.
    /// 5. Writes the per-PID response deadline (BDEW WiM / BK6-24-174) and
    ///    the `tx_id` correlation key in the same batch as the events.
    /// 6. Returns `Ok(())` → axum sends `202 Accepted`.
    fn on_anmeldung(
        &self,
        tx_id: String,
        _creation_dt: String,
        request: WimAnmeldungRequest,
    ) -> impl std::future::Future<Output = Result<(), energy_api::Error>> + Send {
        let store = self.store.clone();
        let tenant_id = self.tenant_id;
        let caller = caller_mp_id();
        async move {
            // Attribution first, and above the idempotency guard: `accept`
            // consumes the tx_id, so checking afterwards would let a spoofed
            // request burn the key and turn the legitimate order into a
            // silently-swallowed duplicate.
            //
            // `netzbetreiber_id` is a claim in the body; the client certificate
            // is the authenticated fact. Both must name the same participant.
            let claimed = request.netzbetreiber_id.to_string();
            let caller = caller.ok_or_else(missing_caller_error)?;
            if caller != claimed {
                return Err(caller_mismatch_error(&caller, &claimed));
            }

            // Idempotency — accept only the first delivery of this tx_id.
            let inbox_key = format!("wim-order:{tenant_id}:{tx_id}");
            match claim_inbox(&store, &inbox_key).await? {
                InboxClaim::Duplicate => {
                    info!(
                        tx_id,
                        "duplicate WiM Anmeldung — returning early (idempotent)"
                    );
                    return Ok(());
                }
                InboxClaim::InFlight => {
                    // A concurrent request holds an unsettled claim. Not a
                    // duplicate — nothing is recorded yet — so answering `Ok`
                    // would report work that may still fail.
                    info!(
                        tx_id,
                        "WiM Anmeldung already in flight — refusing the concurrent request"
                    );
                    return Err(energy_api::Error::Http {
                        status: 409,
                        body: "a request with this transactionId is already in flight".to_owned(),
                    });
                }
                InboxClaim::Claimed => {}
            }

            let sender_mp_id = party_id_to_marktpartner(caller);
            let melo_id = MeLo::new(&*request.melo_id);
            // Represent device_category as a string; the workflow records it
            // in DeviceChangeData.document_date (process_date|category=...).
            let device_category = format!("{:?}", request.device_category);

            let domain_cmd = DeviceChangeCommand::ReceiveRestOrder {
                tx_id: tx_id.clone(),
                sender_mp_id,
                melo_id,
                device_category,
                process_date: request.process_date.clone(),
            };

            let process_id =
                match spawn_device_change(store.clone(), tenant_id, &tx_id, domain_cmd).await {
                    Ok(id) => id,
                    Err(e) => {
                        settle_abandoned(&store, &inbox_key).await;
                        return Err(energy_api::Error::Http {
                            status: 500,
                            body: e.to_string(),
                        });
                    }
                };
            settle_accepted(&store, &inbox_key).await;

            info!(
                tx_id,
                melo_id = %request.melo_id,
                process_date = %request.process_date,
                %process_id,
                "WiM Anmeldung accepted — WimDeviceChangeWorkflow (PID 55042, REST channel) spawned"
            );
            Ok(())
        }
    }
}

/// The business-answer Frist for the order this command opens.
///
/// The MSB-Wechsel windows differ per Prüfidentifikator (BK6-24-174 WiM Teil 1):
/// 55039 → 3 WT, 55042 → 5 WT, 55051 → 7 WT, 55168 → 1 WT. `mako_wim` owns the
/// table so the REST and AS4 doors cannot drift apart.
///
/// `ReceiveRestOrder` carries no PID on the wire — the workflow stamps it with
/// 55042 (Anmeldung MSB), so this reads the same value rather than falling
/// through to the default. Commands with no request PID at all fall back to
/// 5 WT, the Anmeldung's value and the most common order.
fn device_change_frist_wt(command: &DeviceChangeCommand) -> u32 {
    let pid = match command {
        DeviceChangeCommand::ReceiveUtilmd { pid, .. }
        | DeviceChangeCommand::InitiateDeviceChange { pid, .. } => Some(pid.as_u32()),
        // `ReceiveRestOrder` is the /wimBestellung REST door; the workflow
        // stamps PID 55042 on it (`geraetewechsel.rs`).
        DeviceChangeCommand::ReceiveRestOrder { .. } => Some(55_042),
        _ => None,
    };
    pid.and_then(mako_wim::antwort_frist_werktage).unwrap_or(5)
}

/// The deadline label that matches the state `command` leaves the process in.
///
/// `WimDeviceChangeWorkflow::on_deadline` is keyed on `(label, state)` and
/// answers `None` to any label it does not know — a mismatch produces a
/// deadline that fires into the void and never transitions the process. The two
/// windows are also different obligations: `ANTWORT_FRIST_WINDOW_LABEL` is
/// *our* answer on an order we received, `AUFTRAG_ANTWORT_WINDOW_LABEL` is the
/// counterparty's answer on an order we sent.
fn device_change_window_label(command: &DeviceChangeCommand) -> &'static str {
    match command {
        // We sent the order → `AuftragGesendet`; the counterparty owes us.
        DeviceChangeCommand::InitiateDeviceChange { .. } => {
            mako_wim::geraetewechsel::AUFTRAG_ANTWORT_WINDOW_LABEL
        }
        // We received the order → `Initiated`; we owe the answer.
        _ => mako_wim::geraetewechsel::ANTWORT_FRIST_WINDOW_LABEL,
    }
}

/// Spawn a `WimDeviceChangeWorkflow` process for an inbound REST order
/// (`/wimBestellung/v1/anmeldung/`) and execute the first command.
///
/// Writes the per-PID answer deadline and the `tx_id` correlation key in the
/// spawn's own batch.
async fn spawn_device_change(
    store: SlateDbStore,
    tenant_id: TenantId,
    tx_id: &str,
    command: DeviceChangeCommand,
) -> Result<ProcessId, mako_engine::error::EngineError> {
    use mako_engine::version::WorkflowId;

    let fv = latest_format_version();
    let workflow_id = WorkflowId::new(DEVICE_CHANGE_WORKFLOW_NAME, fv);

    // Business-answer Frist, sized from the order's own Prüfidentifikator —
    // 55039 → 3 WT, 55042 → 5 WT, 55051 → 7 WT, 55168 → 1 WT (BK6-24-174 WiM
    // Teil 1, Kap. 2.2.2 / 2.3.2 / 2.4.2 / 2.5.2). This REST door has to agree
    // with the AS4 door; a flat 5 WT here would give the same order two
    // different deadlines depending on which transport it arrived on.
    //
    // deadline_at_werktage runs to the end of the due Werktag in
    // Europe/Berlin, correctly handling CET/CEST transitions.
    let frist_wt = device_change_frist_wt(&command);
    let due_at = mako_fristen::deadline_at_werktage(
        time::OffsetDateTime::now_utc(),
        frist_wt,
        mako_fristen::HolidayCalendar::BdewMaKo,
    );
    let label = device_change_window_label(&command);
    // Events, the Frist and the `tx_id` key in one write: the ERP's
    // `wim.geraetewechsel.*` answer finds the process only by `tx_id`.
    crate::commands_api::spawn_process::<WimDeviceChangeWorkflow>(
        &std::sync::Arc::new(store),
        tenant_id,
        workflow_id,
        command,
        &[(label, due_at)],
        &[tx_id],
    )
    .await
}

/// Spawn a `WimSteuerungsauftragWorkflow` process for an inbound Control
/// Measures order and execute the first command.
///
/// Stamps the latest format version in force and writes the 5-Werktage
/// confirmation deadline and the `tx_id` correlation key in the spawn's own
/// batch.
async fn spawn_steuerungsauftrag(
    store: SlateDbStore,
    tenant_id: TenantId,
    tx_id: &str,
    command: SteuerungsauftragCommand,
) -> Result<ProcessId, mako_engine::error::EngineError> {
    use mako_engine::version::WorkflowId;

    let fv = latest_format_version();
    let workflow_id = WorkflowId::new(STEUERUNGSAUFTRAG_WORKFLOW_NAME, fv);

    // The API-Webdienste channel states no Frist for the Steuerungsauftrag; the
    // 5 Werktage are mako's operating convention (`mako_wim::steuerungsauftrag`),
    // unrelated to the per-PID MSB-Wechsel Fristen above.
    // deadline_at_werktage runs to the end of the due Werktag in
    // Europe/Berlin, correctly handling CET/CEST transitions.
    let due_at = mako_fristen::deadline_at_werktage(
        time::OffsetDateTime::now_utc(),
        5,
        mako_fristen::HolidayCalendar::BdewMaKo,
    );
    // Events, the Frist and the `tx_id` key in one write:
    // `wim.steuerungsauftrag.bestaetigen` / `.ablehnen` find the process only
    // by `tx_id`.
    crate::commands_api::spawn_process::<WimSteuerungsauftragWorkflow>(
        &std::sync::Arc::new(store),
        tenant_id,
        workflow_id,
        command,
        &[(
            mako_wim::steuerungsauftrag::STEUERUNGSAUFTRAG_DEADLINE_LABEL,
            due_at,
        )],
        &[tx_id],
    )
    .await
}

/// The latest BDEW format version **in force today**, for stamping an outbound
/// process.
///
/// Not simply the registry's newest: a profile is compiled in as soon as BDEW
/// publishes it, six months before its Anwendungszeitpunkt (Allgemeine
/// Festlegungen 6.1d § 2.5). EDIFACT has no Übergangsfrist, so a message
/// stamped with the next release before its Stichtag is refused by the
/// counterparty — the registry is filtered to `valid_from <= heute()` first.
fn latest_format_version() -> mako_engine::version::FormatVersion {
    edi_energy::registry::ReleaseRegistry::global()
        .format_versions_in_force_on(mako_fristen::heute())
        .into_iter()
        .filter_map(|s| mako_engine::version::FormatVersion::parse(&s).ok())
        .max()
        .unwrap_or_else(|| {
            mako_engine::version::FormatVersion::parse("FV2025-10-01")
                .expect("fallback FV is valid")
        })
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Build the axum [`Router`] for the API-Webdienste Strom server.
///
/// Both the Control Measures and MaLo Identification APIs are mounted on the
/// same port.
pub fn router(handler: Arc<MakodApiHandler>) -> Router {
    Router::new()
        .merge(control_measures::router(Arc::clone(&handler)))
        .merge(malo_ident::router(Arc::clone(&handler)))
        .merge(wim_order::router(handler))
}

/// Assemble the `:8090` application: routes, body limit, and — unless `auth`
/// is `None` — the bearer/OIDC + Cedar authentication layer.
///
/// This exists as a function rather than inline wiring because it *is* the
/// access-control decision for the port. Composed at the call site, the only
/// way to test that `:8090` rejects an anonymous request would be to rebuild
/// the same layer stack in the test, which proves the test's copy correct and
/// says nothing about the binary's.
///
/// `auth: None` corresponds to `--webdienste-allow-unauthenticated` and leaves
/// every route open — valid only behind a proxy terminating mTLS against the
/// BDEW PKI CA.
///
/// `trust_client_mp_id_header` decides whether the caller's Marktpartner-ID is
/// read from [`CLIENT_MP_ID_HEADER`]. False, the header is ignored and a
/// handler that needs a caller refuses, because nothing distinguishes a proxy's
/// assertion from a value the client set itself.
///
/// Health routes are deliberately **not** included: the caller merges them
/// afterwards so that Kubernetes probes stay reachable without a token.
pub fn build_app(
    handler: Arc<MakodApiHandler>,
    auth: Option<WebdiensteAuthState>,
    max_body_bytes: usize,
    trust_client_mp_id_header: bool,
) -> Router {
    let routes = router(handler)
        // Innermost, so the handlers only ever see a request whose signature,
        // when it carries one, verified.
        .layer(axum::middleware::from_fn_with_state(
            max_body_bytes,
            content_signature_middleware,
        ))
        .layer(axum::extract::DefaultBodyLimit::max(max_body_bytes))
        // Below the auth layer, so the identity is in scope for the handlers
        // whether or not `makod` itself checks a bearer token.
        .layer(axum::middleware::from_fn_with_state(
            TrustClientMpId(trust_client_mp_id_header),
            client_identity_middleware,
        ));
    match auth {
        Some(state) => routes.layer(axum::middleware::from_fn_with_state(
            state,
            webdienste_auth_middleware,
        )),
        None => routes,
    }
}

// ── Content signature (Inhaltsdatensicherung) ────────────────────────────────

/// Header carrying the Base64 signature value (Regelungen zum Übertragungsweg
/// für API-Webdienste 1.2, Kap. 3.3): `X-BDEW-SIGNATURE`, in the lower case
/// HTTP/2 transmits. The sending side is `energy_api`'s
/// `content_security::HEADER_SIGNATURE`; a test pins the two together.
pub const HEADER_BDEW_SIGNATURE: &str = "x-bdew-signature";
/// Header carrying the Base64 signed digest `Hash(M)`: `X-BDEW-DIGEST` (same
/// chapter).
pub const HEADER_BDEW_DIGEST: &str = "x-bdew-digest";
/// Header carrying the signer's certificate, encoded as RFC 9440 §2.1 does for
/// a TLS certificate: `X-BDEW-CERT` (same chapter).
pub const HEADER_BDEW_CERT: &str = "x-bdew-cert";

/// Verify the content-layer signature of an inbound request before any handler
/// sees it.
///
/// Regelungen zum Übertragungsweg für API-Webdienste 1.2, Kap. 3.3 signs the
/// URI with its query, the RFC 8785 canonical payload, the
/// `creationDateTime` / `transactionId` headers and, when present, the
/// `initialTransactionId` header; the signature, the digest and
/// the signer's certificate travel in [`HEADER_BDEW_SIGNATURE`],
/// [`HEADER_BDEW_DIGEST`] and [`HEADER_BDEW_CERT`]. A request carrying a
/// signature that does not verify against the certificate it names is refused
/// with `401` — a forged or altered order never reaches the workflow.
///
/// What this checks and what it does not:
///
/// - The signature is checked against the public key of the certificate in
///   [`HEADER_BDEW_CERT`]. That certificate's SM-PKI chain is not validated
///   here; the mTLS-terminating proxy authenticates the caller.
/// - The signed URI is rebuilt as `https://{Host}{path?query}`, the URL the
///   caller addressed through the proxy.
/// - A request carrying no signature at all passes with a warning.
pub async fn content_signature_middleware(
    axum::extract::State(max_body_bytes): axum::extract::State<usize>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let refuse = |why: String| {
        tracing::warn!(reason = %why, "API-Webdienste: content signature refused");
        (
            axum::http::StatusCode::UNAUTHORIZED,
            format!("content signature refused: {why}"),
        )
            .into_response()
    };
    let headers = request.headers();
    let one = |name: &str| mako_service::headers::single_str(headers, name);
    let (signature, digest, cert) = match (
        one(HEADER_BDEW_SIGNATURE),
        one(HEADER_BDEW_DIGEST),
        one(HEADER_BDEW_CERT),
    ) {
        (Ok(None), Ok(None), Ok(None)) => {
            tracing::warn!(
                path = %request.uri().path(),
                "API-Webdienste: inbound request carries no content signature"
            );
            return next.run(request).await;
        }
        (Ok(Some(s)), Ok(Some(d)), Ok(Some(c))) => (s.to_owned(), d.to_owned(), c.to_owned()),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
            return refuse(format!("{} must appear at most once", e.name));
        }
        _ => {
            return refuse(format!(
                "{HEADER_BDEW_SIGNATURE}, {HEADER_BDEW_DIGEST} and {HEADER_BDEW_CERT} \
                 travel together"
            ));
        }
    };
    let (Ok(Some(creation_dt)), Ok(Some(tx_id)), Ok(Some(host))) =
        (one("creationdatetime"), one("transactionid"), one("host"))
    else {
        return refuse("creationDateTime, transactionId and Host are signed inputs".to_owned());
    };
    // Signed „soweit vorhanden": absent, it is not part of `M`.
    let initial_tx_id = match one("initialtransactionid") {
        Ok(v) => v.map(str::to_owned),
        Err(e) => return refuse(format!("{} must appear at most once", e.name)),
    };
    let (creation_dt, tx_id) = (creation_dt.to_owned(), tx_id.to_owned());
    let uri = format!(
        "https://{host}{}",
        request
            .uri()
            .path_and_query()
            .map_or("/", axum::http::uri::PathAndQuery::as_str)
    );
    let key = match verifying_key_from_rfc9440(&cert) {
        Ok(k) => k,
        Err(e) => return refuse(e),
    };

    let (parts, body) = request.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, max_body_bytes).await else {
        return refuse("the body could not be read".to_owned());
    };
    let canonical = if bytes.is_empty() {
        Vec::new()
    } else {
        let parsed: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => return refuse(format!("the signed payload is not JSON: {e}")),
        };
        match energy_api::transport::content_security::canonical_json(&parsed) {
            Ok(c) => c,
            Err(e) => return refuse(e.to_string()),
        }
    };
    if let Err(e) = energy_api::transport::content_security::verify_request(
        &energy_api::transport::content_security::SignedInputs {
            uri: &uri,
            payload: &canonical,
            creation_date_time: &creation_dt,
            transaction_id: &tx_id,
            initial_transaction_id: initial_tx_id.as_deref(),
        },
        &digest,
        &signature,
        &key,
    ) {
        return refuse(e.to_string());
    }
    let request = axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes));
    next.run(request).await
}

/// The operator's content-signing identity: the EMT.API key and the
/// certificate sent beside it as `X-BDEW-CERT` (Regelungen zum Übertragungsweg
/// für API-Webdienste 1.2, Kap. 3.3).
#[derive(Clone)]
pub struct ContentSigning {
    /// The EMT.API private key.
    pub key: energy_api::transport::content_security::ContentSigningKey,
    /// The DER of the certificate naming its public key.
    pub cert_der: Vec<u8>,
}

impl std::fmt::Debug for ContentSigning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContentSigning").finish_non_exhaustive()
    }
}

impl ContentSigning {
    /// Parse the key and certificate PEMs and prove they belong together.
    ///
    /// A certificate naming another key would make every signed request fail
    /// the receiver's check, so the pair is refused here rather than on the
    /// wire.
    ///
    /// # Errors
    ///
    /// When either PEM does not parse, or the certificate's key is not the
    /// signing key's.
    pub fn from_pem(key_pem: &str, cert_pem: &str) -> Result<Self, String> {
        use base64::Engine as _;
        use energy_api::transport::content_security as cs;
        let key = cs::signing_key_from_pem(key_pem).map_err(|e| e.to_string())?;
        let body: String = cert_pem
            .lines()
            .map(str::trim)
            .skip_while(|l| *l != "-----BEGIN CERTIFICATE-----")
            .skip(1)
            .take_while(|l| *l != "-----END CERTIFICATE-----")
            .collect();
        let cert_der = base64::engine::general_purpose::STANDARD
            .decode(body)
            .map_err(|e| format!("signing certificate is not a PEM CERTIFICATE: {e}"))?;
        let named = verifying_key_from_rfc9440(&cs::cert_header_value(&cert_der))?;
        if named != *key.verifying_key() {
            return Err("the signing certificate names a different key".to_owned());
        }
        Ok(Self { key, cert_der })
    }
}

/// The verifying key of the certificate in an RFC 9440 §2.1 header value — the
/// DER, standard Base64, between colons.
fn verifying_key_from_rfc9440(
    value: &str,
) -> Result<energy_api::transport::content_security::ContentVerifyingKey, String> {
    use base64::Engine as _;
    let inner = value
        .trim()
        .strip_prefix(':')
        .and_then(|v| v.strip_suffix(':'))
        .ok_or_else(|| format!("{HEADER_BDEW_CERT} is not an RFC 9440 byte sequence"))?;
    let der = base64::engine::general_purpose::STANDARD
        .decode(inner)
        .map_err(|e| format!("{HEADER_BDEW_CERT} Base64: {e}"))?;
    let spki = subject_public_key_info(&der)
        .ok_or_else(|| format!("{HEADER_BDEW_CERT} is not an X.509 certificate"))?;
    // RFC 7468 §2: the Base64 body wraps at 64 characters.
    let b64 = base64::engine::general_purpose::STANDARD.encode(spki);
    let body: Vec<&str> = b64
        .as_bytes()
        .chunks(64)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect();
    let pem = format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
        body.join("\n")
    );
    energy_api::transport::content_security::verifying_key_from_pem(&pem).map_err(|e| e.to_string())
}

/// The `subjectPublicKeyInfo` of a DER X.509 certificate (RFC 5280 §4.1):
/// `Certificate ::= SEQUENCE { tbsCertificate, … }`, and inside the
/// `tbsCertificate` an optional `[0]` version, then serialNumber, signature,
/// issuer, validity and subject before it.
fn subject_public_key_info(der: &[u8]) -> Option<&[u8]> {
    /// One DER TLV at the front of `b`: (tag, header length, total length).
    fn tlv(b: &[u8]) -> Option<(u8, usize, usize)> {
        let tag = *b.first()?;
        let first = *b.get(1)?;
        let (len, hdr) = if first < 0x80 {
            (usize::from(first), 2)
        } else {
            let n = usize::from(first & 0x7f);
            if n == 0 || n > 4 {
                return None;
            }
            let mut len = 0usize;
            for i in 0..n {
                len = (len << 8) | usize::from(*b.get(2 + i)?);
            }
            (len, 2 + n)
        };
        let total = hdr.checked_add(len)?;
        (total <= b.len()).then_some((tag, hdr, total))
    }
    let (tag, hdr, _) = tlv(der)?;
    if tag != 0x30 {
        return None;
    }
    let cert = &der[hdr..];
    let (tag, hdr, total) = tlv(cert)?;
    if tag != 0x30 {
        return None;
    }
    let mut tbs = &cert[hdr..total];
    if tbs.first() == Some(&0xa0) {
        let (_, _, t) = tlv(tbs)?;
        tbs = &tbs[t..];
    }
    // serialNumber, signature, issuer, validity, subject.
    for _ in 0..5 {
        let (_, _, t) = tlv(tbs)?;
        tbs = &tbs[t..];
    }
    let (tag, _, t) = tlv(tbs)?;
    (tag == 0x30).then(|| &tbs[..t])
}

/// Bearer/OIDC authentication state for the `:8090` API-Webdienste port.
#[derive(Clone)]
pub struct WebdiensteAuthState {
    /// Cedar-based authenticator/authorizer.
    pub cedar: Arc<crate::cedar_authz::CedarAuthorizer>,
    /// Operator tenant (MP-ID) — the Cedar resource scope.
    pub tenant: Arc<str>,
}

/// Scope the caller's Marktpartner-ID into the `CLIENT_MP_ID` task-local for
/// the request.
///
/// Runs on every `:8090` request, including when the auth layer is disabled
/// because a fronting proxy terminates mTLS — that proxy is exactly what sets
/// the header, so the identity must be read whether or not `makod` also checks
/// a bearer token.
///
/// A malformed value is dropped rather than propagated: the handlers treat a
/// missing caller as a refusal, which is the safe reading of "the proxy did not
/// give me a usable identity".
///
/// A **repeated** header is refused outright instead. The value decides whose
/// name a § 14a Steuerungsauftrag or a WiM Anmeldung is placed in, so resolving
/// it first-wins would let a second copy ride along behind the proxy's — the
/// same reason every other identity header here goes through
/// [`mako_service::headers::single_str`].
pub async fn client_identity_middleware(
    axum::extract::State(TrustClientMpId(trusted)): axum::extract::State<TrustClientMpId>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    if !trusted {
        // No proxy has declared itself, so the header is the caller's own claim
        // about which Marktpartner it is. Scope `None`: the handlers treat an
        // absent caller as a refusal, which is the safe reading.
        return CLIENT_MP_ID.scope(None, next.run(request)).await;
    }
    let raw = match mako_service::headers::single_str(request.headers(), CLIENT_MP_ID_HEADER) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(header = %e.name, count = e.count, "API-Webdienste: repeated caller-identity header — refusing");
            return (
                axum::http::StatusCode::BAD_REQUEST,
                format!("{} must appear at most once", CLIENT_MP_ID_HEADER),
            )
                .into_response();
        }
    };
    let mp_id = raw
        .map(str::trim)
        .filter(|v| {
            (v.len() == 13 && v.bytes().all(|b| b.is_ascii_digit()))
                || (v.len() == 16 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        })
        .map(str::to_owned);
    CLIENT_MP_ID.scope(mp_id, next.run(request)).await
}

/// Authentication middleware for every `:8090` route.
///
/// The BDEW API-Webdienste specification requires authenticated access. The
/// caller must present a bearer token (named key or OIDC JWT) and hold the
/// Cedar `UseWebdienste` action; the body-size limit is applied by the caller
/// via [`axum::extract::DefaultBodyLimit`].
pub async fn webdienste_auth_middleware(
    axum::extract::State(state): axum::extract::State<WebdiensteAuthState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let Some(identity) = state.cedar.authenticate(request.headers()) else {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            "Authorization: Bearer <token> required for API-Webdienste",
        )
            .into_response();
    };
    if !state.cedar.authorize_webdienste(
        &identity,
        &crate::cedar_authz::WebdiensteResource {
            tenant: &state.tenant,
        },
    ) {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "403 Forbidden: UseWebdienste permission denied",
        )
            .into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod header_name_tests {
    use energy_api::transport::content_security as cs;

    /// The headers `makod` reads are the ones `energy-api` writes: the
    /// published names of Kap. 3.3, compared case-insensitively as HTTP does.
    #[test]
    fn the_verified_headers_are_the_ones_energy_api_signs() {
        for (read, written) in [
            (super::HEADER_BDEW_DIGEST, cs::HEADER_DIGEST),
            (super::HEADER_BDEW_SIGNATURE, cs::HEADER_SIGNATURE),
            (super::HEADER_BDEW_CERT, cs::HEADER_CERT),
        ] {
            assert!(read.eq_ignore_ascii_case(written), "{read} vs {written}");
        }
    }
}

#[cfg(test)]
mod retry_tests {
    //! A request whose work fails after its inbox claim must be processed when
    //! the caller retries the same `transactionId`; one whose work completed
    //! must answer the retry as a duplicate.
    //!
    //! The order is the whole mechanism: a handler that accepts the claim
    //! before the work succeeds answers every retry of a failed request as a
    //! duplicate of work that never happened, and the caller stops retrying.

    use energy_api::server::control_measures::ControlMeasuresHandler as _;
    use energy_api::server::malo_ident::MaloIdentHandler as _;
    use energy_api::server::wim_order::WimOrderHandler as _;
    use mako_engine::outbox::OutboxStore as _;
    use mako_engine::registry::ProcessRegistry as _;

    use super::*;

    const TENANT: &str = "9900357000004";
    const CALLER: &str = "9900001000001";
    const TX: &str = "f81d4fae-7dec-11d0-a765-00a0c91e6bf6";

    async fn store() -> SlateDbStore {
        SlateDbStore::open_in_memory()
            .await
            .expect("open in-memory SlateDB")
    }

    fn handler(store: SlateDbStore) -> MakodApiHandler {
        MakodApiHandler {
            store,
            tenant_id: TenantId::from_party_id(TENANT),
        }
    }

    /// Every write to a fresh stream is refused, so a spawn fails in the
    /// store, after the claim.
    fn failing(store: &SlateDbStore) -> MakodApiHandler {
        handler(store.clone().with_max_stream_events(0))
    }

    async fn processes_under(store: &SlateDbStore, key: &str) -> usize {
        store
            .as_process_registry()
            .lookup_correlated(TenantId::from_party_id(TENANT), key)
            .await
            .expect("lookup")
            .len()
    }

    fn location() -> energy_api::models::electricity::LocationId {
        serde_json::from_value(serde_json::json!("E1234848431")).expect("NeLo")
    }

    fn control() -> energy_api::models::electricity::CommandControl {
        serde_json::from_value(serde_json::json!({
            "maximumPowerValue": "4.2",
            "executionTimeFrom": "2026-09-01T00:00:00Z",
        }))
        .expect("CommandControl")
    }

    fn regular() -> energy_api::models::electricity::CommandRegular {
        serde_json::from_value(serde_json::json!({
            "executionTimeFrom": "2026-09-01T00:00:00Z",
        }))
        .expect("CommandRegular")
    }

    fn anmeldung() -> WimAnmeldungRequest {
        serde_json::from_value(serde_json::json!({
            "meloId": "DE0001234567890123456789012345678",
            "netzbetreiberId": CALLER.parse::<i64>().expect("13 digits"),
            "processDate": "2026-10-01",
            "deviceCategory": "iMSys",
        }))
        .expect("WimAnmeldungRequest")
    }

    fn identification() -> IdentificationParameter {
        serde_json::from_value(serde_json::json!({
            "identificationDateTime": "2026-09-30T22:00:00Z",
            "energyDirection": "consumption",
            "identificationParameterAddress": {},
        }))
        .expect("IdentificationParameter")
    }

    /// Run `call` with the caller identity in scope. Pass an `async` block
    /// that makes the handler call: the handlers read the identity when they
    /// are called, not when their future is polled.
    async fn as_caller<F: std::future::Future>(call: F) -> F::Output {
        CLIENT_MP_ID.scope(Some(CALLER.to_owned()), call).await
    }

    #[tokio::test]
    async fn konfiguration_is_processed_on_retry_after_a_failure() {
        let store = store().await;
        let first = as_caller(async {
            failing(&store)
                .on_konfiguration(TX.into(), String::new(), location(), control())
                .await
        })
        .await;
        assert!(first.is_err(), "the spawn failed, so the request failed");
        assert_eq!(processes_under(&store, TX).await, 0);

        as_caller(async {
            handler(store.clone())
                .on_konfiguration(TX.into(), String::new(), location(), control())
                .await
        })
        .await
        .expect("the retry is processed, not answered as a duplicate");
        assert_eq!(processes_under(&store, TX).await, 1);

        as_caller(async {
            handler(store.clone())
                .on_konfiguration(TX.into(), String::new(), location(), control())
                .await
        })
        .await
        .expect("a completed request answers its retry as a duplicate");
        assert_eq!(processes_under(&store, TX).await, 1, "no second process");
    }

    #[tokio::test]
    async fn initial_zustand_is_processed_on_retry_after_a_failure() {
        let store = store().await;
        let first = as_caller(async {
            failing(&store)
                .on_initial_zustand(TX.into(), String::new(), location(), regular())
                .await
        })
        .await;
        assert!(first.is_err());
        assert_eq!(processes_under(&store, TX).await, 0);

        as_caller(async {
            handler(store.clone())
                .on_initial_zustand(TX.into(), String::new(), location(), regular())
                .await
        })
        .await
        .expect("the retry is processed");
        assert_eq!(processes_under(&store, TX).await, 1);

        as_caller(async {
            handler(store.clone())
                .on_initial_zustand(TX.into(), String::new(), location(), regular())
                .await
        })
        .await
        .expect("duplicate");
        assert_eq!(processes_under(&store, TX).await, 1);
    }

    #[tokio::test]
    async fn wim_anmeldung_is_processed_on_retry_after_a_failure() {
        let store = store().await;
        let first = as_caller(async {
            failing(&store)
                .on_anmeldung(TX.into(), String::new(), anmeldung())
                .await
        })
        .await;
        assert!(first.is_err());
        assert_eq!(processes_under(&store, TX).await, 0);

        as_caller(async {
            handler(store.clone())
                .on_anmeldung(TX.into(), String::new(), anmeldung())
                .await
        })
        .await
        .expect("the retry is processed");
        assert_eq!(processes_under(&store, TX).await, 1);

        as_caller(async {
            handler(store.clone())
                .on_anmeldung(TX.into(), String::new(), anmeldung())
                .await
        })
        .await
        .expect("duplicate");
        assert_eq!(processes_under(&store, TX).await, 1);
    }

    #[tokio::test]
    async fn malo_ident_request_is_queued_on_retry_after_a_failure() {
        let store = store().await;
        let queued = |store: SlateDbStore| async move {
            store.pending_now(100).await.expect("outbox").len()
        };

        let first = OUTBOX_FAULT
            .scope(
                true,
                handler(store.clone()).on_request(
                    TX.into(),
                    String::new(),
                    CALLER.into(),
                    identification(),
                ),
            )
            .await;
        assert!(first.is_err(), "the enqueue failed, so the request failed");
        assert_eq!(queued(store.clone()).await, 0);

        handler(store.clone())
            .on_request(TX.into(), String::new(), CALLER.into(), identification())
            .await
            .expect("the retry is processed");
        assert_eq!(queued(store.clone()).await, 1);

        handler(store.clone())
            .on_request(TX.into(), String::new(), CALLER.into(), identification())
            .await
            .expect("duplicate");
        assert_eq!(queued(store.clone()).await, 1, "no second callback");
    }
}

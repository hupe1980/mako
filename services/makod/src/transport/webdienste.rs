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
//! | API                    | Path prefix              | Handler trait             | Status |
//! |------------------------|--------------------------|---------------------------|--------|
//! | Control Measures v1    | `/steuerbefehl/`         | `ControlMeasuresHandler`  | ✅ wired |
//! | MaLo Identification v1 | `/maloId/`               | `MaloIdentHandler`        | ✅ active |
//! | WiM Order v1           | `/wimBestellung/v1/`     | `WimOrderHandler`         | ✅ wired |
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
use mako_engine::deadline::Deadline;
use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
use mako_engine::inbox::InboxStore as _;
use mako_engine::outbox::{OutboxMessage, OutboxStore as _};
use mako_engine::registry::ProcessRegistry as _;
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
use tracing::{info, warn};

// ── Caller identity ───────────────────────────────────────────────────────────

/// Header naming the Marktpartner-ID of the calling market participant.
///
/// The BDEW API-Webdienste identify the caller by their mTLS client
/// certificate, which is validated and terminated by the fronting proxy. The
/// proxy forwards the certificate's Marktpartner-ID in this header; nothing in
/// the request body carries it, because the specification does not put it there.
pub const CLIENT_MP_ID_HEADER: &str = "x-mako-client-mp-id";

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
/// proxy. A `sender_party_id` field used to stand in for it and recorded the
/// operator's own code as the sender of orders it received.
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
            let is_new = store
                .as_inbox_store()
                .accept(&inbox_key)
                .await
                .map_err(|e| energy_api::Error::Http {
                    status: 500,
                    body: format!("inbox error: {e}"),
                })?;

            if !is_new {
                info!(
                    tx_id,
                    "duplicate MaLo-ID request — returning early (idempotent)"
                );
                return Ok(());
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
            store.enqueue(&[msg]).await.map_err(|e| {
                tracing::error!("outbox enqueue error: {e}");
                energy_api::Error::Http {
                    status: 500,
                    body: "internal error".to_string(),
                }
            })?;

            info!(
                tx_id,
                "MaLo-ID request accepted and queued for async lookup"
            );
            Ok(())
        }
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
            let is_new = store
                .as_inbox_store()
                .accept(&inbox_key)
                .await
                .map_err(|e| energy_api::Error::Http {
                    status: 500,
                    body: format!("inbox error: {e}"),
                })?;
            if !is_new {
                info!(
                    tx_id,
                    "duplicate Steuerungsauftrag konfiguration — returning early (idempotent)"
                );
                return Ok(());
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

            spawn_steuerungsauftrag(store, tenant_id, &tx_id, domain_cmd)
                .await
                .map_err(|e| energy_api::Error::Http {
                    status: 500,
                    body: e.to_string(),
                })?;

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
            let is_new = store
                .as_inbox_store()
                .accept(&inbox_key)
                .await
                .map_err(|e| energy_api::Error::Http {
                    status: 500,
                    body: format!("inbox error: {e}"),
                })?;
            if !is_new {
                info!(
                    tx_id,
                    "duplicate Steuerungsauftrag initialZustand — returning early (idempotent)"
                );
                return Ok(());
            }

            let domain_cmd = SteuerungsauftragCommand::ReceiveInitialZustand {
                tx_id: tx_id.clone(),
                sender_mp_id: party_id_to_marktpartner(sender),
                location_id: location_id_to_domain(&location_id),
                execution_time_from: command.execution_time_from.clone(),
            };

            spawn_steuerungsauftrag(store, tenant_id, &tx_id, domain_cmd)
                .await
                .map_err(|e| energy_api::Error::Http {
                    status: 500,
                    body: e.to_string(),
                })?;

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
    /// 5. Registers the per-PID response deadline (BDEW WiM / BK6-22-024).
    /// 6. Registers a correlated index under `tx_id` for later ERP lookup.
    /// 7. Returns `Ok(())` → axum sends `202 Accepted`.
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
            let is_new = store
                .as_inbox_store()
                .accept(&inbox_key)
                .await
                .map_err(|e| energy_api::Error::Http {
                    status: 500,
                    body: format!("inbox error: {e}"),
                })?;
            if !is_new {
                info!(
                    tx_id,
                    "duplicate WiM Anmeldung — returning early (idempotent)"
                );
                return Ok(());
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

            let process_id = spawn_device_change(store, tenant_id, &tx_id, domain_cmd)
                .await
                .map_err(|e| energy_api::Error::Http {
                    status: 500,
                    body: e.to_string(),
                })?;

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

/// Spawn a new `WimDeviceChangeWorkflow` process for an inbound REST order
/// and execute the first command.
///
/// Used by the WiM Order API (`/wimBestellung/v1/anmeldung/`).
/// Registers a 5-Werktage deadline and a correlated index under `tx_id`.
/// The business-answer Frist for the order this command opens.
///
/// The MSB-Wechsel windows differ per Prüfidentifikator (BK6-22-024 WiM Teil 1):
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

async fn spawn_device_change(
    store: SlateDbStore,
    tenant_id: TenantId,
    tx_id: &str,
    command: DeviceChangeCommand,
) -> Result<ProcessId, mako_engine::error::EngineError> {
    use mako_engine::version::WorkflowId;

    let fv = latest_format_version();
    let workflow_id = WorkflowId::new(DEVICE_CHANGE_WORKFLOW_NAME, fv);

    let store_arc = std::sync::Arc::new(store.clone());
    let process = mako_engine::process::Process::<
        WimDeviceChangeWorkflow,
        std::sync::Arc<SlateDbStore>,
    >::new(
        std::sync::Arc::clone(&store_arc),
        tenant_id,
        workflow_id.clone(),
    );

    let process_id = process.process_id();
    let stream_id = process.stream_id().clone();
    let identity = process.identity();

    // Business-answer Frist, sized from the order's own Prüfidentifikator —
    // 55039 → 3 WT, 55042 → 5 WT, 55051 → 7 WT, 55168 → 1 WT (BK6-22-024 WiM
    // Teil 1, Kap. 2.2.2 / 2.3.2 / 2.4.2 / 2.5.2). This REST door has to agree
    // with the AS4 door; a flat 5 WT here would give the same order two
    // different deadlines depending on which transport it arrived on.
    //
    // deadline_at_werktage computes 17:00 Europe/Berlin on the due Werktag,
    // correctly handling CET/CEST transitions.
    let frist_wt = device_change_frist_wt(&command);
    let due_at = mako_fristen::deadline_at_werktage(
        time::OffsetDateTime::now_utc(),
        frist_wt,
        mako_fristen::HolidayCalendar::BdewMaKo,
    );
    let deadline = Deadline::new(
        stream_id,
        process_id,
        tenant_id,
        workflow_id,
        device_change_window_label(&command),
        due_at,
    );
    // Atomically persist events + deadline in one WriteBatch (F-043 fix).
    // A crash between separate writes would lose the deadline permanently.
    process
        .execute_and_enqueue_with_deadlines(command, &[deadline])
        .await?;

    // Register correlated index so ERP commands can look up this process
    // by tx_id via `ProcessRegistry::find_correlated`.
    if let Err(e) = store
        .as_process_registry()
        .register_correlated(tenant_id, tx_id, process_id, identity)
        .await
    {
        warn!(
            tx_id,
            process_id = %process_id,
            error = %e,
            "WiM Anmeldung: business-key registration failed \
             (non-fatal — process spawned; ERP correlation will fail)"
        );
    }

    Ok(process_id)
}

///
/// Uses the latest BDEW format version from the compiled `edi-energy` registry.
/// Also registers the Steuerungsauftrag's 5-Werktage confirmation deadline
/// (BDEW WiM / BK6-22-024).
async fn spawn_steuerungsauftrag(
    store: SlateDbStore,
    tenant_id: TenantId,
    tx_id: &str,
    command: SteuerungsauftragCommand,
) -> Result<ProcessId, mako_engine::error::EngineError> {
    use mako_engine::version::WorkflowId;

    let fv = latest_format_version();
    let workflow_id = WorkflowId::new(STEUERUNGSAUFTRAG_WORKFLOW_NAME, fv);

    let store_arc = std::sync::Arc::new(store.clone());
    let process = mako_engine::process::Process::<
        WimSteuerungsauftragWorkflow,
        std::sync::Arc<SlateDbStore>,
    >::new(
        std::sync::Arc::clone(&store_arc),
        tenant_id,
        workflow_id.clone(),
    );

    let process_id = process.process_id();
    let stream_id = process.stream_id().clone();
    // Capture identity before consume-by-execute.
    let identity = process.identity();

    // The Steuerungsauftrag has its own flat 5-Werktage confirmation window —
    // unrelated to the per-PID MSB-Wechsel Fristen above.
    // deadline_at_werktage computes 17:00 Europe/Berlin on the due Werktag,
    // correctly handling CET/CEST transitions.
    let due_at = mako_fristen::deadline_at_werktage(
        time::OffsetDateTime::now_utc(),
        5,
        mako_fristen::HolidayCalendar::BdewMaKo,
    );
    let deadline = Deadline::new(
        stream_id,
        process_id,
        tenant_id,
        workflow_id,
        mako_wim::steuerungsauftrag::STEUERUNGSAUFTRAG_DEADLINE_LABEL,
        due_at,
    );
    // Atomically persist events + deadline in one WriteBatch (F-043 fix).
    // A crash between separate writes would lose the deadline permanently.
    process
        .execute_and_enqueue_with_deadlines(command, &[deadline])
        .await?;

    // Register the process under the tx_id business key so that ERP commands
    // `wim.steuerungsauftrag.bestaetigen` / `.ablehnen` can look it up via the
    // `ProcessRegistry` correlated index.
    if let Err(e) = store
        .as_process_registry()
        .register_correlated(tenant_id, tx_id, process_id, identity)
        .await
    {
        warn!(
            tx_id,
            process_id = %process_id,
            error = %e,
            "Steuerungsauftrag: business-key registration failed \
             (non-fatal — process was spawned; bestaetigen/ablehnen will fail until re-registered)"
        );
    }

    Ok(process_id)
}

/// Latest BDEW format version from the `edi-energy` registry.
fn latest_format_version() -> mako_engine::version::FormatVersion {
    edi_energy::registry::ReleaseRegistry::global()
        .format_versions()
        .into_iter()
        .filter_map(|s| mako_engine::version::FormatVersion::parse(&s).ok())
        .max_by(|a, b| a.as_str().cmp(b.as_str()))
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
/// Health routes are deliberately **not** included: the caller merges them
/// afterwards so that Kubernetes probes stay reachable without a token.
pub fn build_app(
    handler: Arc<MakodApiHandler>,
    auth: Option<WebdiensteAuthState>,
    max_body_bytes: usize,
) -> Router {
    let routes = router(handler)
        .layer(axum::extract::DefaultBodyLimit::max(max_body_bytes))
        // Below the auth layer, so the identity is in scope for the handlers
        // whether or not `makod` itself checks a bearer token.
        .layer(axum::middleware::from_fn(client_identity_middleware));
    match auth {
        Some(state) => routes.layer(axum::middleware::from_fn_with_state(
            state,
            webdienste_auth_middleware,
        )),
        None => routes,
    }
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
pub async fn client_identity_middleware(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mp_id = request
        .headers()
        .get(CLIENT_MP_ID_HEADER)
        .and_then(|v| v.to_str().ok())
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

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
//! ## Authentication
//!
//! **Production requirement**: BDEW API-Webdienste Strom requires mTLS with
//! certificates issued by the BDEW PKI CA. Deploy this port behind a reverse
//! proxy (e.g. Nginx, Envoy, AWS ALB with mutual TLS) that validates the
//! client certificate against the BDEW PKI CA before forwarding requests.
//!
//! At startup, `makod` logs a warning when `--api-webdienste-addr` is set
//! without an mTLS guard:
//!
//! ```text
//! WARN makod: API-Webdienste Strom port started WITHOUT authentication. \
//!      BDEW API-Webdienste requires mTLS (BDEW PKI CA). Deploy behind a \
//!      reverse proxy with mTLS termination before exposing to public networks.
//! ```
//!
//! Internal deployments behind a service mesh or VPC with network-level
//! access controls enforced may operate without mTLS if the threat model
//! permits unauthenticated inbound requests from network-adjacent peers.

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

// ── MakodApiHandler ───────────────────────────────────────────────────────────

/// Handler state for the API-Webdienste Strom server.
///
/// - `store`          — shared SlateDB instance for inbox idempotency, outbox
///   persistence, and event-sourced workflow dispatch.
/// - `tenant_id`      — the operator's [`TenantId`], derived from their BDEW
///   code / GLN via [`TenantId::from_party_id`].
/// - `sender_party_id` — GLN string used as the MSB's `from` party in control
///   measure responses.
#[derive(Clone)]
pub struct MakodApiHandler {
    pub store: SlateDbStore,
    pub tenant_id: TenantId,
    pub sender_party_id: String,
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
        let sender_party_id = self.sender_party_id.clone();
        async move {
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
                sender_mp_id: party_id_to_marktpartner(sender_party_id),
                location_id: location_id_to_domain(&location_id),
                execution_time_from: command.execution_time_from.clone(),
                max_power_kw: command.maximum_power_value.0.clone(),
                execution_time_until: command.execution_time_until.clone(),
                // The `produkt_code` is not carried in the API-Webdienste Strom
                // REST request body — it is part of the AS4 ORDERS message only.
                // When present, it would be extracted by the EDIFACT adapter.
                // Set to None here so the M1 guard is skipped for REST-originated
                // commands (operators using the direct REST API are trusted).
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
        let sender_party_id = self.sender_party_id.clone();
        async move {
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
                sender_mp_id: party_id_to_marktpartner(sender_party_id),
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
    /// 1. Inbox idempotency guard — rejects duplicate `tx_id` values.
    /// 2. Converts the REST payload to a `DeviceChangeCommand::ReceiveRestOrder`.
    /// 3. Spawns a `WimDeviceChangeWorkflow` process.
    /// 4. Registers a 5-Werktage response deadline (BDEW WiM / BK6-18-032).
    /// 5. Registers a correlated index under `tx_id` for later ERP lookup.
    /// 6. Returns `Ok(())` → axum sends `202 Accepted`.
    fn on_anmeldung(
        &self,
        tx_id: String,
        _creation_dt: String,
        request: WimAnmeldungRequest,
    ) -> impl std::future::Future<Output = Result<(), energy_api::Error>> + Send {
        let store = self.store.clone();
        let tenant_id = self.tenant_id;
        async move {
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

            let sender_mp_id = party_id_to_marktpartner(request.netzbetreiber_id.to_string());
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

    // Build 5-Werktage deadline before the atomic write (BK6-24-174).
    // deadline_at_werktage computes 17:00 Europe/Berlin on the due Werktag,
    // correctly handling CET/CEST transitions.
    let due_at = mako_engine::fristen::deadline_at_werktage(
        time::OffsetDateTime::now_utc(),
        5,
        mako_engine::fristen::HolidayCalendar::BdewMaKo,
    );
    let deadline = Deadline::new(
        stream_id,
        process_id,
        tenant_id,
        workflow_id,
        "wim-anmeldung-antwort-5-werktage",
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
/// Also registers a 5-Werktage deadline (BDEW WiM / BK6-18-032).
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

    // Build 5-Werktage deadline before the atomic write (BK6-24-174).
    // deadline_at_werktage computes 17:00 Europe/Berlin on the due Werktag,
    // correctly handling CET/CEST transitions.
    let due_at = mako_engine::fristen::deadline_at_werktage(
        time::OffsetDateTime::now_utc(),
        5,
        mako_engine::fristen::HolidayCalendar::BdewMaKo,
    );
    let deadline = Deadline::new(
        stream_id,
        process_id,
        tenant_id,
        workflow_id,
        "steuerungsauftrag-antwort-5-werktage",
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

/// Bearer/OIDC authentication state for the `:8090` API-Webdienste port.
#[derive(Clone)]
pub struct WebdiensteAuthState {
    /// Cedar-based authenticator/authorizer.
    pub cedar: Arc<crate::cedar_authz::CedarAuthorizer>,
    /// Operator tenant (GLN) — the Cedar resource scope.
    pub tenant: Arc<str>,
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

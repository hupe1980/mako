//! §20b EnWG Netzzugangsplattform adapter — outbox sender + command layer.
//!
//! ## Regulatory basis
//!
//! §20b EnWG (inserted by G. v. 18.12.2025, BGBl. 2025 I Nr. 347, in force
//! 23.12.2025) obliges the Netzbetreiber to operate a joint nationwide
//! internet platform carrying, at minimum (Abs. 2):
//!
//! 1. erstmalige Bestellung / Änderung / Abbestellung von
//!    **Zählpunktanordnungen** (umgangssprachlich Messkonzepte) hinter einem
//!    Netzanschluss,
//! 2. erstmalige Bestellung / Änderung / Abbestellung von
//!    **Verrechnungskonzepten** (Verrechnungsformeln), and
//! 3. die **Registrierung von Vereinbarungen nach §42c** (Energy Sharing).
//!
//! The statute sets **no dates** — timing, Anwendungsfälle, Nutzergruppen and
//! Berechtigungskonzepte are BNetzA Festlegungskompetenz (Abs. 3), and no
//! Festlegung has been issued as of 2026-07. **No platform or published API
//! exists yet**; the statutory minimum interface is a Webportal, an API "soll
//! Berücksichtigung finden" (RefE-Begründung).
//!
//! ## Design
//!
//! mako is the **client** side (acting for an Anschlussnehmer/-nutzer or a
//! §20-Anspruchsberechtigter — e.g. a Mieterstrom/Energy-Sharing operator).
//! The adapter is therefore transport-agnostic with reliable delivery:
//!
//! - The `netzzugang.*` commands validate a request, project it into the
//!   marktd registry (`netzzugang_antraege`, status `erfasst`) and enqueue a
//!   [`NETZZUGANG_MESSAGE_TYPE`] outbox message — the same at-least-once
//!   machinery every market message uses.
//! - [`NetzzugangSender`] delivers the message: to the configured platform
//!   endpoint (`--netzzugang-endpoint-url`) once one exists, or — while the
//!   platform does not — to the ERP webhook as a
//!   `de.mako.netzzugang.uebermittlungsbedarf` CloudEvent so the operator can
//!   submit via the Netzbetreiber's Webportal (the statutory minimum channel).
//! - After delivery the sender advances the marktd projection to
//!   `uebermittelt` (or `fehlgeschlagen` when no channel is configured).
//!
//! When the BNetzA Festlegung publishes a real interface, only the endpoint
//! transport inside [`NetzzugangSender`] changes; commands, registry and
//! events stay stable.

use std::sync::Arc;

use mako_engine::error::EngineError;
use mako_engine::outbox::OutboxMessage;
use mako_markt::marktd_client::MarktdClient;
use mako_markt::repository::{NetzzugangAntrag, NetzzugangStatus};
use reqwest::Client;
use secrecy::{ExposeSecret as _, SecretString};

/// Outbox `message_type` for §20b requests.
pub const NETZZUGANG_MESSAGE_TYPE: &str = "NetzzugangAntrag";

/// CloudEvents type delivered to the ERP webhook while no platform endpoint
/// is configured: the operator must submit the request via the NB Webportal.
pub const CE_TYPE_UEBERMITTLUNGSBEDARF: &str = mako_events::mako::NETZZUGANG_UEBERMITTLUNGSBEDARF;

/// Delivers [`NETZZUGANG_MESSAGE_TYPE`] outbox messages.
///
/// See the module docs for the transport decision tree.
#[derive(Clone)]
pub struct NetzzugangSender {
    http: Client,
    /// Platform endpoint — absent until a §20b interface exists.
    endpoint_url: Option<Arc<str>>,
    /// ERP webhook for the operator-submits-manually fallback.
    webhook_url: Option<Arc<str>>,
    /// Shared secret for the ERP webhook — when set, the fallback CloudEvent
    /// body is signed with HMAC-SHA256 (`X-Mako-Signature`), matching
    /// [`crate::erp_adapter::WebhookErpAdapter`].
    webhook_secret: Option<Arc<SecretString>>,
    /// marktd projection (best-effort; delivery does not depend on it).
    marktd: Option<Arc<MarktdClient>>,
}

impl NetzzugangSender {
    #[must_use]
    pub fn new(
        http: Client,
        endpoint_url: Option<String>,
        webhook_url: Option<String>,
        webhook_secret: Option<SecretString>,
        marktd: Option<Arc<MarktdClient>>,
    ) -> Self {
        Self {
            http,
            endpoint_url: endpoint_url.map(Into::into),
            webhook_url: webhook_url.map(Into::into),
            webhook_secret: webhook_secret.map(Arc::new),
            marktd,
        }
    }

    /// Best-effort projection update in marktd — never fails the delivery.
    async fn project_status(
        &self,
        antrag_id: uuid::Uuid,
        status: NetzzugangStatus,
        platform_ref: Option<&str>,
    ) {
        if let Some(marktd) = &self.marktd
            && let Err(e) = marktd
                .set_netzzugang_status(antrag_id, status, platform_ref)
                .await
        {
            tracing::warn!(
                antrag_id = %antrag_id,
                error = %e,
                "netzzugang: marktd projection update failed (non-fatal)",
            );
        }
    }

    /// Deliver one §20b outbox message.
    ///
    /// # Errors
    ///
    /// Returns an [`EngineError`] when a configured channel is unreachable —
    /// the outbox worker retries. Permanent failures — a missing channel
    /// configuration or a malformed stored payload — are acknowledged (with
    /// the projection set to `fehlgeschlagen`, best-effort) instead of
    /// poison-looping.
    pub async fn send(&self, msg: &OutboxMessage) -> Result<(), EngineError> {
        // A malformed stored payload can never succeed on retry — log, mark
        // the projection fehlgeschlagen (best-effort, if the raw payload still
        // carries a usable id) and acknowledge instead of poison-looping.
        let antrag: NetzzugangAntrag = match serde_json::from_value(msg.payload.clone()) {
            Ok(antrag) => antrag,
            Err(e) => {
                tracing::error!(
                    message_id = %msg.message_id,
                    error = %e,
                    "netzzugang: malformed NetzzugangAntrag payload — permanent \
                     failure, acknowledging (request marked fehlgeschlagen)",
                );
                if let Some(id) = msg
                    .payload
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|s| uuid::Uuid::parse_str(s).ok())
                {
                    self.project_status(id, NetzzugangStatus::Fehlgeschlagen, None)
                        .await;
                }
                return Ok(());
            }
        };

        // ── Path 1: real platform endpoint ───────────────────────────────────
        if let Some(endpoint) = &self.endpoint_url {
            let resp = self
                .http
                .post(endpoint.as_ref())
                .json(&antrag)
                .send()
                .await
                .map_err(|e| EngineError::store(format!("netzzugang endpoint: {e}")))?;
            if !resp.status().is_success() {
                return Err(EngineError::store(format!(
                    "netzzugang endpoint returned {}",
                    resp.status()
                )));
            }
            // The (future) platform may assign a reference.
            let platform_ref = resp.json::<serde_json::Value>().await.ok().and_then(|v| {
                v.get("platform_ref")
                    .and_then(|r| r.as_str().map(String::from))
            });
            self.project_status(
                antrag.id,
                NetzzugangStatus::Uebermittelt,
                platform_ref.as_deref(),
            )
            .await;
            return Ok(());
        }

        // ── Path 2: ERP webhook — operator submits via the NB Webportal ──────
        if let Some(webhook) = &self.webhook_url {
            let body = serde_json::json!({
                "specversion": "1.0",
                "type": CE_TYPE_UEBERMITTLUNGSBEDARF,
                "source": "urn:mako:netzzugang",
                "id": msg.message_id.to_string(),
                "subject": antrag.id.to_string(),
                "time": time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
                "datacontenttype": "application/json",
                "data": antrag,
            });
            // Serialize once so the (optional) HMAC signature covers the raw
            // bytes actually sent — same construction and header as
            // `WebhookErpAdapter` (`X-Mako-Signature: HMAC-SHA256 hex`).
            let body_bytes = serde_json::to_vec(&body).map_err(|e| {
                EngineError::Deserialization(format!("netzzugang CloudEvent serialize: {e}"))
            })?;
            let mut builder = self
                .http
                .post(webhook.as_ref())
                .header("Content-Type", "application/json");
            if let Some(secret) = &self.webhook_secret {
                let sig =
                    mako_service::webhook::sign(secret.expose_secret().as_bytes(), &body_bytes);
                builder = builder.header("X-Mako-Signature", sig);
            }
            let resp = builder
                .body(body_bytes)
                .send()
                .await
                .map_err(|e| EngineError::store(format!("netzzugang webhook: {e}")))?;
            if !resp.status().is_success() {
                return Err(EngineError::store(format!(
                    "netzzugang webhook returned {}",
                    resp.status()
                )));
            }
            self.project_status(antrag.id, NetzzugangStatus::Uebermittelt, None)
                .await;
            return Ok(());
        }

        // ── No channel configured — acknowledge, don't poison-loop ───────────
        tracing::error!(
            antrag_id = %antrag.id,
            "netzzugang: neither --netzzugang-endpoint-url nor an ERP webhook \
             is configured — request marked fehlgeschlagen",
        );
        self.project_status(antrag.id, NetzzugangStatus::Fehlgeschlagen, None)
            .await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
    use mako_markt::repository::{NetzzugangAktion, NetzzugangAntragTyp};

    fn antrag() -> NetzzugangAntrag {
        NetzzugangAntrag {
            id: uuid::Uuid::new_v4(),
            tenant: "9900000000004".into(),
            antrag_typ: NetzzugangAntragTyp::EnergySharingVereinbarung,
            aktion: NetzzugangAktion::Registrierung,
            netzanschluss_id: "NA-0001".into(),
            nb_mp_id: "9900001000001".into(),
            antragsteller_ref: "kunde-42".into(),
            status: NetzzugangStatus::Erfasst,
            payload: serde_json::json!({ "vereinbarung_ref": "ES-2026-001" }),
            platform_ref: None,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            submitted_at: None,
        }
    }

    fn outbox_msg(payload: serde_json::Value) -> OutboxMessage {
        OutboxMessage::new(
            StreamId::new("netzzugang/test"),
            ProcessId::new(),
            TenantId::from_party_id("9900000000004"),
            CorrelationId::new(),
            ConversationId::new(),
            EventId::new(),
            NETZZUGANG_MESSAGE_TYPE,
            "internal://netzzugang",
            payload,
        )
    }

    /// Without any configured channel the message is acknowledged (no retry
    /// loop) — misconfiguration must not poison the outbox.
    #[tokio::test]
    async fn unconfigured_sender_acknowledges_instead_of_poison_looping() {
        let sender = NetzzugangSender::new(Client::new(), None, None, None, None);
        let msg = outbox_msg(serde_json::to_value(antrag()).expect("serialize"));
        sender.send(&msg).await.expect("acknowledged");
    }

    /// A malformed stored payload can never succeed on retry — it is a
    /// permanent failure: logged, projected `fehlgeschlagen` (best-effort)
    /// and acknowledged so the outbox does not poison-loop.
    #[tokio::test]
    async fn malformed_payload_is_acknowledged_not_retried() {
        let sender = NetzzugangSender::new(Client::new(), None, None, None, None);
        let msg = outbox_msg(serde_json::json!({ "not": "an antrag" }));
        sender.send(&msg).await.expect("acknowledged");
    }

    /// Spin up a local HTTP server that captures one request (signature
    /// header + raw body) from the Path-2 webhook delivery.
    async fn capture_one_webhook_request() -> (
        std::net::SocketAddr,
        Arc<std::sync::Mutex<Option<(Option<String>, Vec<u8>)>>>,
    ) {
        use axum::routing::post;

        type Captured = Arc<std::sync::Mutex<Option<(Option<String>, Vec<u8>)>>>;
        let captured: Captured = Arc::new(std::sync::Mutex::new(None));
        let state = Arc::clone(&captured);
        let app = axum::Router::new().route(
            "/hook",
            post(
                move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                    let state = Arc::clone(&state);
                    async move {
                        let sig = headers
                            .get("X-Mako-Signature")
                            .and_then(|v| v.to_str().ok())
                            .map(String::from);
                        *state.lock().expect("lock") = Some((sig, body.to_vec()));
                        "ok"
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (addr, captured)
    }

    /// The Path-2 CloudEvent POST carries `X-Mako-Signature` — HMAC-SHA256 of
    /// the raw body with the configured secret, exactly like the general ERP
    /// webhook adapter.
    #[tokio::test]
    async fn webhook_cloudevent_is_hmac_signed_when_secret_configured() {
        let (addr, captured) = capture_one_webhook_request().await;

        let sender = NetzzugangSender::new(
            Client::new(),
            None,
            Some(format!("http://{addr}/hook")),
            Some("test-secret".into()),
            None,
        );
        let msg = outbox_msg(serde_json::to_value(antrag()).expect("serialize"));
        sender.send(&msg).await.expect("delivered");

        let (sig, body) = captured
            .lock()
            .expect("lock")
            .take()
            .expect("request captured");
        let sig = sig.expect("X-Mako-Signature header present");
        assert_eq!(
            sig,
            mako_service::webhook::sign(b"test-secret", &body),
            "signature must be the canonical sha256=<hex> of the raw body",
        );
        // Sanity: the signed body is the expected CloudEvent.
        let ce: serde_json::Value = serde_json::from_slice(&body).expect("CloudEvent JSON");
        assert_eq!(ce["type"], CE_TYPE_UEBERMITTLUNGSBEDARF);
    }

    /// Without a secret the fallback stays unsigned (current behaviour for
    /// deployments that have not configured `--erp-webhook-secret`).
    #[tokio::test]
    async fn webhook_cloudevent_is_unsigned_without_secret() {
        let (addr, captured) = capture_one_webhook_request().await;

        let sender = NetzzugangSender::new(
            Client::new(),
            None,
            Some(format!("http://{addr}/hook")),
            None,
            None,
        );
        let msg = outbox_msg(serde_json::to_value(antrag()).expect("serialize"));
        sender.send(&msg).await.expect("delivered");

        let (sig, _body) = captured
            .lock()
            .expect("lock")
            .take()
            .expect("request captured");
        assert!(sig.is_none(), "no secret configured — no signature header");
    }
}

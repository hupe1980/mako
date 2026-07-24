//! Outbox sender that delivers `MaloIdentCallback` messages.
//!
//! ## Role
//!
//! When `makod` enqueues a `MaloIdentCallback` message (from
//! `POST /maloId/request/v1`), the `OutboxWorker` calls
//! `MaloIdentSender::send` for that message. This sender:
//!
//! 1. Deserialises the payload `{ tx_id, tenant_id, sender_market_partner_id, params }`.
//! 2. Looks up the MaLo record in [`SlateDbMaloCache`].
//! 3. Resolves the LF's callback URL using one of two paths (in priority order):
//!    a. The static `partner_urls` override map (keyed by GLN) — for testing and Verzeichnisdienst outages.
//!    b. A [`VerzeichnisdienstLookup`] that queries the BDEW Verzeichnisdienst and caches the result in the `SlateDbPartnerStore`.
//! 4. Calls `MaloIdentClient::send_positive_response` or
//!    `MaloIdentClient::send_negative_response` on the LF's callback endpoint.
//! 5. On a positive callback, writes the resolved `(tx_id, malo_id, nb_mp_id)`
//!    to the [`MaloIdentResultCache`] (`mc_txres/` prefix) so the ERP can
//!    trigger `maloid.lieferbeginn.fortsetzen` without knowing the MaLo-ID
//!    in advance.
//!
//! ## URL resolution priority
//!
//! | Priority | Source |
//! |---|---|
//! | 1 | `--maloid-partner GLN=URL` CLI override (static map) |
//! | 2 | `PartnerStore` `"AW"` channel (previously cached Verzeichnisdienst entry) |
//! | 3 | Live Verzeichnisdienst lookup via `DirectoryServiceClient` |
//!
//! All other message types fall through to a log-and-acknowledge path.

use std::collections::HashMap;
use std::sync::Arc;

use energy_api::client::malo_ident::MaloIdentClient;
use energy_api::models::electricity::{IdentificationParameter, MaloIdentResultNegative};
use energy_api::server::malo_ident::MaloRegistry as _;
use mako_engine::builder::As4Sender;
use mako_engine::error::EngineError;
use mako_engine::ids::OutboxMessageId;
use mako_engine::outbox::{OutboxMessage, OutboxStore as _};
use mako_engine::store_slatedb::SlateDbStore;
use reqwest::{Client, Url};
use serde::Deserialize;
use time::OffsetDateTime;
use tracing::{info, warn};
use uuid::Uuid;

use crate::malo_cache::{MaloIdentResolved, MaloIdentResultCache, SlateDbMaloCache};
use crate::verzeichnisdienst_worker::VerzeichnisdienstLookup;

// ── Payload shape ─────────────────────────────────────────────────────────────

/// Serialised payload of a `MaloIdentCallback` outbox message.
#[derive(Debug, Deserialize)]
struct MaloIdentCallbackPayload {
    tx_id: String,
    tenant_id: String,
    /// 13-digit market-partner code of the requesting Lieferant, extracted
    /// from the `marketPartnerId` HTTP header on ingest.
    #[serde(default)]
    sender_market_partner_id: String,
    params: IdentificationParameter,
}

// ── MaloIdentSender ───────────────────────────────────────────────────────────

/// Outbox sender that delivers `MaloIdentCallback` messages to the LF.
///
/// ## URL resolution
///
/// The LF's API-Webdienste Strom base URL is resolved in priority order:
///
/// 1. **Static override** (`partner_urls`) — populated from `--maloid-partner
///    GLN=URL` CLI flags. Takes precedence over all other sources.  Useful
///    for testing and as a fallback when the Verzeichnisdienst is unreachable.
/// 2. **Verzeichnisdienst** (`verzeichnisdienst`) — when configured, performs
///    a live BDEW Verzeichnisdienst lookup and caches the result in the partner
///    store for future deliveries.
///
/// After a positive callback is delivered:
/// - A `MaloIdentified` outbox message is written to the `OutboxStore` so
///   the [`OutboxErpWorker`] can forward the event to the ERP system.
/// - The resolved `(tx_id, malo_id, nb_mp_id)` is persisted to the
///   [`MaloIdentResultCache`] so the ERP can trigger
///   `maloid.lieferbeginn.fortsetzen` using only the `tx_id`.
///
/// Instantiate once and pass to the `OutboxWorker`.
///
/// [`OutboxErpWorker`]: crate::erp_adapter::OutboxErpWorker
#[derive(Clone)]
pub struct MaloIdentSender {
    cache: SlateDbMaloCache,
    http_client: Client,
    /// Static overrides: GLN → callback base URL.
    ///
    /// Takes priority over Verzeichnisdienst lookups.  Configure via
    /// `--maloid-partner <GLN>=<URL>`.
    partner_urls: Arc<HashMap<String, Url>>,
    /// Optional Verzeichnisdienst lookup for dynamic URL discovery.
    ///
    /// When `None`, only the static `partner_urls` map is used.
    verzeichnisdienst: Option<VerzeichnisdienstLookup>,
    /// Store used to write the `MaloIdentified` ERP outbox event after a
    /// successful positive callback delivery.
    outbox_store: SlateDbStore,
    /// Store for `tx_id → MaloIdentResolved` mappings — enables the ERP to
    /// use `maloid.lieferbeginn.fortsetzen` without pre-supplying the malo_id.
    result_cache: MaloIdentResultCache,
}

impl MaloIdentSender {
    /// Create a new sender.
    ///
    /// - `partner_urls`: static GLN → URL overrides (`--maloid-partner`).
    /// - `verzeichnisdienst`: optional live Verzeichnisdienst lookup.
    /// - `outbox_store`: store for `MaloIdentified` ERP events.
    #[must_use]
    pub fn new(
        cache: SlateDbMaloCache,
        http_client: Client,
        partner_urls: HashMap<String, Url>,
        verzeichnisdienst: Option<VerzeichnisdienstLookup>,
        outbox_store: SlateDbStore,
    ) -> Self {
        let result_cache = MaloIdentResultCache::new(outbox_store.clone());
        Self {
            cache,
            http_client,
            partner_urls: Arc::new(partner_urls),
            verzeichnisdienst,
            outbox_store,
            result_cache,
        }
    }
}

impl As4Sender for MaloIdentSender {
    fn send(
        &self,
        msg: &OutboxMessage,
    ) -> impl std::future::Future<Output = Result<(), EngineError>> + Send {
        let cache = self.cache.clone();
        let http_client = self.http_client.clone();
        let partner_urls: Arc<HashMap<String, Url>> = Arc::clone(&self.partner_urls);
        let verzeichnisdienst = self.verzeichnisdienst.clone();
        let outbox_store = self.outbox_store.clone();
        let result_cache = self.result_cache.clone();
        let message_type = msg.message_type.to_string();
        let payload = msg.payload.clone();
        let message_id = msg.message_id.to_string();
        // Capture correlation metadata for the downstream ERP outbox entry.
        let orig_stream_id = msg.stream_id.clone();
        let orig_process_id = msg.process_id;
        let orig_tenant_id = msg.tenant_id;
        let orig_correlation_id = msg.correlation_id;
        let orig_conversation_id = msg.conversation_id;
        let orig_causation_id = msg.causation_event_id;

        async move {
            if message_type != "MaloIdentCallback" {
                // Log and acknowledge — no AS4 gateway wired for other types yet.
                warn!(
                    message_id,
                    message_type,
                    "MaloIdentSender: non-MaloIdentCallback message — \
                     AS4 gateway not wired"
                );
                return Ok(());
            }

            // Deserialise the callback payload.
            let cb: MaloIdentCallbackPayload = match serde_json::from_value(payload) {
                Ok(v) => v,
                Err(e) => {
                    warn!(
                        message_id,
                        error = %e,
                        "MaloIdentCallback: failed to deserialise payload — \
                         acknowledging to avoid retry loop"
                    );
                    return Ok(());
                }
            };

            if cb.sender_market_partner_id.is_empty() {
                warn!(
                    tx_id = %cb.tx_id,
                    "MaloIdentCallback: sender_market_partner_id missing — \
                     cannot deliver callback. Ensure the LF sends the \
                     'marketPartnerId' header."
                );
                return Ok(());
            }

            // Resolve the LF's callback base URL.
            //
            // Priority:
            //   1. Static CLI override (`--maloid-partner GLN=URL`)
            //   2. Verzeichnisdienst lookup (caches result in partner store)
            let lf_base_url: Url = if let Some(url) = partner_urls.get(&cb.sender_market_partner_id)
            {
                url.clone()
            } else if let Some(ref vz) = verzeichnisdienst {
                match vz.lookup_malo_ident_url(&cb.sender_market_partner_id).await {
                    Ok(Some(url)) => url,
                    Ok(None) => {
                        warn!(
                            tx_id         = %cb.tx_id,
                            lf_partner_id = %cb.sender_market_partner_id,
                            "MaloIdentCallback: LF not found in Verzeichnisdienst — \
                             cannot deliver callback"
                        );
                        return Ok(());
                    }
                    Err(e) => {
                        warn!(
                            tx_id         = %cb.tx_id,
                            lf_partner_id = %cb.sender_market_partner_id,
                            error         = %e,
                            "MaloIdentCallback: Verzeichnisdienst lookup error — \
                             rescheduling for retry"
                        );
                        return Err(EngineError::Transport {
                            endpoint: "verzeichnisdienst".into(),
                            message: e.to_string(),
                        });
                    }
                }
            } else {
                warn!(
                    tx_id         = %cb.tx_id,
                    lf_partner_id = %cb.sender_market_partner_id,
                    "MaloIdentCallback: no callback URL for this LF and Verzeichnisdienst \
                     not configured. Add --maloid-partner {}=<URL> or \
                     --verzeichnisdienst-url <URL>.",
                    cb.sender_market_partner_id
                );
                // Acknowledge — retries will not help without operator intervention.
                return Ok(());
            };

            let client = MaloIdentClient::new(lf_base_url, http_client);

            let now_dt = OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned());
            let reference_id = Uuid::new_v4();
            let tx_uuid = Uuid::parse_str(&cb.tx_id).unwrap_or_else(|_| Uuid::new_v4());

            // Perform the cache lookup and deliver the appropriate callback.
            match cache.lookup(&cb.tenant_id, &cb.params).await {
                Ok(Some(result)) => {
                    let malo_id_str = result.data_market_location.malo_id.as_ref().to_owned();

                    // Resolve the NB GLN from the MaLo record — needed both for
                    // the result cache and for the `maloid.lieferbeginn.fortsetzen`
                    // continuation path.
                    let nb_mp_id_str = result
                        .data_market_location
                        .data_market_location_network_operators
                        .iter()
                        .max_by_key(|p| (p.execution_time_until.is_none(), &p.execution_time_from))
                        .map(|p| format!("{:013}", p.market_partner_id))
                        .unwrap_or_default();

                    info!(
                        tx_id     = %cb.tx_id,
                        tenant_id = %cb.tenant_id,
                        malo_id   = %malo_id_str,
                        lf        = %cb.sender_market_partner_id,
                        "MaloIdentCallback: MaLo found — delivering positive callback to LF"
                    );
                    client
                        .send_positive_response(tx_uuid, &now_dt, reference_id, &result, None)
                        .await
                        .map_err(|e| EngineError::Transport {
                            endpoint: "maloid-callback".into(),
                            message: e.to_string(),
                        })?;
                    info!(tx_id = %cb.tx_id, "MaloIdentCallback: positive callback delivered");

                    // Write MaloIdentified ERP outbox entry so OutboxErpWorker
                    // delivers the resolved MaLo data to the ERP system.
                    let erp_msg = OutboxMessage {
                        message_id: OutboxMessageId::new(),
                        stream_id: orig_stream_id,
                        process_id: orig_process_id,
                        tenant_id: orig_tenant_id,
                        correlation_id: orig_correlation_id,
                        conversation_id: orig_conversation_id,
                        causation_event_id: orig_causation_id,
                        message_type: "MaloIdentified".into(),
                        recipient: cb.sender_market_partner_id.as_str().into(),
                        payload: serde_json::json!({
                            "tx_id":                    cb.tx_id,
                            "malo_id":                  malo_id_str,
                            "nb_mp_id":                   nb_mp_id_str,
                            "sender_market_partner_id": cb.sender_market_partner_id,
                            "tenant_id":                cb.tenant_id,
                        }),
                        payload_schema: Some(
                            "https://raw.githubusercontent.com/BO4E/BO4E-Schemas/v202607.0.0/\
                             src/bo4e_schemas/bo/Marktlokation.json"
                                .into(),
                        ),
                        created_at: OffsetDateTime::now_utc(),
                        deliver_after: None,
                        attempt_count: 0,
                        workflow_name: "".into(),
                        trace_context: mako_engine::trace_ctx::current().map(Into::into),
                    };
                    if let Err(e) = outbox_store.enqueue(&[erp_msg]).await {
                        warn!(
                            tx_id = %cb.tx_id,
                            error = %e,
                            "MaloIdentCallback: failed to enqueue MaloIdentified ERP event \
                             — callback was delivered but ERP notification is delayed"
                        );
                    }

                    // Persist resolved (tx_id → malo_id, nb_mp_id) so the ERP
                    // can trigger `maloid.lieferbeginn.fortsetzen`
                    // using only the tx_id without needing to know the malo_id
                    // in advance.
                    let resolved = MaloIdentResolved {
                        tx_id: cb.tx_id.clone(),
                        malo_id: malo_id_str.clone(),
                        nb_mp_id: nb_mp_id_str.clone(),
                        resolved_at: OffsetDateTime::now_utc(),
                    };
                    if let Err(e) = result_cache.store_result(&cb.tenant_id, &resolved).await {
                        warn!(
                            tx_id  = %cb.tx_id,
                            error  = %e,
                            "MaloIdentCallback: failed to persist result to tx_id cache — \
                             ERP must supply malo_id explicitly for Lieferbeginn"
                        );
                    } else {
                        info!(
                            tx_id   = %cb.tx_id,
                            malo_id = %malo_id_str,
                            nb_mp_id  = %nb_mp_id_str,
                            "MaloIdentCallback: resolved result cached — \
                             ERP may now call maloid.lieferbeginn.fortsetzen"
                        );
                    }
                }
                Ok(None) => {
                    info!(
                        tx_id     = %cb.tx_id,
                        tenant_id = %cb.tenant_id,
                        lf        = %cb.sender_market_partner_id,
                        "MaloIdentCallback: MaLo not found — delivering negative callback to LF"
                    );
                    // E_0594 / A10: "Marktlokation konnte nicht eindeutig identifiziert werden"
                    let negative = MaloIdentResultNegative {
                        decision_tree: "E_0594".to_owned(),
                        response_code: "A10".to_owned(),
                        reason: Some(
                            "No market location found for the supplied identification parameters."
                                .to_owned(),
                        ),
                        network_operator: None,
                    };
                    client
                        .send_negative_response(tx_uuid, &now_dt, reference_id, &negative, None)
                        .await
                        .map_err(|e| EngineError::Transport {
                            endpoint: "maloid-callback".into(),
                            message: e.to_string(),
                        })?;
                    info!(tx_id = %cb.tx_id, "MaloIdentCallback: negative callback delivered");
                }
                Err(e) => {
                    warn!(
                        tx_id     = %cb.tx_id,
                        tenant_id = %cb.tenant_id,
                        error     = %e,
                        "MaloIdentCallback: cache lookup failed — rescheduling for retry"
                    );
                    return Err(EngineError::transient_store(format!(
                        "malo cache lookup: {e}"
                    )));
                }
            }

            Ok(())
        }
    }
}

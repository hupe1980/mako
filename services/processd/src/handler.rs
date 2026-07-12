//! Axum webhook handler for `processd`.
//!
//! Receives `de.mako.process.*` CloudEvents from `marktd` (HMAC-signed).
//!
//! ## Event routing
//!
//! | `ce_type`                   | Module       | PIDs handled |
//! |-----------------------------|--------------|--------------|
//! | `de.mako.process.initiated` | NB module    | 55001, 55016, 44001 |
//! | `de.mako.process.initiated` | LF module    | 55008 |
//! | `de.mako.process.initiated` | MSB M3 module | 35001–35005 (REQOTE → auto QUOTES) |
//! | `wim-steuerungsauftrag`     | N5 auto-ORDRSP | §14a Steuerungsauftrag |
//! | *(all other types)*         | *(ignored)*  | — |

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_markt::cloudevents::verify_signature;
use secrecy::ExposeSecret;
use tracing::{debug, warn};

use crate::server::ProcessdState;

/// `POST /webhook` — receive a `de.mako.*` event from `marktd`.
pub async fn handle_webhook(
    State(state): State<ProcessdState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // ── 1. Verify HMAC signature ──────────────────────────────────────────
    if let Some(secret) = state.inbound_secret.as_ref() {
        let provided = headers
            .get("x-mako-signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !verify_signature(secret.expose_secret().as_bytes(), &body, provided) {
            warn!("processd: webhook HMAC signature mismatch");
            return (StatusCode::UNAUTHORIZED, "signature mismatch").into_response();
        }
    }

    // ── 2. Parse JSON body ────────────────────────────────────────────────
    let event: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(e) => e,
        Err(err) => {
            warn!(%err, "processd: failed to parse CloudEvent");
            return (StatusCode::BAD_REQUEST, "invalid JSON").into_response();
        }
    };

    let ce_type = event["type"].as_str().unwrap_or("").to_owned();

    // ── 3. Route by event type ────────────────────────────────────────────
    if ce_type != "de.mako.process.initiated" {
        debug!(ce_type, "processd: non-initiated event ignored");
        return StatusCode::NO_CONTENT.into_response();
    }

    // ── 4. NB module ──────────────────────────────────────────────────────
    #[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
    {
        use crate::nb_module;
        if let Some(ref nb) = state.nb {
            match nb_module::evaluate_and_decide(
                &event, &nb.config, &nb.reader, &nb.makod, &nb.repo,
            )
            .await
            {
                Ok(true) => return StatusCode::OK.into_response(),
                Ok(false) => {} // not an NB PID, fall through to LF module
                Err(e) => {
                    warn!(error = %e, "processd NB: evaluation error");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        }
    }

    // ── 5. LF module ──────────────────────────────────────────────────────
    #[cfg(any(feature = "role-lf-strom", feature = "role-lf-gas"))]
    {
        use crate::lf_module;
        if let Some(ref lf) = state.lf {
            match lf_module::process_e0624(&event, &lf.config, &lf.reader, &lf.makod, &lf.queue)
                .await
            {
                Ok(true) => return StatusCode::OK.into_response(),
                Ok(false) => {} // not an LF PID
                Err(e) => {
                    warn!(error = %e, "processd LF: evaluation error");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        }
    }

    // ── 6. §14a Steuerungsauftrag auto-ORDRSP (N5) ────────────────────────
    //
    // When mako acts as MSB and receives a wim-steuerungsauftrag initiation,
    // auto-confirm if SteuerbareRessource.istFernschaltbar=true AND the
    // dispatched produktcode is in the contracted konfigurationsprodukte.
    //
    // BK6-24-174 §4.3: MSB MUST only confirm a Steuerungsauftrag for
    // products that are under contract.  Uncontracted produktcode → ablehnen.
    let workflow = event
        .get("makoworkflow")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if workflow == "wim-steuerungsauftrag" {
        let process_id = event["subject"].as_str().unwrap_or("");
        let data = event.get("data").unwrap_or(&serde_json::Value::Null);
        let sr_id = data.get("sr_id").and_then(|v| v.as_str()).unwrap_or("");
        // The dispatched produktcode is in the payload; empty means uncoded command.
        let dispatched_produktcode = data
            .get("produktcode")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Fetch SR + contracted konfigurationsprodukte in parallel.
        let (sr_result, kp_result) = tokio::join!(
            state.marktd.get_steuerbare_ressource(sr_id),
            state.marktd.get_konfigurationsprodukte(sr_id),
        );

        let is_fernschaltbar: Option<bool> = if !sr_id.is_empty() {
            sr_result.ok().flatten().and_then(|sr| {
                sr.get("ist_fernschaltbar")
                    .or_else(|| sr.get("istFernschaltbar"))
                    .and_then(|v| v.as_bool())
            })
        } else {
            None
        };

        // Check whether the dispatched produktcode is contracted.
        // If no konfigurationsprodukte are stored yet, we cannot confirm.
        // If konfigurationsprodukte is an empty array, no products are contracted.
        let contracted: Option<Vec<serde_json::Value>> = kp_result.ok().flatten();
        let produktcode_contracted = if dispatched_produktcode.is_empty() {
            // No produktcode in event — legacy: accept only if there's at least
            // one contracted product (non-empty konfigurationsprodukte).
            contracted.as_ref().is_some_and(|a| !a.is_empty())
        } else {
            contracted.as_ref().is_some_and(|arr| {
                arr.iter().any(|item| {
                    item.get("produktcode")
                        .and_then(|v| v.as_str())
                        .map(|code| code == dispatched_produktcode)
                        .unwrap_or(false)
                })
            })
        };

        match (is_fernschaltbar, produktcode_contracted) {
            (Some(true), true) => {
                // Auto-confirm: SR is remote-switchable and produktcode is contracted.
                let cmd = mako_markt::makod_client::ForwardCommand {
                    command: "wim.steuerungsauftrag.bestaetigen".to_owned(),
                    marktrolle: None,
                    malo_id: None,
                    melo_id: None,
                    payload: serde_json::json!({
                        "process_id": process_id,
                        "auto_ordrsp": true,
                        "produktcode": dispatched_produktcode,
                    }),
                };
                let idem_key = format!("steuerungsauftrag-bestaetigen-{process_id}");
                if let Err(e) = state.makod.post_command(&idem_key, &cmd).await {
                    warn!(sr_id, error = %e, "processd: Steuerungsauftrag auto-bestaetigen failed");
                } else {
                    debug!(
                        sr_id,
                        process_id,
                        produktcode = dispatched_produktcode,
                        "processd: Steuerungsauftrag auto-confirmed (istFernschaltbar=true, produktcode contracted)"
                    );
                }
            }
            (Some(true), false) => {
                // SR is remote-switchable but produktcode is NOT contracted — must ablehnen.
                // BK6-24-174 §4.3: dispatch only for contracted products.
                warn!(
                    sr_id,
                    process_id,
                    produktcode = dispatched_produktcode,
                    "processd: Steuerungsauftrag ablehnen — produktcode not in contracted konfigurationsprodukte (BK6-24-174 §4.3)"
                );
                let cmd = mako_markt::makod_client::ForwardCommand {
                    command: "wim.steuerungsauftrag.ablehnen".to_owned(),
                    marktrolle: None,
                    malo_id: None,
                    melo_id: None,
                    payload: serde_json::json!({
                        "process_id": process_id,
                        "reason": "produktcode not in contracted konfigurationsprodukte (BK6-24-174 §4.3)",
                        "produktcode": dispatched_produktcode,
                    }),
                };
                let idem_key = format!("steuerungsauftrag-ablehnen-{process_id}");
                let _ = state.makod.post_command(&idem_key, &cmd).await;
            }
            (Some(false), _) => {
                // SR is not remote-switchable — escalate to operator.
                warn!(
                    sr_id,
                    process_id,
                    "processd: Steuerungsauftrag escalated — istFernschaltbar=false; manual ORDRSP required"
                );
            }
            (None, _) => {
                // Unknown SR or marktd unavailable — escalate.
                warn!(
                    sr_id,
                    process_id,
                    "processd: Steuerungsauftrag escalated — SR not found in marktd or ist_fernschaltbar unknown"
                );
            }
        }
        return StatusCode::OK.into_response();
    }

    // ── 7. M3: Preisanfrage REQOTE auto-response ──────────────────────────
    //
    // When the MSB receives a REQOTE (PIDs 35001–35005) from an nMSB, auto-dispatch
    // a QUOTES response sourced from the current PreisblattMessung in marktd.
    // This eliminates the manual ERP trigger that previously risked APERAK deadline
    // breaches (ERC A97).
    #[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
    if state.msb_auto_preisanfrage {
        use crate::msb_module;
        // Build a minimal MsbModuleConfig for the preisanfrage handler.
        // We only need own_mp_id — marktd + makod are passed as refs.
        let msb_cfg = msb_module::MsbModuleConfig {
            marktd_url: String::new(), // unused — using pre-built client
            marktd_api_key: secrecy::SecretString::from(""),
            own_mp_id: state.own_mp_id.clone(),
            tenant: state.tenant.clone(),
            auto_accept: true, // auto_preisanfrage enabled = dispatch QUOTES
        };
        if msb_module::handle_preisanfrage_reqote(&event, &msb_cfg, &state.marktd, &state.makod)
            .await
        {
            return StatusCode::OK.into_response();
        }
    }

    StatusCode::NO_CONTENT.into_response()
}

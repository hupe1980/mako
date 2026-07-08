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

    StatusCode::NO_CONTENT.into_response()
}

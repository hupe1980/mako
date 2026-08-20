//! Customer authorization for `portald`.
//!
//! # One gate, applied everywhere
//!
//! `portald` serves one customer's consumption, invoices, ledger and contract.
//! Which customer is decided by [`authorize`], and by nothing else: it is the
//! single place that turns an inbound `Authorization: Bearer` header plus a
//! `malo_id` path parameter into a [`PortalAuthCtx`]. Every route takes that
//! context as a *value*, so a route that forgets to authorise does not compile
//! — it has no `kunden_id` to work with.
//!
//! `tests/authorization_guard.rs` drives every route the router exposes against
//! a `vertragd` that refuses everything, so a route that skips the gate fails
//! the build rather than serving one customer's ledger to another.
//!
//! # `vertragd` decides, `portald` relays
//!
//! `vertragd` owns the OIDC verifier, the customer record and the customer↔MaLo
//! mapping. This module forwards the customer's token unchanged and relays the
//! verdict. The service credential rides as `X-Api-Key`: sending it as a second
//! `Authorization` header makes which identity `vertragd` sees depend on header
//! ordering, and the identity it must see is the customer's.

use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::{clients::PortalClients, config::PortaldConfig};

/// A customer proven to own the requested Marktlokation.
///
/// Constructed only by [`authorize`]. Handlers receive it by value, so the type
/// is the proof: no context, no customer-scoped work.
#[derive(Debug, Clone)]
pub struct PortalAuthCtx {
    /// `vertragd`'s customer identifier — the key for contract lookups.
    pub kunden_id: uuid::Uuid,
    /// `B2C` or `B2B`, as classified by `vertragd`.
    pub kundentyp: String,
    /// The Marktlokation this context authorises, echoed back from the request.
    pub malo_id: String,
}

/// Authorize the caller for `malo_id`, or produce the HTTP error to return.
///
/// # Errors
///
/// - `401` — no `Authorization` header, an unverifiable token, or no customer
///   profile behind it.
/// - `403` — a valid customer who does not own this Marktlokation.
/// - `503` — `vertragd` unreachable or erroring. Never "allow": an
///   authorization service that cannot answer is not an answer of yes.
pub async fn authorize(
    cfg: &PortaldConfig,
    clients: &PortalClients,
    headers: &HeaderMap,
    malo_id: &str,
) -> Result<PortalAuthCtx, Response> {
    let Some(vertragd_url) = cfg.vertragd_url.as_deref() else {
        // Reachable only with `allow_insecure_no_auth`; startup refuses
        // otherwise (see `server::build`). Logged per request rather than once,
        // because the interesting fact is *which* data went out unauthorised.
        tracing::warn!(
            malo_id,
            "portald: allow_insecure_no_auth — serving customer data without ownership check"
        );
        return Ok(PortalAuthCtx {
            kunden_id: uuid::Uuid::nil(),
            kundentyp: "B2C".to_owned(),
            malo_id: malo_id.to_owned(),
        });
    };

    let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return Err(unauthorized("Authorization: Bearer required"));
    };

    let mut req = clients
        .auth_client
        .get(format!(
            "{}/api/v1/kunden/authenticate",
            vertragd_url.trim_end_matches('/')
        ))
        .query(&[("malo_id", malo_id)])
        .header(header::AUTHORIZATION, token);
    if let Some(key) = cfg.vertragd_api_key.as_deref() {
        // `X-Api-Key`, never `bearer_auth`: that appends a second
        // `Authorization` header and leaves the identity vertragd reads up to
        // header ordering.
        req = req.header("X-Api-Key", key);
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(malo_id, error = %e, "portald: vertragd unreachable");
            return Err(unavailable("authorization service unavailable"));
        }
    };

    match resp.status() {
        s if s.is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            // A success without a customer id is a contract violation, not a
            // customer to serve: proceeding would run every contract lookup
            // against the nil UUID and return someone else's — or nothing at
            // all — under an authorised-looking response.
            let Some(kunden_id) = body["kunden_id"]
                .as_str()
                .and_then(|s| s.parse::<uuid::Uuid>().ok())
            else {
                tracing::warn!(
                    malo_id,
                    "portald: vertragd authorised without a kunden_id — refusing"
                );
                return Err(unavailable("authorization service returned no customer"));
            };
            Ok(PortalAuthCtx {
                kunden_id,
                kundentyp: body["kundentyp"].as_str().unwrap_or("B2C").to_owned(),
                malo_id: malo_id.to_owned(),
            })
        }
        StatusCode::UNAUTHORIZED => Err(unauthorized("not authenticated")),
        StatusCode::FORBIDDEN => {
            tracing::warn!(malo_id, "portald: customer not authorized for this MaLo");
            Err((
                StatusCode::FORBIDDEN,
                "not authorized to access this delivery point",
            )
                .into_response())
        }
        StatusCode::NOT_FOUND => Err(unauthorized("no customer profile found for this identity")),
        s => {
            tracing::warn!(malo_id, status = %s, "portald: vertragd auth check failed");
            Err(unavailable("authorization service unavailable"))
        }
    }
}

fn unauthorized(msg: &'static str) -> Response {
    (StatusCode::UNAUTHORIZED, msg).into_response()
}

fn unavailable(msg: &'static str) -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, msg).into_response()
}

//! Webhook-subscription REST handlers.
//!
//! Routes:
//!   GET    /api/v1/subscriptions
//!   GET    /api/v1/subscriptions/{id}
//!   PUT    /api/v1/subscriptions/{id}
//!   DELETE /api/v1/subscriptions/{id}
//!   POST   /api/v1/subscriptions/{id}/test
//!
//! ## Access control
//!
//! Every route requires the `manage-subscription` Cedar action (ADMIN role). A
//! subscription receives *every* event this hub emits that matches its filter —
//! MaLo/MeLo master data, VersorgungsStatus transitions, ESA consent lifecycle —
//! so registering one is an outbound data-export decision rather than a
//! market-role one. `POST /{id}/test` additionally reports the target endpoint's
//! HTTP status, which is why it is not readable by an ordinary tenant principal.
//!
//! ## Endpoint validation
//!
//! `webhook_url` is caller-supplied and the fan-out worker POSTs to it from
//! inside the deployment's network. [`validate_webhook_url`] therefore rejects
//! anything but `http`/`https` and refuses literal loopback, link-local and
//! private-range hosts, so a subscription cannot be used to reach cluster
//! infrastructure. The shared client refuses redirects
//! ([`mako_service::http::default_client`]), which closes the redirect bypass of
//! the same check.

use std::{net::IpAddr, sync::Arc};

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::{
    cloudevents::MarktEvent,
    repository::{
        AppState, CorrelationIndex, MaloRepository, MeloRepository, PartnerRepository,
        Subscription, SubscriptionRepository,
    },
};
use mako_service::cedar::CedarEnforcer;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{Claims, IntoMdmResponse as _};

// ── DTOs ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct SubscriptionUpsertRequest {
    pub webhook_url: String,
    pub webhook_secret: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub event_types: Vec<String>,
    #[serde(default)]
    pub sparten: Vec<String>,
    #[serde(default = "default_active")]
    pub active: bool,
}

fn default_active() -> bool {
    true
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SubscriptionResponse {
    pub subscriber_id: String,
    pub webhook_url: String,
    pub roles: Vec<String>,
    pub event_types: Vec<String>,
    pub sparten: Vec<String>,
    pub active: bool,
    pub version: i64,
}

// ── Endpoint validation ───────────────────────────────────────────────────────

/// Reject a webhook endpoint that would let a subscription reach infrastructure
/// the operator never named.
///
/// Only `http` and `https` are accepted, and a *literal* IP host must be
/// globally routable. A DNS name is not resolved here — resolution happens at
/// delivery time and could differ from what a check saw (DNS rebinding), so the
/// deployment's egress policy is the control for that case; this check removes
/// the trivially exploitable form.
///
/// # Errors
///
/// Returns a human-readable reason when the URL must not be stored.
pub fn validate_webhook_url(raw: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(raw).map_err(|e| format!("not a valid URL: {e}"))?;

    match url.scheme() {
        "http" | "https" => {}
        other => return Err(format!("unsupported scheme {other:?}: use http or https")),
    }

    let Some(host) = url.host_str() else {
        return Err("URL has no host".to_owned());
    };

    if host.eq_ignore_ascii_case("localhost") {
        return Err("loopback host is not an acceptable webhook endpoint".to_owned());
    }

    // An IPv6 literal arrives bracketed (`[::1]`); a registered name never
    // parses as an address and is left to delivery-time resolution.
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>()
        && !is_globally_routable(ip)
    {
        return Err(format!(
            "{ip} is a loopback, link-local, private or otherwise non-routable address"
        ));
    }

    Ok(())
}

/// Whether an address is one a webhook may legitimately point at.
///
/// Deliberately conservative: anything not plainly a public unicast address is
/// refused, including the IPv4-mapped IPv6 forms of private ranges, which are
/// the usual way such a filter is bypassed.
fn is_globally_routable(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                // 100.64.0.0/10 carrier-grade NAT
                || (v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1]))
                // 0.0.0.0/8 "this network"
                || v4.octets()[0] == 0)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_globally_routable(IpAddr::V4(v4));
            }
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // fc00::/7 unique-local
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // fe80::/10 link-local
                || (v6.segments()[0] & 0xffc0) == 0xfe80)
        }
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

fn forbidden() -> axum::response::Response {
    mako_markt::error::MdmError::Forbidden {
        reason: "manage-subscription denied",
    }
    .into_response()
}

/// `PUT /api/v1/subscriptions/{id}`
pub async fn put_subscription<Ma, Me, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Path(id): Path<String>,
    Json(req): Json<SubscriptionUpsertRequest>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    if enforcer
        .check(
            &claims.principal(),
            "manage-subscription",
            &state.tenant_gln,
        )
        .is_err()
    {
        return forbidden();
    }

    if let Err(reason) = validate_webhook_url(&req.webhook_url) {
        return mako_markt::error::MdmError::Unprocessable {
            reason: format!("webhook_url: {reason}"),
        }
        .into_response();
    }

    let sub = Subscription {
        subscriber_id: id,
        webhook_url: req.webhook_url,
        webhook_secret: req.webhook_secret,
        roles: req.roles,
        event_types: req.event_types,
        sparten: req.sparten,
        active: req.active,
        version: 0, // set by repository
    };

    match state.subscription_repo.upsert(sub).await {
        Ok(version) => axum::Json(serde_json::json!({ "version": version })).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/subscriptions/{id}`
pub async fn get_subscription<Ma, Me, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Path(id): Path<String>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    if enforcer
        .check(
            &claims.principal(),
            "manage-subscription",
            &state.tenant_gln,
        )
        .is_err()
    {
        return forbidden();
    }
    match state.subscription_repo.find(&id).await {
        Ok(Some(s)) => axum::Json(sub_to_response(s)).into_response(),
        Ok(None) => mako_markt::error::MdmError::NotFound {
            resource_type: "subscription",
            id,
        }
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/subscriptions`
pub async fn list_subscriptions<Ma, Me, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    if enforcer
        .check(
            &claims.principal(),
            "manage-subscription",
            &state.tenant_gln,
        )
        .is_err()
    {
        return forbidden();
    }
    match state.subscription_repo.list_active().await {
        Ok(subs) => {
            axum::Json(subs.into_iter().map(sub_to_response).collect::<Vec<_>>()).into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// `DELETE /api/v1/subscriptions/{id}`
///
/// Deactivates rather than deletes: `event_delivery` rows reference the
/// subscriber and are § 147 AO / GoBD evidence that a market event was (or was
/// not) delivered. A hard delete would erase that trail, so the subscription is
/// marked inactive and stops matching future fan-outs.
pub async fn delete_subscription<Ma, Me, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Path(id): Path<String>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    if enforcer
        .check(
            &claims.principal(),
            "manage-subscription",
            &state.tenant_gln,
        )
        .is_err()
    {
        return forbidden();
    }
    match state.subscription_repo.deactivate(&id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => mako_markt::error::MdmError::NotFound {
            resource_type: "subscription",
            id,
        }
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// `POST /api/v1/subscriptions/{id}/test`
///
/// Sends a test ping event **directly** to the specific subscriber's webhook URL.
///
/// Unlike the fan-out worker, this is a synchronous targeted delivery — only the
/// named subscriber receives the ping, even if the event type/role would match
/// other subscriptions.  Returns the delivery result synchronously.
pub async fn test_subscription<Ma, Me, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(http): Extension<reqwest::Client>,
    claims: Claims,
    Path(id): Path<String>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    if enforcer
        .check(
            &claims.principal(),
            "manage-subscription",
            &state.tenant_gln,
        )
        .is_err()
    {
        return forbidden();
    }

    let sub = match state.subscription_repo.find(&id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return mako_markt::error::MdmError::NotFound {
                resource_type: "subscription",
                id,
            }
            .into_response();
        }
        Err(e) => return e.into_response(),
    };

    // A stored endpoint is re-checked before this deliberate, synchronous probe:
    // the rule may have tightened since the row was written.
    if let Err(reason) = validate_webhook_url(&sub.webhook_url) {
        return mako_markt::error::MdmError::Unprocessable {
            reason: format!("webhook_url: {reason}"),
        }
        .into_response();
    }

    let ping = MarktEvent::new(
        &state.tenant_gln,
        mako_events::markt::SUBSCRIPTION_TEST,
        format!("subscriptions/{}", sub.subscriber_id),
        serde_json::json!({ "message": "ping" }),
    );

    let body = match serde_json::to_vec(&ping) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    // A test delivery is signed exactly like a real one, so a subscriber that
    // verifies this one verifies the fan-out too — which is the only thing a
    // test delivery is for.
    let webhook_id = format!("test-delivery/{}", uuid::Uuid::new_v4());
    let mut req = http
        .post(&sub.webhook_url)
        .header("Content-Type", "application/cloudevents+json")
        .header(mako_service::webhook::ID_HEADER, &webhook_id)
        .body(body.clone());

    if let Some(secret) = sub.webhook_secret.as_deref() {
        for (name, value) in mako_service::webhook::headers(
            secret.as_bytes(),
            &webhook_id,
            time::OffsetDateTime::now_utc().unix_timestamp(),
            &body,
        ) {
            req = req.header(name, value);
        }
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => (
            StatusCode::OK,
            Json(serde_json::json!({
                "subscriber_id": sub.subscriber_id,
                "delivered": true,
                "webhook_status": resp.status().as_u16(),
            })),
        )
            .into_response(),
        Ok(resp) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "subscriber_id": sub.subscriber_id,
                "delivered": false,
                "webhook_status": resp.status().as_u16(),
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "subscriber_id": sub.subscriber_id,
                "delivered": false,
                "error": e.to_string(),
            })),
        )
            .into_response(),
    }
}

fn sub_to_response(s: Subscription) -> SubscriptionResponse {
    SubscriptionResponse {
        subscriber_id: s.subscriber_id,
        webhook_url: s.webhook_url,
        roles: s.roles,
        event_types: s.event_types,
        sparten: s.sparten,
        active: s.active,
        version: s.version,
    }
}

#[cfg(test)]
mod tests {
    use super::validate_webhook_url;

    #[test]
    fn a_public_https_endpoint_is_accepted() {
        assert!(validate_webhook_url("https://erp.example.com/markt/events").is_ok());
    }

    #[test]
    fn a_non_http_scheme_is_refused() {
        // `file://` would make the fan-out worker read from the container's disk.
        assert!(validate_webhook_url("file:///etc/passwd").is_err());
        assert!(validate_webhook_url("gopher://example.com/").is_err());
    }

    #[test]
    fn loopback_and_link_local_literals_are_refused() {
        for url in [
            "http://127.0.0.1:9200/",
            "http://localhost/hook",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/hook",
        ] {
            assert!(
                validate_webhook_url(url).is_err(),
                "{url} must not be an acceptable webhook endpoint"
            );
        }
    }

    #[test]
    fn private_ranges_are_refused_including_their_ipv4_mapped_form() {
        for url in [
            "http://10.0.0.5/hook",
            "http://192.168.1.10/hook",
            "http://172.16.4.4/hook",
            // The usual bypass: the same private address written as IPv6.
            "http://[::ffff:10.0.0.5]/hook",
        ] {
            assert!(
                validate_webhook_url(url).is_err(),
                "{url} must not be an acceptable webhook endpoint"
            );
        }
    }
}

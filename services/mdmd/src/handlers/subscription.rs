//! Subscription REST handlers.
//!
//! Routes:
//!   PUT  /api/v1/subscriptions/:id
//!   GET  /api/v1/subscriptions/:id
//!   GET  /api/v1/subscriptions
//!   POST /api/v1/subscriptions/:id/test

use std::{sync::Arc, time::Duration};

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use mako_mdm::{
    cloudevents::{MdmEvent, compute_signature},
    repository::{
        AppState, ContractRepository, CorrelationIndex, MaloRepository, MeloRepository,
        PartnerRepository, Subscription, SubscriptionRepository,
    },
};
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

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `PUT /api/v1/subscriptions/:id`
pub async fn put_subscription<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    _claims: Claims,
    Path(id): Path<String>,
    Json(req): Json<SubscriptionUpsertRequest>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
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

/// `GET /api/v1/subscriptions/:id`
pub async fn get_subscription<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    _claims: Claims,
    Path(id): Path<String>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    match state.subscription_repo.find(&id).await {
        Ok(Some(s)) => axum::Json(sub_to_response(s)).into_response(),
        Ok(None) => mako_mdm::error::MdmError::NotFound {
            resource_type: "resource",
            id,
        }
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/subscriptions`
pub async fn list_subscriptions<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    _claims: Claims,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    match state.subscription_repo.list_active().await {
        Ok(subs) => {
            axum::Json(subs.into_iter().map(sub_to_response).collect::<Vec<_>>()).into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// `POST /api/v1/subscriptions/:id/test`
///
/// Sends a test ping event **directly** to the specific subscriber's webhook URL.
///
/// Unlike the fan-out worker, this is a synchronous targeted delivery — only the
/// named subscriber receives the ping, even if the event type/role would match
/// other subscriptions.  Returns the delivery result synchronously.
pub async fn test_subscription<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    Extension(http): Extension<reqwest::Client>,
    _claims: Claims,
    Path(id): Path<String>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    let sub = match state.subscription_repo.find(&id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return mako_mdm::error::MdmError::NotFound {
                resource_type: "subscription",
                id,
            }
            .into_response();
        }
        Err(e) => return e.into_response(),
    };

    let ping = MdmEvent::new(
        &state.tenant_gln,
        "de.mdm.subscription.test",
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

    let mut req = http
        .post(&sub.webhook_url)
        .header("Content-Type", "application/cloudevents+json")
        .timeout(Duration::from_secs(10))
        .body(body.clone());

    if let Some(secret) = sub.webhook_secret.as_deref() {
        let sig = compute_signature(secret.as_bytes(), &body);
        req = req.header("X-Mdm-Signature", sig);
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

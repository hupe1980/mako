//! Health endpoint — `GET /health`.

use axum::{Json, response::IntoResponse};
use serde_json::json;

/// `GET /health` — liveness check.
pub async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

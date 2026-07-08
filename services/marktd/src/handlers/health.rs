//! Health endpoints — `GET /health`, `GET /health/live`, `GET /health/ready`.

use axum::{Extension, Json, http::StatusCode, response::IntoResponse};
use serde_json::json;
use sqlx::PgPool;

/// `GET /health` and `GET /health/live` — liveness check (always 200).
pub async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

/// `GET /health/ready` — readiness check: verifies the database connection.
/// Returns 503 when the PostgreSQL pool cannot be reached.
pub async fn health_ready(Extension(pool): Extension<PgPool>) -> impl IntoResponse {
    match sqlx::query("SELECT 1").execute(&pool).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "marktd: readiness probe: DB unreachable");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

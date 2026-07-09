//! Prometheus-compatible metrics endpoint for `marktd` (F-006).

use axum::{Extension, response::IntoResponse};
use sqlx::PgPool;

/// `GET /metrics`
///
/// Returns key operational metrics in Prometheus text format.
/// No authentication is required — metrics are considered non-sensitive
/// operational data (no personal data, no tenant data).
pub async fn metrics_handler(Extension(pool): Extension<PgPool>) -> impl IntoResponse {
    let mut out = String::with_capacity(1024);

    // ── fanout_dlq depth ──────────────────────────────────────────────────────
    let dlq_depth: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM fanout_dlq WHERE resolved_at IS NULL")
            .fetch_one(&pool)
            .await
            .unwrap_or(0);

    out.push_str("# HELP marktd_fanout_dlq_depth Unresolved fanout dead-letter queue entries.\n");
    out.push_str("# TYPE marktd_fanout_dlq_depth gauge\n");
    out.push_str(&format!("marktd_fanout_dlq_depth {dlq_depth}\n"));

    // ── subscriptions ─────────────────────────────────────────────────────────
    let subscription_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM subscriptions WHERE active = true")
            .fetch_one(&pool)
            .await
            .unwrap_or(0);

    out.push_str("# HELP marktd_active_subscriptions Active EventBus webhook subscriptions.\n");
    out.push_str("# TYPE marktd_active_subscriptions gauge\n");
    out.push_str(&format!(
        "marktd_active_subscriptions {subscription_count}\n"
    ));

    // ── processed_events (idempotency table size) ─────────────────────────────
    let processed_events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM processed_events")
        .fetch_one(&pool)
        .await
        .unwrap_or(0);

    out.push_str("# HELP marktd_processed_events_total Total deduplicated event IDs in the idempotency table.\n");
    out.push_str("# TYPE marktd_processed_events_total gauge\n");
    out.push_str(&format!(
        "marktd_processed_events_total {processed_events}\n"
    ));

    // ── DB connection pool ────────────────────────────────────────────────────
    let pool_size = pool.size();
    let pool_idle = pool.num_idle();

    out.push_str(
        "# HELP marktd_db_pool_size Current number of connections in the PostgreSQL pool.\n",
    );
    out.push_str("# TYPE marktd_db_pool_size gauge\n");
    out.push_str(&format!("marktd_db_pool_size {pool_size}\n"));

    out.push_str("# HELP marktd_db_pool_idle Idle connections in the PostgreSQL pool.\n");
    out.push_str("# TYPE marktd_db_pool_idle gauge\n");
    out.push_str(&format!("marktd_db_pool_idle {pool_idle}\n"));

    (
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        out,
    )
}

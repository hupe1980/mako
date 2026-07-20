//! HTTP surface for `einsd`.
//!
//! The router is built here rather than inline in `main`, so integration tests
//! can drive the real routes — auth layers, extractors and all — instead of
//! calling handler functions directly and missing everything the layers do.

use std::sync::Arc;

use axum::{
    Extension, Router,
    routing::{get, post, put},
};
use mako_service::health::health_routes;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use crate::config::EinsdConfig;
use crate::mcp_server::EinsdMcpState;

/// Build the complete `einsd` router with every layer applied.
pub fn build_router(
    cfg: Arc<EinsdConfig>,
    http_client: Arc<reqwest::Client>,
    cedar: Arc<mako_service::cedar::CedarEnforcer>,
    oidc: mako_service::oidc::OidcVerifier,
    pool: PgPool,
    mcp_state: Arc<EinsdMcpState>,
    shutdown: CancellationToken,
) -> Router {
    Router::new()
        .merge(crate::mcp_server::router(mcp_state, shutdown))
        .merge(health_routes(|| async { true }))
        // ── Anlage CRUD ────────────────────────────────────────────────────────
        .route(
            "/api/v1/anlagen",
            post(crate::handlers::post_anlage).get(crate::handlers::get_anlagen),
        )
        .route(
            "/api/v1/anlagen/foerderung-auslaufend",
            get(crate::handlers::get_foerderung_auslaufend),
        )
        .route(
            "/api/v1/anlagen/{tr_id}",
            get(crate::handlers::get_anlage)
                .put(crate::handlers::put_anlage)
                .delete(crate::handlers::delete_anlage),
        )
        // ── Settlement ─────────────────────────────────────────────────────────
        .route(
            "/api/v1/anlagen/{tr_id}/settle/{year}/{month}",
            post(crate::handlers::post_settle),
        )
        .route(
            "/api/v1/anlagen/{tr_id}/settlements",
            get(crate::handlers::get_settlements),
        )
        // ── Repowering (§22 EEG 2023) ──────────────────────────────────────────
        .route(
            "/api/v1/anlagen/{tr_id}/repowering",
            post(crate::handlers::post_repowering),
        )
        // ── MaStR registration confirmation ────────────────────────────────────
        .route(
            "/api/v1/anlagen/{tr_id}/mastr-registrierung",
            post(crate::handlers::post_mastr_registrierung),
        )
        // ── Zusammenlegung (§24 EEG 2023) ─────────────────────────────────────
        .route(
            "/api/v1/anlagen/{tr_id}/zusammenlegen",
            post(crate::handlers::post_zusammenlegen),
        )
        // ── §21b EEG 2023 — Veräußerungsform switch ───────────────────────────
        .route(
            "/api/v1/anlagen/{tr_id}/switch-veraeusserungsform",
            post(crate::handlers::post_switch_veraeusserungsform),
        )
        // ── §22 MessZV — Correction settlement ────────────────────────────────
        .route(
            "/api/v1/anlagen/{tr_id}/settlements/{year}/{month}/correction",
            post(crate::handlers::post_correction_settle),
        )
        // ── Jahresabrechnung (annual reconciliation) ───────────────────────────
        .route(
            "/api/v1/anlagen/{tr_id}/jahresabrechnung/{year}",
            post(crate::handlers::post_jahresabrechnung),
        )
        // ── Batch settlement ───────────────────────────────────────────────────
        .route(
            "/api/v1/settle/{year}/{month}",
            post(crate::handlers::post_batch_settle),
        )
        // ── EPEX monthly prices ────────────────────────────────────────────────
        .route(
            "/api/v1/epex-monthly/{year}/{month}",
            put(crate::handlers::put_epex_price).get(crate::handlers::get_epex_price),
        )
        // ── §20 Abs. 2 Jahresmarktwert prices (ÜNB-published) ─────────────────
        .route(
            "/api/v1/jahresmarktwert/{year}/{month}/{erzeugungsart}",
            put(crate::handlers::put_jahresmarktwert).get(crate::handlers::get_jahresmarktwert),
        )
        // ── EEG tariff rate lookup ─────────────────────────────────────────────
        .route(
            "/api/v1/verguetungssatz-lookup",
            post(crate::handlers::post_verguetungssatz_lookup),
        )
        .layer(Extension(Arc::clone(&cfg)))
        .layer(Extension(Arc::clone(&http_client)))
        .layer(Extension(cedar))
        .layer(Extension(oidc))
        .layer(Extension(pool))
}

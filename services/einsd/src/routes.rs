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
        // ── Repowering (§3 Nr. 30 i.V.m. §25 EEG 2023) ─────────────────────────
        .route(
            "/api/v1/anlagen/{tr_id}/repowering",
            post(crate::handlers::post_repowering),
        )
        // ── §36h Abs. 2 Wind Standortgüte re-evaluation (year 6/11/16) ─────────
        .route(
            "/api/v1/anlagen/{tr_id}/wind-reevaluation",
            post(crate::handlers::post_wind_reevaluation),
        )
        // ── MaStR registration confirmation ────────────────────────────────────
        .route(
            "/api/v1/anlagen/{tr_id}/mastr-registrierung",
            post(crate::handlers::post_mastr_registrierung),
        )
        // ── §§53b–54 EEG 2023 — facts that cut the anzulegender Wert ─────────
        // Recording them through the API keeps a change that silently reduces a
        // Gutschrift behind the same Cedar gate as every other lifecycle event.
        .route(
            "/api/v1/anlagen/{tr_id}/aw-reduktionen",
            get(crate::handlers::get_aw_reduktionen),
        )
        .route(
            "/api/v1/anlagen/{tr_id}/aw-reduktionen/regionalnachweis",
            post(crate::handlers::post_regionalnachweis),
        )
        .route(
            "/api/v1/anlagen/{tr_id}/aw-reduktionen/stromsteuerbefreiung",
            post(crate::handlers::post_stromsteuerbefreiung),
        )
        .route(
            "/api/v1/anlagen/{tr_id}/aw-reduktionen/sect54-defekt",
            post(crate::handlers::post_sect54_defekt),
        )
        .route(
            "/api/v1/anlagen/{tr_id}/aw-reduktionen/sect54-defekt/{id}/nachweis-erbracht",
            post(crate::handlers::post_sect54_nachweis_erbracht),
        )
        // ── Zusammenlegung (§24 EEG 2023) ─────────────────────────────────────
        .route(
            "/api/v1/anlagen/{tr_id}/zusammenlegen",
            post(crate::handlers::post_zusammenlegen),
        )
        // ── Veräußerungsform lookup by MaLo — read by processd's NB module ────
        // `E_0622` Prüfschritte 400–830 choose the Vorlauffrist from the
        // *bestehende* Veräußerungsform, which is register data and not on the
        // wire. See `get_veraeusserungsform_by_malo`.
        .route(
            "/api/v1/anlagen/by-malo/{malo_id}/veraeusserungsform",
            get(crate::handlers::get_veraeusserungsform_by_malo),
        )
        // ── §21b EEG 2023 — Veräußerungsform switch ───────────────────────────
        .route(
            "/api/v1/anlagen/{tr_id}/switch-veraeusserungsform",
            post(crate::handlers::post_switch_veraeusserungsform),
        )
        // ── § 147 AO / GoBD — Correction settlement ────────────────────────────────
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
        // ── EPEX spot per-interval prices (§51 Negativpreisregel) ──────────────
        .route("/api/v1/epex-spot", put(crate::handlers::put_epex_spot))
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

//! HTTP surface of `vertragd`.
//!
//! | Module | Routes |
//! |---|---|
//! | [`kunden`] | Kunden, portal identities, DSGVO Auskunft/Löschung, portal authorization |
//! | [`vertraege`] | Versorgungsverträge: create, Kündigung, Widerruf, Stornierung, Tarifwechsel, Preisgarantie |
//! | [`rahmenvertraege`] | B2B framework contracts and their cascade Kündigung |
//! | [`stammdaten`] | GGV-Betreiber, § 41e Aggregatorverträge, dead-lettered outbound tasks |
//! | [`inbound`] | Signed CloudEvent webhooks: MaKo outcomes and CPQ Angebote |
//!
//! Every route is authenticated. The `Claims` extractor verifies the token
//! *and* rejects one issued for another tenant, so a route added later cannot
//! forget the check without also dropping authentication.

pub mod inbound;
pub mod kunden;
pub mod rahmenvertraege;
pub mod stammdaten;
pub mod vertraege;

use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    routing::{delete, get, post, put},
};
use mako_service::{ApiError, ApiResult};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::config::VertragdConfig;

/// Everything a handler needs, in one extension instead of four.
pub struct Ctx {
    pub pool: PgPool,
    pub cfg: Arc<VertragdConfig>,
    pub http: reqwest::Client,
}

impl Ctx {
    /// This deployment's data-isolation key.
    #[must_use]
    pub fn tenant(&self) -> &str {
        &self.cfg.tenant
    }
}

/// The domain router. `mako_service::run` adds health, tracing and shutdown.
pub fn router(ctx: Arc<Ctx>) -> Router {
    Router::new()
        // ── Kunden ───────────────────────────────────────────────────────────
        .route(
            "/api/v1/kunden",
            get(kunden::list_kunden).post(kunden::create_kunde),
        )
        .route(
            "/api/v1/kunden/{id}",
            get(kunden::get_kunde).put(kunden::update_kunde),
        )
        .route("/api/v1/kunden/by-sub/{sub}", get(kunden::get_kunde_by_sub))
        .route("/api/v1/kunden/authenticate", get(kunden::authenticate))
        .route(
            "/api/v1/kunden/{id}/identitaeten",
            post(kunden::upsert_identitaet).get(kunden::list_identitaeten),
        )
        .route(
            "/api/v1/kunden/{id}/identitaeten/{sub}",
            delete(kunden::delete_identitaet),
        )
        .route(
            "/api/v1/kunden/{id}/person",
            get(kunden::get_person).put(kunden::put_person),
        )
        .route(
            "/api/v1/kunden/{id}/zahlungsinformation",
            get(kunden::get_zahlungsinformation).put(kunden::put_zahlungsinformation),
        )
        .route("/api/v1/kunden/{id}/export", get(kunden::gdpr_export))
        .route("/api/v1/kunden/{id}/anonymize", post(kunden::anonymize))
        .route("/api/v1/kunden/{id}/portfolio", get(kunden::portfolio))
        .route(
            "/api/v1/kunden/{id}/rahmenvertraege",
            get(rahmenvertraege::list_by_kunde).post(rahmenvertraege::create),
        )
        .route(
            "/api/v1/kunden/{id}/vertraege",
            get(vertraege::list_by_kunde).post(vertraege::create),
        )
        // ── Rahmenverträge ───────────────────────────────────────────────────
        .route("/api/v1/rahmenvertraege", get(rahmenvertraege::list))
        .route("/api/v1/rahmenvertraege/{id}", get(rahmenvertraege::get))
        .route(
            "/api/v1/rahmenvertraege/{id}/malos",
            get(rahmenvertraege::malos),
        )
        .route(
            "/api/v1/rahmenvertraege/{id}/kuendigen",
            post(rahmenvertraege::kuendigen),
        )
        // ── Versorgungsverträge ──────────────────────────────────────────────
        .route("/api/v1/vertraege", get(vertraege::list_open))
        // The MaLo→product mapping, valid-time. billingd bills from it.
        .route(
            "/api/v1/malo/{malo_id}/produkte",
            get(vertraege::malo_produkte),
        )
        .route(
            "/api/v1/vertraege/billing-candidates",
            get(vertraege::billing_candidates),
        )
        .route("/api/v1/vertraege/expiring", get(vertraege::expiring))
        .route(
            "/api/v1/vertraege/by-malo/{malo_id}",
            get(vertraege::by_malo),
        )
        .route("/api/v1/vertraege/{id}", get(vertraege::get))
        .route(
            "/api/v1/vertraege/{id}/kuendigen",
            post(vertraege::kuendigen),
        )
        .route(
            "/api/v1/vertraege/{id}/kuendigungsfrist",
            get(vertraege::kuendigungsfrist),
        )
        .route(
            "/api/v1/vertraege/{id}/widerruf-kuendigung",
            post(vertraege::widerruf_kuendigung),
        )
        .route(
            "/api/v1/vertraege/{id}/stornieren",
            post(vertraege::stornieren),
        )
        .route(
            "/api/v1/vertraege/{id}/tarifwechsel",
            post(vertraege::tarifwechsel),
        )
        .route(
            "/api/v1/vertraege/{id}/preisgarantie",
            get(vertraege::get_preisgarantie).put(vertraege::put_preisgarantie),
        )
        // ── Stammdaten & Betrieb ─────────────────────────────────────────────
        .route(
            "/api/v1/ggv/{ggv_id}/betreiber",
            get(stammdaten::get_ggv_betreiber).put(stammdaten::put_ggv_betreiber),
        )
        .route(
            "/api/v1/aggregatorvertraege",
            get(stammdaten::list_aggregatorvertraege),
        )
        .route(
            "/api/v1/aggregatorvertraege/{sr_id}",
            put(stammdaten::put_aggregatorvertrag),
        )
        // § 9, § 10 MsbG Messstellenverträge — read by processd to answer a
        // WiM Kündigung MSB out of `E_0200`.
        .route(
            "/api/v1/messstellenvertraege/{melo_id}/{msb_mp_id}",
            get(stammdaten::get_messstellenvertrag).put(stammdaten::put_messstellenvertrag),
        )
        .route("/api/v1/outbound/dead", get(stammdaten::list_dead_tasks))
        .route(
            "/api/v1/outbound/dead/{id}/retry",
            post(stammdaten::retry_dead_task),
        )
        // ── Inbound webhooks (HMAC-authenticated, not OIDC) ───────────────────
        .route("/api/v1/events", post(inbound::cloud_event))
        .route("/api/v1/webhooks/angebot", post(inbound::angebot))
        .layer(Extension(ctx))
}

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Wrap a serialisable value as a JSON response body.
pub(crate) fn ok<T: Serialize>(v: T) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(
        serde_json::to_value(v).map_err(|e| ApiError::Internal(e.into()))?,
    ))
}

/// Load a Kunde or answer 404 — the tenant check every customer route needs.
pub(crate) async fn require_kunde(ctx: &Ctx, id: Uuid) -> ApiResult<crate::pg::KundeRow> {
    crate::pg::fetch_kunde(&ctx.pool, id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::NotFound)
}

/// Load a Versorgungsvertrag or answer 404.
pub(crate) async fn require_vertrag(
    ctx: &Ctx,
    id: Uuid,
) -> ApiResult<crate::pg::VersorgungsvertragRow> {
    crate::pg::fetch_vertrag(&ctx.pool, id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::NotFound)
}

/// `true` when the error is a Postgres exclusion-constraint violation (23P01).
pub(crate) fn is_exclusion_violation(e: &anyhow::Error) -> bool {
    e.downcast_ref::<sqlx::Error>()
        .and_then(sqlx::Error::as_database_error)
        .and_then(|d| d.code().map(std::borrow::Cow::into_owned))
        .is_some_and(|c| c == "23P01")
}

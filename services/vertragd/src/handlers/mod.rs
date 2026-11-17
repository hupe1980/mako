//! HTTP surface of `vertragd`.
//!
//! | Module | Routes |
//! |---|---|
//! | [`kunden`] | Kunden, portal identities, DSGVO Auskunft/Löschung, portal authorization |
//! | [`vertraege`] | Versorgungsverträge: create, Kündigung, Widerruf, Stornierung, Tarifwechsel, Preisgarantie |
//! | [`rahmenvertraege`] | B2B framework contracts and their cascade Kündigung |
//! | [`stammdaten`] | GGV-Betreiber, § 41e Aggregatorverträge, dead-lettered outbound tasks |
//! | [`inbound`] | Signed CloudEvent webhooks: MaKo outcomes and CPQ Angebote |
//! | [`bestellungen`] | Online Vertragsschluss: Rechtsdokumente, Bestellungen, Annahme, Widerruf |
//!
//! ## Authentication and authorization are two gates
//!
//! The `Claims` extractor verifies the token and rejects one issued for another
//! tenant. That says *who* is calling and for which deployment — it does not say
//! what they may do, and the token reaching these routes is not always an
//! operator's: `portald` forwards an end customer's own token to the two routes
//! whose answer is about that token's subject. So every route additionally calls
//! `authorize`, which evaluates `policies/vertragd.cedar` and separates the
//! customer-scoped actions from the operator ones. `tests/it/authorization_guard.rs`
//! pins that every routed handler does both.

pub mod bestellungen;
pub mod inbound;
pub mod kunden;
pub mod rahmenvertraege;
pub mod stammdaten;
pub mod vertraege;

use std::sync::Arc;

use axum::{Extension, Router};
use mako_service::{ApiError, ApiResult, Json, oidc::Claims};
use serde::Serialize;
use sqlx::PgPool;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

pub(crate) use mako_service::cedar::CedarEnforcer;

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

/// The OpenAPI document's frame; every path comes from a handler's
/// `#[utoipa::path]`, registered with its route in [`api`].
#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "vertragd — Verträge und Kunden",
        description = "REST API of `vertragd`: Kunden, Versorgungs- and Rahmenverträge, the online Vertragsschluss, the master data billing reads, and the webhook subscriptions to its `de.vertrag.*` CloudEvents."
    ),
    tags(
        (name = "kunden", description = "Kunden, portal identities, DSGVO Auskunft and Löschung"),
        (name = "vertraege", description = "Versorgungsverträge: Kündigung, Stornierung, Tarifwechsel, Preisgarantie, § 14a EnWG"),
        (name = "rahmenvertraege", description = "B2B framework contracts"),
        (name = "bestellungen", description = "Online Vertragsschluss: Rechtsdokumente, Bestellungen, Widerruf"),
        (name = "stammdaten", description = "GGV-Betreiber, Aggregatorverträge, Messstellenverträge, dead-lettered outbound tasks"),
        (name = "inbound", description = "Signed inbound CloudEvents (HMAC, no token)"),
        (name = "webhooks", description = "Webhook endpoints, deliveries, replay and the event feed"),
    )
)]
pub struct ApiDoc;

/// Every `/api/v1` route, each registered with its document entry.
///
/// Handlers take their state as extensions ([`Ctx`], the Cedar enforcer),
/// which [`router`] and `Vertragd::build` layer on.
pub fn api() -> OpenApiRouter {
    OpenApiRouter::with_openapi(<ApiDoc as utoipa::OpenApi>::openapi())
        // ── Kunden ───────────────────────────────────────────────────────────
        .routes(routes!(kunden::list_kunden, kunden::create_kunde))
        .routes(routes!(kunden::get_kunde, kunden::update_kunde))
        .routes(routes!(kunden::get_kunde_by_sub))
        .routes(routes!(kunden::authenticate))
        .routes(routes!(
            kunden::upsert_identitaet,
            kunden::list_identitaeten
        ))
        .routes(routes!(kunden::delete_identitaet))
        .routes(routes!(kunden::get_person, kunden::put_person))
        .routes(routes!(
            kunden::get_zahlungsinformation,
            kunden::put_zahlungsinformation
        ))
        .routes(routes!(kunden::gdpr_export))
        .routes(routes!(kunden::anonymize))
        .routes(routes!(kunden::portfolio))
        .routes(routes!(
            rahmenvertraege::list_by_kunde,
            rahmenvertraege::create
        ))
        .routes(routes!(vertraege::list_by_kunde, vertraege::create))
        // ── Rahmenverträge ───────────────────────────────────────────────────
        .routes(routes!(rahmenvertraege::list))
        .routes(routes!(rahmenvertraege::get))
        .routes(routes!(rahmenvertraege::malos))
        .routes(routes!(rahmenvertraege::kuendigen))
        // ── Versorgungsverträge ──────────────────────────────────────────────
        .routes(routes!(vertraege::list_open))
        // The MaLo→product mapping, valid-time. billingd bills from it.
        .routes(routes!(vertraege::malo_produkte))
        // The § 14a EnWG module state, valid-time. billingd selects the
        // Modul 1/2/3 reductions from it.
        .routes(routes!(vertraege::malo_sect14a, vertraege::record_sect14a))
        .routes(routes!(vertraege::billing_candidates))
        .routes(routes!(vertraege::expiring))
        .routes(routes!(vertraege::by_malo))
        .routes(routes!(vertraege::get))
        .routes(routes!(vertraege::kuendigen))
        .routes(routes!(vertraege::kuendigungsfrist))
        .routes(routes!(vertraege::widerruf_kuendigung))
        .routes(routes!(vertraege::stornieren))
        .routes(routes!(vertraege::tarifwechsel))
        .routes(routes!(
            vertraege::get_preisgarantie,
            vertraege::put_preisgarantie
        ))
        // ── Online-Vertragsschluss ───────────────────────────────────────────
        .routes(routes!(
            bestellungen::get_rechtsdokument,
            bestellungen::put_rechtsdokument
        ))
        .routes(routes!(bestellungen::bestellen))
        .routes(routes!(bestellungen::annehmen))
        .routes(routes!(bestellungen::ablehnen))
        .routes(routes!(bestellungen::nachweis))
        .routes(routes!(bestellungen::widerruf_bestellung))
        .routes(routes!(bestellungen::widerruf_vertrag))
        // ── Stammdaten & Betrieb ─────────────────────────────────────────────
        .routes(routes!(
            stammdaten::get_ggv_betreiber,
            stammdaten::put_ggv_betreiber
        ))
        .routes(routes!(stammdaten::list_aggregatorvertraege))
        .routes(routes!(stammdaten::put_aggregatorvertrag))
        // § 9, § 10 MsbG Messstellenverträge — read by processd to answer a
        // WiM Kündigung MSB out of `E_0200`.
        .routes(routes!(
            stammdaten::get_messstellenvertrag,
            stammdaten::put_messstellenvertrag
        ))
        .routes(routes!(stammdaten::list_dead_tasks))
        .routes(routes!(stammdaten::retry_dead_task))
        // ── Inbound webhooks (HMAC-authenticated, not OIDC) ───────────────────
        .routes(routes!(inbound::cloud_event))
        .routes(routes!(inbound::angebot))
}

/// [`api`] and the webhook subscription routes, split into the router and the
/// document `GET /api/v1/openapi.json` serves — what `Vertragd::build` mounts
/// and `openapi.json` commits.
pub fn documented(
    subs: &mako_service::subscriptions::Subscriptions,
) -> (Router, utoipa::openapi::OpenApi) {
    api().merge(subs.router()).split_for_parts()
}

/// The domain router without the webhook subscriptions — what the tests that
/// drive a handler mount. `Vertragd::build` mounts [`api`] beside the
/// subscription routes and the served document.
///
/// The Cedar enforcer is a parameter rather than a layer the caller adds
/// afterwards: every handler extracts it, so a router built without one would
/// answer 500 on every business route.
pub fn router(ctx: Arc<Ctx>, enforcer: Arc<CedarEnforcer>) -> Router {
    let (routes, _) = api().split_for_parts();
    layered(routes, ctx, enforcer)
}

/// `routes` with the extensions every handler extracts.
pub fn layered(routes: Router, ctx: Arc<Ctx>, enforcer: Arc<CedarEnforcer>) -> Router {
    routes.layer(Extension(ctx)).layer(Extension(enforcer))
}

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Authorize `action` for the caller against this deployment's tenant.
///
/// Authentication established *who* is calling; this decides what they may do.
/// Every business route runs one of these before it touches the database:
/// without it, the customer token `portald` forwards for the portal
/// authorization check is a first-class principal on every other route — it
/// could grant itself a portal login on another customer's account, read that
/// customer's DSGVO export, or terminate their contract.
///
/// The denial is a bare `403`. Which rule refused, and for which subject, goes
/// to the log: a caller holding a valid token learns nothing about the policy
/// or about what exists behind the route it was refused.
///
/// # Errors
///
/// [`ApiError::Forbidden`] when the policy does not permit the action.
pub(crate) fn authorize(
    enforcer: &CedarEnforcer,
    claims: &Claims,
    action: &'static str,
    tenant: &str,
) -> ApiResult<()> {
    enforcer
        .check(&claims.principal(), action, tenant)
        .map_err(|e| {
            tracing::warn!(action, sub = %claims.sub(), error = %e, "vertragd: authorization denied");
            ApiError::Forbidden
        })
}

/// Wrap a serialisable value as a JSON response body.
pub(crate) fn ok<T: Serialize>(v: T) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(
        serde_json::to_value(v).map_err(|e| ApiError::Internal(e.into()))?,
    ))
}

/// Refuse a BO4E document an operator entered that carries a field BO4E does
/// not define — a `422` naming every path, with the gate's
/// `bo4e.unknown_field` code ([`Bo4e::ensure_no_undefined_fields`]).
///
/// [`Bo4e::ensure_no_undefined_fields`]: mako_markt::bo4e::Bo4e::ensure_no_undefined_fields
///
/// # Errors
///
/// [`ApiError`] `422` when any field, at any depth, is not one BO4E defines.
pub(crate) fn refuse_undefined_fields<T>(bo: &mako_markt::bo4e::Bo4e<T>) -> ApiResult<()>
where
    T: mako_markt::bo4e::Bo4eTyped + rubo4e::json::Bo4eExtensions,
{
    bo.ensure_no_undefined_fields().map_err(|rejection| {
        ApiError::unprocessable_with(rejection.to_string(), rejection.detail().into())
    })
}

/// The canonical BO4E JSON of a gated document — what gets stored.
pub(crate) fn canonical_bo4e<T>(bo: &mako_markt::bo4e::Bo4e<T>) -> ApiResult<serde_json::Value>
where
    T: mako_markt::bo4e::Bo4eTyped + Serialize,
{
    bo.canonical_json()
        .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))
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

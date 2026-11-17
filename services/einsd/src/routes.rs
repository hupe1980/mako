//! HTTP surface for `einsd`.
//!
//! The router is built here rather than inline in `main`, so integration tests
//! can drive the real routes — auth layers, extractors and all — instead of
//! calling handler functions directly and missing everything the layers do.
//!
//! Every `/api/v1` route mounts through `utoipa-axum`'s `OpenApiRouter`
//! ([`api_router`]), so none exists without its entry in the document
//! `GET /api/v1/openapi.json` serves and `openapi.json` commits.

use std::sync::Arc;

use axum::{Extension, Router};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::config::EinsdConfig;
use crate::mcp_server::EinsdMcpState;

/// What the router is built from: the resolved pieces of the boot it layers in.
pub struct RouterDeps {
    /// The loaded configuration.
    pub cfg: Arc<EinsdConfig>,
    /// The shared outbound HTTP client.
    pub http_client: Arc<reqwest::Client>,
    /// The Cedar enforcer every route checks.
    pub cedar: Arc<mako_service::cedar::CedarEnforcer>,
    /// The bearer-token verifier.
    pub oidc: mako_service::oidc::OidcVerifier,
    /// The database pool.
    pub pool: PgPool,
    /// The MCP server's state.
    pub mcp_state: Arc<EinsdMcpState>,
    /// Cancelled on shutdown.
    pub shutdown: CancellationToken,
    /// The readiness report the runner built at boot.
    pub einrichtung: Arc<mako_service::einrichtung::Einrichtung>,
}

/// The document's frame; the paths come from the routes.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "einsd — EEG/KWKG feed-in settlement",
        description = "The `/api/v1` surface of `einsd`: the plant register, monthly and corrective settlement, the §§ 52–54 EEG 2023 records, market data, the EnFG Belastungsausgleich and the operators. The typed reads are also MCP tools answering the same type.",
    ),
    tags(
        (name = "anlagen", description = "EEG/KWKG plants and their lifecycle"),
        (name = "settlements", description = "Monthly, corrective, batch and annual settlement"),
        (name = "marktdaten", description = "EPEX prices, Marktwerte and the Vergütungssatz lookup"),
        (name = "enfg", description = "EnFG Belastungsausgleich ledger and Mitteilung"),
        (name = "einspeiser", description = "Plant operators (Anlagenbetreiber)"),
    )
)]
struct ApiDoc;

/// Every `/api/v1` route, each mounted with its document entry.
#[must_use]
pub fn api_router() -> OpenApiRouter {
    use crate::handlers;
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        // ── The documented reads, each also an MCP tool answering the same
        // type (`reads`): the plant, Förderende, MaStR and settlement-receipt reads
        .merge(crate::reads::api_router())
        // ── Anlage CRUD ────────────────────────────────────────────────────────
        .routes(routes!(handlers::post_anlage, handlers::get_anlagen))
        .routes(routes!(handlers::put_anlage, handlers::delete_anlage))
        // ── Settlement ─────────────────────────────────────────────────────────
        .routes(routes!(handlers::post_settle))
        // ── Repowering (§3 Nr. 30 i.V.m. §25 EEG 2023) ─────────────────────────
        .routes(routes!(handlers::post_repowering))
        // ── §36h Abs. 2 Wind Standortgüte re-evaluation (year 6/11/16) ─────────
        .routes(routes!(handlers::post_wind_reevaluation))
        // ── MaStR registration confirmation ────────────────────────────────────
        .routes(routes!(handlers::post_mastr_registrierung))
        // ── §52 Abs. 1 EEG 2023 — the Pflichtverstoß register ─────────────────
        // Nine of the thirteen Nummern are not derivable from a plant row, so
        // this is the only path by which they ever reach a settlement; for the
        // four that are, a record supplies the start date and the Abs. 3
        // reductions.
        .routes(routes!(
            handlers::get_pflichtverstoesse,
            handlers::post_pflichtverstoss
        ))
        .routes(routes!(handlers::put_pflichtverstoss_behoben))
        // ── §§53b–54 EEG 2023 — facts that cut the anzulegender Wert ─────────
        // Recording them through the API keeps a change that silently reduces a
        // Gutschrift behind the same Cedar gate as every other lifecycle event.
        .routes(routes!(handlers::get_aw_reduktionen))
        .routes(routes!(handlers::post_regionalnachweis))
        .routes(routes!(handlers::post_stromsteuerbefreiung))
        .routes(routes!(handlers::post_sect54_defekt))
        .routes(routes!(handlers::post_sect54_nachweis_erbracht))
        // ── Zusammenlegung (§24 EEG 2023) ─────────────────────────────────────
        .routes(routes!(handlers::post_zusammenlegen))
        // ── Veräußerungsform lookup by MaLo — read by processd's NB module ────
        // `E_0622` Prüfschritte 400–830 choose the Vorlauffrist from the
        // *bestehende* Veräußerungsform, which is register data and not on the
        // wire. See `get_veraeusserungsform_by_malo`.
        .routes(routes!(handlers::get_veraeusserungsform_by_malo))
        // ── §21b EEG 2023 — Veräußerungsform switch ───────────────────────────
        .routes(routes!(handlers::post_switch_veraeusserungsform))
        // ── § 147 AO / GoBD — Correction settlement ────────────────────────────
        .routes(routes!(handlers::post_correction_settle))
        // ── Jahresabrechnung (annual reconciliation) ───────────────────────────
        .routes(routes!(handlers::post_jahresabrechnung))
        // ── EnFG Belastungsausgleich (§§ 13, 14, 50 Nr. 1 EnFG) ───────────────
        // The ledger is built from the payments accountingd reports as made;
        // the Mitteilung is read from the booked ledger.
        .routes(routes!(
            handlers::post_enfg_ledger,
            handlers::get_enfg_ledger
        ))
        .routes(routes!(handlers::get_enfg_mitteilung))
        // ── Batch settlement ───────────────────────────────────────────────────
        .routes(routes!(handlers::post_batch_settle))
        // ── EPEX monthly prices ────────────────────────────────────────────────
        .routes(routes!(handlers::put_epex_price, handlers::get_epex_price))
        // ── EPEX spot per-interval prices (§51 Negativpreisregel) ──────────────
        .routes(routes!(handlers::put_epex_spot))
        // ── Anlage 1 Nr. 2–4 Marktwert prices (ÜNB-published) ─────────────────
        // `art` is `monat` or `jahr`: two series, and which one a plant settles
        // on follows its Inbetriebnahme/Zuschlag date, never the operator.
        .routes(routes!(handlers::put_marktwert, handlers::get_marktwert))
        .routes(routes!(handlers::get_marktwert_nachbewertung))
        // ── Einspeiser (Anlagenbetreiber) ──────────────────────────────────────
        // The § 19 UStG election and the payout account belong to the operator,
        // so one PUT here switches the VAT on every one of its plants at once.
        .routes(routes!(handlers::list_einspeiser))
        .routes(routes!(handlers::get_einspeiser, handlers::put_einspeiser))
        // ── EEG tariff rate lookup ─────────────────────────────────────────────
        .routes(routes!(handlers::post_verguetungssatz_lookup))
}

/// The OpenAPI 3.1 document `GET /api/v1/openapi.json` serves; its
/// `info.version` is the crate's (utoipa's default).
#[must_use]
pub fn openapi_document() -> utoipa::openapi::OpenApi {
    api_router().split_for_parts().1
}

/// Build the complete `einsd` router with every layer applied.
pub fn build_router(deps: RouterDeps) -> Router {
    let RouterDeps {
        cfg,
        http_client,
        cedar,
        oidc,
        pool,
        mcp_state,
        shutdown,
        einrichtung,
    } = deps;
    let (api, doc) = api_router().split_for_parts();
    Router::new()
        .merge(crate::mcp_server::router(mcp_state, shutdown))
        // ── Readiness and operator policy: GET /admin/einrichtung (Cedar
        // `read-config`) and GET /api/v1/policy (Cedar `read-policy`), read-only
        .merge(mako_service::einrichtung::router(einrichtung))
        // ── Every `/api/v1` route, and `GET /api/v1/openapi.json` describing them
        .merge(api)
        .merge(mako_service::openapi::serve(doc))
        .layer(Extension(mako_service::oidc::ExpectedTenant(
            cfg.tenant.clone(),
        )))
        .layer(Extension(Arc::clone(&cfg)))
        .layer(Extension(Arc::clone(&http_client)))
        .layer(Extension(cedar))
        .layer(Extension(oidc))
        .layer(Extension(pool))
}

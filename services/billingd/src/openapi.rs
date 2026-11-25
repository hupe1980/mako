//! The OpenAPI 3.1 document for `billingd`, and the router that mounts every
//! `/api/v1` route with its document entry.
//!
//! A route registered through `utoipa-axum`'s `OpenApiRouter` cannot exist
//! without its entry; `GET /api/v1/openapi.json` serves the document
//! (`mako_service::openapi::serve`) and `openapi.json` beside the crate is the
//! committed copy integrators generate clients from.

use mako_service::subscriptions::Subscriptions;
use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::handlers;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "billingd — multi-product billing",
        description = "Calculates, issues, corrects and renders invoices (BO4E `Rechnung`, EN 16931 CII/UBL, ZUGFeRD), settles § 41e VPP dispatches, and watches the § 40c Abs. 2 EnWG deadlines. Every refusal answers with `Problem`; branch on `error.code`.",
    ),
    tags(
        (name = "billing", description = "Calculate, preview and correct invoices"),
        (name = "records", description = "Billing records"),
        (name = "documents", description = "Rendered documents and their issue"),
        (name = "risk", description = "The risk gate's review queue, release and review"),
        (name = "deadlines", description = "§ 40c Abs. 2 EnWG: unbilled periods and the per-Lieferant standing"),
        (name = "nachrechnen", description = "Rechnung nachrechnen: the recomputation and its verify token"),
        (name = "vpp", description = "§ 41e EnWG VPP dispatch settlement"),
        (name = "webhooks", description = "Webhook endpoints, deliveries, replay and the event feed"),
    ),
    components(schemas(Problem, ProblemDetail)),
)]
pub struct ApiDoc;

/// The body every billingd refusal answers with.
#[derive(utoipa::ToSchema)]
pub struct Problem {
    /// What was refused.
    pub error: ProblemDetail,
}

/// The refusal inside [`Problem`]. A conflict or an unprocessable request
/// carries further keys that name what decided it (`record_id`, `warnings`, …).
#[derive(utoipa::ToSchema)]
pub struct ProblemDetail {
    /// A stable machine code (`PERIOD_ALREADY_BILLED`); branch on it.
    pub code: String,
    /// What went wrong, for the developer.
    pub message: String,
    /// On a 502: the upstream service that did not answer.
    pub service: Option<String>,
}

/// Every `/api/v1` route billingd serves, each with its document entry, and
/// the webhook routes of `subs`.
pub fn api(subs: &Subscriptions) -> OpenApiRouter {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(handlers::post_calculate))
        .routes(routes!(handlers::post_preview))
        .routes(routes!(handlers::list_records))
        .routes(routes!(handlers::get_review_queue))
        .routes(routes!(handlers::get_unbilled_periods))
        .routes(routes!(handlers::get_deadline_summary))
        .routes(routes!(handlers::post_billing_sweep))
        .routes(routes!(handlers::get_nachrechnung))
        .routes(routes!(handlers::post_public_nachrechnung))
        .routes(routes!(handlers::post_revoke_verify_token))
        .routes(routes!(handlers::post_release))
        .routes(routes!(handlers::post_reviewed))
        .routes(routes!(handlers::get_record))
        .routes(routes!(handlers::get_xrechnung))
        .routes(routes!(handlers::get_invoice_pdf))
        .routes(routes!(handlers::post_invoice_versenden))
        .routes(routes!(handlers::post_correction))
        .routes(routes!(handlers::post_tarifwechsel))
        .routes(routes!(handlers::post_sammelrechnung))
        .routes(routes!(handlers::post_ggv_billing))
        .routes(routes!(handlers::post_vpp_billing))
        .routes(routes!(handlers::post_vpp_webhook))
        .routes(routes!(handlers::post_submit_b2g))
        .routes(routes!(handlers::get_ubl))
        .merge(subs.router())
}

/// The document `GET /api/v1/openapi.json` serves.
///
/// Built over a lazy pool that never connects: the document depends on the
/// routes, not on a database. The pool needs a Tokio runtime to be created in.
#[must_use]
pub fn document() -> utoipa::openapi::OpenApi {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused@127.0.0.1:1/unused")
        .expect("a lazy pool opens no connection");
    let subs = Subscriptions::new(
        pool,
        "billingd",
        "unused",
        mako_service::subscriptions::SubscriptionsConfig::default(),
    );
    api(&subs).split_for_parts().1
}

#[cfg(test)]
mod tests {
    /// The committed `openapi.json` is the document billingd serves
    /// (`BLESS_MCP_SNAPSHOT=1` rewrites it); integrators and the Desk generate
    /// their clients from the committed file.
    #[tokio::test]
    async fn the_openapi_document_matches_the_committed_one() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json");
        mako_service::openapi::assert_committed(&super::document(), &path);
    }
}

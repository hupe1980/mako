//! The OpenAPI 3.1 document for `processd`, served at
//! `GET /api/v1/openapi.json` and committed as `services/processd/openapi.json`.
//!
//! It describes every `/api/v1` route, each mounted through [`api`]: a route
//! mounted there cannot exist without its document entry. `GET /api/v1/policy`
//! is `mako_service::einrichtung::router`'s and the document route itself
//! `mako_service::openapi::serve`'s; `POST /webhook` and the `/obs/*` reads are
//! outside `/api/v1`.

use mako_service::subscriptions::Subscriptions;
use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::server::{ProcessdState, rest};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "processd — process decisions",
        description = "Answers the published Entscheidungsbäume for NB, LF and MSB, queues what it cannot decide for an operator, starts and ends supply on the LF's behalf, and keeps the process read model. Every refusal answers with `ApiError`; branch on `code`.",
    ),
    tags(
        (name = "decisions", description = "The NB Anmeldung decision log"),
        (name = "queue", description = "The approval queue: Freigaben an operator decides"),
        (name = "klaerfaelle", description = "Klärfälle: take, resolve, correct and send"),
        (name = "supply", description = "Lieferbeginn and Lieferende the LF's ERP initiates"),
        (name = "nb", description = "NB case logs: Ersatz-/Grundversorgung and Neuanlage"),
        (name = "obs", description = "The process read model"),
        (name = "counterparties", description = "How a Marktpartner answered the processes this operator started"),
        (name = "sperr", description = "Sperr- und Entsperraufträge (ORDERS 17115/17117, IFTSTA 21039)"),
        (name = "webhooks", description = "Webhook endpoints, deliveries, replay and the event feed"),
    ),
)]
struct ApiDoc;

/// Every `/api/v1` route processd serves, each with its document entry, and
/// the webhook routes of `subs`.
///
/// The Sperrauftrag routes are the NB's and mount only in an NB build. The
/// approve/reject and Klärfall action paths are the [`crate::server::QueueRoute`]
/// constants the MCP loopback calls; `mcp_route_contract` holds the two to
/// each other.
pub fn api(subs: &Subscriptions) -> OpenApiRouter<ProcessdState> {
    let api = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(rest::rebuild_projection))
        .routes(routes!(rest::list_decisions))
        .routes(routes!(rest::list_queue))
        .routes(routes!(rest::approve_queue_entry))
        .routes(routes!(rest::reject_queue_entry))
        .routes(routes!(rest::list_klaerfaelle))
        .routes(routes!(rest::get_klaerfall))
        .routes(routes!(rest::assign_klaerfall))
        .routes(routes!(rest::resolve_klaerfall))
        .routes(routes!(rest::dispatch_klaerfall))
        .routes(routes!(rest::start_supply))
        .routes(routes!(rest::start_supply_gas))
        .routes(routes!(rest::end_supply))
        .routes(routes!(rest::end_supply_gas))
        .routes(routes!(rest::kuendigung))
        .routes(routes!(rest::kuendigung_gas))
        .routes(routes!(rest::list_eog_cases))
        .routes(routes!(rest::list_neuanlage_cases))
        .routes(routes!(rest::set_neuanlage_identifikation))
        .routes(routes!(crate::obs::rest::get_gleichbehandlung_report))
        .routes(routes!(crate::obs::counterparty::get_counterparties))
        .routes(routes!(crate::obs::counterparty::get_counterparty_record));
    #[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
    let api = api
        .routes(routes!(crate::sperr::handlers::get_stats))
        .routes(routes!(
            crate::sperr::handlers::list_orders,
            crate::sperr::handlers::create_order
        ))
        .routes(routes!(crate::sperr::handlers::get_order))
        .routes(routes!(crate::sperr::handlers::execute_order))
        .routes(routes!(crate::sperr::handlers::fail_order))
        .routes(routes!(crate::sperr::handlers::cancel_order))
        .routes(routes!(crate::sperr::handlers::put_external_ref));
    api.merge(subs.router())
}

/// The document `GET /api/v1/openapi.json` serves.
///
/// Built over a lazy pool that never connects: the document depends on the
/// routes, not on a database.
#[must_use]
pub fn document() -> utoipa::openapi::OpenApi {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused@127.0.0.1:1/unused")
        .expect("a lazy pool opens no connection");
    let subs = Subscriptions::new(
        pool,
        "processd",
        "unused",
        mako_service::subscriptions::SubscriptionsConfig::default(),
    );
    let mut document = api(&subs).split_for_parts().1;
    document.merge(crate::shadow::api().split_for_parts().1);
    document
}

#[cfg(test)]
mod tests {
    /// The committed `openapi.json` is the document an integrated build serves
    /// (`BLESS_MCP_SNAPSHOT=1` rewrites it); integrators and the Desk generate
    /// their clients from the committed file. A build without the NB role
    /// serves the same document less the Sperrauftrag routes.
    #[cfg(all(
        feature = "role-lf-strom",
        feature = "role-lf-gas",
        feature = "role-nb-strom",
        feature = "role-nb-gas",
        feature = "role-msb"
    ))]
    #[tokio::test]
    async fn the_openapi_document_matches_the_committed_one() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json");
        mako_service::openapi::assert_committed(&super::document(), &path);
    }

    /// The approve/reject and Klärfall action routes are documented under the
    /// path and method of the constants the MCP tools call back into.
    #[tokio::test]
    async fn the_loopback_routes_are_documented_under_their_constants() {
        use crate::server::{
            KLAERFALL_ASSIGN, KLAERFALL_DISPATCH, KLAERFALL_RESOLVE, QUEUE_APPROVE, QUEUE_REJECT,
        };
        let doc = serde_json::to_value(super::document()).expect("the document serializes");
        for route in [
            QUEUE_APPROVE,
            QUEUE_REJECT,
            KLAERFALL_ASSIGN,
            KLAERFALL_RESOLVE,
            KLAERFALL_DISPATCH,
        ] {
            assert!(
                doc["paths"][route.path_template][route.method.as_str()].is_object(),
                "{} {} is not documented",
                route.method.as_str(),
                route.path_template
            );
        }
    }
}

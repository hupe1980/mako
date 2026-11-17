//! The OpenAPI 3.1 document for `accountingd`, served at
//! `GET /api/v1/openapi.json` and committed as `services/accountingd/openapi.json`.
//!
//! It describes the routes mounted through [`api_router`] — the reads shared
//! with the MCP tools (`crate::reads`). A route mounted there cannot exist
//! without its document entry.

use axum::Router;
use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::handlers;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "accountingd — Customer Account Ledger",
        description = "REST API of the supplier sub-ledger: customer accounts, the Kontokorrent, open items, dunning and payment matching.",
    ),
    tags(
        (name = "accounts", description = "One customer account: balance and Kontokorrent"),
        (name = "books", description = "Tenant-wide book reads: open items and dunning"),
        (name = "payments", description = "Payment reconciliation"),
    ),
)]
struct ApiDoc;

/// The documented routes and the document describing them.
pub fn api_router() -> (Router, utoipa::openapi::OpenApi) {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(handlers::get_balance))
        .routes(routes!(handlers::get_ledger))
        .routes(routes!(handlers::get_offene_posten))
        .routes(routes!(handlers::get_dunning))
        .routes(routes!(handlers::match_payment))
        .split_for_parts()
}

/// `GET /api/v1/openapi.json` serving `doc`.
pub fn document_route(doc: utoipa::openapi::OpenApi) -> Router {
    Router::new().route(
        "/api/v1/openapi.json",
        axum::routing::get(move || {
            let doc = doc.clone();
            async move { axum::Json(doc) }
        }),
    )
}

/// The document as served.
#[must_use]
pub fn document() -> serde_json::Value {
    serde_json::to_value(api_router().1).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    /// The committed `openapi.json` is the document the service serves;
    /// `BLESS_MCP_SNAPSHOT=1` rewrites it.
    #[test]
    fn the_openapi_document_matches_the_committed_one() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json");
        if let Err(e) = mako_service::mcp_snapshot::compare_json(&super::document(), &path) {
            panic!("{e}");
        }
    }
}

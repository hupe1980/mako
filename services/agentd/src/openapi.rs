//! The OpenAPI 3.1 document for agentd's own routes, served at
//! `GET /api/v1/openapi.json` and committed as `services/agentd/openapi.json`.
//!
//! It describes every `/api/v1` route agentd serves itself, each mounted
//! through [`api_router`]: a route mounted there cannot exist without its
//! document entry. The oversight surface under `/api/v1/oversight` is
//! agentplane's and is described by agentplane's own document
//! (`agentplane::api::openapi::document`), whose paths are relative to that
//! mount; this document does not restate it.

use std::sync::Arc;

use axum::Router;
use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::handlers::{self, AppState};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "agentd — Advisory Agent Plane",
        description = "agentd's own REST API: runs a person starts and follows, the Desk's Hinweise and waiting proposals, verdicts, erasure and the specialist inventory. The worklist, run views and case history under `/api/v1/oversight` are agentplane's operator surface, described by agentplane's own OpenAPI document.",
    ),
    tags(
        (name = "runs", description = "Runs a person starts, follows, stops and judges"),
        (name = "desk", description = "The Desk's Hinweise and waiting proposals"),
        (name = "erasure", description = "Crypto-shredding a case and forgetting a memory subject"),
        (name = "agents", description = "The specialists activated here and compiled in"),
    ),
)]
struct ApiDoc;

/// Every `/api/v1` route agentd serves itself, and the document describing them.
pub fn api_router() -> (Router<Arc<AppState>>, utoipa::openapi::OpenApi) {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(handlers::start_runs))
        .routes(routes!(handlers::answer))
        .routes(routes!(handlers::schritte))
        .routes(routes!(handlers::stopp))
        .routes(routes!(handlers::bewerten))
        .routes(routes!(handlers::hinweise))
        .routes(routes!(handlers::vorschlaege))
        .routes(routes!(handlers::erase))
        .routes(routes!(handlers::list_agents))
        .routes(routes!(handlers::agents_catalog))
        .split_for_parts()
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
        mako_service::openapi::assert_committed(&super::api_router().1, &path);
    }
}

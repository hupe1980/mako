//! The OpenAPI 3.1 document for `portald`, served at
//! `GET /api/v1/openapi.json` and committed as `services/portald/openapi.json`.
//!
//! It describes every `/api/v1` route, each mounted through [`api_router`]: a
//! route mounted there cannot exist without its document entry.
//!
//! `GET /nachrechnen/{token}` is not in it. That page is public, its token is
//! the credential, and it answers HTML for a person, not JSON for a client, so
//! [`crate::server::router`] mounts it as a plain route beside this one.

use axum::Router;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::handlers;

/// The name every operation's security requirement refers to.
pub const CUSTOMER_SCHEME: &str = "portal_customer";

/// Registers [`CUSTOMER_SCHEME`]: the customer's own bearer token.
struct CustomerAuth;

impl Modify for CustomerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        openapi
            .components
            .get_or_insert_with(Default::default)
            .add_security_scheme(
                CUSTOMER_SCHEME,
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .description(Some(
                            "The customer's access token from the portal's identity provider, \
                             as `Authorization: Bearer <token>`. portald verifies nothing itself: \
                             it forwards the token to vertragd, which verifies it and answers \
                             whether its subject owns the `malo_id` in the path. A repeated \
                             `Authorization` header is refused.",
                        ))
                        .build(),
                ),
            );
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "portald — Customer Portal",
        description = "REST API a customer portal calls for one customer's Marktlokation: consumption, invoices and documents, account, supply status, contract and the § 41 EnWG self-service writes.",
    ),
    modifiers(&CustomerAuth),
    security(("portal_customer" = [])),
    tags(
        (name = "read", description = "Consumption, invoices, account and supply status"),
        (name = "documents", description = "The documents issued to the customer"),
        (name = "contract", description = "The supply contract, contact details and the self-service writes"),
        (name = "sepa", description = "SEPA direct-debit mandate"),
    ),
)]
struct ApiDoc;

/// Every `/api/v1` route and the document describing them.
pub fn api_router() -> (Router, utoipa::openapi::OpenApi) {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        // ── Read model ───────────────────────────────────────────────────
        .routes(routes!(handlers::get_dashboard))
        .routes(routes!(handlers::get_lastgang))
        .routes(routes!(handlers::get_invoices))
        .routes(routes!(handlers::get_dokumente))
        .routes(routes!(handlers::get_dokument))
        .routes(routes!(handlers::get_portal_invoice_download))
        .routes(routes!(handlers::get_balance))
        .routes(routes!(handlers::get_kontoauszug))
        .routes(routes!(handlers::get_portal_vorauszahlung))
        .routes(routes!(handlers::get_eeg_status))
        .routes(routes!(handlers::get_versorgung))
        // ── Contract + self-service writes (§ 41 EnWG customer rights) ───
        .routes(routes!(handlers::get_portal_vertrag))
        .routes(routes!(handlers::get_portal_kuendigungsfrist))
        .routes(routes!(handlers::post_portal_tarifwechsel))
        .routes(routes!(handlers::post_portal_kuendigen))
        .routes(routes!(
            handlers::get_portal_kontakt,
            handlers::put_portal_kontakt
        ))
        .routes(routes!(handlers::put_portal_sepa))
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

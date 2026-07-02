//! OpenAPI 3.1 specification for the `makod` HTTP API.
//!
//! Exposes:
//! - `GET /api/v1/openapi.json` — raw OpenAPI JSON
//! - `GET /api/v1/docs/`        — Swagger UI (interactive browser)
//!
//! ## Security
//!
//! All endpoints that carry business data are protected by an opaque Bearer
//! token (`Authorization: Bearer <token>`) configured with `--http-api-token`.
//! The health probe at `GET /health` is always unauthenticated.

use utoipa::{
    Modify, OpenApi,
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
};
use utoipa_swagger_ui::SwaggerUi;

/// Security scheme modifier — registers the `bearer_token` HTTP Bearer scheme.
struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer_token",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some(
                        "Opaque token set via `--http-api-token`. \
                         Pass as `Authorization: Bearer <token>`.",
                    ))
                    .build(),
            ),
        );
    }
}

/// Root API document — collected from all annotated handlers.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "makod — MaKo process engine",
        version = env!("CARGO_PKG_VERSION"),
        description = "German energy market process engine (BDEW EDI@Energy / MaKo).

Implements the full EDIFACT ingest pipeline, ERP command gateway, and \
administrative APIs for MaLo cache and partner directory management.",
        license(name = "MIT OR Apache-2.0"),
        contact(name = "EDI-Energy-RS", url = "https://github.com/hupe1980/edi-energy-rs")
    ),
    paths(
        // ERP command gateway
        crate::commands_api::handle_command,
        // EDIFACT ingest
        crate::edifact_api::ingest_edifact,
        // MaLo admin
        crate::malo_admin_api::handle_stats,
        crate::malo_admin_api::handle_get,
        crate::malo_admin_api::handle_put,
        crate::malo_admin_api::handle_delete,
        // Partner admin
        crate::partner_api::handle_list,
        crate::partner_api::handle_get,
        crate::partner_api::handle_put,
        crate::partner_api::handle_delete,
        crate::partner_api::handle_import,
        // Health
        crate::health::handler,
    ),
    components(
        schemas(
            // commands
            crate::commands_api::ErpCommand,
            crate::commands_api::CommandAccepted,
            // edifact
            crate::edifact_api::IngestResponse,
            crate::edifact_api::MessageResult,
            crate::edifact_api::MessageStatus,
            // malo admin
            crate::malo_admin_api::UpsertRequest,
            crate::malo_admin_api::UpsertResponse,
            crate::malo_admin_api::DeleteResponse,
            crate::malo_admin_api::StatsResponse,
            crate::malo_admin_api::TenantStats,
            // partner admin
            crate::partner_api::UpsertRequest,
            crate::partner_api::PartnerResponse,
            crate::partner_api::ListResponse,
            crate::partner_api::DeleteResponse,
            crate::partner_api::ImportResponse,
            crate::partner_api::ErrorResponse,
            // health
            crate::health::HealthResponse,
        )
    ),
    modifiers(&SecurityAddon),
    tags(
        (name = "commands",  description = "ERP command gateway — submit MaKo process commands"),
        (name = "edifact",   description = "Raw EDIFACT interchange ingestion"),
        (name = "admin",     description = "MaLo cache and partner directory administration"),
        (name = "health",    description = "Liveness and readiness probes"),
    )
)]
pub struct ApiDoc;

/// Build the OpenAPI spec + Swagger UI router.
///
/// Mounts:
/// - `GET /api/v1/openapi.json` — machine-readable OpenAPI 3.1 JSON
/// - `GET /api/v1/docs/`        — Swagger UI (browser-based exploration)
pub fn router() -> axum::Router {
    SwaggerUi::new("/api/v1/docs")
        .url("/api/v1/openapi.json", ApiDoc::openapi())
        .into()
}

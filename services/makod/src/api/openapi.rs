//! OpenAPI 3.1 specification for the `makod` HTTP API.
//!
//! Exposes `GET /api/v1/openapi.json` — the OpenAPI 3.1 document, for a client
//! generator or any Swagger UI / Redoc the operator points at it. makod serves
//! no browser UI of its own.
//!
//! ## Security
//!
//! All endpoints that carry business data are protected by Bearer token
//! authentication and Cedar ABAC authorization. Tokens are named API keys
//! provisioned via `--auth-key NAME=TOKEN`. The health probes under `/health`
//! are always unauthenticated.

use std::sync::Arc;

use utoipa::{
    Modify, OpenApi,
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
};
use utoipa_axum::router::OpenApiRouter;

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
                        "Named API key provisioned with `--auth-key NAME=TOKEN` \
                         (env: MAKOD_AUTH_KEYS). \
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
    // The `/api/v1` operations enter through `OpenApiRouter`'s `routes!`
    // (`startup::servers::serve_http`); the paths below are makod's other
    // ports and the operator routes outside `/api/v1`.
    paths(
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
        crate::health::live,
        crate::health::ready,
    ),
    components(
        schemas(
            // commands
            crate::commands_api::ErpCommand,
            crate::commands_api::CommandAccepted,
            crate::commands_api::probe::ProbeRequest,
            crate::commands_api::probe::Probe,
            crate::commands_api::probe::ProbeNachricht,
            crate::commands_api::probe::ProbeSegment,
            crate::commands_api::probe::ProbeBefund,
            crate::commands_api::probe::Urteil,
            crate::commands_api::probe::Stufe,
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
            crate::partner_api::PartnerResponse,
            crate::partner_api::ListResponse,
            crate::partner_api::DeleteResponse,
            crate::partner_api::ImportResponse,
            // health
            crate::health::HealthResponse,
        )
    ),
    modifiers(&SecurityAddon),
    tags(
        (name = "commands",  description = "ERP command gateway — submit MaKo process commands"),
        (name = "edifact",   description = "Raw EDIFACT interchange ingestion"),
        (name = "admin",     description = "MaLo cache and partner directory administration, the policy report"),
        (name = "invoic",    description = "The BO4E Rechnung an inbound INVOIC carried"),
        (name = "format-candidates", description = "Which of the operator's own messages a candidate Formatversion would refuse"),
        (name = "health",    description = "Liveness and readiness probes"),
    )
)]
pub struct ApiDoc;

/// The document's frame: `info`, the security scheme, the tags, and the
/// operations outside `/api/v1`. [`api_router`] adds every `/api/v1` operation.
#[must_use]
pub fn document() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}

/// makod's `/api/v1` routes on the REST port, each with its document entry:
/// `split_for_parts` gives the router and the document
/// `GET /api/v1/openapi.json` serves, and `services/makod/openapi.json` commits.
pub fn api_router(
    commands: Arc<crate::commands_api::CommandsApiState>,
    invoic: Arc<crate::api::invoic_api::InvoicApiState>,
    einrichtung: Arc<crate::api::einrichtung_api::EinrichtungState>,
    format_impact: Arc<crate::api::format_impact_api::FormatImpactApiState>,
) -> OpenApiRouter {
    OpenApiRouter::with_openapi(document())
        .merge(crate::commands_api::routes(commands))
        .merge(crate::api::invoic_api::routes(invoic))
        .merge(crate::api::einrichtung_api::api_routes(einrichtung))
        .merge(crate::api::format_impact_api::routes(format_impact))
}

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

use utoipa::{
    Modify, OpenApi,
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
};

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
        crate::health::live,
        crate::health::ready,
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
        (name = "admin",     description = "MaLo cache and partner directory administration"),
        (name = "health",    description = "Liveness and readiness probes"),
    )
)]
pub struct ApiDoc;

/// Mounts `GET /api/v1/openapi.json`, the OpenAPI 3.1 document built once.
pub fn router<S: Clone + Send + Sync + 'static>() -> axum::Router<S> {
    let doc = ApiDoc::openapi();
    axum::Router::new().route(
        "/api/v1/openapi.json",
        axum::routing::get(move || {
            let doc = doc.clone();
            async move { axum::Json(doc) }
        }),
    )
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// The OpenAPI document is served at the platform-wide path, and no
    /// browser UI is mounted: a bundled Swagger UI would fetch and unpack an
    /// unverified archive at build time, so the service serves the document alone.
    #[tokio::test]
    async fn the_openapi_document_is_served_and_no_ui_is_mounted() {
        let app: axum::Router = super::router();
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::get("/api/v1/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let doc: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            doc["openapi"]
                .as_str()
                .is_some_and(|v| v.starts_with("3.1")),
            "{doc}"
        );
        let ui = app
            .oneshot(
                axum::http::Request::get("/api/v1/docs/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ui.status(), axum::http::StatusCode::NOT_FOUND);
    }
}

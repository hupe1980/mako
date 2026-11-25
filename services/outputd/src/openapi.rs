//! The document delivery evidence API.

use utoipa::openapi::security::{Http, HttpAuthScheme, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_axum::{router::OpenApiRouter, routes};

/// An outputd business refusal.
#[derive(utoipa::ToSchema)]
pub struct DocumentProblem {
    pub error: DocumentProblemDetail,
}

#[derive(utoipa::ToSchema)]
pub struct DocumentProblemDetail {
    /// Stable code identifying the refusal.
    pub code: String,
    /// Diagnostic for the caller.
    pub message: String,
    /// Correlation identifier on internal failures.
    pub error_id: Option<uuid::Uuid>,
}

struct Bearer;

impl Modify for Bearer {
    fn modify(&self, doc: &mut utoipa::openapi::OpenApi) {
        doc.components
            .get_or_insert_with(Default::default)
            .add_security_scheme(
                "oidc",
                SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
            );
    }
}

#[derive(OpenApi)]
#[openapi(
    info(title = "outputd — document delivery evidence", description = "Read channel evidence for an issued document by its kind and issuer reference, within the authenticated tenant."),
    components(schemas(crate::template_store::TemplateKind)),
    modifiers(&Bearer),
    security(("oidc" = [])),
    tags((name = "delivery evidence", description = "Document channel states and recorded times")),
)]
struct ApiDoc;

pub(crate) fn router() -> (axum::Router, utoipa::openapi::OpenApi) {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(crate::handlers::get_document_by_reference))
        .split_for_parts()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_delivery_evidence_contract_matches_the_committed_document() {
        let (_, doc) = super::router();
        mako_service::openapi::assert_committed(
            &doc,
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json"),
        );
    }
}

//! OpenAPI / SwaggerUI setup for `mdmd`.

use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "mdmd — Master Data Manager",
        version = "1.0",
        description = "REST API for the `mdmd` master-data daemon (Marktlokation, Messlokation, Verträge, Partner, Subscriptions, Process Correlation).",
    ),
    tags(
        (name = "malo", description = "Marktlokation (MaLo) management"),
        (name = "melo", description = "Messlokation (MeLo) management"),
        (name = "contracts", description = "Contract management"),
        (name = "subscriptions", description = "Webhook subscription management"),
        (name = "correlations", description = "Process correlation index"),
        (name = "partners", description = "Trading partner directory"),
        (name = "health", description = "Health endpoints"),
    ),
    paths(
        crate::handlers::malo::put_malo,
        crate::handlers::malo::get_malo,
        crate::handlers::malo::list_malo,
        crate::handlers::melo::put_melo,
        crate::handlers::melo::get_melo,
        crate::handlers::contract::put_contract,
        crate::handlers::contract::get_contract,
    ),
    components(schemas(
        crate::handlers::malo::MaloUpsertRequest,
        crate::handlers::malo::MaloResponse,
        crate::handlers::melo::MeloUpsertRequest,
        crate::handlers::melo::MeloResponse,
        crate::handlers::contract::ContractUpsertRequest,
        crate::handlers::contract::ContractResponse,
        crate::handlers::subscription::SubscriptionUpsertRequest,
        crate::handlers::subscription::SubscriptionResponse,
    )),
)]
pub struct ApiDoc;

/// Build the Swagger UI router mounted at `/swagger-ui`.
#[must_use]
pub fn swagger_ui() -> SwaggerUi {
    SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi())
}

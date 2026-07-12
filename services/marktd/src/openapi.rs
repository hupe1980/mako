//! OpenAPI / SwaggerUI setup for `marktd`.

use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "marktd — Master Data Manager",
        version = "1.0",
        description = "REST API for the `marktd` master-data daemon (Marktlokation, Messlokation, Verträge, Partner, Subscriptions, Process Correlation, VersorgungsStatus, PRICAT).",
    ),
    tags(
        (name = "malo", description = "Marktlokation (MaLo) management"),
        (name = "melo", description = "Messlokation (MeLo) management"),
        (name = "contracts", description = "Contract management"),
        (name = "subscriptions", description = "Webhook subscription management"),
        (name = "correlations", description = "Process correlation index"),
        (name = "partners", description = "Trading partner directory"),
        (name = "versorgung", description = "VersorgungsStatus per MaLo"),
        (name = "pricat", description = "PRICAT 27003 version history and dispatch"),
        (name = "health", description = "Health endpoints"),
    ),
    paths(
        crate::handlers::malo::put_malo,
        crate::handlers::malo::get_malo,
        crate::handlers::malo::list_malo,
        crate::handlers::melo::put_melo,
        crate::handlers::melo::get_melo,
        crate::handlers::melo::get_melo_standorteigenschaften,
        crate::handlers::contract::put_contract,
        crate::handlers::contract::get_contract,
        crate::handlers::pricat::get_pricat_history,
        crate::handlers::pricat::get_dispatch_log,
        crate::handlers::pricat::post_pricat_dispatch,
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
        crate::handlers::pricat::PriCatVersionSummary,
        crate::handlers::pricat::DispatchLogEntry,
    )),
)]
pub struct ApiDoc;

/// Build the Swagger UI router mounted at `/swagger-ui`.
#[must_use]
pub fn swagger_ui() -> SwaggerUi {
    SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi())
}

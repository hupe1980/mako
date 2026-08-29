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
        (name = "subscriptions", description = "Webhook subscription management"),
        (name = "correlations", description = "Process correlation index"),
        (name = "partners", description = "Trading partner directory"),
        (name = "versorgung", description = "VersorgungsStatus per MaLo"),
        (name = "pricat", description = "PRICAT 27003 version history and dispatch"),
        (name = "netzzugang", description = "§20b EnWG Netzzugangsplattform request registry"),
        (name = "msb-rahmenvertraege-gas", description = "Gas MSB-Rahmenvertrag registry (GeLi Gas 3.0, KoV XV Anlage 8)"),
        (name = "health", description = "Health endpoints"),
    ),
    paths(
        crate::handlers::malo::put_malo,
        crate::handlers::malo::get_malo,
        crate::handlers::malo::list_malo,
        crate::handlers::melo::put_melo,
        crate::handlers::melo::get_melo,
        crate::handlers::melo::get_melo_standorteigenschaften,
        crate::handlers::pricat::get_pricat_history,
        crate::handlers::pricat::get_dispatch_log,
        crate::handlers::pricat::post_pricat_dispatch,
        crate::handlers::netzzugang::upsert_antrag,
        crate::handlers::netzzugang::list_antraege,
        crate::handlers::netzzugang::get_antrag,
        crate::handlers::netzzugang::set_antrag_status,
        crate::handlers::msb_rahmenvertrag_gas::upsert_msb_rv_gas,
        crate::handlers::msb_rahmenvertrag_gas::list_msb_rv_gas,
        crate::handlers::msb_rahmenvertrag_gas::get_msb_rv_gas,
    ),
    components(schemas(
        crate::handlers::malo::MaloUpsertRequest,
        crate::handlers::malo::MaloResponse,
        crate::handlers::melo::MeloUpsertRequest,
        crate::handlers::melo::MeloResponse,
        crate::handlers::subscription::SubscriptionUpsertRequest,
        crate::handlers::subscription::SubscriptionResponse,
        crate::handlers::pricat::PriCatVersionSummary,
        crate::handlers::pricat::DispatchLogEntry,
        crate::handlers::netzzugang::StatusBody,
        crate::pg::msb_rahmenvertrag_gas::MsbRahmenvertragGas,
        crate::pg::msb_rahmenvertrag_gas::MsbRvGasStatus,
    )),
)]
pub struct ApiDoc;

/// Build the Swagger UI router.
///
/// Mounts the same two paths `makod` does — `GET /api/v1/docs/` for the browser
/// and `GET /api/v1/openapi.json` for a generator. One well-known pair across
/// the platform is what lets an integrator point a client generator at any
/// service without looking the path up per daemon.
#[must_use]
pub fn swagger_ui() -> SwaggerUi {
    SwaggerUi::new("/api/v1/docs").url("/api/v1/openapi.json", ApiDoc::openapi())
}

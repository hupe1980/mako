//! The OpenAPI 3.1 document for `marktd`.

use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "marktd — Master Data Manager",
        description = "REST API for the `marktd` master-data daemon (Marktlokation, Messlokation, Verträge, Partner, Webhooks, Process Correlation, VersorgungsStatus, PRICAT).",
    ),
    tags(
        (name = "malo", description = "Marktlokation (MaLo) management"),
        (name = "melo", description = "Messlokation (MeLo) management"),
        (name = "webhooks", description = "Webhook endpoints, deliveries, replay and the event feed"),
        (name = "correlations", description = "Process correlation index"),
        (name = "partners", description = "Trading partner directory"),
        (name = "versorgung", description = "VersorgungsStatus per MaLo"),
        (name = "pricat", description = "PRICAT 27003 version history and dispatch"),
        (name = "netzzugang", description = "§20b EnWG Netzzugangsplattform request registry"),
        (name = "msb-rahmenvertraege", description = "MSB-Rahmenvertrag registry, both Sparten (§ 9 Abs. 1 Nr. 3 MsbG; Gas: GeLi Gas 3.0, KoV XV Anlage 8)"),
        (name = "health", description = "Health endpoints"),
    ),
    components(schemas(
        // Named by `ImportQuery` through `IntoParams`, which registers no schema.
        crate::referenzimport::Format,
        crate::handlers::malo::MaloUpsertRequest,
        crate::handlers::malo::MaloResponse,
        crate::handlers::melo::MeloUpsertRequest,
        crate::handlers::melo::MeloResponse,
        crate::handlers::pricat::PriCatVersionSummary,
        crate::handlers::pricat::DispatchLogEntry,
        crate::handlers::netzzugang::StatusBody,
        crate::pg::msb_rahmenvertrag::MsbRahmenvertrag,
        crate::pg::msb_rahmenvertrag::MsbRvUpsertRequest,
        crate::pg::msb_rahmenvertrag::MsbRvStatus,
        crate::pg::msb_rahmenvertrag::MsbRvBefund,
        crate::pg::msb_rahmenvertrag::MsbRvPruefung,
    )),
)]
pub struct ApiDoc;

type S = std::sync::Arc<crate::state::AppState>;

/// The routes mounted through `utoipa-axum`'s `OpenApiRouter` — a route there
/// cannot exist without its document entry — and the document: [`ApiDoc`]'s
/// listed paths plus theirs: the shared reads of [`crate::reads`], whose MCP
/// tools declare the same response type, and the webhook routes of `subs`.
pub fn documented(
    subs: &mako_service::subscriptions::Subscriptions,
) -> (axum::Router<S>, utoipa::openapi::OpenApi) {
    use crate::handlers::{self, lokationsbuendel, versorgung};
    use utoipa_axum::{router::OpenApiRouter, routes};
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(handlers::malo::list_malo))
        .routes(routes!(handlers::malo::get_malo, handlers::malo::put_malo))
        .routes(routes!(handlers::malo::get_malo_lastprofil))
        .routes(routes!(handlers::bundesland::get_malo_bundesland))
        .routes(routes!(
            handlers::malo_grid::get_malo_grid,
            handlers::malo_grid::put_malo_grid
        ))
        .routes(routes!(
            handlers::bilanzierung::get_bilanzierung_at,
            handlers::bilanzierung::put_bilanzierung
        ))
        .routes(routes!(handlers::bilanzierung::get_bilanzierung_history))
        .routes(routes!(
            handlers::device::list_technische_ressourcen_by_malo
        ))
        .routes(routes!(handlers::utilmd_stammdaten::get_utilmd_stammdaten))
        .routes(routes!(handlers::utilmd_stammdaten::put_netznutzungsdaten))
        .routes(routes!(
            handlers::lokationsbuendel::put_lokationsbuendel_angaben
        ))
        .routes(routes!(handlers::lokationszuordnung::get_malo_lokationen))
        .routes(routes!(handlers::lokationszuordnung::get_malo_buendel))
        .routes(routes!(handlers::versorgung::put_versorgungsstatus))
        .routes(routes!(handlers::melo::get_melo, handlers::melo::put_melo))
        .routes(routes!(handlers::melo::get_melo_standorteigenschaften))
        .routes(routes!(
            handlers::melo_msb::get_melo_msb_at,
            handlers::melo_msb::put_melo_msb
        ))
        .routes(routes!(handlers::melo_msb::get_melo_msb_history))
        .routes(routes!(handlers::lokationszuordnung::get_melo_lokationen))
        .routes(routes!(handlers::device::list_zaehler))
        .routes(routes!(handlers::device::get_sharing_eligibility))
        .routes(routes!(handlers::device::put_zaehler))
        .routes(routes!(handlers::device::list_geraete))
        .routes(routes!(handlers::device::get_geraet))
        .routes(routes!(
            handlers::device::get_geraet_konfigurationen,
            handlers::device::put_geraet_konfigurationen
        ))
        .routes(routes!(handlers::device::get_zaehlwerke))
        .routes(routes!(
            handlers::device::list_zaehler_register,
            handlers::device::put_zaehler_register
        ))
        .routes(routes!(handlers::device::get_zaehlzeitdefinitionen))
        .routes(routes!(handlers::device::get_tariff_zone))
        .routes(routes!(
            handlers::device::list_zaehler_saisons,
            handlers::device::put_zaehler_saison
        ))
        .routes(routes!(handlers::device::put_geraet))
        .routes(routes!(
            handlers::device::get_steuerbare_ressource,
            handlers::device::put_steuerbare_ressource
        ))
        .routes(routes!(
            handlers::device::get_konfigurationsprodukte,
            handlers::device::put_konfigurationsprodukte
        ))
        .routes(routes!(handlers::device::delete_konfigurationsprodukt))
        .routes(routes!(
            handlers::device::get_technische_ressource,
            handlers::device::put_technische_ressource
        ))
        .routes(routes!(
            handlers::lokationszuordnung::put_lokationszuordnung
        ))
        .routes(routes!(
            handlers::lokationszuordnung::delete_lokationszuordnung
        ))
        .routes(routes!(handlers::partner::list_partners))
        .routes(routes!(
            handlers::partner::get_partner,
            handlers::partner::put_partner
        ))
        .routes(routes!(handlers::partner::get_as4_address))
        .routes(routes!(handlers::partner::get_partner_marktteilnehmer))
        .routes(routes!(
            handlers::preisblatt::get_preisblatt,
            handlers::preisblatt::put_preisblatt
        ))
        .routes(routes!(
            handlers::preisblatt::get_preisblatt_messung,
            handlers::preisblatt::put_preisblatt_messung
        ))
        .routes(routes!(
            handlers::preisblatt::get_preisblatt_ka,
            handlers::preisblatt::put_preisblatt_ka
        ))
        .routes(routes!(
            handlers::preisblatt::get_preisblatt_dienstleistung,
            handlers::preisblatt::put_preisblatt_dienstleistung
        ))
        .routes(routes!(
            handlers::preisblatt::get_preisblatt_hardware,
            handlers::preisblatt::put_preisblatt_hardware
        ))
        .routes(routes!(handlers::mmma_preise::list_mmma_gas))
        .routes(routes!(
            handlers::mmma_preise::get_mmma_gas,
            handlers::mmma_preise::put_mmma_gas
        ))
        .routes(routes!(
            handlers::mmma_preise::get_mmm_strom,
            handlers::mmma_preise::put_mmm_strom
        ))
        .routes(routes!(handlers::mmma_preise::post_import_trigger))
        .routes(routes!(handlers::referenzreihen::list_referenzreihen))
        .routes(routes!(handlers::referenzreihen::get_werte))
        .routes(routes!(handlers::referenzreihen::get_wert))
        .routes(routes!(handlers::referenzreihen::post_import))
        .routes(routes!(handlers::pricat::get_pricat_history))
        .routes(routes!(handlers::pricat::get_dispatch_log))
        .routes(routes!(handlers::pricat::post_pricat_dispatch))
        .routes(routes!(handlers::nb_contract::list_nb_contracts))
        .routes(routes!(handlers::nb_contract::get_nb_contract_by_malo))
        .routes(routes!(
            handlers::nb_contract::get_nb_contract,
            handlers::nb_contract::put_nb_contract
        ))
        .routes(routes!(
            handlers::nb_energiemix::get_nb_energiemix,
            handlers::nb_energiemix::put_nb_energiemix
        ))
        .routes(routes!(handlers::nb_energiemix::get_nb_energiemix_history))
        .routes(routes!(handlers::nelo::list_nelos))
        .routes(routes!(handlers::nelo::get_nelo, handlers::nelo::put_nelo))
        .routes(routes!(handlers::tranche::list_tranchen))
        .routes(routes!(
            handlers::tranche::get_tranche,
            handlers::tranche::put_tranche
        ))
        .routes(routes!(handlers::mabis_zp::list_mabis_zp))
        .routes(routes!(
            handlers::mabis_zp::get_mabis_zp,
            handlers::mabis_zp::put_mabis_zp
        ))
        .routes(routes!(
            handlers::zuordnungsermaechtigung::list_zuordnungsermaechtigungen
        ))
        .routes(routes!(
            handlers::grundversorger::get_grundversorger,
            handlers::grundversorger::put_grundversorger
        ))
        .routes(routes!(
            handlers::grundzustaendiger_msb::get_grundzustaendiger_msb,
            handlers::grundzustaendiger_msb::put_grundzustaendiger_msb
        ))
        .routes(routes!(
            handlers::netzzugang::upsert_antrag,
            handlers::netzzugang::list_antraege
        ))
        .routes(routes!(handlers::netzzugang::get_antrag))
        .routes(routes!(handlers::netzzugang::set_antrag_status))
        .routes(routes!(
            handlers::msb_rahmenvertrag::upsert_msb_rv,
            handlers::msb_rahmenvertrag::list_msb_rv
        ))
        .routes(routes!(handlers::msb_rahmenvertrag::check_msb_rv))
        .routes(routes!(handlers::msb_rahmenvertrag::get_msb_rv))
        .routes(routes!(handlers::correlation::list_correlations))
        .routes(routes!(handlers::correlation::get_correlation))
        .routes(routes!(
            handlers::einwilligung::grant_einwilligung,
            handlers::einwilligung::list_einwilligungen
        ))
        .routes(routes!(
            handlers::einwilligung::get_einwilligung,
            handlers::einwilligung::revoke_einwilligung
        ))
        .routes(routes!(
            handlers::einwilligung::put_framework,
            handlers::einwilligung::get_framework
        ))
        .routes(routes!(
            handlers::einwilligung::put_esa_preise,
            handlers::einwilligung::get_esa_preise
        ))
        .routes(routes!(handlers::einwilligung::put_esa_messprodukt_katalog))
        .routes(routes!(handlers::einwilligung::get_esa_messprodukt_angebot))
        .routes(routes!(handlers::einwilligung::get_esa_subscription))
        .routes(routes!(handlers::einwilligung::consent_check))
        .routes(routes!(versorgung::get_versorgungsstatus))
        .routes(routes!(versorgung::get_versorgungsstatus_history))
        .routes(routes!(lokationsbuendel::get_lokationsbuendel))
        .routes(routes!(lokationsbuendel::list_lokationsbuendelstrukturen))
        .merge(subs.router())
        .split_for_parts()
}

/// The document `GET /api/v1/openapi.json` serves, built over a lazy pool
/// that never connects: the document depends on the routes, not on a
/// database. Needs a Tokio runtime to create the pool in.
#[must_use]
pub fn openapi_document() -> utoipa::openapi::OpenApi {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused@127.0.0.1:1/unused")
        .expect("a lazy pool opens no connection");
    let subs = mako_service::subscriptions::Subscriptions::new(
        pool,
        "marktd",
        "unused",
        mako_service::subscriptions::SubscriptionsConfig::default(),
    );
    documented(&subs).1
}

/// The document as JSON.
#[must_use]
pub fn document() -> serde_json::Value {
    serde_json::to_value(openapi_document()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    /// The committed `openapi.json` is the document marktd serves
    /// (`BLESS_MCP_SNAPSHOT=1` rewrites it); integrators and the Desk generate
    /// their clients from the committed file.
    #[tokio::test]
    async fn the_openapi_document_matches_the_committed_one() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json");
        mako_service::openapi::assert_committed(&super::openapi_document(), &path);
    }
}

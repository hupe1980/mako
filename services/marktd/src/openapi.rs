//! The OpenAPI 3.1 document for `marktd`.

use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "marktd — Master Data Manager",
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
        (name = "msb-rahmenvertraege", description = "MSB-Rahmenvertrag registry, both Sparten (§ 9 Abs. 1 Nr. 3 MsbG; Gas: GeLi Gas 3.0, KoV XV Anlage 8)"),
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
        crate::handlers::msb_rahmenvertrag::upsert_msb_rv,
        crate::handlers::msb_rahmenvertrag::list_msb_rv,
        crate::handlers::msb_rahmenvertrag::check_msb_rv,
        crate::handlers::msb_rahmenvertrag::get_msb_rv,
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
/// listed paths plus theirs. These are the shared reads of [`crate::reads`],
/// whose MCP tools declare the same response type.
pub fn documented() -> (axum::Router<S>, utoipa::openapi::OpenApi) {
    use crate::handlers::{lokationsbuendel, versorgung};
    use utoipa_axum::{router::OpenApiRouter, routes};
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(versorgung::get_versorgungsstatus))
        .routes(routes!(versorgung::get_versorgungsstatus_history))
        .routes(routes!(lokationsbuendel::get_lokationsbuendel))
        .routes(routes!(lokationsbuendel::list_lokationsbuendelstrukturen))
        .split_for_parts()
}

/// The document `GET /api/v1/openapi.json` serves, as JSON.
#[must_use]
pub fn document() -> serde_json::Value {
    serde_json::to_value(documented().1).unwrap_or_default()
}

/// Mounts `GET /api/v1/openapi.json`, the OpenAPI 3.1 document built once — the
/// same path `makod` serves, so an integrator points a client generator, or any
/// Swagger UI / Redoc, at any service without looking the path up per daemon.
pub fn openapi_router<St: Clone + Send + Sync + 'static>(
    doc: utoipa::openapi::OpenApi,
) -> axum::Router<St> {
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
        let app: axum::Router = super::openapi_router(super::documented().1);
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

    /// The committed `openapi.json` is the document marktd serves
    /// (`BLESS_MCP_SNAPSHOT=1` rewrites it); integrators and the Desk generate
    /// their clients from the committed file.
    #[test]
    fn the_openapi_document_matches_the_committed_one() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json");
        if let Err(e) = mako_service::mcp_snapshot::compare_json(&super::document(), &path) {
            panic!("{e}");
        }
    }

    /// The documented `GET /api/v1/versorgung/{malo_id}` and the undocumented
    /// `PUT` on the same path merge into one route rather than panicking at
    /// boot.
    #[test]
    fn the_documented_get_merges_beside_the_put_on_its_path() {
        let (documented, _) = super::documented();
        let _app: axum::Router<super::S> = documented.merge(axum::Router::new().route(
            "/api/v1/versorgung/{malo_id}",
            axum::routing::put(|| async {}),
        ));
    }
}

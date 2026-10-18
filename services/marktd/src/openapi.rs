//! The OpenAPI 3.1 document for `marktd`.

use utoipa::OpenApi;

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
        crate::pg::msb_rahmenvertrag_gas::MsbRvGasUpsertRequest,
        crate::pg::msb_rahmenvertrag_gas::MsbRvGasStatus,
    )),
)]
pub struct ApiDoc;

/// Mounts `GET /api/v1/openapi.json`, the OpenAPI 3.1 document built once — the
/// same path `makod` serves, so an integrator points a client generator, or any
/// Swagger UI / Redoc, at any service without looking the path up per daemon.
pub fn openapi_router<S: Clone + Send + Sync + 'static>() -> axum::Router<S> {
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
        let app: axum::Router = super::openapi_router();
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

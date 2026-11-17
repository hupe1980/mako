//! The OpenAPI 3.1 document a service serves, and the copy it commits.
//!
//! A service mounts its `/api/v1` routes through `utoipa-axum`'s
//! `OpenApiRouter`, so a route cannot exist without its document entry, and
//! hands the document to [`serve`], which mounts `GET /api/v1/openapi.json` —
//! the one path every service answers on. A test per service holds the
//! committed `services/<svc>/openapi.json` to the document with
//! [`assert_committed`]; integrators and the Desk generate their clients from
//! the committed file.
//!
//! [`ApiError`] is a schema component: a route documents its refusals as
//! `responses((status = 404, body = ApiError))`, and the document then carries
//! the problem body every handler answers with — `error`, `detail`, the stable
//! `code` a client branches on.
//!
//! ```rust,ignore
//! let (router, doc) = OpenApiRouter::with_openapi(ApiDoc::openapi())
//!     .routes(routes!(handlers::get_thing))
//!     .split_for_parts();
//! let app = router.merge(mako_service::openapi::serve(doc));
//! ```

use std::borrow::Cow;
use std::path::Path;

use utoipa::openapi::schema::{
    AdditionalProperties, KnownFormat, ObjectBuilder, Schema, SchemaFormat, Type,
};
use utoipa::openapi::{OpenApi, RefOr};

use crate::ApiError;

/// The path every service serves its document on.
pub const PATH: &str = "/api/v1/openapi.json";

/// `GET /api/v1/openapi.json` serving `doc`, built once.
///
/// No browser UI is mounted: a bundled Swagger UI fetches and unpacks an
/// unverified archive at build time, so a service serves the document alone
/// and an operator points any viewer at it.
pub fn serve<S: Clone + Send + Sync + 'static>(doc: OpenApi) -> axum::Router<S> {
    let doc = std::sync::Arc::new(doc);
    axum::Router::new().route(
        PATH,
        axum::routing::get(move || {
            let doc = std::sync::Arc::clone(&doc);
            async move { axum::Json((*doc).clone()) }
        }),
    )
}

/// Hold the committed document at `path` to `doc`; `BLESS_MCP_SNAPSHOT=1`
/// rewrites the file instead.
///
/// # Panics
///
/// When the committed file differs from `doc`, printing the lines that differ,
/// or when the file cannot be read or written.
pub fn assert_committed(doc: &OpenApi, path: &Path) {
    let value = serde_json::to_value(doc).expect("an OpenAPI document serializes");
    if let Err(e) = crate::mcp_snapshot::compare_json(&value, path) {
        panic!("{e}");
    }
}

impl utoipa::PartialSchema for ApiError {
    fn schema() -> RefOr<Schema> {
        let text = |description: &str| {
            ObjectBuilder::new()
                .schema_type(Type::String)
                .description(Some(description))
        };
        ObjectBuilder::new()
            .description(Some(
                "The problem body every refusal answers with. Branch on `code`; `detail` is \
                 for the developer.",
            ))
            .property("error", text("The HTTP status's reason phrase."))
            .required("error")
            .property("detail", text("What went wrong, for the developer."))
            .required("detail")
            .property(
                "code",
                text("A stable English machine code (`limit_too_large`), where the refusal has one."),
            )
            .property(
                "error_id",
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .format(Some(SchemaFormat::KnownFormat(KnownFormat::Uuid)))
                    .description(Some(
                        "On a 5xx: the id the service's log line carries; the cause is never in the body.",
                    )),
            )
            .property(
                "upstream",
                text("On a 502/503/504: the service this one called that failed."),
            )
            .additional_properties(Some(AdditionalProperties::FreeForm(true)))
            .into()
    }
}

impl utoipa::ToSchema for ApiError {
    fn name() -> Cow<'static, str> {
        Cow::Borrowed("ApiError")
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::response::IntoResponse as _;
    use tower::ServiceExt as _;
    use utoipa_axum::{router::OpenApiRouter, routes};

    use super::*;

    #[utoipa::path(
        get,
        path = "/api/v1/ding",
        responses((status = 200, body = String), (status = 404, body = ApiError))
    )]
    async fn ding() -> &'static str {
        "ding"
    }

    fn document() -> OpenApi {
        OpenApiRouter::<()>::new()
            .routes(routes!(ding))
            .split_for_parts()
            .1
    }

    /// The document is served at the platform-wide path and nothing else is
    /// mounted beside it.
    #[tokio::test]
    async fn the_document_is_served_and_no_ui_is_mounted() {
        let app: axum::Router = serve(document());
        let res = app
            .clone()
            .oneshot(axum::http::Request::get(PATH).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            doc["openapi"]
                .as_str()
                .is_some_and(|v| v.starts_with("3.1"))
        );
        assert!(doc["paths"]["/api/v1/ding"].is_object(), "{doc}");
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

    /// A route documenting a refusal pulls the `ApiError` component into the
    /// document, and the component names every key a rendered refusal
    /// carries.
    #[tokio::test]
    async fn api_error_is_a_component_matching_the_rendered_body() {
        let doc = serde_json::to_value(document()).unwrap();
        let schema = &doc["components"]["schemas"]["ApiError"];
        assert!(schema.is_object(), "ApiError is not a component: {doc}");
        assert_eq!(
            doc["paths"]["/api/v1/ding"]["get"]["responses"]["404"]["content"]["application/json"]
                ["schema"]["$ref"],
            "#/components/schemas/ApiError"
        );
        let res = ApiError::bad_request("nope")
            .with_code("nope")
            .into_response();
        let body = axum::body::to_bytes(res.into_body(), 1 << 16)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        for key in body.as_object().unwrap().keys() {
            assert!(
                schema["properties"][key].is_object(),
                "the rendered key `{key}` is not in the ApiError schema"
            );
        }
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(required, ["error", "detail"]);
    }

    /// A committed copy that differs from the document fails, naming the file.
    #[test]
    fn a_stale_committed_document_fails() {
        let dir = std::env::temp_dir().join(format!("openapi-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("openapi.json");
        let doc = document();
        let mut text = serde_json::to_string_pretty(&doc).unwrap();
        text.push('\n');
        std::fs::write(&path, &text).unwrap();
        assert_committed(&doc, &path);
        std::fs::write(&path, text.replace("/api/v1/ding", "/api/v1/dong")).unwrap();
        let stale = std::panic::catch_unwind(|| assert_committed(&doc, &path));
        std::fs::remove_dir_all(&dir).ok();
        assert!(stale.is_err(), "a stale committed document passed");
    }
}

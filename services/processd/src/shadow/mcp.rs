use super::*;
use rmcp::{
    ServerHandler, handler::server::router::tool::ToolRouter, model::*, tool, tool_handler,
    tool_router, transport::streamable_http_server::StreamableHttpServerConfig,
};

#[derive(Clone)]
struct Handler {
    review: Review,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Handler {
    #[tool(
        description = "Read the configured Strom NB cohort, coverage, comparisons and resolutions."
    )]
    async fn get_shadow_review(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        mako_service::mcp::answer(self.review.read(false).await)
    }
}

#[tool_handler(router=self.tool_router)]
impl ServerHandler for Handler {
    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        mako_service::mcp::supported_revisions()
    }
    fn get_info(&self) -> ServerConfig {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "processd-shadow",
                env!("CARGO_PKG_VERSION"),
            ))
    }
}

async fn reader(
    State(s): State<Review>,
    claims: Claims,
    request: axum::extract::Request,
    next: middleware::Next,
) -> ApiResult<axum::response::Response> {
    s.authorize(&claims, false)?;
    Ok(next.run(request).await)
}

pub(super) fn router(review: Review) -> Router {
    let handler = Handler {
        review: review.clone(),
        tool_router: Handler::tool_router(),
    };
    let service = mako_service::mcp::service(
        move || Ok(handler.clone()),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(review, reader))
}

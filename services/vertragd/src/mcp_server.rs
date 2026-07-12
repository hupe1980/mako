//! MCP server for `vertragd` — Contract & Customer Management.

use axum::{
    Router,
    http::StatusCode,
    middleware::{self, Next},
    response::IntoResponse,
};
use mako_service::{cedar::CedarEnforcer, oidc::OidcVerifier};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    prompt, prompt_handler, prompt_router, schemars, tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use schemars::JsonSchema;
use serde::Deserialize;
use sqlx::PgPool;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
#[allow(dead_code)]
pub struct VertragdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub oidc: OidcVerifier,
    pub cedar: Arc<CedarEnforcer>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VertragIdParams {
    pub id: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct KundeSubParams {
    pub oidc_sub: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListParams {
    pub limit: Option<i64>,
}

#[derive(Clone)]
pub struct VertragdMcpHandler {
    state: Arc<VertragdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<VertragdMcpHandler>,
    #[allow(dead_code)]
    prompt_router: rmcp::handler::server::router::prompt::PromptRouter<VertragdMcpHandler>,
}

#[tool_router]
impl VertragdMcpHandler {
    fn new(state: Arc<VertragdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    #[tool(
        description = "Get a Versorgungsvertrag and all its Vertragskomponenten (STROM/GAS/HEMS/...) by contract UUID.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_vertrag_status(
        &self,
        Parameters(p): Parameters<VertragIdParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::{fetch_vertrag, list_komponenten};
        let id = uuid::Uuid::parse_str(&p.id)
            .map_err(|_| McpError::invalid_params("invalid UUID", None))?;
        match fetch_vertrag(&self.state.pool, id, &self.state.tenant).await {
            Ok(Some(v)) => {
                let komp = list_komponenten(&self.state.pool, id)
                    .await
                    .unwrap_or_default();
                ContentBlock::json(serde_json::json!({ "vertrag": v, "komponenten": komp }))
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(None) => Err(McpError::resource_not_found(
                format!("Vertrag {} not found", id),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List all open Versorgungsverträge (AKTIV, IN_BEARBEITUNG, TEILERFUELLUNG, GEKÜNDIGT).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_offene_vertraege(
        &self,
        Parameters(p): Parameters<ListParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_offene_vertraege;
        match list_offene_vertraege(
            &self.state.pool,
            &self.state.tenant,
            p.limit.unwrap_or(50).min(200),
        )
        .await
        {
            Ok(rows) => {
                ContentBlock::json(serde_json::json!({ "count": rows.len(), "vertraege": rows }))
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Resolve an OIDC sub to a customer profile and their active MaLo IDs. Used for portald authorization.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_kunde_by_sub(
        &self,
        Parameters(p): Parameters<KundeSubParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::{fetch_kunde_by_sub, list_aktive_malo_ids};
        match fetch_kunde_by_sub(&self.state.pool, &p.oidc_sub, &self.state.tenant).await {
            Ok(Some(k)) => {
                let malo_ids = list_aktive_malo_ids(&self.state.pool, k.id, &self.state.tenant)
                    .await
                    .unwrap_or_default();
                ContentBlock::json(serde_json::json!({ "kunde": k, "active_malo_ids": malo_ids }))
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(None) => Err(McpError::resource_not_found(
                format!("No customer with sub={}", p.oidc_sub),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
}

#[prompt_router]
impl VertragdMcpHandler {
    #[prompt(
        description = "Review open contracts and identify stuck MaKo workflows or expiring Verträge"
    )]
    fn o2c_review(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "Review the open contract fulfillment pipeline and identify any stuck or expiring contracts.",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. Use `list_offene_vertraege` to see all contracts IN_BEARBEITUNG or TEILERFUELLUNG.\n\
                 2. For each, use `get_vertrag_status` to see which Vertragskomponenten are stuck.\n\
                 3. ANGEMELDET > 5 WT (Strom) / 10 WT (Gas) → operator escalation.\n\
                 4. ABGELEHNT: check abgelehnt_erc (A02=MaLo not in NB grid, A05=LF not registered).\n\
                 5. Check preisgarantie_bis dates — contracts expiring within 30 days need renewal contact.\n\
                 6. B2B Rahmenverträge with gueltig_bis within 60 days → trigger renewal workflow.",
            ),
        ]
    }
}

#[prompt_handler]
#[tool_handler]
impl ServerHandler for VertragdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("vertragd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "vertragd MCP — B2B + B2C Contract & Customer Management.\n\
                 Manages Kunden, Rahmenverträge (B2B framework), and Versorgungsverträge.\n\
                 Use `get_kunde_by_sub` for portald OIDC authorization → MaLo IDs.\n\
                 Use `list_offene_vertraege` to monitor the O2C fulfillment pipeline.",
        )
    }
}

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<VertragdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    match request
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
    {
        Some(t) if state.oidc.verify(t).is_ok() => next.run(request).await,
        Some(_) => (StatusCode::UNAUTHORIZED, "invalid token").into_response(),
        None => (StatusCode::UNAUTHORIZED, "Authorization: Bearer required").into_response(),
    }
}

pub fn router(state: Arc<VertragdMcpState>, _shutdown: CancellationToken) -> Router {
    let handler = VertragdMcpHandler::new(Arc::clone(&state));
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, mcp_auth_middleware))
}

//! MCP (Model Context Protocol) server for `marktd`.
//!
//! Exposes master data reads to LLM tooling via the MCP Streamable HTTP
//! transport (spec 2025-11-25).  Mounted at `/mcp` on the existing HTTP port.
//!
//! ## Authentication
//!
//! Every request requires `Authorization: Bearer <token>` (OIDC JWT).
//! The principal is checked against Cedar policy action `use-mcp`.
//!
//! ## Tools
//!
//! | Tool | Description |
//! |---|---|
//! | `get_malo`                | Read a MaLo record by 11-digit ID |
//! | `list_partners`           | List registered trading partners |
//! | `get_preisblatt`          | Read the current PreisblattNetznutzung for an NB |
//! | `get_versorgungsstatus`   | Read the VersorgungsStatus (delivery status) for a MaLo |

use std::sync::Arc;

use axum::{
    Router,
    http::StatusCode,
    middleware::{self, Next},
    response::IntoResponse,
};
use mako_service::{
    cedar::CedarEnforcer,
    oidc::{Claims, OidcVerifier},
};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use schemars::JsonSchema;
use serde::Deserialize;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

// ── Shared state ──────────────────────────────────────────────────────────────

/// State injected into each MCP session.
#[derive(Clone)]
pub struct MdmdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub oidc: OidcVerifier,
    pub cedar: Arc<CedarEnforcer>,
}

// ── Tool parameters ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMaloParams {
    /// 11-digit Marktlokations-ID.
    #[schemars(example = "\"51238696781\"")]
    pub malo_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetPreisblattParams {
    /// 13-digit GLN of the Netzbetreiber.
    #[schemars(example = "\"9904234560001\"")]
    pub nb_mp_id: String,
    /// Billing date in ISO 8601 format (YYYY-MM-DD).  Defaults to today.
    pub date: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetVersorgungsstatusParams {
    /// 11-digit Marktlokations-ID.
    #[schemars(example = "\"51238696781\"")]
    pub malo_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListPartnersParams {
    /// Maximum number of results (1–500, default 100).
    pub limit: Option<u32>,
    /// Pagination cursor (opaque string from previous response).
    pub cursor: Option<String>,
}

// ── MCP handler ───────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct MdmdMcpHandler {
    state: Arc<MdmdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<MdmdMcpHandler>,
}

#[tool_router]
impl MdmdMcpHandler {
    fn new(state: Arc<MdmdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    /// Read a Marktlokation (MaLo) record by its 11-digit ID.
    ///
    /// Returns the full MaLo record including address, Sparte, NB and MSB GLNs,
    /// and associated MeLo IDs.  Returns an error when the MaLo has not been
    /// registered with this instance.
    #[tool(description = "Read a Marktlokation (MaLo) record by 11-digit malo_id")]
    async fn get_malo(
        &self,
        Parameters(p): Parameters<GetMaloParams>,
    ) -> Result<CallToolResult, McpError> {
        let row = sqlx::query_as::<
            _,
            (
                uuid::Uuid,
                String,
                String,
                Option<String>,
                Option<String>,
                Option<String>,
            ),
        >(
            r#"
            SELECT id, malo_id, sparte, nb_mp_id, msb_mp_id, address_json::text
            FROM malos
            WHERE tenant = $1 AND malo_id = $2
            "#,
        )
        .bind(&self.state.tenant)
        .bind(&p.malo_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        match row {
            Some((id, malo_id, sparte, nb_mp_id, msb_mp_id, address_json)) => {
                ContentBlock::json(serde_json::json!({
                    "id": id,
                    "malo_id": malo_id,
                    "sparte": sparte,
                    "nb_mp_id": nb_mp_id,
                    "msb_mp_id": msb_mp_id,
                    "address": address_json.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()),
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "malo_not_found: MaLo '{}' not registered in tenant '{}'.",
                p.malo_id, self.state.tenant
            ))])),
        }
    }

    /// List registered trading partners (paginated, max 500).
    ///
    /// Returns partner records with GLN, name, AS4 endpoint URL, and
    /// configured market roles.  Use `limit` and `cursor` to page through
    /// large directories.
    #[tool(description = "List registered trading partners (paginated, max 500)")]
    async fn list_partners(
        &self,
        Parameters(p): Parameters<ListPartnersParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = p.limit.unwrap_or(100).clamp(1, 500) as i64;
        let offset: i64 = p
            .cursor
            .as_deref()
            .and_then(|c| c.parse().ok())
            .unwrap_or(0i64);

        let rows = sqlx::query_as::<_, (String, String, Option<String>, Option<String>)>(
            r#"
            SELECT mp_id, name, as4_endpoint, roles_json::text
            FROM partners
            WHERE tenant = $1
            ORDER BY mp_id
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(&self.state.tenant)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let partners: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|(mp_id, name, as4_endpoint, roles_json)| {
                serde_json::json!({
                    "mp_id": mp_id,
                    "name": name,
                    "as4_endpoint": as4_endpoint,
                    "roles": roles_json
                        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()),
                })
            })
            .collect();

        let next_cursor: Option<String> = if partners.len() == limit as usize {
            Some((offset + limit).to_string())
        } else {
            None
        };

        ContentBlock::json(serde_json::json!({
            "partners": partners,
            "next_cursor": next_cursor,
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Read the current PreisblattNetznutzung for a Netzbetreiber.
    ///
    /// Returns the price sheet used by `invoicd` to validate INVOIC billing
    /// plausibility (§22 MessZV).  Pass `date` to retrieve the sheet valid
    /// on a specific billing date (default: today).
    #[tool(description = "Read the PreisblattNetznutzung (NNE price sheet) for an NB GLN")]
    async fn get_preisblatt(
        &self,
        Parameters(p): Parameters<GetPreisblattParams>,
    ) -> Result<CallToolResult, McpError> {
        use time::format_description::well_known::Rfc3339;
        let date = p
            .date
            .as_deref()
            .and_then(|s| {
                time::Date::parse(s, time::macros::format_description!("[year]-[month]-[day]")).ok()
            })
            .unwrap_or_else(|| time::OffsetDateTime::now_utc().date());

        let row = sqlx::query_as::<_, (uuid::Uuid, time::OffsetDateTime, serde_json::Value)>(
            r#"
            SELECT id, valid_from, preisblatt
            FROM preisblaetter
            WHERE tenant = $1
              AND nb_mp_id = $2
              AND valid_from <= $3
            ORDER BY valid_from DESC
            LIMIT 1
            "#,
        )
        .bind(&self.state.tenant)
        .bind(&p.nb_mp_id)
        .bind(
            time::OffsetDateTime::new_utc(date, time::Time::MIDNIGHT)
                .format(&Rfc3339)
                .unwrap_or_default(),
        )
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        match row {
            Some((id, valid_from, preisblatt)) => ContentBlock::json(serde_json::json!({
                "id": id,
                "nb_mp_id": p.nb_mp_id,
                "valid_from": valid_from,
                "preisblatt": preisblatt,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "preisblatt_not_found: No price sheet for NB '{}' valid on '{date}'.",
                p.nb_mp_id
            ))])),
        }
    }

    /// Read the VersorgungsStatus (supply status) for a Marktlokation.
    ///
    /// Returns the current delivery status (`Beliefert`, `Unbeliefert`, etc.),
    /// active LF and MSB GLNs, and the delivery start/end dates.
    /// This is the authoritative source for automated LFA E_0624 responses
    /// and process routing in `makod`.
    #[tool(
        description = "Read the VersorgungsStatus (delivery status) for a MaLo by 11-digit malo_id"
    )]
    async fn get_versorgungsstatus(
        &self,
        Parameters(p): Parameters<GetVersorgungsstatusParams>,
    ) -> Result<CallToolResult, McpError> {
        let row = sqlx::query_as::<
            _,
            (
                String,               // malo_id
                String,               // lieferstatus
                Option<String>,       // lf_mp_id
                Option<String>,       // lf_gln_next
                Option<time::Date>,   // lieferbeginn
                Option<time::Date>,   // lieferende
                Option<String>,       // msb_mp_id
                String,               // nb_mp_id
                i64,                  // version
                time::OffsetDateTime, // updated_at
            ),
        >(
            r#"SELECT malo_id, lieferstatus, lf_mp_id, lf_gln_next,
                      lieferbeginn, lieferende, msb_mp_id, nb_mp_id,
                      version, updated_at
               FROM versorgungsstatus
               WHERE tenant = $1 AND malo_id = $2"#,
        )
        .bind(&self.state.tenant)
        .bind(&p.malo_id)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        match row {
            Some((
                malo_id,
                lieferstatus,
                lf_mp_id,
                lf_gln_next,
                lieferbeginn,
                lieferende,
                msb_mp_id,
                nb_mp_id,
                version,
                updated_at,
            )) => ContentBlock::json(serde_json::json!({
                "malo_id": malo_id,
                "lieferstatus": lieferstatus,
                "lf_mp_id": lf_mp_id,
                "lf_gln_next": lf_gln_next,
                "lieferbeginn": lieferbeginn.map(|d| d.to_string()),
                "lieferende": lieferende.map(|d| d.to_string()),
                "msb_mp_id": msb_mp_id,
                "nb_mp_id": nb_mp_id,
                "version": version,
                "updated_at": updated_at,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "versorgungsstatus_not_found: No VersorgungsStatus for MaLo '{}' in tenant '{}'.",
                p.malo_id, self.state.tenant
            ))])),
        }
    }
}

#[tool_handler]
impl ServerHandler for MdmdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("marktd", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "# marktd — Master Data Management\n\
             \n\
             Provides MaLo/MeLo master data, trading partner registry, NNE price sheets, and supply status.\n\
             \n\
             ## Tools\n\
             - `get_malo` — read a MaLo by 11-digit ID\n\
             - `list_partners` — list registered trading partners (GLN, AS4 endpoint, roles)\n\
             - `get_preisblatt` — read the PreisblattNetznutzung for an NB (used by invoicd)\n\
             - `get_versorgungsstatus` — read the VersorgungsStatus (Beliefert/Unbeliefert/…) for a MaLo\n\
             \n\
             All reads are scoped to your tenant.  Cross-tenant access is denied by Cedar ABAC.",
            )
    }
}

// ── Auth middleware ───────────────────────────────────────────────────────────

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<MdmdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    let token = match request
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
    {
        Some(t) => t.to_owned(),
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                "Authorization: Bearer <token> required for /mcp",
            )
                .into_response();
        }
    };

    let claims = match state.oidc.verify(&token) {
        Ok(c) => Claims(c),
        Err(_) => {
            return (StatusCode::UNAUTHORIZED, "401 Unauthorized: invalid token").into_response();
        }
    };

    if let Err(e) = state
        .cedar
        .check(&claims.principal(), "use-mcp", &state.tenant)
    {
        return (StatusCode::FORBIDDEN, format!("403 Forbidden: {e}")).into_response();
    }

    next.run(request).await
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Build the `/mcp` Axum router for `marktd`.
pub fn router(state: Arc<MdmdMcpState>, shutdown: CancellationToken) -> Router {
    let config = StreamableHttpServerConfig::default()
        .disable_allowed_hosts()
        .with_sse_keep_alive(Some(std::time::Duration::from_secs(30)))
        .with_cancellation_token(shutdown);

    let mcp_service = StreamableHttpService::new(
        {
            let state = state.clone();
            move || Ok(MdmdMcpHandler::new(state.clone()))
        },
        Arc::new(LocalSessionManager::default()),
        config,
    );

    Router::new()
        .route_service("/mcp", mcp_service)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            mcp_auth_middleware,
        ))
}

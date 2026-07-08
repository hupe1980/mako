//! MCP (Model Context Protocol) server for `invoicd`.
//!
//! Exposes INVOIC receipt reads, dispute management, and billing deadline
//! monitoring to LLM tooling via the MCP Streamable HTTP transport.
//! Mounted at `/mcp` on the existing HTTP port.
//!
//! ## Tools
//!
//! | Tool | Description |
//! |------|-------------|
//! | `get_receipt`         | Read a single INVOIC receipt by UUID |
//! | `list_disputes`       | List all disputed INVOIC receipts |
//! | `get_check_result`    | Get the plausibility findings for a receipt |
//! | `list_overdue_remadv` | Receipts approaching Zahlungsziel without dispatched REMADV |

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

#[derive(Clone)]
pub struct InvoicdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub oidc: OidcVerifier,
    pub cedar: Arc<CedarEnforcer>,
}

// ── Tool parameters ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetReceiptParams {
    /// UUID of the receipt row (from list response).
    pub id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetCheckResultParams {
    /// UUID of the receipt whose plausibility findings you want.
    pub id: String,
}

// ── MCP handler ───────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct InvoicdMcpHandler {
    state: Arc<InvoicdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<InvoicdMcpHandler>,
}

#[tool_router]
impl InvoicdMcpHandler {
    fn new(state: Arc<InvoicdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    /// Read a single INVOIC receipt by UUID.
    ///
    /// Returns the receipt metadata including PID, sender GLN, outcome
    /// (`Ok`/`Warn`/`Dispute`), and timestamps.  The full `rechnung` BO4E
    /// payload is not returned — use `get_check_result` to inspect findings.
    #[tool(description = "Read a single INVOIC receipt by UUID")]
    async fn get_receipt(
        &self,
        Parameters(p): Parameters<GetReceiptParams>,
    ) -> Result<CallToolResult, McpError> {
        let id: uuid::Uuid =
            p.id.parse()
                .map_err(|_| McpError::invalid_params("id is not a valid UUID", None))?;

        let row = sqlx::query_as::<
            _,
            (
                uuid::Uuid,
                uuid::Uuid,
                i16,
                String,
                String,
                time::OffsetDateTime,
                Option<time::OffsetDateTime>,
                String,
            ),
        >(
            r#"
            SELECT id, process_id, pid, sender_mp_id, outcome,
                   received_at, dispatched_at, bo4e_version
            FROM invoic_receipts
            WHERE id = $1 AND tenant = $2
            "#,
        )
        .bind(id)
        .bind(&self.state.tenant)
        .fetch_optional(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        match row {
            Some((
                id,
                process_id,
                pid,
                sender_mp_id,
                outcome,
                received_at,
                dispatched_at,
                bo4e_version,
            )) => ContentBlock::json(serde_json::json!({
                "id": id,
                "process_id": process_id,
                "pid": pid,
                "sender_mp_id": sender_mp_id,
                "outcome": outcome,
                "received_at": received_at,
                "dispatched_at": dispatched_at,
                "bo4e_version": bo4e_version,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "receipt_not_found: No receipt with id '{}'.",
                p.id
            ))])),
        }
    }

    /// List all disputed INVOIC receipts for this tenant.
    ///
    /// Returns receipts with `outcome = 'Dispute'` ordered by `received_at`
    /// descending (most recent first).  Disputes require manual review and
    /// are not auto-settled by `invoicd`.
    ///
    /// Returns up to 200 results.  For full pagination use the REST API.
    #[tool(description = "List all open INVOIC disputes (outcome=Dispute) for this tenant")]
    async fn list_disputes(&self) -> Result<CallToolResult, McpError> {
        let rows = sqlx::query_as::<
            _,
            (
                uuid::Uuid,
                uuid::Uuid,
                i16,
                String,
                time::OffsetDateTime,
                String,
            ),
        >(
            r#"
            SELECT id, process_id, pid, sender_mp_id, received_at, bo4e_version
            FROM invoic_receipts
            WHERE tenant = $1 AND outcome = 'Dispute'
            ORDER BY received_at DESC
            LIMIT 200
            "#,
        )
        .bind(&self.state.tenant)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let disputes: Vec<serde_json::Value> = rows
            .into_iter()
            .map(
                |(id, process_id, pid, sender_mp_id, received_at, bo4e_version)| {
                    serde_json::json!({
                        "id": id,
                        "process_id": process_id,
                        "pid": pid,
                        "sender_mp_id": sender_mp_id,
                        "received_at": received_at,
                        "bo4e_version": bo4e_version,
                    })
                },
            )
            .collect();

        ContentBlock::json(serde_json::json!({
            "disputes": disputes,
            "count": disputes.len(),
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Get plausibility check findings for an INVOIC receipt.
    ///
    /// Returns the structured `findings` array from the `invoic-checker`
    /// plausibility run: each finding has a `code`, `severity`
    /// (`Ok`/`Warn`/`Error`), and a human-readable `message`.
    ///
    /// Useful for understanding why a receipt was disputed or flagged.
    #[tool(description = "Get invoic-checker plausibility findings for a receipt")]
    async fn get_check_result(
        &self,
        Parameters(p): Parameters<GetCheckResultParams>,
    ) -> Result<CallToolResult, McpError> {
        let id: uuid::Uuid =
            p.id.parse()
                .map_err(|_| McpError::invalid_params("id is not a valid UUID", None))?;

        let row =
            sqlx::query_as::<_, (uuid::Uuid, String, serde_json::Value, time::OffsetDateTime)>(
                r#"
            SELECT id, outcome, findings, checked_at
            FROM invoic_receipts
            WHERE id = $1 AND tenant = $2
            "#,
            )
            .bind(id)
            .bind(&self.state.tenant)
            .fetch_optional(&self.state.pool)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        match row {
            Some((id, outcome, findings, checked_at)) => ContentBlock::json(serde_json::json!({
                "id": id,
                "outcome": outcome,
                "findings": findings,
                "checked_at": checked_at,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "receipt_not_found: No receipt with id '{}'.",
                p.id
            ))])),
        }
    }

    /// List INVOIC receipts approaching their Zahlungsziel without a dispatched REMADV.
    ///
    /// Returns receipts where `pay_by < now() + 3 days` AND `dispatched_at IS NULL`.
    /// These are at risk of missing the payment deadline. Alert if non-empty.
    ///
    /// Source: GPKE BK6-22-024; Allgemeine Festlegungen §7 Zahlungsziel.
    #[tool(
        description = "List receipts approaching Zahlungsziel without dispatched REMADV (regulatory deadline risk). Returns up to 50 overdue entries."
    )]
    async fn list_overdue_remadv(&self) -> Result<CallToolResult, McpError> {
        let rows = sqlx::query(
            r#"SELECT id, process_id, pid, sender_mp_id, outcome, pay_by, received_at
               FROM invoic_receipts
               WHERE tenant = $1
                 AND outcome IN ('Ok', 'AcceptedPartial', 'Warn')
                 AND pay_by IS NOT NULL
                 AND pay_by < now() + INTERVAL '3 days'
                 AND dispatched_at IS NULL
               ORDER BY pay_by ASC
               LIMIT 50"#,
        )
        .bind(&self.state.tenant)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                use sqlx::Row;
                use time::format_description::well_known::Rfc3339;
                serde_json::json!({
                    "id": r.try_get::<uuid::Uuid, _>("id").ok(),
                    "process_id": r.try_get::<uuid::Uuid, _>("process_id").ok(),
                    "pid": r.try_get::<i16, _>("pid").ok(),
                    "sender_mp_id": r.try_get::<String, _>("sender_mp_id").ok(),
                    "outcome": r.try_get::<String, _>("outcome").ok(),
                    "pay_by": r.try_get::<time::OffsetDateTime, _>("pay_by").ok()
                        .and_then(|t| t.format(&Rfc3339).ok()),
                })
            })
            .collect();

        ContentBlock::json(serde_json::json!({
            "overdue_count": items.len(),
            "items": items,
            "alert": !items.is_empty(),
            "note": "REMADV dispatch must occur before pay_by to satisfy §7 Allgemeine Festlegungen",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }
}

#[tool_handler]
impl ServerHandler for InvoicdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("invoicd", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "# invoicd — INVOIC Billing Validation\n\
             \n\
             Validates INVOIC billing from NB against NNE price sheets (§22 MessZV).\n\
             \n\
             ## Tools\n\
             - `get_receipt` — read a receipt by UUID (outcome, timestamps)\n\
             - `list_disputes` — list all disputed INVOIC receipts (outcome=Dispute)\n\
             - `get_check_result` — get invoic-checker findings for a receipt\n\
             - `list_overdue_remadv` — receipts approaching Zahlungsziel without REMADV (deadline risk)\n\
             \n\
             ## Outcomes\n\
             - `Ok` — all checks passed; REMADV auto-dispatched\n\
             - `Warn` — checks passed with warnings; may be auto-approved or disputed\n\
             - `Dispute` — plausibility failure; COMDIS dispatched; requires manual review\n\
             - `Dispatched` — outbound 31006 selbstausgestellt sent; awaiting NB REMADV\n\
             - `Paid` — outbound 31006 settled by NB",
            )
    }
}

// ── Auth middleware ───────────────────────────────────────────────────────────

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<InvoicdMcpState>>,
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

pub fn router(state: Arc<InvoicdMcpState>, shutdown: CancellationToken) -> Router {
    let config = StreamableHttpServerConfig::default()
        .disable_allowed_hosts()
        .with_sse_keep_alive(Some(std::time::Duration::from_secs(30)))
        .with_cancellation_token(shutdown);

    let mcp_service = StreamableHttpService::new(
        {
            let state = state.clone();
            move || Ok(InvoicdMcpHandler::new(state.clone()))
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

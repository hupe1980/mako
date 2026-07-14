//! MCP server for `sperrd` — Sperrung Execution Tracking (NB role).
//!
//! ## Tools (5)
//!
//! | Tool | Description |
//! |---|---|
//! | `list_sperr_orders`     | List orders — filterable by status + older_than_hours |
//! | `get_sperr_order`       | Get a single order by UUID |
//! | `get_sperr_stats`       | Aggregate stats: pending, executed, overdue, missing IFTSTA |
//! | `list_overdue_orders`   | Orders past their planned_date still in pending state |
//! | `cancel_sperr_order`    | Cancel a pending order (operator action; no IFTSTA dispatched) |
//!
//! ## Prompts (2)
//!
//! | Prompt | Description |
//! |---|---|
//! | `execute-sperrung`      | Step-by-step: execute a Sperrung and confirm IFTSTA 21039 |
//! | `compliance-sweep`      | Step-by-step: daily BK6-22-024 compliance check |

use axum::{
    Router,
    middleware::{self, Next},
};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{
        router::{prompt::PromptRouter, tool::ToolRouter},
        wrapper::Parameters,
    },
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
pub struct SperrdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub auth: mako_service::mcp_auth::McpAuth,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListSperrParams {
    /// Filter by status: `pending`, `executed`, `failed`, `cancelled`.
    pub status: Option<String>,
    /// Only return orders created more than N hours ago.
    ///
    /// Use `older_than_hours=48` in the daily compliance sweep to find
    /// stuck orders that have exceeded the BK6-22-024 2-Werktage window.
    pub older_than_hours: Option<i64>,
    /// Maximum results (default 50).
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetSperrParams {
    /// UUID of the Sperrung order.
    pub id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CancelSperrParams {
    /// UUID of the pending order to cancel.
    pub id: String,
}

/// Shared handler + router state.
#[derive(Clone)]
pub struct SperrdMcpHandler {
    state: Arc<SperrdMcpState>,
    #[allow(dead_code)]
    tool_router: ToolRouter<SperrdMcpHandler>,
    #[allow(dead_code)]
    prompt_router: PromptRouter<SperrdMcpHandler>,
}

#[tool_router]
impl SperrdMcpHandler {
    fn new(state: Arc<SperrdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    #[tool(
        description = "List Sperrung/Entsperrung execution orders. Filter by status (pending/executed/failed/cancelled) and/or older_than_hours (returns orders stuck for more than N hours). GPKE BK6-22-024: use older_than_hours=48 in daily sweep to detect 2-Werktage violations.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_sperr_orders(
        &self,
        Parameters(p): Parameters<ListSperrParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_orders_pg;
        match list_orders_pg(
            &self.state.pool,
            &self.state.tenant,
            p.status.as_deref(),
            None,
            p.older_than_hours,
            p.limit.unwrap_or(50),
        )
        .await
        {
            Ok(orders) => {
                let pending = orders.iter().filter(|o| o.status == "pending").count();
                ContentBlock::json(serde_json::json!({
                    "count": orders.len(),
                    "orders": orders,
                    "pending_count": pending,
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Get a single Sperrung/Entsperrung order by UUID. Returns execution timestamps, iftsta_ref (makod command ID), iftsta_dispatched_at, and associated ORDERS process_id.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_sperr_order(
        &self,
        Parameters(p): Parameters<GetSperrParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_order_pg;
        let Ok(id) = p.id.parse::<uuid::Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        match fetch_order_pg(&self.state.pool, id).await {
            Ok(Some(order)) => ContentBlock::json(serde_json::to_value(order).unwrap_or_default())
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => Err(McpError::invalid_params(
                format!("order {id} not found"),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Aggregate Sperrung stats for the compliance sweep: pending, executed, failed, cancelled counts + overdue_pending (past planned_date) + executed_missing_iftsta (GPKE protocol violations requiring immediate action).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_sperr_stats(&self) -> Result<CallToolResult, McpError> {
        use crate::pg::stats_pg;
        match stats_pg(&self.state.pool, &self.state.tenant).await {
            Ok(s) => ContentBlock::json(serde_json::json!({
                "total":                   s.total,
                "pending":                 s.pending,
                "executed":                s.executed,
                "failed":                  s.failed,
                "cancelled":               s.cancelled,
                "overdue_pending":         s.overdue_pending,
                "executed_missing_iftsta": s.executed_missing_iftsta,
                "compliance": {
                    "overdue_ok":  s.overdue_pending == 0,
                    "iftsta_ok":   s.executed_missing_iftsta == 0,
                    "note": if s.overdue_pending > 0 || s.executed_missing_iftsta > 0 {
                        "ACTION REQUIRED: see overdue_pending and executed_missing_iftsta counts"
                    } else {
                        "All orders within BK6-22-024 compliance window"
                    },
                },
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List pending Sperrung orders whose planned_date is in the past. These are overdue per BK6-22-024 and require immediate operator action or escalation to the LF. Returns planned_date, malo_id, lf_mp_id, and days overdue.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_overdue_orders(&self) -> Result<CallToolResult, McpError> {
        use sqlx::Row;
        let rows = sqlx::query(
            r"SELECT id::TEXT, malo_id, lf_mp_id, planned_date, created_at,
                     (CURRENT_DATE - planned_date)::INT AS days_overdue
              FROM sperr_orders
              WHERE (tenant = $1 OR $1 = '')
                AND status = 'pending'
                AND planned_date IS NOT NULL
                AND planned_date < CURRENT_DATE
              ORDER BY planned_date ASC",
        )
        .bind(&self.state.tenant)
        .fetch_all(&self.state.pool)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let overdue: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| serde_json::json!({
                "id":            r.try_get::<String, _>("id").unwrap_or_default(),
                "malo_id":       r.try_get::<String, _>("malo_id").unwrap_or_default(),
                "lf_mp_id":      r.try_get::<String, _>("lf_mp_id").unwrap_or_default(),
                "planned_date":  r.try_get::<Option<time::Date>, _>("planned_date").unwrap_or(None),
                "days_overdue":  r.try_get::<Option<i32>, _>("days_overdue").unwrap_or(None),
            }))
            .collect();

        ContentBlock::json(serde_json::json!({
            "count": overdue.len(),
            "overdue_orders": overdue,
            "regulatory_note": "BK6-22-024: Sperrung must be executed within 2 Werktage. Escalate to LF and field team immediately.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Cancel a PENDING Sperrung order. Only call on explicit operator instruction. Terminal orders (executed/failed) cannot be cancelled. No IFTSTA is dispatched for cancelled orders — they were never physically executed.",
        annotations(
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn cancel_sperr_order(
        &self,
        Parameters(p): Parameters<CancelSperrParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::cancel_order_pg;
        let Ok(id) = p.id.parse::<uuid::Uuid>() else {
            return Err(McpError::invalid_params("id must be a valid UUID", None));
        };
        match cancel_order_pg(&self.state.pool, id).await {
            Ok(true) => ContentBlock::json(serde_json::json!({
                "cancelled": true, "id": p.id,
                "note": "Order cancelled. No IFTSTA dispatched. Inform LF if the Sperrung was already communicated.",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(false) => Err(McpError::invalid_params(
                format!("order {} not found or not in pending state", p.id),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
}

#[prompt_router]
impl SperrdMcpHandler {
    #[prompt(
        name = "execute-sperrung",
        description = "Step-by-step: execute a Sperrung order and confirm IFTSTA 21039 dispatch"
    )]
    async fn execute_sperrung_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "The field team has executed a Sperrung. How do I confirm it?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "**Sperrung execution confirmation (BK6-22-024)**\n\n\
                 1. Call `list_sperr_orders` with `status=pending` to find the order.\n\
                 2. Match `malo_id` and `process_id` to the field report.\n\
                 3. Call REST `PUT /api/v1/sperr-orders/{id}/execute` with:\n\
                    - `executed_at`: actual field execution time (RFC 3339 + UTC offset)\n\
                    - `note`: field reference (e.g. TW-2026-0714-001)\n\
                 4. sperrd auto-dispatches IFTSTA 21039 to the LF via makod.\n\
                 5. Verify with `get_sperr_order({id})`: `status=executed`, `iftsta_dispatched_at` set.\n\n\
                 **GPKE BK6-22-024**: IFTSTA 21039 must reach the LF within the ORDERS execution window.\n\
                 A missed dispatch can only be corrected by a new IFTSTA — contact the LF immediately.\n\n\
                 **AWH billing**: each executed Sperrung triggers INVOIC 31011 (Rechnung sonstige Leistung).\n\
                 Check netzbilanzd for a pending draft after execution.",
            ),
        ]
    }

    #[prompt(
        name = "compliance-sweep",
        description = "Daily BK6-22-024 compliance sweep: find stuck orders, missing IFTSTA, billing gaps"
    )]
    async fn compliance_sweep_prompt(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "Run the daily Sperrung compliance check for BK6-22-024.",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "**Daily BK6-22-024 Sperrung compliance sweep**\n\n\
                 **Step 1 — Aggregate status**\n\
                 Call `get_sperr_stats`.\n\
                 - `overdue_pending > 0`: BK6-22-024 violation risk — go to Step 2.\n\
                 - `executed_missing_iftsta > 0`: GPKE protocol violation — go to Step 3.\n\
                 - All zeros: compliance OK. Log and continue.\n\n\
                 **Step 2 — Overdue pending orders**\n\
                 Call `list_overdue_orders` to get full list with `days_overdue`.\n\
                 For each: escalate to field team. If >= 2 Werktage overdue, notify LF.\n\
                 Per BK6-22-024 §9: failed/delayed Sperrung must be reported to LF within 3 Werktage.\n\n\
                 **Step 3 — Missing IFTSTA 21039**\n\
                 Call `list_sperr_orders(status=executed)` and filter where `iftsta_dispatched_at IS NULL`.\n\
                 For each affected order: call REST `PUT /api/v1/sperr-orders/{id}/execute` again\n\
                 (idempotent — same idempotency key prevents double-dispatch).\n\
                 OR: manually trigger via makod.\n\n\
                 **Step 4 — AWH billing check**\n\
                 Cross-reference executed Sperrungen with netzbilanzd INVOIC 31011 drafts.\n\
                 Each Sperrung close must generate one INVOIC 31011 (Rechnung sonstige Leistung, GeLi Gas NB→LF).\n\n\
                 **Output format**: { overdue: N, missing_iftsta: N, billing_gaps: N, status: ok|action_required }",
            ),
        ]
    }
}

#[tool_handler]
#[prompt_handler]
impl ServerHandler for SperrdMcpHandler {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("sperrd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "sperrd MCP — Sperrung/Entsperrung Execution Tracking (NB role).\n\
             Tracks ORDERS 17115/17117 execution and auto-dispatches IFTSTA 21039.\n\
             A missing IFTSTA 21039 = GPKE BK6-22-024 protocol violation.\n\n\
             ## Tools (5)\n\
             - `list_sperr_orders(status, older_than_hours, limit)` — filter by status + age\n\
             - `get_sperr_order(id)` — full order with timestamps and IFTSTA dispatch status\n\
             - `get_sperr_stats` — aggregate: pending/executed/failed + overdue + missing IFTSTA\n\
             - `list_overdue_orders` — pending orders past planned_date (BK6-22-024 violations)\n\
             - `cancel_sperr_order(id)` — cancel a pending order (operator action only)\n\n\
             ## Prompts (2)\n\
             - `execute-sperrung` — confirm field execution + IFTSTA 21039 dispatch workflow\n\
             - `compliance-sweep` — daily BK6-22-024 sweep: stuck + missing IFTSTA + billing\n\n\
             ## GPKE timing\n\
             Sperrung execution: within 2 Werktage of ORDERS Bestelldatum (BK6-22-024).\n\
             IFTSTA 21039: must reach LF within execution window. Overdue = regulatory violation.",
        )
    }
}

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<SperrdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    state.auth.authenticate(request, next).await
}

pub fn router(state: Arc<SperrdMcpState>, _shutdown: CancellationToken) -> Router {
    let handler = SperrdMcpHandler::new(Arc::clone(&state));
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, mcp_auth_middleware))
}
